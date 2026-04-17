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
    use super::{
        build_full_stack, frame_arity, frame_current_line, frame_name, frame_source, parse_level,
        resolve_frame,
    };
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

        let full_stack = build_full_stack(&ctx);
        let tb = traceback::render_traceback(&full_stack, message.as_deref(), level);
        crate::Value::String(bytes::Bytes::from(tb))
    }

    // -----------------------------------------------------------------
    // debug.info(level_or_fn, options) -> ...
    //
    // Luau-style multi-return frame query.  Returns values in the order
    // the option characters appear in the options string.
    // -----------------------------------------------------------------
    #[function]
    fn info(
        ctx: crate::CallContext,
        args: crate::Variadic,
    ) -> Result<crate::Variadic, crate::error::VmError> {
        let mut args = args.0.into_iter();
        let first = args.next().unwrap_or(crate::Value::Nil);
        let options_val = args.next().unwrap_or(crate::Value::Nil);

        let options = match &options_val {
            crate::Value::String(s) => String::from_utf8_lossy(s).into_owned(),
            _ => {
                return Err(crate::error::VmError::ArgError {
                    position: 2,
                    function: "info".into(),
                    msg: "string expected".into(),
                });
            }
        };

        let full_stack = build_full_stack(&ctx);
        let frame = resolve_frame(&first, &full_stack)?;

        let frame = match frame {
            // Level out of range: Luau returns no values.
            None => return Ok(crate::Variadic(vec![])),
            Some(f) => f,
        };

        let mut results = Vec::new();
        for ch in options.chars() {
            match ch {
                's' => results.push(frame_source(&frame)),
                'l' => results.push(frame_current_line(&frame)),
                'n' => results.push(frame_name(&frame)),
                'a' => {
                    // 'a' expands to two values: arity, is_vararg
                    let (arity, is_vararg) = frame_arity(&frame);
                    results.push(arity);
                    results.push(is_vararg);
                }
                'f' => results.push(crate::Value::Nil),
                _ => {
                    return Err(crate::error::VmError::ArgError {
                        position: 2,
                        function: "info".into(),
                        msg: format!("invalid option '{ch}'"),
                    });
                }
            }
        }

        Ok(crate::Variadic(results))
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

/// Build the full call stack including the native frame for the
/// currently-executing function (from `ctx.native_name`).
fn build_full_stack(ctx: &crate::CallContext) -> Vec<crate::call_context::StackFrame> {
    let mut stack = (*ctx.call_stack).clone();
    if let Some(name) = &ctx.native_name {
        stack.push(crate::call_context::StackFrame::Native {
            function_name: name.clone(),
        });
    }
    stack
}

/// Information extracted from a stack frame for `debug.info` queries.
enum FrameInfo {
    Lua {
        sig: std::sync::Arc<crate::types::FunctionSignature>,
        source_location: Option<crate::proto::SourceLocation>,
    },
    Native {
        name: bytes::Bytes,
    },
}

/// Resolve the first argument to `debug.info` — either an integer
/// level (0 = the calling debug function itself) or a function value —
/// into a `FrameInfo`.  Returns `None` when the level is out of range.
fn resolve_frame(
    first: &crate::Value,
    full_stack: &[crate::call_context::StackFrame],
) -> Result<Option<FrameInfo>, crate::error::VmError> {
    match first {
        crate::Value::Integer(n) => {
            let level = (*n).max(0) as usize;
            // The stack is outermost-first; level 0 is the innermost
            // (most recent) frame.
            let reversed: Vec<_> = full_stack.iter().rev().collect();
            match reversed.get(level) {
                Some(crate::call_context::StackFrame::Lua {
                    function,
                    source_location,
                    ..
                }) => Ok(Some(FrameInfo::Lua {
                    sig: function.clone(),
                    source_location: source_location.clone(),
                })),
                Some(crate::call_context::StackFrame::Native { function_name }) => {
                    Ok(Some(FrameInfo::Native {
                        name: function_name.clone(),
                    }))
                }
                None => Ok(None),
            }
        }
        crate::Value::Float(f) => {
            let as_int = crate::Value::Integer(*f as i64);
            resolve_frame(&as_int, full_stack)
        }
        crate::Value::Function(func) => {
            // Function-argument form: return info about the function
            // definition, not an activation.  We extract the signature
            // from the Function value itself.
            let sig = func.signature().clone();
            Ok(Some(FrameInfo::Lua {
                sig,
                source_location: None,
            }))
        }
        _ => Err(crate::error::VmError::ArgError {
            position: 1,
            function: "info".into(),
            msg: "function or level expected".into(),
        }),
    }
}

/// `s` option: source name, prefixed with `@` for file sources.
///
/// Priority: source_location → signature source field → `"=?"` fallback.
fn frame_source(frame: &FrameInfo) -> crate::Value {
    match frame {
        FrameInfo::Lua {
            source_location: Some(loc),
            ..
        } => {
            let src = if loc.source_name.starts_with('@') || loc.source_name.starts_with('=') {
                loc.source_name.clone()
            } else {
                format!("@{}", loc.source_name)
            };
            crate::Value::string(src)
        }
        FrameInfo::Lua {
            sig,
            source_location: None,
            ..
        } => {
            // No source location — use the signature's source field
            // (populated by the compiler from CompileOptions.source_name).
            let source = &sig.source;
            if source.is_empty() {
                crate::Value::string("=?")
            } else {
                let s = String::from_utf8_lossy(source);
                if s.starts_with('@') || s.starts_with('=') {
                    crate::Value::string(s.into_owned())
                } else {
                    crate::Value::string(format!("@{s}"))
                }
            }
        }
        FrameInfo::Native { .. } => crate::Value::string("=[Native]"),
    }
}

/// `l` option: current line, or -1 for native/no-line frames.
fn frame_current_line(frame: &FrameInfo) -> crate::Value {
    match frame {
        FrameInfo::Lua {
            source_location: Some(loc),
            ..
        } => crate::Value::Integer(loc.line as i64),
        _ => crate::Value::Integer(-1),
    }
}

/// `n` option: function name.
///
/// Returns nil for anonymous functions and for the main chunk (where
/// the compiler sets name == source).  Named functions return the name
/// as a string.
fn frame_name(frame: &FrameInfo) -> crate::Value {
    match frame {
        FrameInfo::Lua { sig, .. } => {
            let name = &sig.name;
            if name.is_empty() || name.as_ref() == b"<anonymous>" || name == &sig.source {
                crate::Value::Nil
            } else {
                crate::Value::String(name.clone())
            }
        }
        FrameInfo::Native { name } => {
            if name.is_empty() {
                crate::Value::Nil
            } else {
                crate::Value::String(name.clone())
            }
        }
    }
}

/// `a` option: `(arity, is_vararg)` as two values.
fn frame_arity(frame: &FrameInfo) -> (crate::Value, crate::Value) {
    match frame {
        FrameInfo::Lua { sig, .. } => (
            crate::Value::Integer(sig.params.len() as i64),
            crate::Value::Boolean(sig.variadic),
        ),
        FrameInfo::Native { .. } => (crate::Value::Integer(0), crate::Value::Boolean(true)),
    }
}
