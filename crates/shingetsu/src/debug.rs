//! Implementation of the `debug` standard library module.

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

/// Inspection of the running program's call stack and functions.
///
/// The functions in this module let a script look at where it is in
/// the call stack, what function it's running inside, what the local
/// variables are, and so on.  They are most useful when writing
/// error handlers, loggers, and debug tools.
///
/// Most of the module is sandbox-safe (`debug.traceback`,
/// `debug.info`, `debug.getinfo`) and is always available even in
/// restricted environments.  Functions that read or write program
/// state (`debug.getlocal`, `debug.getupvalue`, `debug.setupvalue`,
/// `debug.upvalueid`) are gated behind a separate library option
/// because they can break encapsulation, and are typically only
/// turned on for development.
///
/// Stack levels in this module count outwards from the caller of the
/// `debug.*` function: `1` is the function that called `debug.*`,
/// `2` is its caller, and so on.
#[crate::module(name = "debug")]
pub mod debug_mod {
    use super::{
        build_full_stack, fill_getinfo_table, frame_arity, frame_current_line, frame_name,
        frame_source, parse_level, resolve_frame, FrameInfo,
    };
    use crate::pretty_print::PrettyPrintConfig;
    use crate::{traceback, valuevec};

    /// Build a stack traceback string.
    ///
    /// Returns a multi-line string showing the call stack from the
    /// most recent frame outwards, optionally prefixed with a
    /// caller-supplied `message`.  Each frame names the function and
    /// the source location it's executing in; native (Rust) frames
    /// are marked `[Native]`.
    ///
    /// `level` controls which frame the traceback starts from:
    /// `1` (the default) starts at the function that called
    /// `debug.traceback`; `2` skips one extra level, useful inside
    /// helpers that want to point at *their* caller.
    ///
    /// When `message` is a non-string, non-numeric value, the value
    /// is returned as-is with no traceback.  This matches Lua 5.4's
    /// convention so error handlers can be written as
    /// `xpcall(f, debug.traceback)` and pass through table errors
    /// untouched.
    ///
    /// # Parameters
    ///
    /// - `message` — prefix string for the traceback; defaults to no
    ///   prefix.  When the first argument is a number it's treated
    ///   as `level` instead.
    /// - `level` — stack level at which to start; defaults to `1`
    ///
    /// # Returns
    ///
    /// - the traceback string, or the original `message` value
    ///   verbatim when it isn't a string or number
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Used as the message handler for xpcall, traceback turns a
    /// -- runtime error into a string with location info.
    /// local ok, err = xpcall(function() error("boom") end, debug.traceback)
    /// assert(not ok)
    /// print(err)
    /// ```
    ///
    /// ```lua
    /// -- Capture a traceback at an arbitrary point.
    /// print(debug.traceback("checkpoint"))
    /// ```
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
        crate::Value::string(tb)
    }

    /// Inspect a stack frame or function and return values directly.
    ///
    /// `level_or_fn` selects the frame to inspect: an integer is a
    /// stack level (counted from the caller of `debug.info`), and a
    /// function value asks about the function's static definition
    /// rather than a particular activation.
    ///
    /// `options` is a string of single-character codes; each
    /// character requests one value (`'a'` requests two).  The
    /// values come back in the same order the codes appear:
    ///
    /// - `s` — source name, prefixed with `@` for file paths.
    /// - `l` — current line number, or `-1` for native or unknown.
    /// - `n` — function name, or `nil` for anonymous functions and
    ///   the main chunk.
    /// - `a` — two values: the declared parameter count, and a
    ///   boolean for whether the function is variadic.
    /// - `f` — the function value itself; currently always `nil`.
    ///
    /// When the level is out of range, no values are returned.
    /// Raises an error when `options` contains an unknown character.
    ///
    /// `debug.getinfo` is the table-returning equivalent and is
    /// preferred when many fields are needed at once.
    ///
    /// # Parameters
    ///
    /// - `level_or_fn` — stack level (1 is the caller of
    ///   `debug.info`) or a function value
    /// - `options` — string of single-character option codes
    ///
    /// # Returns
    ///
    /// - one value per option character, in source order; nothing
    ///   when the level is out of range
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Every option in one call.  The 'a' option expands to two
    /// -- values (nparams, isvararg); 'f' is currently always nil.
    /// -- The values come back in the order the option chars appear,
    /// -- so "slnaf" yields source, line, name, nparams, isvararg, func.
    /// local function example(x, y, ...)
    ///     print(debug.info(1, "slnaf"))
    /// end
    /// example(1, 2, 3)
    /// ```
    ///
    /// ```lua
    /// -- Inspect the parameter list of a function value.
    /// local function greet(name, greeting) end
    /// local nparams, isvararg = debug.info(greet, "a")
    /// assert(nparams == 2)
    /// assert(isvararg == false)
    /// ```
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
                    }
                    .or_suggest_with_mapping(
                        ch.to_string(),
                        "`debug.info` option",
                        &[
                            (b"s", "source"),
                            (b"l", "line"),
                            (b"n", "name"),
                            (b"a", "arity"),
                            (b"f", "function value"),
                        ],
                    ));
                }
            }
        }

        Ok(crate::Variadic(results.into()))
    }

    /// Inspect a stack frame or function and return a table of
    /// fields.
    ///
    /// `level_or_fn` selects what to inspect: an integer stack level
    /// (counted from the caller of `debug.getinfo`) or a function
    /// value.  `what` is a string of single-character codes that
    /// selects which field groups to populate; the default `"flnStu"`
    /// includes everything except `L`.
    ///
    /// Field groups:
    ///
    /// - `n` — `name`, `namewhat`
    /// - `S` — `source`, `short_src`, `linedefined`, `lastlinedefined`,
    ///   `what` (one of `"Lua"`, `"Native"`, or `"main"`)
    /// - `l` — `currentline`
    /// - `t` — `istailcall`
    /// - `u` — `nups`, `nparams`, `isvararg`
    /// - `f` — `func` (the function value; currently always `nil`)
    /// - `L` — `activelines` (currently always an empty table)
    ///
    /// `debug.info` is the multi-return equivalent and is more
    /// convenient when only one or two fields are needed.
    ///
    /// # Parameters
    ///
    /// - `level_or_fn` — stack level (1 is the caller of
    ///   `debug.getinfo`) or a function value
    /// - `what` — string of field-group codes; defaults to `"flnStu"`
    ///
    /// # Returns
    ///
    /// - a table with the requested fields, or `nil` when the level
    ///   is out of range
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Inspect the function we're currently running in.
    /// -- The default 'what' returns every field except active lines.
    /// local function show()
    ///     print(debug.pretty_print(debug.getinfo(1)))
    /// end
    /// show()
    /// ```
    ///
    /// ```lua
    /// -- Inspect a function's declared parameters.
    /// local function add(a, b) return a + b end
    /// local info = debug.getinfo(add, "u")
    /// assert(info.nparams == 2)
    /// assert(info.isvararg == false)
    /// assert(info.nups == 0)
    /// ```
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

    /// Render a value as a human-readable string.
    ///
    /// Returns the same kind of string `print` would emit for the
    /// value, but with table contents recursively expanded so you
    /// can see what's inside.  Useful for ad-hoc debugging when
    /// you want to see the shape of a value without writing a
    /// custom formatter.
    ///
    /// Tables narrower than `wrap_width` render on a single line;
    /// wider tables wrap to one entry per line, indented by
    /// `indent` spaces per nesting level.
    ///
    /// `options` controls the layout:
    ///
    /// - `max_depth` — how many levels of nested tables to
    ///   recurse into; deeper tables render as `{...}`.
    ///   Defaults to `4`.
    /// - `max_entries` — how many entries to render per table
    ///   before truncating with `…`.  Defaults to `32`.
    /// - `wrap_width` — width threshold above which a table is
    ///   rendered multi-line.  Defaults to `60`.
    /// - `indent` — spaces of indentation per nesting level when
    ///   wrapping.  Defaults to `2`.
    ///
    /// Cycles in tables are detected and rendered as `<cycle>`.
    ///
    /// # Parameters
    ///
    /// - `value` — the value to render
    /// - `options` — rendering limits; defaults to
    ///   `{max_depth = 4, max_entries = 32, wrap_width = 60, indent = 2}`
    ///
    /// # Returns
    ///
    /// - the rendered string
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Scalars and short tables render compactly.
    /// print(debug.pretty_print(42))
    /// print(debug.pretty_print({1, 2, 3}))
    /// print(debug.pretty_print({name = "Alice", age = 30}))
    /// ```
    ///
    /// ```lua
    /// -- Wide tables wrap automatically.
    /// print(debug.pretty_print({
    ///     first_name = "Alice", last_name = "Liddell",
    ///     age = 7, hometown = "Oxford",
    /// }))
    /// ```
    ///
    /// ```lua
    /// -- Nested tables expand up to max_depth; wrapping nests indents.
    /// local t = {outer = {inner = {leaf = "hello"}}}
    /// print(debug.pretty_print(t))
    /// print(debug.pretty_print(t, {max_depth = 1}))
    /// ```
    #[function]
    fn pretty_print(
        value: crate::Value,
        options: ::std::option::Option<PrettyPrintConfig>,
    ) -> String {
        let config = options.unwrap_or_default();
        crate::pretty_print::pretty_print(&value, &config)
    }
}

