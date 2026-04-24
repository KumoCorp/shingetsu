use crate::valuevec;
use std::sync::Arc;

use crate::byte_string::Bytes;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};

use crate::call_context::CallContext;

use crate::error::VmError;
use crate::function::{Function, FunctionState, NativeFunction};
use crate::gc::GcColor;
use crate::proto::Proto;
use crate::table::{Table, TableState};
use crate::task::Task;
use crate::types::{
    infer_type_from_value, FunctionSignature, GlobalTypeMap, ModuleTypeInfo, ModuleTypeRegistry,
};
use crate::value::{Value, ValueVec};

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
    /// The `_ENV` table — all global variables live here.  `GetGlobal` /
    /// `SetGlobal` instructions read and write through this table.
    pub(crate) env: Table,
    /// Loaded top-level prototypes.
    #[allow(dead_code)]
    pub(crate) protos: RwLock<Vec<Arc<Proto>>>,
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
    /// Compile-time type information inferred from `set_global` values.
    /// The compiler consumes a snapshot via `GlobalEnv::global_type_map()`.
    global_types: RwLock<GlobalTypeMap>,
    /// Search path templates for file-based `require`.  `None` means
    /// file-based search is disabled (only preload and loaded caches
    /// are checked).  Populated by the embedder or CLI.
    package_path: RwLock<Option<String>>,
    /// Optional module loader for file-based `require`.  Set by the
    /// embedder to enable compiling and executing `.lua`/`.luau` files
    /// found via `package_path`.
    module_loader: RwLock<Option<Arc<dyn crate::module_loader::ModuleLoader>>>,
    /// Compile-time type info for preloaded native modules.
    /// Populated by `register_preload` when the caller provides type info.
    preload_types: DashMap<Bytes, ModuleTypeInfo>,
}

