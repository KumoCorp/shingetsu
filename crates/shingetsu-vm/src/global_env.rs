use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};

use crate::call_context::CallContext;
use crate::error::VmError;
use crate::function::{Function, FunctionState, NativeFunction};
use crate::gc::GcColor;
use crate::proto::Proto;
use crate::table::TableState;
use crate::task::Task;
use crate::types::FunctionSignature;
use crate::value::Value;

/// Shared compiled environment.  Cheap to clone (Arc-backed).
/// `Send + Sync`; safe to share across threads and async tasks.
#[derive(Clone)]
pub struct GlobalEnv(pub(crate) Arc<GlobalEnvInner>);

pub(crate) struct GlobalEnvInner {
    /// Global variable table.  Fine-grained sharded locking: concurrent
    /// readers never block each other; a write only locks the relevant shard.
    pub(crate) globals: DashMap<Bytes, Value>,
    /// Loaded top-level prototypes.
    #[allow(dead_code)]
    pub(crate) protos: RwLock<Vec<Arc<Proto>>>,
    /// Registered native functions (also inserted into `globals`).
    pub(crate) natives: DashMap<Bytes, Arc<NativeFunction>>,
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
}

impl GlobalEnv {
    pub fn new() -> Self {
        let env = GlobalEnv(Arc::new(GlobalEnvInner {
            globals: DashMap::new(),
            protos: RwLock::new(Vec::new()),
            natives: DashMap::new(),
            gc_tables: Mutex::new(Vec::new()),
            gc_functions: Mutex::new(Vec::new()),
            pending_finalizers: Mutex::new(Vec::new()),
        }));
        env.register_builtins();
        env
    }

