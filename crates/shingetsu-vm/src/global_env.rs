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
