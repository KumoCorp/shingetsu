use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};

use crate::call_context::CallContext;
use crate::convert::FromLuaMulti;
use crate::error::{VmError, VmResultExt};
use crate::function::{Function, FunctionState, NativeFunction};
use crate::gc::GcColor;
use crate::proto::Proto;
use crate::table::{Table, TableState};
use crate::task::Task;
use crate::types::FunctionSignature;
use crate::value::Value;

/// Shared compiled environment.  Cheap to clone (Arc-backed).
/// `Send + Sync`; safe to share across threads and async tasks.
#[derive(Clone)]
pub struct GlobalEnv(pub(crate) Arc<GlobalEnvInner>);

/// Opaque opener type stored in the preload registry.
///
/// Returns the module table.  The opener should be idempotent — the caller
/// only invokes it once per module name and caches the result.
pub(crate) type PreloadOpener =
    Arc<dyn Fn(&GlobalEnv) -> Result<crate::table::Table, VmError> + Send + Sync>;

pub(crate) struct GlobalEnvInner {
    /// Global variable table.  Fine-grained sharded locking: concurrent
    /// readers never block each other; a write only locks the relevant shard.
    pub(crate) globals: DashMap<Bytes, Value>,
    /// Loaded top-level prototypes.
    #[allow(dead_code)]
    pub(crate) protos: RwLock<Vec<Arc<Proto>>>,
    /// Registered native functions (also inserted into `globals`).
    pub(crate) natives: DashMap<Bytes, Arc<NativeFunction>>,
    /// `package.preload`-equivalent registry: module name → opener function.
    /// Populated by `GlobalEnv::register_preload`; consumed by `require`.
    pub(crate) preload: DashMap<Bytes, PreloadOpener>,
    /// `package.loaded`-equivalent cache: module name → already-loaded value.
    /// `require` writes here on first load; subsequent calls return the cache.
    pub(crate) loaded: DashMap<Bytes, Value>,
    /// Strong references to every `Table` allocated in this environment.
    /// Keeping strong refs prevents `__gc` finalizers from being silently
    /// skipped: the registry is the only thing that keeps unreachable tables
    /// alive until the collector can call their finalizer.
    pub(crate) gc_tables: Mutex<Vec<Arc<TableState>>>,
    /// Strong references to every Lua function (closure) allocated here.
    pub(crate) gc_functions: Mutex<Vec<Arc<FunctionState>>>,
    /// Tables (and their `__gc` function) that were found unreachable during
    /// the last `collect_cycles()` call but have a finalizer that must be
    /// called before the storage is released.
    pub(crate) pending_finalizers: Mutex<Vec<(crate::table::Table, Function)>>,
    /// Shared metatable for all string values.  When a `GetTable` instruction
    /// encounters a `Value::String`, the VM consults this metatable's
    /// `__index` so that `("hello"):upper()` works.
    string_metatable: RwLock<Option<Table>>,
}

impl GlobalEnv {
    pub fn new() -> Self {
        let env = GlobalEnv(Arc::new(GlobalEnvInner {
            globals: DashMap::new(),
            protos: RwLock::new(Vec::new()),
            natives: DashMap::new(),
            preload: DashMap::new(),
            loaded: DashMap::new(),
            gc_tables: Mutex::new(Vec::new()),
            gc_functions: Mutex::new(Vec::new()),
            pending_finalizers: Mutex::new(Vec::new()),
            string_metatable: RwLock::new(None),
        }));
        env.register_builtins();
        env
    }