#[crate::module(name = "debug")]
pub mod debug_introspection_mod {
    use super::{resolve_frame, FrameInfo};
    use crate::Bytes;

    /// Read a local variable from a stack frame.
    ///
    /// `level_or_fn` selects the frame: an integer stack level
    /// (counted from the caller of `debug.getlocal`) or a function
    /// value.  `idx` is the 1-based local-variable index.
    ///
    /// When `idx` is out of range or the frame is a native (Rust)
    /// function, returns `nil`.  When passed a function value
    /// instead of a stack level, the function returns the parameter
    /// names from the signature and a `nil` value, since there is
    /// no activation to read values from.
    ///
    /// # Parameters
    ///
    /// - `level_or_fn` — stack level or function value
    /// - `idx` — 1-based local variable index
    ///
    /// # Returns
    ///
    /// - the local's name and current value, or `nil` when out of
    ///   range
    ///
    /// # Examples
    ///
    /// ```lua
    /// local function probe()
    ///     local greeting = "hello"
    ///     local name, value = debug.getlocal(1, 1)
    ///     assert(name == "greeting")
    ///     assert(value == "hello")
    /// end
    /// probe()
    /// ```
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

    /// Read a closure's upvalue.
    ///
    /// Upvalues are the variables a closure captured from its
    /// enclosing scope when it was defined.  This function lets you
    /// inspect them by 1-based index.  Returns `nil` when `up` is
    /// out of range or the function is a native (Rust) function with
    /// no upvalues.
    ///
    /// `debug.setupvalue` is the writing counterpart and
    /// `debug.upvalueid` returns an opaque identifier that lets you
    /// detect when two closures share an upvalue cell.
    ///
    /// # Parameters
    ///
    /// - `func` — a function value
    /// - `up` — 1-based upvalue index
    ///
    /// # Returns
    ///
    /// - the upvalue's name and current value, or `nil` when out of
    ///   range
    ///
    /// # Examples
    ///
    /// ```lua
    /// local x = 42
    /// local function get_x() return x end
    /// local name, value = debug.getupvalue(get_x, 1)
    /// assert(name == "x")
    /// assert(value == 42)
    /// ```
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