impl GlobalEnv {
    pub fn new() -> Self {
        let env = GlobalEnv(Arc::new(GlobalEnvInner {
            env: Table::new(),
            protos: RwLock::new(Vec::new()),
            preload: DashMap::new(),
            loaded: DashMap::new(),
            gc_tables: Mutex::new(Vec::new()),
            gc_functions: Mutex::new(Vec::new()),
            pending_finalizers: Mutex::new(Vec::new()),
            string_metatable: RwLock::new(None),
            global_types: RwLock::new(GlobalTypeMap::new()),
            package_path: RwLock::new(None),
            module_loader: RwLock::new(None),
            preload_types: DashMap::new(),
        }));
        // Store a self-reference so that Lua code can read `_ENV` to get
        // the global environment table (mirrors Lua 5.4's `_ENV`).
        env.0
            .env
            .raw_set(
                Value::String(Bytes::from("_ENV")),
                Value::Table(env.0.env.clone()),
            )
            .ok();
        // `_G` is an alias for `_ENV` (same table, not a copy).
        env.0
            .env
            .raw_set(
                Value::String(Bytes::from("_G")),
                Value::Table(env.0.env.clone()),
            )
            .ok();
        env.0
            .env
            .raw_set(
                Value::String(Bytes::from("_VERSION")),
                Value::String(Bytes::from("Shingetsu dev")),
            )
            .ok();
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
        // pcall(f, ...)
        // ----------------------------------------------------------------
        self.register_native(make_native("pcall", 1, |ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let func = match it.next() {
                    Some(Value::Function(f)) => f,
                    Some(other) => {
                        return Ok(valuevec![
                            Value::Boolean(false),
                            Value::string(format!("attempt to call a {} value", other.type_name())),
                        ])
                    }
                    None => {
                        return Ok(valuevec![
                            Value::Boolean(false),
                            Value::string("bad argument #1 to 'pcall' (value expected)"),
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
                        return Ok(valuevec![
                            Value::Boolean(false),
                            Value::string(format!("attempt to call a {} value", other.type_name())),
                        ])
                    }
                    None => {
                        return Ok(valuevec![
                            Value::Boolean(false),
                            Value::string("bad argument #1 to 'xpcall' (value expected)"),
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
                        let mut out = valuevec![Value::Boolean(false)];
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
        self.register_function(Function::wrap(
            "require",
            async |ctx: CallContext, name: Bytes| -> Result<Value, VmError> {
                let env = &ctx.global;
                let name_str = std::str::from_utf8(&name).map_err(|_| VmError::HostError {
                    name: "require".to_owned(),
                    source: "module name is not valid UTF-8".into(),
                })?;

                // Fast path: already loaded.
                if let Some(cached) = env.0.loaded.get(&name) {
                    return Ok(cached.clone());
                }

                // Try the preload registry.
                if let Some(opener) = env.0.preload.get(&name).map(|e| Arc::clone(&*e)) {
                    let table = opener(env)?;
                    let value = Value::Table(table);
                    env.track_table(match &value {
                        Value::Table(t) => t,
                        _ => unreachable!(),
                    });
                    env.0.loaded.insert(name, value.clone());
                    return Ok(value);
                }

                // Try file-based search if package_path and a loader are set.
                let package_path = env.0.package_path.read().clone();
                let loader = env.0.module_loader.read().clone();
                if let (Some(path_str), Some(loader)) = (package_path, loader) {
                    let candidates = crate::module_loader::candidate_paths(name_str, &path_str);

                    if !candidates.is_empty() {
                        let mut errors: Vec<(std::path::PathBuf, String)> = Vec::new();

                        for candidate in &candidates {
                            match loader.load(name_str, candidate).await {
                                Ok(loaded) => {
                                    // Insert a sentinel into `loaded` before
                                    // execution to handle circular requires
                                    // (Lua 5.4 semantics).
                                    env.0.loaded.insert(name.clone(), Value::Boolean(true));

                                    let func = Function::lua(loaded.proto, vec![]);
                                    let task = Task::new(env.clone(), func, vec![]);
                                    let results = task.await.map_err(|re| re.error)?;
                                    let value = results.into_iter().next().unwrap_or(Value::Nil);

                                    // Replace sentinel with actual return value.
                                    env.0.loaded.insert(name, value.clone());
                                    return Ok(value);
                                }
                                Err(e) => {
                                    errors.push((candidate.clone(), e.to_string()));
                                }
                            }
                        }

                        // All candidates failed — build composite error.
                        let mut msg = format!("module '{name_str}' not found:");
                        msg.push_str(&format!("\n\tno field package.preload['{name_str}']"));
                        for (path, reason) in &errors {
                            let reason = reason.replace("error in 'require': ", "");
                            msg.push_str(&format!("\n\t{}: {reason}", path.display()));
                        }
                        return Err(VmError::HostError {
                            name: "require".to_owned(),
                            source: msg.into(),
                        });
                    }
                }

                Err(VmError::HostError {
                    name: "require".to_owned(),
                    source: format!("module '{name_str}' not found").into(),
                })
            },
        ));
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
    ///
    /// Automatically infers a [`LuaType`] from the value and stores it
    /// in the global type map.  The compiler can later consume this via
    /// [`global_type_map()`](Self::global_type_map) for compile-time
    /// diagnostics.
    ///
    /// [`LuaType`]: crate::types::LuaType
    pub fn set_global(&self, name: impl Into<Bytes>, value: Value) {
        // Track host-created tables and closures so the GC can see them.
        match &value {
            Value::Table(t) => self.track_table(t),
            Value::Function(f) => self.track_function(f),
            _ => {}
        }
        let name = name.into();
        // Infer type from the value and store alongside it.
        if let Some(ty) = infer_type_from_value(&value) {
            self.0.global_types.write().types.insert(name.clone(), ty);
        } else {
            // No meaningful type — remove any stale entry.
            self.0.global_types.write().types.remove(&name);
        }
        // raw_set on a non-frozen table with a string key cannot fail.
        self.0.env.raw_set(Value::String(name), value).ok();
    }

    /// Return a snapshot of the inferred type information for all globals.
    ///
    /// The returned [`GlobalTypeMap`] is a lightweight clone suitable for
    /// passing to the compiler as part of its `TypeContext`.
    pub fn global_type_map(&self) -> GlobalTypeMap {
        self.0.global_types.read().clone()
    }

    /// Set the search path templates for file-based `require`.
    ///
    /// Each template is separated by `;`.  Within each template, `?` is
    /// replaced by the module name (with `.` converted to the platform
    /// path separator).  Example: `"./?.lua;./?.luau"`.
    ///
    /// Pass `None` to disable file-based search (only `preload` and
    /// `loaded` caches will be consulted).
    pub fn set_package_path(&self, path: Option<String>) {
        *self.0.package_path.write() = path;
    }

    /// Return the current package search path, if set.
    pub fn package_path(&self) -> Option<String> {
        self.0.package_path.read().clone()
    }

    /// Set the module loader used by `require` for file-based loading.
    ///
    /// The loader is called when a module is not found in `preload` or
    /// `loaded` and `package_path` is set.  The `shingetsu` top-level
    /// crate provides a default loader that compiles Lua source.
    pub fn set_module_loader(&self, loader: Arc<dyn crate::module_loader::ModuleLoader>) {
        *self.0.module_loader.write() = Some(loader);
    }

    /// Return the `_ENV` table that backs all global variables.
    pub fn env_table(&self) -> Table {
        self.0.env.clone()
    }

    /// Get a global variable by name.
    pub fn get_global(&self, name: impl AsRef<[u8]>) -> Option<Value> {
        let key = Value::String(Bytes::from(name.as_ref()));
        match self.0.env.raw_get(&key) {
            Ok(Value::Nil) => None,
            Ok(v) => Some(v),
            Err(_) => None,
        }
    }

    /// Mark a module as already loaded so that `require(name)` returns
    /// the given value without searching preload or the filesystem.
    pub fn set_loaded(&self, name: impl Into<Bytes>, value: Value) {
        self.0.loaded.insert(name.into(), value);
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
        self.register_function(Function::native(func));
    }

    /// Register a [`Function`] as a global, keyed by its signature name.
    pub fn register_function(&self, func: Function) {
        let name = match func.state() {
            FunctionState::Native(n) => n.signature.name.clone(),
            FunctionState::Lua(l) => l.proto.signature.name.clone(),
        };
        self.set_global(name, Value::Function(func));
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

    /// Register a module opener together with its compile-time type info.
    ///
    /// This is the preferred form for `#[shingetsu::module]`-generated
    /// modules: the derive macro produces a `module_type()` function that
    /// builds [`ModuleTypeInfo`] statically from the function signatures,
    /// so the compiler can type-check `require`'d native modules without
    /// calling the opener.
    pub fn register_preload_typed(
        &self,
        name: impl Into<Bytes>,
        opener: impl Fn(&GlobalEnv) -> Result<crate::table::Table, VmError> + Send + Sync + 'static,
        type_info: ModuleTypeInfo,
    ) {
        let name = name.into();
        self.0.preload.insert(name.clone(), Arc::new(opener));
        self.0.preload_types.insert(name, type_info);
    }

    /// Return a [`ModuleTypeRegistry`] populated with the type info for all
    /// preloaded modules that were registered with
    /// [`register_preload_typed`](Self::register_preload_typed).
    pub fn preload_module_types(&self) -> ModuleTypeRegistry {
        let registry = ModuleTypeRegistry::default();
        for entry in self.0.preload_types.iter() {
            registry.insert(entry.key().clone(), entry.value().clone());
        }
        registry
    }

    /// Create a task that calls the named global function with the given args.
    pub fn task(&self, function: &str, args: Vec<Value>) -> Result<Task, VmError> {
        let func = self
            .get_global(function)
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
        {
            let env_inner = self.0.env.0.inner.read();
            for v in &env_inner.array {
                mark_value_gray(v, &mut worklist);
            }
            for (_, (_, v)) in &env_inner.hash {
                mark_value_gray(v, &mut worklist);
            }
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
                    let key = Value::string("__gc");
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
        self.0.env.raw_clear().ok();
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
        ) -> futures::future::BoxFuture<'static, Result<ValueVec, VmError>>
        + Send
        + Sync
        + 'static,
) -> NativeFunction {
    NativeFunction {
        signature: Arc::new(FunctionSignature {
            name: Bytes::from(name.as_bytes()),
            source: Bytes::from("=[vm]"),
            type_params: vec![],
            params: vec![],
            variadic: true,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
        }),
        call: crate::function::NativeCall::Async(Arc::new(call)),
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
) -> Result<ValueVec, VmError> {
    match ctx.call_function(func, args).await {
        Ok(results) => {
            let mut out = ValueVec::with_capacity(results.len() + 1);
            out.push(Value::Boolean(true));
            out.extend(results);
            Ok(out)
        }
        // `os.exit` raises `ExitRequested` as a one-way, non-catchable
        // signal: re-propagate it past `pcall`/`xpcall` so the embedder
        // sees it at the task boundary.  Matches reference Lua where
        // `os.exit` is a C `exit()` call that never returns to `pcall`.
        Err(re) if matches!(re.error, VmError::ExitRequested { .. }) => Err(re.error),
        Err(re) => match re.error {
            VmError::LuaError { value, .. } => Ok(valuevec![Value::Boolean(false), value]),
            e => Ok(valuevec![
                Value::Boolean(false),
                Value::string(e.to_string())
            ]),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionLuaType, LuaType, TableLuaType};

    /// `set_global` with a simple value populates the type map.
    #[test]
    fn set_global_infers_integer_type() {
        let env = GlobalEnv::new();
        env.set_global("count", Value::Integer(42));
        let map = env.global_type_map();
        k9::assert_equal!(map.get(b"count"), Some(&LuaType::Integer));
    }

    /// `set_global` with nil stores LuaType::Nil.
    #[test]
    fn set_global_infers_nil_type() {
        let env = GlobalEnv::new();
        env.set_global("x", Value::Nil);
        let map = env.global_type_map();
        k9::assert_equal!(map.get(b"x"), Some(&LuaType::Nil));
    }

    /// Overwriting a global replaces the type entry.
    #[test]
    fn set_global_overwrites_type() {
        let env = GlobalEnv::new();
        env.set_global("x", Value::Integer(1));
        k9::assert_equal!(env.global_type_map().get(b"x"), Some(&LuaType::Integer));
        env.set_global("x", Value::string("hello"));
        k9::assert_equal!(env.global_type_map().get(b"x"), Some(&LuaType::String));
    }

    /// An empty table produces no type entry.
    #[test]
    fn set_global_empty_table_no_type() {
        let env = GlobalEnv::new();
        let t = crate::table::Table::new();
        env.set_global("t", Value::Table(t));
        k9::assert_equal!(env.global_type_map().get(b"t"), None);
    }

    /// A table with function entries produces a structural Table type.
    #[test]
    fn set_global_table_with_functions() {
        let env = GlobalEnv::new();
        let t = crate::table::Table::new();
        // || Ok(Value::string("hi"))  →  no params, returns: any
        let f = Function::wrap("greet", || Ok(Value::string("hi")));
        t.raw_set(Value::string("greet"), Value::Function(f))
            .expect("set");
        env.set_global("mymod", Value::Table(t));
        let map = env.global_type_map();
        k9::assert_equal!(
            map.get(b"mymod"),
            Some(&LuaType::Table(Box::new(TableLuaType {
                fields: vec![(
                    Bytes::from("greet"),
                    LuaType::Function(Box::new(FunctionLuaType {
                        type_params: vec![],
                        params: vec![],
                        variadic: None,
                        returns: vec![LuaType::Any],
                        is_method: false,
                        inferred_unannotated: false,
                    }))
                )],
                indexer: None,
            })))
        );
    }

    /// register_function populates the type map.
    #[test]
    fn register_function_populates_type_map() {
        let env = GlobalEnv::new();
        // |x: i64| Ok(x + 1)  →  param: integer, returns: integer
        let f = Function::wrap("myfunc", |x: i64| Ok(x + 1));
        env.register_function(f);
        let map = env.global_type_map();
        k9::assert_equal!(
            map.get(b"myfunc"),
            Some(&LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![(None, LuaType::Number)],
                variadic: None,
                returns: vec![LuaType::Number],
                is_method: false,
                inferred_unannotated: false,
            })))
        );
    }

    /// Builtins registered during GlobalEnv::new() have type entries.
    #[test]
    fn builtins_have_type_entries() {
        let env = GlobalEnv::new();
        let map = env.global_type_map();
        // pcall and require are registered in register_builtins as
        // variadic functions with no typed params.
        k9::assert_equal!(
            map.get(b"pcall"),
            Some(&LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![],
                variadic: Some(Box::new(LuaType::Any)),
                returns: vec![],
                is_method: false,
                inferred_unannotated: false,
            })))
        );
        k9::assert_equal!(
            map.get(b"require"),
            Some(&LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![(None, LuaType::String)],
                variadic: None,
                returns: vec![LuaType::Any],
                is_method: false,
                inferred_unannotated: false,
            })))
        );
    }
}
