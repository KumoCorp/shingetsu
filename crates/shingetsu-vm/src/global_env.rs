use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::RwLock;

use crate::{
    call_context::CallContext,
    error::VmError,
    function::{Function, NativeFunction},
    proto::Proto,
    task::Task,
    types::FunctionSignature,
    value::Value,
};

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
}

impl GlobalEnv {
    pub fn new() -> Self {
        let env = GlobalEnv(Arc::new(GlobalEnvInner {
            globals: DashMap::new(),
            protos: RwLock::new(Vec::new()),
            natives: DashMap::new(),
        }));
        env.register_builtins();
        env
    }

    /// Register the core built-in functions (`error`, `assert`, `pcall`,
    /// `xpcall`).
    fn register_builtins(&self) {

        // ----------------------------------------------------------------
        // error(msg [, level])
        // ----------------------------------------------------------------
        self.register_native(make_native("error", 1, |_ctx, args| {
            Box::pin(async move {
                let msg = args.into_iter().next().unwrap_or(Value::Nil);
                let display = value_to_error_string(&msg);
                Err(VmError::LuaError { display, value: msg })
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
                    let msg = args.into_iter().nth(1).unwrap_or_else(|| {
                        Value::String(Bytes::from_static(b"assertion failed!"))
                    });
                    let display = value_to_error_string(&msg);
                    Err(VmError::LuaError { display, value: msg })
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
                    other => return Err(VmError::BadArgument {
                        position: 1,
                        function: "setmetatable".to_owned(),
                        expected: "table",
                        got: other.type_name(),
                    }),
                };
                let mt = match it.next().unwrap_or(Value::Nil) {
                    Value::Table(t) => Some(t),
                    Value::Nil => None,
                    other => return Err(VmError::BadArgument {
                        position: 2,
                        function: "setmetatable".to_owned(),
                        expected: "table or nil",
                        got: other.type_name(),
                    }),
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
                    other => return Err(VmError::BadArgument {
                        position: 1,
                        function: "rawget".to_owned(),
                        expected: "table",
                        got: other.type_name(),
                    }),
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
                    other => return Err(VmError::BadArgument {
                        position: 1,
                        function: "rawset".to_owned(),
                        expected: "table",
                        got: other.type_name(),
                    }),
                };
                let key = it.next().unwrap_or(Value::Nil);
                let val = it.next().unwrap_or(Value::Nil);
                table.raw_set(key, val)?;
                Ok(vec![Value::Table(table)])
            })
        }));

        // ----------------------------------------------------------------
        // collectgarbage([opt [, arg]])
        // Stub: the GC is not yet implemented; accept the call and return
        // sensible defaults so host code doesn't break.
        // ----------------------------------------------------------------
        self.register_native(make_native("collectgarbage", 0, |_ctx, args| {
            Box::pin(async move {
                let opt = args.first().cloned().unwrap_or_else(|| {
                    Value::String(Bytes::from_static(b"collect"))
                });
                match &opt {
                    Value::String(s) => match s.as_ref() {
                        b"count" => Ok(vec![Value::Float(0.0), Value::Float(0.0)]),
                        b"isrunning" => Ok(vec![Value::Boolean(true)]),
                        // "collect", "stop", "restart", "step", "setpause",
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
                                expected: "index out of range",
                                got: "0",
                            });
                        };
                        Ok(rest.into_iter().skip(idx).collect())
                    }
                    other => Err(VmError::BadArgument {
                        position: 1,
                        function: "select".to_owned(),
                        expected: "number or string \"#\"",
                        got: other.type_name(),
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
                    other => return Err(VmError::BadArgument {
                        position: 1,
                        function: "next".to_owned(),
                        expected: "table",
                        got: other.type_name(),
                    }),
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
                    other => return Err(VmError::BadArgument {
                        position: 1,
                        function: "pairs".to_owned(),
                        expected: "table",
                        got: other.type_name(),
                    }),
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
                    other => return Err(VmError::BadArgument {
                        position: 1,
                        function: "ipairs".to_owned(),
                        expected: "table",
                        got: other.type_name(),
                    }),
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
                    Some(other) => return Ok(vec![
                        Value::Boolean(false),
                        Value::String(Bytes::from(format!(
                            "attempt to call a {} value",
                            other.type_name()
                        ))),
                    ]),
                    None => return Ok(vec![
                        Value::Boolean(false),
                        Value::String(Bytes::from_static(
                            b"bad argument #1 to 'pcall' (value expected)"
                        )),
                    ]),
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
                    Some(other) => return Ok(vec![
                        Value::Boolean(false),
                        Value::String(Bytes::from(format!(
                            "attempt to call a {} value",
                            other.type_name()
                        ))),
                    ]),
                    None => return Ok(vec![
                        Value::Boolean(false),
                        Value::String(Bytes::from_static(
                            b"bad argument #1 to 'xpcall' (value expected)"
                        )),
                    ]),
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
                        let handler_result =
                            protected_call_ctx(ctx, h, vec![err_val]).await?;
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

    /// Set a global variable by name.
    pub fn set_global(&self, name: impl Into<Bytes>, value: Value) {
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
        self.0
            .globals
            .insert(name.clone(), Value::Function(crate::function::Function::native((*func).clone())));
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

    /// Run a cycle-collection pass over `Table` and `Function` values.
    /// Phase 1 stub — no cycle tracking yet.
    pub fn collect_cycles(&self) {
        // Phase 3 will implement GcHeader and tri-color mark-and-sweep.
    }
}

impl Default for GlobalEnv {
    fn default() -> Self {
        Self::new()
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
    call: impl Fn(CallContext, Vec<Value>) -> futures::future::BoxFuture<'static, Result<Vec<Value>, VmError>>
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
