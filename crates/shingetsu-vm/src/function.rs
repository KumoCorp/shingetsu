use std::sync::Arc;

use futures::future::BoxFuture;

use crate::call_context::CallContext;
use crate::call_stack::FrameLocals;
use crate::error::VmError;
use crate::gc::GcHeader;
use crate::proto::Proto;
use crate::types::FunctionSignature;
use crate::upvalue::UpvalueCell;
use crate::value::{Value, ValueVec};

/// A Lua function value — either a compiled Lua closure or a host native.
#[derive(Clone)]
pub struct Function(pub(crate) Arc<FunctionState>);

pub(crate) enum FunctionState {
    Lua(LuaFunctionState),
    Native(NativeFunction),
}

pub(crate) struct LuaFunctionState {
    /// GC tri-colour header.
    pub(crate) gc: GcHeader,
    pub(crate) proto: Arc<Proto>,
    /// The closure's upvalue cells.  Slot 0 is conventionally the
    /// `_ENV` upvalue — the table that `GetGlobal`/`SetGlobal` read
    /// and write through.  For top-level chunks this is populated by
    /// `Task::new` (defaulting to `GlobalEnv._G`) or by
    /// `Function::lua_with_env` (when `load(..., env)` supplied an
    /// explicit env table).  Nested closures inherit slot 0 from
    /// their parent's slot 0 via the normal upvalue capture pipeline.
    pub(crate) upvalues: Vec<UpvalueCell>,
}

/// Backing closure for [`NativeCall::SyncPlain`].
type SyncPlainFn =
    Arc<dyn Fn(&crate::global_env::GlobalEnv, &[Value]) -> Result<ValueVec, VmError> + Send + Sync>;
/// Backing closure for [`NativeCall::SyncWithCtx`].
type SyncWithCtxFn = Arc<dyn Fn(CallContext, &[Value]) -> Result<ValueVec, VmError> + Send + Sync>;
/// Backing closure for [`NativeCall::SyncWithLocals`].
type SyncWithLocalsFn =
    Arc<dyn Fn(CallContext, FrameLocals, &[Value]) -> Result<ValueVec, VmError> + Send + Sync>;
/// Backing closure for [`NativeCall::Async`].
type AsyncFn = Arc<
    dyn Fn(CallContext, ValueVec) -> BoxFuture<'static, Result<ValueVec, VmError>> + Send + Sync,
>;
/// Backing closure for [`NativeCall::AsyncWithLocals`].
type AsyncWithLocalsFn = Arc<
    dyn Fn(CallContext, FrameLocals, ValueVec) -> BoxFuture<'static, Result<ValueVec, VmError>>
        + Send
        + Sync,
>;

/// Dispatch enum for native function implementations.
///
/// `SyncPlain` functions do not receive a `CallContext`, so the VM
/// skips the expensive call-stack snapshot entirely.  `SyncWithCtx`
/// functions are called inline but receive a `CallContext`.  `Async`
/// functions receive owned arguments and return a boxed future.
#[derive(Clone)]
pub enum NativeCall {
    /// Synchronous, no `CallContext` — cheapest path.
    SyncPlain(SyncPlainFn),
    /// Synchronous, receives `CallContext`.
    SyncWithCtx(SyncWithCtxFn),
    /// Synchronous, receives `CallContext` and `FrameLocals`.
    SyncWithLocals(SyncWithLocalsFn),
    /// Asynchronous — yields a future.
    Async(AsyncFn),
    /// Asynchronous with access to local variables in the call stack.
    /// Used by debug introspection functions like `debug.getlocal`.
    AsyncWithLocals(AsyncWithLocalsFn),
}

/// A host-provided function registered in `GlobalEnv`.
#[derive(Clone)]
pub struct NativeFunction {
    pub signature: Arc<FunctionSignature>,
    /// The implementation — either synchronous (called inline) or
    /// asynchronous (yields a future).
    pub call: NativeCall,
}

impl Function {
    /// Construct a Lua closure.
    ///
    /// `upvalues` is interpreted as the closure's full upvalue list,
    /// matching the layout of `proto.upvalues` exactly — including
    /// the synthetic `_ENV` slot indicated by `proto.env_upvalue_idx`
    /// when it is `Some`.  Two embedder paths supply that slot:
    ///
    /// * Top-level chunks created via `Task::new` (or its
    ///   `new_with_parent` variant) where the caller passed an empty
    ///   `upvalues`: `Task::new` synthesises an `_ENV` cell pointing
    ///   at `GlobalEnv._G`.  This is the convenience path used by
    ///   the CLI, tests, and benchmarks.
    /// * Nested closures created at runtime by the `NewClosure`
    ///   opcode: each upvalue desc is honoured, so the parent's env
    ///   cell flows down via the normal capture pipeline.
    ///
    /// Embedders that build a closure value to call into directly
    /// (without going through `Task::new`) and that expect free-name
    /// access to work should use [`Function::lua_with_env`] instead.
    pub fn lua(proto: Arc<Proto>, upvalues: Vec<UpvalueCell>) -> Self {
        Function(Arc::new(FunctionState::Lua(LuaFunctionState {
            gc: GcHeader::new(),
            proto,
            upvalues,
        })))
    }

