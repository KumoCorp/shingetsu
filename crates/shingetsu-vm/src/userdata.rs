use std::sync::Arc;

use crate::{error::VmError, value::Value};

/// Trait implemented by host-provided Rust objects exposed to Lua.
///
/// All metamethod calls are async so that getters, setters, and metamethods
/// can dispatch to async operations (database reads, network calls, etc.)
/// without the VM needing to know whether the implementation is sync or async.
///
/// Arbitrary metamethod names are supported through the single `dispatch`
/// entry point.  Standard names (`__index`, `__add`, etc.) and any
/// host-defined custom names are handled uniformly.
///
/// The `Arc<Self>` receiver on `dispatch` ensures the produced future is
/// `'static` so it can be stored across yield points without lifetime
/// complications.
#[async_trait::async_trait]
pub trait Userdata: Send + Sync {
    /// The name shown in error messages and stack traces.
    fn type_name(&self) -> &'static str;

    /// Dispatch a metamethod call.
    ///
    /// `metamethod` is the full name, e.g. `"__index"`, `"__add"`, or any
    /// arbitrary host-defined name.
    ///
    /// `args` contains the arguments in standard Lua order:
    /// - `__index`:    `[receiver, key]`
    /// - `__newindex`: `[receiver, key, value]`
    /// - `__add`:      `[lhs, rhs]`
    /// - `__call`:     `[callee, arg1, arg2, …]`
    ///
    /// The default returns `VmError::HostError` indicating the metamethod is
    /// not implemented.
    async fn dispatch(
        self: Arc<Self>,
        metamethod: &str,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, VmError> {
        let _ = args;
        Err(VmError::HostError {
            name: format!("{}:{}", self.type_name(), metamethod),
            source: format!(
                "metamethod '{}' not implemented for '{}'",
                metamethod,
                self.type_name()
            )
            .into(),
        })
    }
}