    /// Replace a closure's upvalue value.
    ///
    /// Sets the upvalue at 1-based index `up` in `func` to
    /// `new_value`.  All closures that share the upvalue cell see
    /// the new value on subsequent reads.
    ///
    /// Returns the upvalue's name on success.  Returns `nil` when
    /// `up` is out of range or the function has no upvalues; in
    /// either case the value is not stored.
    ///
    /// # Parameters
    ///
    /// - `func` — a function value
    /// - `up` — 1-based upvalue index
    /// - `new_value` — the value to assign to the upvalue
    ///
    /// # Returns
    ///
    /// - the upvalue's name on success, or `nil` when out of range
    ///
    /// # Examples
    ///
    /// ```lua
    /// local x = 1
    /// local function get_x() return x end
    /// local name = debug.setupvalue(get_x, 1, 99)
    /// assert(name == "x")
    /// assert(get_x() == 99)
    /// ```
    #[function]
    fn setupvalue(func: crate::Function, up: i64, new_value: crate::Value) -> Option<Bytes> {
        if up < 1 {
            return None;
        }
        let idx = (up - 1) as usize;
        func.set_upvalue(idx, new_value)
    }

    /// Return an opaque identifier for an upvalue cell.
    ///
    /// Two closures that share the same captured variable return the
    /// same id from this function, even though they are different
    /// closures.  Comparing ids is the only way to detect such
    /// sharing without modifying the upvalue and observing the
    /// effect.
    ///
    /// Returns `nil` when `up` is out of range or for native
    /// functions (which have no upvalue cells).
    ///
    /// # Parameters
    ///
    /// - `func` — a function value
    /// - `up` — 1-based upvalue index
    ///
    /// # Returns
    ///
    /// - an opaque integer identifier, or `nil` when out of range
    ///
    /// # Examples
    ///
    /// ```lua
    /// local x = 1
    /// local function read() return x end
    /// local function bump() x = x + 1 end
    /// -- Both closures captured the same `x`, so the upvalue ids match.
    /// assert(debug.upvalueid(read, 1) == debug.upvalueid(bump, 1))
    /// ```
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
                crate::Value::string(prefixed)
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
            if name.is_empty() || name == "<anonymous>" || name == &sig.source {
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
                }
                .or_suggest_with_mapping(
                    ch.to_string(),
                    "`debug.getinfo` option",
                    &[
                        (b"n", "name + namewhat"),
                        (b"l", "currentline"),
                        (b"t", "istailcall"),
                        (b"u", "nups"),
                        (b"f", "func"),
                        (b"L", "active lines"),
                    ],
                ));
            }
        }
    }

    match crate::IntoLua::into_lua(result) {
        crate::Value::Table(t) => Ok(t),
        _ => unreachable!(),
    }
}
