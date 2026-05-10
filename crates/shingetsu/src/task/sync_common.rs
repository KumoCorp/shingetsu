//! Helpers shared by the sync-primitive constructors.

use std::any::Any;
use std::sync::Arc;

use crate::{Bytes, SharedRegistry, VmError};

/// Emit a host-visible warning.  Forwards through the `log` crate
/// when the `log` feature is enabled, otherwise falls back to
/// `eprintln!` so the message is still visible during development
/// and in hosts that have not wired up `log`.
pub(crate) fn warn(args: std::fmt::Arguments<'_>) {
    #[cfg(feature = "log")]
    {
        log::warn!("{}", args);
    }
    #[cfg(not(feature = "log"))]
    {
        eprintln!("warning: {args}");
    }
}

/// Look up `name` in the [`SharedRegistry`] for the given env, or
/// create it with `factory`.  On a registry type-mismatch (the same
/// name was previously registered as a different primitive type),
/// returns an [`VmError::ArgError`] attributed to argument
/// `name_position` (1-based) of `function_name`, so the diagnostic
/// renderer points at the offending name argument.
pub(crate) fn shared_lookup<T, F>(
    registry: &SharedRegistry,
    function_name: &str,
    name_position: usize,
    name: Bytes,
    factory: F,
) -> Result<Arc<T>, VmError>
where
    T: Any + Send + Sync,
    F: FnOnce() -> T,
{
    registry
        .get_or_create::<T, _>(name, factory)
        .map_err(|e: shingetsu_vm::SharedRegistryError| VmError::ArgError {
            position: name_position,
            function: function_name.to_owned(),
            msg: e.to_string(),
        })
}