    /// Construct a Lua closure with a custom `_ENV` table.
    ///
    /// `env` is installed at the proto's declared `_ENV` upvalue slot
    /// (`proto.env_upvalue_idx`), so all `GetGlobal`/`SetGlobal`
    /// instructions in this closure read and write through it.  Free
    /// names in the chunk are subject to `__index`/`__newindex`
    /// metamethods on `env` exactly the same way `_ENV.foo` would be:
    /// e.g. an `env` constructed as
    /// `setmetatable({}, {__index = real_g})` produces a sandbox
    /// chunk that reads from the host's globals but writes locally.
    ///
    /// When the proto doesn't reference any free name
    /// (`env_upvalue_idx == None`), `env` is silently ignored — there
    /// are no `GetGlobal`/`SetGlobal` instructions to consult it, and
    /// reading `_ENV` in the source would have already resolved to a
    /// regular upvalue or local.
    ///
    /// `upvalues` is treated as the closure's prefix-of-upvalue-list
    /// for any non-`_ENV` slots; this method pads with `nil` cells if
    /// needed and inserts/overwrites the env cell at the declared
    /// slot.  In practice top-level chunks (the only case Shingetsu
    /// emits where `env_upvalue_idx == Some(0)` and there are no
    /// other captures) take an empty `upvalues` argument.
    pub fn lua_with_env(
        proto: Arc<Proto>,
        mut upvalues: Vec<UpvalueCell>,
        env: crate::table::Table,
    ) -> Self {
        if let Some(idx) = proto.env_upvalue_idx {
            let idx = idx as usize;
            while upvalues.len() < idx {
                upvalues.push(Arc::new(crate::upvalue::UpvalueInner::new_closed(
                    Value::Nil,
                )));
            }
            let cell = Arc::new(crate::upvalue::UpvalueInner::new_closed(Value::Table(env)));
            if upvalues.len() == idx {
                upvalues.push(cell);
            } else {
                upvalues[idx] = cell;
            }
        }
        Function(Arc::new(FunctionState::Lua(LuaFunctionState {
            gc: GcHeader::new(),
            proto,
            upvalues,
        })))
    }

    /// Construct a native function value.
    pub fn native(native: NativeFunction) -> Self {
        Function(Arc::new(FunctionState::Native(native)))
    }

    /// Identity comparison (same underlying allocation).
    pub fn ptr_eq(&self, other: &Function) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Return the function's signature (name, params, returns, etc.).
    pub fn signature(&self) -> &Arc<FunctionSignature> {
        match &*self.0 {
            FunctionState::Lua(lf) => &lf.proto.signature,
            FunctionState::Native(nf) => &nf.signature,
        }
    }

    /// Read the upvalue at 0-based `idx`.
    ///
    /// Returns `(name, current_value)` or `None` when `idx` is out of range.
    /// Native functions have no upvalues and always return `None`.
    pub fn get_upvalue(&self, idx: usize) -> Option<(crate::byte_string::Bytes, Value)> {
        match &*self.0 {
            FunctionState::Lua(lf) => {
                let cell = lf.upvalues.get(idx)?;
                let name = lf
                    .proto
                    .upvalues
                    .get(idx)
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                // Safety: upvalue cells on a Function are always in the
                // Closed state (they were closed when the creating frame
                // exited, or were created closed for the _ENV upvalue).
                Some((name, unsafe { cell.read() }))
            }
            FunctionState::Native(_) => None,
        }
    }

    /// Return an opaque identity token for the upvalue at 0-based `idx`.
    ///
    /// Two closures sharing the same upvalue cell will return the same
    /// value.  Returns `None` when `idx` is out of range or for native
    /// functions (which have no upvalues).
    pub fn upvalue_id(&self, idx: usize) -> Option<i64> {
        match &*self.0 {
            FunctionState::Lua(lf) => {
                let cell = lf.upvalues.get(idx)?;
                Some(Arc::as_ptr(cell) as i64)
            }
            FunctionState::Native(_) => None,
        }
    }

    /// Set the upvalue at 0-based `idx` to `value`.
    ///
    /// Returns the upvalue name on success, or `None` when `idx` is out
    /// of range.  Native functions always return `None`.
    pub fn set_upvalue(&self, idx: usize, value: Value) -> Option<crate::byte_string::Bytes> {
        match &*self.0 {
            FunctionState::Lua(lf) => {
                let cell = lf.upvalues.get(idx)?;
                let name = lf
                    .proto
                    .upvalues
                    .get(idx)
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                // Safety: upvalue cells on a Function are always in the
                // Closed state (they were closed when the creating frame
                // exited, or were created closed for the _ENV upvalue).
                unsafe { cell.write(value) };
                Some(name)
            }
            FunctionState::Native(_) => None,
        }
    }

    pub(crate) fn state(&self) -> &FunctionState {
        &self.0
    }
}