    /// Register the core built-in functions (`error`, `assert`, `pcall`,
    /// `xpcall`).
    fn register_builtins(&self) {
        // ----------------------------------------------------------------
        // error(msg [, level])
        // level 1 (default) = position of the caller; 2 = caller's caller;
        // 0 = no position info.
        // ----------------------------------------------------------------
        self.register_native(make_native("error", 1, |ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let msg = it.next().unwrap_or(Value::Nil);
                let level = it
                    .next()
                    .and_then(|v| match v {
                        Value::Integer(n) => Some(n as usize),
                        Value::Float(f) => Some(f as usize),
                        _ => None,
                    })
                    .unwrap_or(1);

                // Prepend "source:line: " to string messages when level > 0.
                let (display, value) = if level > 0 {
                    if let Value::String(ref s) = msg {
                        let stack = &ctx.call_stack;
                        // Level 1 = last Lua frame in the stack.
                        let lua_frames: Vec<_> = stack
                            .iter()
                            .filter(|f| matches!(f, crate::call_context::StackFrame::Lua { .. }))
                            .collect();
                        let loc = lua_frames.len().checked_sub(level).and_then(|i| {
                            if let crate::call_context::StackFrame::Lua {
                                source_location, ..
                            } = lua_frames[i]
                            {
                                source_location.as_ref()
                            } else {
                                None
                            }
                        });
                        if let Some(loc) = loc {
                            let prefixed = Bytes::from(format!(
                                "{}:{}: {}",
                                loc.source_name,
                                loc.line,
                                String::from_utf8_lossy(s.as_ref())
                            ));
                            let display = String::from_utf8_lossy(&prefixed).into_owned();
                            let value = Value::String(prefixed);
                            (display, value)
                        } else {
                            (value_to_error_string(&msg), msg)
                        }
                    } else {
                        (value_to_error_string(&msg), msg)
                    }
                } else {
                    (value_to_error_string(&msg), msg)
                };
                Err(VmError::LuaError { display, value })
            })
        }));

        // ----------------------------------------------------------------
        // assert(v [, msg, ...])
        // ----------------------------------------------------------------
        self.register_native(make_native("assert", 1, |_ctx, args| {
            Box::pin(async move {
                let v = args.first().cloned().unwrap_or(Value::Nil);
                if v.is_truthy() {
                    // Return all arguments on success.
                    Ok(args)
                } else {
                    let msg = args
                        .into_iter()
                        .nth(1)
                        .unwrap_or_else(|| Value::String(Bytes::from_static(b"assertion failed!")));
                    let display = value_to_error_string(&msg);
                    Err(VmError::LuaError {
                        display,
                        value: msg,
                    })
                }
            })
        }));

        // ----------------------------------------------------------------
        // setmetatable(table, metatable)
        // ----------------------------------------------------------------
        self.register_native(make_native("setmetatable", 2, |_ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let table = match it.next().unwrap_or(Value::Nil) {
                    Value::Table(t) => t,
                    other => {
                        return Err(VmError::BadArgument {
                            position: 1,
                            function: "setmetatable".to_owned(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        })
                    }
                };
                let mt = match it.next().unwrap_or(Value::Nil) {
                    Value::Table(t) => Some(t),
                    Value::Nil => None,
                    other => {
                        return Err(VmError::BadArgument {
                            position: 2,
                            function: "setmetatable".to_owned(),
                            expected: "table or nil".to_owned(),
                            got: other.type_name().to_owned(),
                        })
                    }
                };
                table.set_metatable(mt);
                Ok(vec![Value::Table(table)])
            })
        }));

        // ----------------------------------------------------------------
        // getmetatable(object)
        // ----------------------------------------------------------------
        self.register_native(make_native("getmetatable", 1, |_ctx, args| {
            Box::pin(async move {
                let obj = args.into_iter().next().unwrap_or(Value::Nil);
                match obj {
                    Value::Table(t) => {
                        // Respect __metatable field: if the metatable has a
                        // __metatable key, return that value instead (Lua 5.2+
                        // protection mechanism).
                        match t.get_metamethod(b"__metatable") {
                            Some(guard) => Ok(vec![guard]),
                            None => match t.get_metatable() {
                                Some(mt) => Ok(vec![Value::Table(mt)]),
                                None => Ok(vec![Value::Nil]),
                            },
                        }
                    }
                    _ => Ok(vec![Value::Nil]),
                }
            })
        }));

        // ----------------------------------------------------------------
        // rawget(table, key)
        // ----------------------------------------------------------------
        self.register_native(make_native("rawget", 2, |_ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let table = match it.next().unwrap_or(Value::Nil) {
                    Value::Table(t) => t,
                    other => {
                        return Err(VmError::BadArgument {
                            position: 1,
                            function: "rawget".to_owned(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        })
                    }
                };
                let key = it.next().unwrap_or(Value::Nil);
                Ok(vec![table.raw_get(&key)?])
            })
        }));

        // ----------------------------------------------------------------
        // rawset(table, key, value)
        // ----------------------------------------------------------------
        self.register_native(make_native("rawset", 3, |_ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let table = match it.next().unwrap_or(Value::Nil) {
                    Value::Table(t) => t,
                    other => {
                        return Err(VmError::BadArgument {
                            position: 1,
                            function: "rawset".to_owned(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        })
                    }
                };
                let key = it.next().unwrap_or(Value::Nil);
                let val = it.next().unwrap_or(Value::Nil);
                table.raw_set(key, val)?;
                Ok(vec![Value::Table(table)])
            })
        }));

        // ----------------------------------------------------------------
        // collectgarbage([opt [, arg]])
        // ----------------------------------------------------------------
        self.register_native(make_native("collectgarbage", 0, |ctx, args| {
            Box::pin(async move {
                let opt = args
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Value::String(Bytes::from_static(b"collect")));
                match &opt {
                    Value::String(s) => match s.as_ref() {
                        b"collect" => {
                            // Synchronous mark-and-sweep.
                            ctx.global.collect_cycles();
                            // Run any __gc finalizers found during sweep.
                            let queue: Vec<_> =
                                std::mem::take(&mut *ctx.global.0.pending_finalizers.lock());
                            for (table, gc_fn) in queue {
                                let _ = ctx.call_function(gc_fn, vec![Value::Table(table)]).await;
                            }
                            Ok(vec![Value::Integer(0)])
                        }
                        b"count" => Ok(vec![Value::Float(0.0), Value::Float(0.0)]),
                        b"isrunning" => Ok(vec![Value::Boolean(true)]),
                        // "stop", "restart", "step", "setpause",
                        // "setstepmul", "incremental", "generational" → 0
                        _ => Ok(vec![Value::Integer(0)]),
                    },
                    _ => Ok(vec![Value::Integer(0)]),
                }
            })
        }));

        // ----------------------------------------------------------------
        // select(index, ...)
        // ----------------------------------------------------------------
        self.register_native(make_native("select", 1, |_ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let index = it.next().unwrap_or(Value::Nil);
                let rest: Vec<Value> = it.collect();
                match index {
                    Value::String(s) if s.as_ref() == b"#" => {
                        Ok(vec![Value::Integer(rest.len() as i64)])
                    }
                    Value::Integer(n) => {
                        let len = rest.len() as i64;
                        let idx = if n < 0 {
                            (len + n).max(0) as usize
                        } else if n >= 1 {
                            (n - 1) as usize
                        } else {
                            return Err(VmError::BadArgument {
                                position: 1,
                                function: "select".to_owned(),
                                expected: "index out of range".to_owned(),
                                got: "0".to_owned(),
                            });
                        };
                        Ok(rest.into_iter().skip(idx).collect())
                    }
                    other => Err(VmError::BadArgument {
                        position: 1,
                        function: "select".to_owned(),
                        expected: "number or string \"#\"".to_owned(),
                        got: other.type_name().to_owned(),
                    }),
                }
            })
        }));

        // ----------------------------------------------------------------
        // type(v)
        // ----------------------------------------------------------------
        self.register_native(make_native("type", 1, |_ctx, args| {
            Box::pin(async move {
                let v = args.into_iter().next().unwrap_or(Value::Nil);
                let t: &[u8] = match &v {
                    Value::Nil => b"nil",
                    Value::Boolean(_) => b"boolean",
                    Value::Integer(_) | Value::Float(_) => b"number",
                    Value::String(_) => b"string",
                    Value::Table(_) => b"table",
                    Value::Function(_) => b"function",
                    Value::Userdata(_) => b"userdata",
                };
                Ok(vec![Value::String(Bytes::from_static(t))])
            })
        }));

        // ----------------------------------------------------------------
        // tostring(v)
        // ----------------------------------------------------------------
        self.register_native(make_native("tostring", 1, |ctx, args| {
            Box::pin(async move {
                let v = args.into_iter().next().unwrap_or(Value::Nil);
                // Check __tostring metamethod on tables.
                if let Value::Table(ref t) = v {
                    if let Some(Value::Function(mm)) = t.get_metamethod(b"__tostring") {
                        return ctx.call_function(mm, vec![v]).await;
                    }
                }
                // Dispatch __tostring on userdata via its dispatch mechanism.
                if let Value::Userdata(ref ud) = v {
                    return Arc::clone(ud).dispatch(ctx, "__tostring", vec![v]).await;
                }
                Ok(vec![Value::String(Bytes::from(v.to_string()))])
            })
        }));

        // ----------------------------------------------------------------
        // tonumber(v [, base])
        // ----------------------------------------------------------------
        self.register_native(make_native("tonumber", 1, |_ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let v = it.next().unwrap_or(Value::Nil);
                let base_arg = it.next();
                match base_arg {
                    Some(Value::Integer(base)) if base >= 2 && base <= 36 => {
                        let s = match &v {
                            Value::String(s) => s.clone(),
                            _ => return Ok(vec![Value::Nil]),
                        };
                        let s_str = String::from_utf8_lossy(&s);
                        match i64::from_str_radix(s_str.trim(), base as u32) {
                            Ok(n) => Ok(vec![Value::Integer(n)]),
                            Err(_) => Ok(vec![Value::Nil]),
                        }
                    }
                    None | Some(Value::Nil) => match &v {
                        Value::Integer(n) => Ok(vec![Value::Integer(*n)]),
                        Value::Float(f) => Ok(vec![Value::Float(*f)]),
                        Value::String(s) => {
                            let trimmed = String::from_utf8_lossy(s);
                            let trimmed = trimmed.trim();
                            if let Ok(n) = trimmed.parse::<i64>() {
                                Ok(vec![Value::Integer(n)])
                            } else if let Ok(f) = trimmed.parse::<f64>() {
                                Ok(vec![Value::Float(f)])
                            } else {
                                Ok(vec![Value::Nil])
                            }
                        }
                        _ => Ok(vec![Value::Nil]),
                    },
                    _ => Ok(vec![Value::Nil]),
                }
            })
        }));

        // ----------------------------------------------------------------
        // next(table [, key])
        // ----------------------------------------------------------------
        self.register_native(make_native("next", 1, |_ctx, args| {
            Box::pin(async move {
                let mut it = args.into_iter();
                let table = match it.next().unwrap_or(Value::Nil) {
                    Value::Table(t) => t,
                    other => {
                        return Err(VmError::BadArgument {
                            position: 1,
                            function: "next".to_owned(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        })
                    }
                };
                let key = it.next().unwrap_or(Value::Nil);
                match table.next(&key)? {
                    Some((k, v)) => Ok(vec![k, v]),
                    None => Ok(vec![Value::Nil]),
                }
            })
        }));

        // ----------------------------------------------------------------
        // pairs(table)
        // Returns (next, table, nil) for use with generic for.
        // Respects __pairs metamethod (Lua 5.2).
        // ----------------------------------------------------------------
        self.register_native(make_native("pairs", 1, |ctx, args| {
            Box::pin(async move {
                let table = match args.into_iter().next().unwrap_or(Value::Nil) {
                    Value::Table(t) => t,
                    other => {
                        return Err(VmError::BadArgument {
                            position: 1,
                            function: "pairs".to_owned(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        })
                    }
                };
                // Lua 5.2: if __pairs is defined on the table's metatable,
                // call it with the table and return its results directly.
                if let Some(Value::Function(mm)) = table.get_metamethod(b"__pairs") {
                    return ctx.call_function(mm, vec![Value::Table(table)]).await;
                }
                let next_fn = ctx.global.get_global(b"next").unwrap_or(Value::Nil);
                Ok(vec![next_fn, Value::Table(table), Value::Nil])
            })
        }));

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
                if let Some(Value::Function(mm)) = table.get_metamethod(b"__ipairs") {
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
    pub fn get_global(&self, name: &[u8]) -> Option<Value> {
        self.0.globals.get::<[u8]>(name).map(|v| v.clone())
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

    /// Create a task that calls the named global function with the given args.
    pub fn task(&self, function: &str, args: Vec<Value>) -> Result<Task, VmError> {
        let name = Bytes::copy_from_slice(function.as_bytes());
        let func = self
            .0
            .globals
            .get(&name)
            .map(|v| v.clone())
            .ok_or_else(|| VmError::CallNonFunction { type_name: "nil" })?;
        match func {
            Value::Function(f) => Ok(Task::new(self.clone(), f, args)),
            other => Err(VmError::CallNonFunction {
                type_name: other.type_name(),
            }),
        }
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
            returns: None,
            lua_returns: None,
        }),
        call: Arc::new(call),
    }
}

/// Convert any Lua value to a string suitable for use as an error display.
fn value_to_error_string(v: &Value) -> String {
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
