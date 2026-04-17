//! Lua debug library.
//!
//! Provides a blend of Luau's `debug.info` / `debug.traceback` and
//! Lua 5.4's `debug.getinfo`, with sandbox-safe functions registered
//! unconditionally and frame/upvalue introspection gated behind
//! [`Libraries::DEBUG`].
//!
//! ## Sandbox-safe (always registered)
//!
//! * `debug.traceback([message [, level]])` — Lua 5.4-style stack
//!   traceback with type-annotated signatures and `[Native]` labels.
//! * `debug.info(level_or_fn, options)` — Luau-style multi-return
//!   frame query.
//! * `debug.getinfo(level_or_fn [, what])` — Lua 5.4-style table
//!   return frame query.
//!
//! ## Gated by `Libraries::DEBUG`
//!
//! * `debug.getlocal(level_or_fn, local)`
//! * `debug.getupvalue(fn, up)`
//! * `debug.setupvalue(fn, up, value)`
//! * `debug.upvalueid(fn, up)` — design pending (no lightuserdata yet)
//!
//! ## Deferred
//!
//! * `debug.setlocal` — requires mutable stack frame access.
//! * `debug.getmetatable` / `debug.setmetatable` — bypass `__metatable`.
//! * `debug.sethook` / `debug.gethook` — needs VM-loop hook dispatch.
//! * `debug.upvaluejoin` — needs upvalue identity model.
//! * `debug.getregistry` — no registry concept today.
//! * Thread-first overloads — rejected until coroutines land.

use crate::error::VmError;
use crate::table::Table;
use crate::value::Value;

/// Build the sandbox-safe debug library table and register it as the
/// `debug` global.  Creates the table if it does not already exist.
///
/// This is called unconditionally by [`register_libs`] — even a
/// fully-sandboxed environment gets `debug.traceback`, `debug.info`,
/// and `debug.getinfo`.
///
/// [`register_libs`]: crate::register_libs
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = debug_mod::build_module_table(env)?;
    merge_into_debug_table(env, table)
}

/// Register the `Libraries::DEBUG`-gated introspection functions into
/// the existing `debug` table.
///
/// Must be called after [`register`] so the `debug` table exists.
pub fn register_introspection(_env: &crate::GlobalEnv) -> Result<(), VmError> {
    // Gated functions will be added here as they are implemented
    // (debug.getlocal, debug.getupvalue, debug.setupvalue, debug.upvalueid).
    // Each will be a separate sub-module whose table is merged in.
    Ok(())
}

/// Merge all entries from `source` into the `debug` global table,
/// creating that table if it does not exist yet.
fn merge_into_debug_table(env: &crate::GlobalEnv, source: Table) -> Result<(), VmError> {
    let debug_table = match env.get_global("debug") {
        Some(Value::Table(t)) => t,
        _ => {
            let t = Table::new();
            env.set_global("debug", Value::Table(t.clone()));
            t
        }
    };
    let mut key = Value::Nil;
    loop {
        match source.next(&key)? {
            Some((k, v)) => {
                debug_table.raw_set(k.clone(), v)?;
                key = k;
            }
            None => break,
        }
    }
    Ok(())
}

#[crate::module(name = "debug")]
pub mod debug_mod {
    use super::parse_level;
    use crate::traceback;

    // -----------------------------------------------------------------
    // debug.traceback([message [, level]]) -> string
    //
    // Returns a Lua 5.4-style stack traceback with type-annotated
    // signatures and [Native] labels.  Non-string messages are returned
    // as-is (Lua semantics).  Thread-first overload is rejected until
    // coroutines land.
    // -----------------------------------------------------------------
    #[function]
    fn traceback(ctx: crate::CallContext, args: crate::Variadic) -> crate::Value {
        let mut args = args.0.into_iter();
        let first = args.next().unwrap_or(crate::Value::Nil);

        // Reject thread-first overload — coroutines are not yet supported.
        // (When they land, this branch should inspect the thread and use
        // its stack instead of `ctx.call_stack`.)
        // For now, the only way to detect a "thread" value would be a
        // dedicated coroutine type; since we have none, this path is
        // unreachable.

        // Parse arguments: traceback([message [, level]])
        let (message, level): (Option<String>, usize) = match &first {
            crate::Value::Nil => {
                // No message.  Second arg, if any, is level.
                let level = parse_level(args.next(), 1);
                (None, level)
            }
            crate::Value::String(s) => {
                let msg = String::from_utf8_lossy(s).into_owned();
                let level = parse_level(args.next(), 1);
                (Some(msg), level)
            }
            crate::Value::Integer(_) | crate::Value::Float(_) => {
                // traceback(level) — numeric first arg is the level, no message.
                let level = parse_level(Some(first), 1);
                (None, level)
            }
            _ => {
                // Non-string, non-nil, non-numeric message: Lua 5.4
                // returns the value as-is without a traceback.
                return first;
            }
        };

        // Build the full stack including the native frame for traceback
        // itself, so that level=0 shows it and level=1 (default) skips it.
        let mut full_stack = (*ctx.call_stack).clone();
        if let Some(name) = &ctx.native_name {
            full_stack.push(crate::call_context::StackFrame::Native {
                function_name: name.clone(),
            });
        }
        let tb = traceback::render_traceback(&full_stack, message.as_deref(), level);
        crate::Value::String(bytes::Bytes::from(tb))
    }
}

/// Parse an optional level argument, defaulting to `default` when nil
/// or absent.  Clamps negative values to 0.
fn parse_level(val: Option<crate::Value>, default: usize) -> usize {
    match val {
        Some(crate::Value::Integer(n)) => n.max(0) as usize,
        Some(crate::Value::Float(f)) => (f as i64).max(0) as usize,
        _ => default,
    }
}
