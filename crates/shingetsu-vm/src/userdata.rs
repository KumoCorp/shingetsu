use std::sync::Arc;

use bytes::Bytes;
use downcast_rs::DowncastSync;

use crate::call_context::CallContext;
use crate::error::VmError;
use crate::types::LuaType;
use crate::value::Value;

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
///
/// Implementors should also call `impl_downcast!(sync YourType)` (or use the
/// `#[derive(UserData)]` macro which does this automatically) to enable
/// downcasting from `Arc<dyn Userdata>` back to `Arc<YourType>`.
#[async_trait::async_trait]
pub trait Userdata: DowncastSync {
    /// The name shown in error messages and stack traces.
    fn type_name(&self) -> &'static str;

    /// Return the full structural type information for this userdata.
    ///
    /// The default returns an opaque `LuaType::Named(type_name)`.  The
    /// `#[shingetsu::userdata]` proc macro overrides this to return a
    /// `LuaType::Table` with the full method/field layout so the
    /// compiler can perform compile-time checks (e.g. dot-vs-colon
    /// call syntax validation).
    fn lua_type_info(&self) -> LuaType {
        LuaType::Named(Bytes::copy_from_slice(self.type_name().as_bytes()))
    }

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
        context: CallContext,
        metamethod: &str,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, VmError> {
        let _ = (context, args);
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

downcast_rs::impl_downcast!(sync Userdata);
