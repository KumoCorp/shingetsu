//! `LuaRuntimeError` userdata — Lua-visible wrapper around
//! [`crate::error::RuntimeError`].

use std::sync::Arc;

use crate::diagnostic::{render_runtime_error, RenderStyle};
use crate::error::RuntimeError;
use crate::{valuevec, Bytes, Value, Variadic};

/// Userdata wrapper around [`RuntimeError`] exposed to Lua.
///
/// Returned as the second value of `Task:pawait()` and the error
/// value of `Task:try_result()` / `task.select`.  Lets Lua code
/// inspect the structured error rather than receive a flattened
/// string.
pub struct LuaRuntimeError(Arc<RuntimeError>);

/// Return shape for `LuaRuntimeError`'s `:location()` method:
/// either a `(source_name, line)` pair or a single `nil` when the
/// error has no associated Lua source location.  The derive
/// expands to `(string, integer) | nil` for the type checker.
#[derive(crate::IntoLuaMulti)]
pub enum LocationResult {
    FileAndLine(Bytes, i64),
    None,
}

impl LuaRuntimeError {
    pub fn new(err: Arc<RuntimeError>) -> Arc<Self> {
        Arc::new(Self(err))
    }

    pub fn inner(&self) -> &RuntimeError {
        &self.0
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "RuntimeError", index_fallback = "nil")]
impl LuaRuntimeError {
    /// The bare error message (no traceback, no source snippets).
    #[lua_method]
    fn message(self: Arc<Self>) -> Bytes {
        self.0.error.to_string().into()
    }

    /// Rendered stack traceback for the error, in the same format
    /// produced by Lua's `debug.traceback`.
    #[lua_method]
    fn traceback(self: Arc<Self>) -> Bytes {
        crate::traceback::render_traceback(&self.0.call_stack, None, 0).into()
    }

    /// Source location of the innermost Lua frame: returns
    /// `(source_name, line)` when the error originated in Lua
    /// code, or `nil` when it was raised outside any Lua frame
    /// (e.g. from a host-only call path).
    #[lua_method]
    fn location(self: Arc<Self>) -> LocationResult {
        match self
            .0
            .call_stack
            .iter()
            .rev()
            .find_map(|f| f.source_location())
        {
            Some(loc) => {
                LocationResult::FileAndLine(loc.source_name.as_str().into(), loc.line as i64)
            }
            None => LocationResult::None,
        }
    }

    /// Array of help-text hints attached to the error, in the order
    /// they were attached.  Empty array if no hints were attached.
    #[lua_method]
    fn hints(self: Arc<Self>) -> Vec<Bytes> {
        self.0
            .hints
            .iter()
            .map(|h| h.message.clone().into())
            .collect()
    }

    /// Render the full annotated diagnostic — the same multi-line
    /// output the CLI prints for an unhandled error, including
    /// source snippets, hints, and the stack trace.
    #[lua_method]
    fn render(self: Arc<Self>) -> Bytes {
        render_runtime_error(&self.0, RenderStyle::Plain).into()
    }

    /// `__tostring`: returns the same string as `:render()`.
    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string(render_runtime_error(
            &self.0,
            RenderStyle::Plain
        ))])
    }
}