    /// Register the built-in functions that cannot be expressed through the
    /// `#[module]` proc macro (they need private VM internals or custom
    /// calling conventions).
    ///
    /// The remaining builtins are registered via
    /// `shingetsu::builtins::register` which uses the proc macro.
    pub(crate) fn register_builtins(&self) {
        // ----------------------------------------------------------------
        // ipairs(table)
        // Returns (iter, table, 0) for sequential integer-keyed iteration.
        // Respects __ipairs metamethod (Lua 5.2).
        // In Lua 5.3+ __ipairs was removed; instead ipairs uses __index, so
        // the inner iterator goes through ctx.call_function which dispatches
        // __index at the VM level.
        // ----------------------------------------------------------------
        self.register_native(make_native("ipairs", 1, |ctx, args| {
            Box::pin(async move {
                let table = match args.into_iter().next().unwrap_or(Value::Nil) {
                    Value::Table(t) => t,
                    other => {
                        return Err(VmError::BadArgument {
                            position: 1,
                            function: "ipairs".to_owned(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        })
                    }
                };
                // Lua 5.2: if __ipairs is defined, delegate entirely.
                if let Some(Value::Function(mm)) = table.get_metamethod("__ipairs") {
                    return ctx.call_function(mm, vec![Value::Table(table)]).await;
                }
                // Lua 5.3+: the iterator uses raw table access (integer keys
                // only); __index is not consulted during ipairs iteration per
                // the 5.3 spec.  We use raw_get here to match that behaviour.
                let iter_fn = make_native("ipairs_iter", 2, |_ctx2, args2| {
                    Box::pin(async move {
                        let mut it = args2.into_iter();
                        let tab = match it.next().unwrap_or(Value::Nil) {
                            Value::Table(t) => t,
                            _ => return Ok(vec![Value::Nil]),
                        };
                        let idx = match it.next().unwrap_or(Value::Nil) {
                            Value::Integer(n) => n + 1,
                            _ => return Ok(vec![Value::Nil]),
                        };
                        let v = tab.raw_get(&Value::Integer(idx))?;
                        if v.is_nil() {
                            Ok(vec![Value::Nil])
                        } else {
                            Ok(vec![Value::Integer(idx), v])
                        }
                    })
                });
                Ok(vec![
                    Value::Function(Function::native(iter_fn)),
                    Value::Table(table),
                    Value::Integer(0),
                ])
            })
        }));

        // ----------------------------------------------------------------
        // pcall(f, ...)
        // ----------------------------------------------------------------
        self.register_native(make_native("pcall", 1, |ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let func = match it.next() {
                    Some(Value::Function(f)) => f,
                    Some(other) => {
                        return Ok(vec![
                            Value::Boolean(false),
                            Value::String(Bytes::from(format!(
                                "attempt to call a {} value",
                                other.type_name()
                            ))),
                        ])
                    }
                    None => {
                        return Ok(vec![
                            Value::Boolean(false),
                            Value::String(Bytes::from_static(
                                b"bad argument #1 to 'pcall' (value expected)",
                            )),
                        ])
                    }
                };
                let func_args: Vec<Value> = it.collect();
                protected_call_ctx(ctx, func, func_args).await
            })
        }));

        // ----------------------------------------------------------------
        // xpcall(f, msgh, ...)
        // ----------------------------------------------------------------
        self.register_native(make_native("xpcall", 2, |ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let func = match it.next() {
                    Some(Value::Function(f)) => f,
                    Some(other) => {
                        return Ok(vec![
                            Value::Boolean(false),
                            Value::String(Bytes::from(format!(
                                "attempt to call a {} value",
                                other.type_name()
                            ))),
                        ])
                    }
                    None => {
                        return Ok(vec![
                            Value::Boolean(false),
                            Value::String(Bytes::from_static(
                                b"bad argument #1 to 'xpcall' (value expected)",
                            )),
                        ])
                    }
                };
                let handler = match it.next() {
                    Some(Value::Function(f)) => Some(f),
                    _ => None,
                };
                let func_args: Vec<Value> = it.collect();
                let result = protected_call_ctx(ctx.clone(), func, func_args).await?;
                // On error (first result is false), run the message handler.
                if result.first() == Some(&Value::Boolean(false)) {
                    if let Some(h) = handler {
                        let err_val = result.into_iter().nth(1).unwrap_or(Value::Nil);
                        let handler_result = protected_call_ctx(ctx, h, vec![err_val]).await?;
                        // Return false + handler output.
                        let mut out = vec![Value::Boolean(false)];
                        out.extend(handler_result.into_iter().skip(1));
                        return Ok(out);
                    }
                }
                Ok(result)
            })
        }));

        // ----------------------------------------------------------------
        // require(modname)
        // ----------------------------------------------------------------
        self.register_native(make_native("require", 1, |ctx, args| {
            Box::pin(async move {
                let name = Bytes::from_lua_multi(args).with_call_context(1, &ctx)?;
                let env = &ctx.global;
                // Fast path: already loaded.
                if let Some(cached) = env.0.loaded.get(&name) {
                    return Ok(vec![cached.clone()]);
                }
                // Look up the preload opener.
                let opener = env.0.preload.get(&name).map(|e| Arc::clone(&*e));
                let opener = opener.ok_or_else(|| VmError::HostError {
                    name: "require".to_owned(),
                    source: format!("module '{}' not found", String::from_utf8_lossy(&name)).into(),
                })?;
                let table = opener(env)?;
                let value = Value::Table(table);
                env.track_table(match &value {
                    Value::Table(t) => t,
                    _ => unreachable!(),
                });
                env.0.loaded.insert(name, value.clone());
                Ok(vec![value])
            })
        }));
    }

    /// Register a `Table` with the GC registry so it will be tracked during
    /// `collect_cycles()`.  Called from the VM on `NewTable` and when a table
    /// is stored as a global via `set_global`.
    pub(crate) fn track_table(&self, t: &crate::table::Table) {
        self.0.gc_tables.lock().push(t.0.clone());
    }

    /// Register a Lua closure with the GC registry.  Native functions are not
    /// tracked (they cannot form cycles and have no `__gc` metamethod).
    pub(crate) fn track_function(&self, f: &Function) {
        if matches!(f.state(), FunctionState::Lua(_)) {
            self.0.gc_functions.lock().push(f.0.clone());
        }
    }

    /// Set a global variable by name.
    pub fn set_global(&self, name: impl Into<Bytes>, value: Value) {
        // Track host-created tables and closures so the GC can see them.
        match &value {
            Value::Table(t) => self.track_table(t),
            Value::Function(f) => self.track_function(f),
            _ => {}
        }
        self.0.globals.insert(name.into(), value);
    }

    /// Get a global variable by name.
    pub fn get_global(&self, name: impl AsRef<[u8]>) -> Option<Value> {
        self.0.globals.get::<[u8]>(name.as_ref()).map(|v| v.clone())
    }

    /// Set the shared metatable used for all string values.
    ///
    /// The VM consults this metatable's `__index` when a `GetTable`
    /// instruction encounters a `Value::String`, enabling method-call
    /// syntax like `("hello"):upper()`.
    pub fn set_string_metatable(&self, mt: Table) {
        *self.0.string_metatable.write() = Some(mt);
    }

    /// Return the shared string metatable, if one has been set.
    pub fn get_string_metatable(&self) -> Option<Table> {
        self.0.string_metatable.read().clone()
    }

    /// Install every key/value pair from `table` as a global.  String keys
    /// become global names; non-string keys are silently skipped.
    pub fn register_from_table(&self, table: &Table) -> Result<(), VmError> {
        let mut key = Value::Nil;
        loop {
            match table.next(&key)? {
                Some((k, v)) => {
                    if let Value::String(name) = &k {
                        self.set_global(name.clone(), v);
                    }
                    key = k;
                }
                None => break,
            }
        }
        Ok(())
    }

    /// Register a native function as a global.
    pub fn register_native(&self, func: NativeFunction) {
        let name = func.signature.name.clone();
        let func = Arc::new(func);
        self.0.globals.insert(
            name.clone(),
            Value::Function(crate::function::Function::native((*func).clone())),
        );
        self.0.natives.insert(name, func);
    }

    /// Register a module opener in the preload registry.
    ///
    /// When `require("name")` is called and the module is not yet in the
    /// loaded cache, the opener is called with the current `GlobalEnv` and
    /// its return value is cached and returned to Lua.
    pub fn register_preload(
        &self,
        name: impl Into<Bytes>,
        opener: impl Fn(&GlobalEnv) -> Result<crate::table::Table, VmError> + Send + Sync + 'static,
    ) {
        self.0.preload.insert(name.into(), Arc::new(opener));
    }

    /// Create a task that calls the named global function with the given args.
    pub fn task(&self, function: &str, args: Vec<Value>) -> Result<Task, VmError> {
        let name = Bytes::copy_from_slice(function.as_bytes());
        let func = self
            .0
            .globals
            .get(&name)
            .map(|v| v.clone())
            .ok_or_else(|| VmError::CallNonFunction {
                type_name: "nil",
                name: None,
            })?;
        match func {
            Value::Function(f) => Ok(Task::new(self.clone(), f, args)),
            other => Err(VmError::CallNonFunction {
                type_name: other.type_name(),
                name: None,
            }),
        }
    }

    /// Drain the queue of `(table, __gc_function)` pairs that were found
    /// during the last `collect_cycles()` pass.  The caller is responsible
    /// for calling each finalizer (e.g. via `CallContext::call_function`).
    #[doc(hidden)]
    pub fn take_pending_finalizers(&self) -> Vec<(Table, Function)> {
        std::mem::take(&mut *self.0.pending_finalizers.lock())
    }

    /// Run a full mark-and-sweep cycle-collection pass.
    ///
    /// After this call, tables that were unreachable and have a `__gc`
    /// metamethod are queued in `pending_finalizers`.  The caller is
    /// responsible for draining that queue (e.g. `collectgarbage("collect")`
    /// does so inline; `dispose()` does it at shutdown).
    pub fn collect_cycles(&self) {
        // ---------------------------------------------------------------
        // Phase 1 — Reset all tracked objects to White.
        // ---------------------------------------------------------------
        {
            let tables = self.0.gc_tables.lock();
            for t in tables.iter() {
                t.gc.set_color(GcColor::White);
            }
        }
        {
            let funcs = self.0.gc_functions.lock();
            for f in funcs.iter() {
                if let FunctionState::Lua(lfs) = f.as_ref() {
                    lfs.gc.set_color(GcColor::White);
                }
            }
        }

        // ---------------------------------------------------------------
        // Phase 2 — Mark roots (globals) Gray, then scan to Black.
        // ---------------------------------------------------------------
        let mut worklist: Vec<Value> = Vec::new();
        for entry in self.0.globals.iter() {
            mark_value_gray(entry.value(), &mut worklist);
        }
        while let Some(val) = worklist.pop() {
            scan_value(&val, &mut worklist);
        }

        // ---------------------------------------------------------------
        // Phase 3 — Sweep: collect still-White objects.
        // ---------------------------------------------------------------
        // Collect White tables outside the lock to avoid holding gc_tables
        // while reading metatables (which may acquire their own locks).
        //
        // Hybrid reachability: a table is garbage only when BOTH:
        //  (a) it is White (not reachable from the global variable table), AND
        //  (b) its Arc strong_count is 1 (the registry is the only reference).
        // Condition (b) ensures that tables still live on the Lua call stack
        // (or in upvalue cells, other tables not reachable from globals, etc.)
        // are never prematurely finalised.  True cycles (White + count > 1)
        // are not collected in this pass — they are a known limitation.
        let white_tables: Vec<Arc<TableState>> = {
            let mut tables = self.0.gc_tables.lock();
            let mut white = Vec::new();
            tables.retain(|t| {
                if t.gc.color() == GcColor::White && Arc::strong_count(t) == 1 {
                    white.push(t.clone());
                    false // tentatively remove; re-add below if finalized
                } else {
                    true
                }
            });
            white
        };
        // Process each White table: queue for finalization or clear.
        let mut to_finalize: Vec<(crate::table::Table, Function)> = Vec::new();
        for t in white_tables {
            let gc_fn = {
                let inner = t.inner.read();
                inner.metatable.as_ref().and_then(|mt| {
                    let key = Value::String(Bytes::from_static(b"__gc"));
                    mt.raw_get(&key).ok().filter(|v| !v.is_nil())
                })
            };
            if let Some(Value::Function(f)) = gc_fn {
                // Resurrect the table and scan the finalizer function and all
                // objects it can reach (its upvalues, etc.) so the functions
                // sweep doesn't clear the shared upvalue cells.
                t.gc.set_color(GcColor::Black);
                let mut worklist2: Vec<Value> = Vec::new();
                mark_value_gray(&Value::Function(f.clone()), &mut worklist2);
                while let Some(v) = worklist2.pop() {
                    scan_value(&v, &mut worklist2);
                }
                to_finalize.push((crate::table::Table(t.clone()), f));
                // Put it back in the registry so a future cycle can see it.
                self.0.gc_tables.lock().push(t);
            } else {
                // No finalizer: break cycles by clearing table contents.
                let mut inner = t.inner.write();
                inner.array.clear();
                inner.hash.clear();
                inner.metatable = None;
                // Drop the Arc at end of loop body — if this was the last
                // strong reference the storage is freed.
            }
        }
        // Queue finalizers (outside the gc_tables lock).
        self.0.pending_finalizers.lock().extend(to_finalize);
        {
            let mut funcs = self.0.gc_functions.lock();
            funcs.retain(|f| {
                if let FunctionState::Lua(lfs) = f.as_ref() {
                    // Same hybrid condition as for tables.
                    if lfs.gc.color() == GcColor::White && Arc::strong_count(f) == 1 {
                        // Break upvalue cycles.
                        for cell in &lfs.upvalues {
                            *cell.write() = Value::Nil;
                        }
                        return false;
                    }
                }
                true
            });
        }
    }

    /// Graceful shutdown: finalize all GC-tracked objects in async context.
    ///
    /// Two-phase collect:
    ///
    /// **Phase A** — run while globals are still intact:
    /// collect objects that are already unreachable (not rooted in globals).
    /// Their `__gc` finalizers can call any global function.
    ///
    /// **Phase B** — after clearing globals:
    /// collect objects that were only kept alive through globals.  Their
    /// `__gc` finalizers cannot call global functions (globals are gone), but
    /// they can still release external resources.
    pub async fn dispose(&self) {
        // Phase A: drain any finalizers queued by earlier explicit collects,
        // then collect objects that are already unreachable while globals are
        // still accessible to finalizers.
        self.run_pending_finalizers().await;
        self.collect_cycles();
        self.run_pending_finalizers().await;

        // Phase B: release global references, then collect and finalize the
        // objects that were globally-rooted.
        self.0.globals.clear();
        self.collect_cycles();
        self.run_pending_finalizers().await;
    }

    /// Drain `pending_finalizers` and call each `__gc` function.
    async fn run_pending_finalizers(&self) {
        // Drain the queue into a local vec first to release the lock.
        let queue: Vec<(crate::table::Table, Function)> =
            std::mem::take(&mut *self.0.pending_finalizers.lock());
        for (table, gc_fn) in queue {
            let task = Task::new(self.clone(), gc_fn, vec![Value::Table(table)]);
            let _ = task.await;
        }
    }
}

