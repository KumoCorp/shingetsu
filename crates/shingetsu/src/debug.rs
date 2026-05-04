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
//! * `debug.upvalueid(fn, up)`
//!
//! ## Deferred
//!
//! * `debug.setlocal` — requires mutable stack frame access.
//! * `debug.getmetatable` / `debug.setmetatable` — bypass `__metatable`.
//! * `debug.sethook` / `debug.gethook` — needs VM-loop hook dispatch.
//! * `debug.upvaluejoin` — needs upvalue identity model.
//! * `debug.getregistry` — no registry concept today.
//! * Thread-first overloads — rejected until coroutines land.

use crate::table::Table;
use crate::value::Value;
use crate::VmError;

use crate::Bytes;

/// Return type for `debug.getlocal` and `debug.getupvalue`:
/// `(name, value)` or `nil`.
#[derive(crate::IntoLuaMulti)]
enum NameValue {
    Found(Bytes, crate::value::Value),
    NotFound,
}

/// First argument to `debug.info`, `debug.getinfo`, and `debug.getlocal`.
///
/// Accepts either a numeric stack level (integer or float, coerced to
/// integer) or a function value.
#[derive(crate::FromLua, crate::LuaTyped)]
enum LevelOrFn {
    Level(i64),
    Func(crate::Function),
}

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
    merge_into_debug_table(env, table)?;
    env.register_module_type("debug", debug_mod::module_type());
    Ok(())
}

/// Register the `Libraries::DEBUG`-gated introspection functions into
/// the existing `debug` table.
///
/// Must be called after [`crate::debug::register`] so the `debug` table exists.
pub fn register_introspection(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = debug_introspection_mod::build_module_table(env)?;
    merge_into_debug_table(env, table)?;
    env.register_module_type("debug", debug_introspection_mod::module_type());
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
        build_full_stack, fill_getinfo_table, frame_arity, frame_current_line, frame_name,
        frame_source, parse_level, resolve_frame, FrameInfo,
    };
    use crate::{traceback, valuevec, Bytes};

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
        crate::Value::String(Bytes::from(tb))
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
        level_or_fn: super::LevelOrFn,
        options: String,
    ) -> Result<crate::Variadic, crate::VmError> {
        let full_stack = build_full_stack(&ctx);
        let frame = resolve_frame(level_or_fn, &full_stack);

        let frame = match frame {
            // Level out of range: Luau returns no values.
            None => return Ok(crate::Variadic(valuevec![])),
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
                    return Err(crate::VmError::ArgError {
                        position: 2,
                        function: "info".into(),
                        msg: format!("invalid option '{ch}'"),
                    });
                }
            }
        }

        Ok(crate::Variadic(results.into()))
    }

    // -----------------------------------------------------------------
    // debug.getinfo(level_or_fn [, what]) -> table | nil
    //
    // Lua 5.4-style table-returning frame query.  Returns a table with
    // fields determined by the `what` string, or nil if the level is
    // out of range.  Default `what` is "flnStu".
    // -----------------------------------------------------------------
    #[function]
    fn getinfo(
        ctx: crate::CallContext,
        level_or_fn: super::LevelOrFn,
        what: Option<String>,
    ) -> Result<crate::Value, crate::VmError> {
        // Default what string matches Lua 5.4: all fields except L.
        let what = what.unwrap_or_else(|| "flnStu".to_owned());

        let full_stack = build_full_stack(&ctx);
        let frame = resolve_frame(level_or_fn, &full_stack);

        let frame = match frame {
            // Out-of-range level: Lua 5.4 returns nil.
            None => return Ok(crate::Value::Nil),
            Some(f) => f,
        };

        let is_main = matches!(&frame, FrameInfo::Lua { sig, .. } if sig.name == sig.source);
        let table = fill_getinfo_table(&frame, &what, is_main)?;
        Ok(crate::Value::Table(table))
    }
}

#[crate::module(name = "debug")]
pub mod debug_introspection_mod {
    use super::{resolve_frame, FrameInfo};
    use crate::Bytes;

    // -----------------------------------------------------------------
    // debug.getlocal(level, local) -> name, value | nil
    //
    // Returns the name and value of the local variable at the given
    // 1-based index in the frame identified by `level`.  Returns nil
    // when the index is out of range.  For the function-argument form,
    // returns param names with nil values (no activation).
    // -----------------------------------------------------------------
    #[function]
    fn getlocal(
        locals: crate::FrameLocals,
        level_or_fn: super::LevelOrFn,
        idx: i64,
    ) -> Result<super::NameValue, crate::VmError> {
        let frame = resolve_frame(level_or_fn, locals.frames());

        let frame = match frame {
            None => return Ok(super::NameValue::NotFound),
            Some(f) => f,
        };

        match frame {
            FrameInfo::Lua { sig, locals, .. } => {
                if idx >= 1 {
                    // Positive index: look up in live locals.
                    let i = (idx - 1) as usize;
                    if let Some((name, value)) = locals.get(i) {
                        return Ok(super::NameValue::Found(name.clone(), value.clone()));
                    }
                    // Fall through to function-argument form: if no
                    // live local, try param names from signature.
                    if locals.is_empty() {
                        if let Some(param) = sig.params.get(i) {
                            if let Some(name) = &param.name {
                                return Ok(super::NameValue::Found(
                                    name.clone(),
                                    crate::Value::Nil,
                                ));
                            }
                        }
                    }
                }
                // Out of range.
                Ok(super::NameValue::NotFound)
            }
            FrameInfo::Native { .. } => {
                // Native frames have no locals.
                Ok(super::NameValue::NotFound)
            }
        }
    }

