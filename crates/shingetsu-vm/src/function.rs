use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::RwLock;

use crate::call_context::CallContext;
use crate::error::VmError;
use crate::gc::GcHeader;
use crate::proto::Proto;
use crate::types::FunctionSignature;
use crate::value::Value;

/// Shared mutable cell for a captured upvalue.
pub type UpvalueCell = Arc<RwLock<Value>>;

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
    pub(crate) upvalues: Vec<UpvalueCell>,
    /// Per-closure `_ENV` override set by `load(chunk, name, mode, env)`.
    /// When present, `GetGlobal`/`SetGlobal` use this table instead of
    /// the shared `GlobalEnv.env`.
    pub(crate) env_override: Option<crate::table::Table>,
}

/// A host-provided function registered in `GlobalEnv`.
#[derive(Clone)]
pub struct NativeFunction {
    pub signature: Arc<FunctionSignature>,
    /// The implementation.  Receives the call context (global env + stack
    /// snapshot) and the argument list; returns a future of the results.
    pub call: Arc<
        dyn Fn(CallContext, Vec<Value>) -> BoxFuture<'static, Result<Vec<Value>, VmError>>
            + Send
            + Sync,
    >,
}

impl Function {
    /// Construct a Lua closure.
    pub fn lua(proto: Arc<Proto>, upvalues: Vec<UpvalueCell>) -> Self {
        Function(Arc::new(FunctionState::Lua(LuaFunctionState {
            gc: GcHeader::new(),
            proto,
            upvalues,
            env_override: None,
        })))
    }

    /// Construct a Lua closure with a custom `_ENV` table.
    ///
    /// `GetGlobal`/`SetGlobal` in this closure will use `env` instead
    /// of the shared `GlobalEnv.env`.
    pub fn lua_with_env(
        proto: Arc<Proto>,
        upvalues: Vec<UpvalueCell>,
        env: crate::table::Table,
    ) -> Self {
        Function(Arc::new(FunctionState::Lua(LuaFunctionState {
            gc: GcHeader::new(),
            proto,
            upvalues,
            env_override: Some(env),
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
    pub fn get_upvalue(&self, idx: usize) -> Option<(bytes::Bytes, Value)> {
        match &*self.0 {
            FunctionState::Lua(lf) => {
                let cell = lf.upvalues.get(idx)?;
                let name = lf
                    .proto
                    .upvalues
                    .get(idx)
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                Some((name, cell.read().clone()))
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
    pub fn set_upvalue(&self, idx: usize, value: Value) -> Option<bytes::Bytes> {
        match &*self.0 {
            FunctionState::Lua(lf) => {
                let cell = lf.upvalues.get(idx)?;
                let name = lf
                    .proto
                    .upvalues
                    .get(idx)
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                *cell.write() = value;
                Some(name)
            }
            FunctionState::Native(_) => None,
        }
    }

    pub(crate) fn state(&self) -> &FunctionState {
        &self.0
    }
}