impl Default for GlobalEnv {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// GC helpers
// ---------------------------------------------------------------------------

/// If `v` is a Table or Lua Function that is currently White, turn it Gray
/// and push it onto the worklist for scanning.
fn mark_value_gray(v: &Value, worklist: &mut Vec<Value>) {
    match v {
        Value::Table(t) => {
            if t.0.gc.color() == GcColor::White {
                t.0.gc.set_color(GcColor::Gray);
                worklist.push(v.clone());
            }
        }
        Value::Function(f) => {
            if let FunctionState::Lua(lfs) = f.state() {
                if lfs.gc.color() == GcColor::White {
                    lfs.gc.set_color(GcColor::Gray);
                    worklist.push(v.clone());
                }
            }
        }
        _ => {}
    }
}

/// Scan a Gray object: mark all its children Gray, then turn it Black.
fn scan_value(v: &Value, worklist: &mut Vec<Value>) {
    match v {
        Value::Table(t) => {
            t.0.gc.set_color(GcColor::Black);
            let inner = t.0.inner.read();
            for child in &inner.array {
                mark_value_gray(child, worklist);
            }
            for (_, (_, child)) in &inner.hash {
                mark_value_gray(child, worklist);
            }
            if let Some(mt) = &inner.metatable {
                mark_value_gray(&Value::Table(mt.clone()), worklist);
            }
        }
        Value::Function(f) => {
            if let FunctionState::Lua(lfs) = f.state() {
                lfs.gc.set_color(GcColor::Black);
                for cell in &lfs.upvalues {
                    let child = cell.read().clone();
                    mark_value_gray(&child, worklist);
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Built-in helpers
// ---------------------------------------------------------------------------

/// Construct a minimal `NativeFunction` with the given name and a fixed
/// minimum arity (for error messages only — no runtime type checking).
fn make_native(
    name: &'static str,
    _min_args: usize,
    call: impl Fn(
            CallContext,
            Vec<Value>,
        ) -> futures::future::BoxFuture<'static, Result<Vec<Value>, VmError>>
        + Send
        + Sync
        + 'static,
) -> NativeFunction {
    NativeFunction {
        signature: Arc::new(FunctionSignature {
            name: Bytes::from_static(name.as_bytes()),
            type_params: vec![],
            params: vec![],
            variadic: true,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
        }),
        call: Arc::new(call),
    }
}

/// Convert any Lua value to a string suitable for use as an error display.
pub fn value_to_error_string(v: &Value) -> String {
    match v {
        Value::String(s) => String::from_utf8_lossy(s).into_owned(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Nil => "nil".to_owned(),
        other => format!("({} value)", other.type_name()),
    }
}

/// Run `func(args)` via the caller's `CallContext`, returning
/// `[true, results...]` on success or `[false, err_value]` on error.
async fn protected_call_ctx(
    ctx: CallContext,
    func: Function,
    args: Vec<Value>,
) -> Result<Vec<Value>, VmError> {
    match ctx.call_function(func, args).await {
        Ok(results) => {
            let mut out = Vec::with_capacity(results.len() + 1);
            out.push(Value::Boolean(true));
            out.extend(results);
            Ok(out)
        }
        Err(VmError::LuaError { value, .. }) => Ok(vec![Value::Boolean(false), value]),
        Err(e) => Ok(vec![
            Value::Boolean(false),
            Value::String(Bytes::from(e.to_string())),
        ]),
    }
}