    // -----------------------------------------------------------------
    // debug.getupvalue(fn, up) -> name, value | nil
    //
    // Returns the name and current value of the upvalue at 1-based
    // index `up` in the given function.  Returns nil when out of range.
    // -----------------------------------------------------------------
    #[function]
    fn getupvalue(func: crate::Function, up: i64) -> Result<super::NameValue, crate::VmError> {
        if up < 1 {
            return Ok(super::NameValue::NotFound);
        }
        let idx = (up - 1) as usize;

        match func.get_upvalue(idx) {
            Some((name, value)) => Ok(super::NameValue::Found(name, value)),
            None => Ok(super::NameValue::NotFound),
        }
    }

    // -----------------------------------------------------------------
    // debug.setupvalue(fn, up, value) -> name | nil
    //
    // Sets the upvalue at 1-based index `up` in the given function to
    // `value`.  Returns the upvalue name on success, or nil when out
    // of range.
    // -----------------------------------------------------------------
    #[function]
    fn setupvalue(func: crate::Function, up: i64, new_value: crate::Value) -> Option<Bytes> {
        if up < 1 {
            return None;
        }
        let idx = (up - 1) as usize;
        func.set_upvalue(idx, new_value)
    }

    // -----------------------------------------------------------------
    // debug.upvalueid(fn, up) -> integer | nil
    //
    // Returns an opaque integer that uniquely identifies the upvalue
    // cell at 1-based index `up` in the given function.  Two closures
    // that share the same captured variable return the same id.
    // Returns nil when out of range or for native functions.
    // -----------------------------------------------------------------
    #[function]
    fn upvalueid(func: crate::Function, up: i64) -> Option<i64> {
        if up < 1 {
            return None;
        }
        func.upvalue_id((up - 1) as usize)
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
fn build_full_stack(ctx: &crate::CallContext) -> Vec<crate::call_stack::StackFrame> {
    let mut stack = ctx.call_stack().to_vec();
    if let Some(name) = &ctx.native_name {
        stack.push(crate::call_stack::StackFrame::Native {
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
        locals: Vec<(Bytes, crate::Value)>,
    },
    Native {
        name: Bytes,
    },
}

/// Resolve the first argument to `debug.info` — either an integer
/// level (0 = the calling debug function itself) or a function value —
/// into a `FrameInfo`.  Returns `None` when the level is out of range.
fn resolve_frame(
    first: LevelOrFn,
    full_stack: &[crate::call_stack::StackFrame],
) -> Option<FrameInfo> {
    match first {
        LevelOrFn::Level(n) => resolve_frame_by_level(n.max(0) as usize, full_stack),
        LevelOrFn::Func(func) => {
            // Function-argument form: return info about the function
            // definition, not an activation.  We extract the signature
            // from the Function value itself.
            let sig = func.signature().clone();
            Some(FrameInfo::Lua {
                sig,
                source_location: None,
                locals: vec![],
            })
        }
    }
}

/// Resolve a numeric stack level into a `FrameInfo`.
fn resolve_frame_by_level(
    level: usize,
    full_stack: &[crate::call_stack::StackFrame],
) -> Option<FrameInfo> {
    // The stack is outermost-first; level 0 is the innermost
    // (most recent) frame.
    let reversed: Vec<_> = full_stack.iter().rev().collect();
    match reversed.get(level) {
        Some(
            frame @ crate::call_stack::StackFrame::Lua {
                function, locals, ..
            },
        ) => Some(FrameInfo::Lua {
            sig: function.clone(),
            source_location: frame.source_location(),
            locals: locals.clone(),
        }),
        Some(crate::call_stack::StackFrame::Native { function_name }) => Some(FrameInfo::Native {
            name: function_name.clone(),
        }),
        None => None,
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
                loc.source_name.as_str().to_owned()
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
            } else if source.starts_with(b"@") || source.starts_with(b"=") {
                crate::Value::String(source.clone())
            } else {
                // Prepend '@' for bare source names.
                let mut prefixed = Vec::with_capacity(1 + source.len());
                prefixed.push(b'@');
                prefixed.extend_from_slice(source);
                crate::Value::String(Bytes::from(prefixed))
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

/// Result table for `debug.getinfo`.  Each field group is gated by a
/// character in the `what` option string; fields left as `None` are
/// omitted from the Lua table.
///
/// Field groups:
/// - `n` → `name`, `namewhat`
/// - `S` → `source`, `short_src`, `linedefined`, `lastlinedefined`, `what`
/// - `l` → `currentline`
/// - `t` → `istailcall`
/// - `u` → `nups`, `nparams`, `isvararg`
/// - `f` → `func`
/// - `L` → `activelines`
#[derive(crate::IntoLua, crate::LuaTyped)]
struct GetInfoResult {
    // -- 'n' group --
    name: Option<Bytes>,
    #[lua(rename = "namewhat")]
    name_what: Option<Bytes>,

    // -- 'S' group --
    source: Option<Bytes>,
    #[lua(rename = "short_src")]
    short_source: Option<Bytes>,
    #[lua(rename = "linedefined")]
    line_defined: Option<i64>,
    #[lua(rename = "lastlinedefined")]
    last_line_defined: Option<i64>,
    /// `"Lua"`, `"Native"`, or `"main"`.
    what: Option<Bytes>,

    // -- 'l' group --
    #[lua(rename = "currentline")]
    current_line: Option<i64>,

    // -- 't' group --
    #[lua(rename = "istailcall")]
    is_tail_call: Option<bool>,

    // -- 'u' group --
    #[lua(rename = "nups")]
    num_upvalues: Option<i64>,
    #[lua(rename = "nparams")]
    num_params: Option<i64>,
    #[lua(rename = "isvararg")]
    is_vararg: Option<bool>,

    // -- 'f' group --
    /// Not yet available from StackFrame; always None.
    #[lua(rename = "func")]
    function: Option<crate::Value>,

    // -- 'L' group --
    /// Per-line active table; requires source_locations (not yet populated).
    #[lua(rename = "activelines")]
    active_lines: Option<crate::table::Table>,
}

impl Default for GetInfoResult {
    fn default() -> Self {
        Self {
            name: None,
            name_what: None,
            source: None,
            short_source: None,
            line_defined: None,
            last_line_defined: None,
            what: None,
            current_line: None,
            is_tail_call: None,
            num_upvalues: None,
            num_params: None,
            is_vararg: None,
            function: None,
            active_lines: None,
        }
    }
}

/// Extract the `Bytes` payload from a `Value::String`, or `None` for
/// any other variant.
fn value_into_bytes(v: crate::Value) -> Option<Bytes> {
    match v {
        crate::Value::String(s) => Some(s),
        _ => None,
    }
}

/// Build the result table for `debug.getinfo` from a `FrameInfo` and
/// the `what` option string.
fn fill_getinfo_table(
    frame: &FrameInfo,
    what: &str,
    is_main: bool,
) -> Result<crate::table::Table, crate::VmError> {
    let mut result = GetInfoResult::default();

    for ch in what.chars() {
        match ch {
            'n' => {
                result.name = value_into_bytes(frame_name(frame));
                // namewhat: always "" for now (deferred).
                result.name_what = Some(Bytes::from(""));
            }
            'S' => {
                let source_bytes =
                    value_into_bytes(frame_source(frame)).unwrap_or_else(|| Bytes::from("=?"));
                result.source = Some(source_bytes.clone());
                let short =
                    shingetsu_vm::format_source_name(&String::from_utf8_lossy(&source_bytes));
                result.short_source = Some(Bytes::from(short));

                let (ld, lld) = match frame {
                    FrameInfo::Lua { sig, .. } => {
                        (sig.line_defined as i64, sig.last_line_defined as i64)
                    }
                    FrameInfo::Native { .. } => (-1, -1),
                };
                result.line_defined = Some(ld);
                result.last_line_defined = Some(lld);

                result.what = Some(Bytes::from(match frame {
                    FrameInfo::Lua { .. } if is_main => "main",
                    FrameInfo::Lua { .. } => "Lua",
                    FrameInfo::Native { .. } => "Native",
                }));
            }
            'l' => {
                result.current_line = Some(match frame {
                    FrameInfo::Lua {
                        source_location: Some(loc),
                        ..
                    } => loc.line as i64,
                    _ => -1,
                });
            }
            't' => {
                // istailcall: always false for now.
                result.is_tail_call = Some(false);
            }
            'u' => {
                let (nups, nparams, isvararg) = match frame {
                    FrameInfo::Lua { sig, .. } => (
                        sig.num_upvalues as i64,
                        sig.params.len() as i64,
                        sig.variadic,
                    ),
                    FrameInfo::Native { .. } => (0, 0, true),
                };
                result.num_upvalues = Some(nups);
                result.num_params = Some(nparams);
                result.is_vararg = Some(isvararg);
            }
            'f' => {
                // func: not available from StackFrame.
            }
            'L' => {
                // active_lines: requires per-instruction source_locations
                // which are not yet populated.  Return an empty table.
                result.active_lines = Some(crate::table::Table::new());
            }
            _ => {
                return Err(crate::VmError::ArgError {
                    position: 2,
                    function: "getinfo".into(),
                    msg: format!("invalid option '{ch}'"),
                });
            }
        }
    }

    match crate::IntoLua::into_lua(result) {
        crate::Value::Table(t) => Ok(t),
        _ => unreachable!(),
    }
}
