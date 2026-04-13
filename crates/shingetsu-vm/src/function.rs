use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::RwLock;

use crate::{
    error::VmError,
    proto::Proto,
    types::FunctionSignature,
    value::Value,
};

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
    pub(crate) proto: Arc<Proto>,
    pub(crate) upvalues: Vec<UpvalueCell>,
}

/// A host-provided function registered in `GlobalEnv`.
#[derive(Clone)]
pub struct NativeFunction {
    pub signature: Arc<FunctionSignature>,
    /// The implementation.  Receives arguments and returns results.
    pub call: Arc<
        dyn Fn(Vec<Value>) -> BoxFuture<'static, Result<Vec<Value>, VmError>>
            + Send
            + Sync,
    >,
}

impl Function {
    /// Construct a Lua closure.
    pub fn lua(proto: Arc<Proto>, upvalues: Vec<UpvalueCell>) -> Self {
        Function(Arc::new(FunctionState::Lua(LuaFunctionState {
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

    pub(crate) fn state(&self) -> &FunctionState {
        &self.0
    }
}
