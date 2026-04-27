//! Core Lua built-in functions expressed via the `#[module]` proc macro.
//!
//! Call [`register`] to install these into a [`GlobalEnv`].  The VM-level
//! builtins that cannot be expressed through the macro (`pcall`, `xpcall`,
//! `require`) are registered separately by `GlobalEnv::register_builtins`.

use crate::valuevec;
use std::sync::Arc;

use shingetsu::Bytes;

use crate::call_context::CallContext;
use crate::call_stack::StackFrame;
use crate::error::VmError;
use crate::global_env::value_to_error_string;
use crate::table::Table;
use crate::value::Value;

/// First argument to `select`: either an integer index or the string `"#"`.
#[derive(crate::FromLua, crate::LuaTyped)]
enum SelectIndex {
    Num(i64),
    Hash(Bytes),
}

/// Return type for `next`: `(key, value)` or `nil`.
#[derive(crate::IntoLuaMulti)]
enum NextResult {
    Pair(Value, Value),
    End,
}

/// Return type for `collectgarbage`: varies by option.
#[derive(crate::IntoLuaMulti)]
enum CollectGarbageResult {
    Integer(i64),
    Count(f64, f64),
    Running(bool),
}

/// Return type for `pairs`: `(next_fn, table, nil)` or metamethod results.
#[derive(crate::IntoLuaMulti)]
enum PairsResult {
    Standard(crate::function::Function, crate::table::Table),
    Metamethod(crate::convert::Variadic),
}

/// Return type for `ipairs`: `(iter_fn, table, 0)` or metamethod results.
#[derive(crate::IntoLuaMulti)]
enum IpairsResult {
    Standard(crate::function::Function, crate::table::Table, i64),
    Metamethod(crate::convert::Variadic),
}

/// Return type for the ipairs iterator: `(index, value)` or nil.
#[derive(crate::IntoLuaMulti)]
enum IpairsIterResult {
    Item(i64, Value),
    End,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a value to its string representation, respecting `__tostring`.
async fn value_tostring(ctx: &CallContext, v: Value) -> Result<String, VmError> {
    if let Some(sv) = v.to_string_value() {
        return Ok(sv.to_string());
    }
    // Check __tostring metamethod on tables.
    if let Value::Table(ref t) = v {
        if let Some(Value::Function(mm)) = t.get_metamethod("__tostring") {
            let results = ctx.call_function(mm, valuevec![v]).await?;
            let s = results.into_iter().next().unwrap_or(Value::Nil);
            return Ok(s.to_string());
        }
    }
    // Dispatch __tostring on userdata via its dispatch mechanism.
    if let Value::Userdata(ref ud) = v {
        let results = Arc::clone(ud)
            .dispatch(ctx.clone(), "__tostring", valuevec![v])
            .await?;
        let s = results.into_iter().next().unwrap_or(Value::Nil);
        return Ok(s.to_string());
    }
    Ok(v.to_string())
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

#[crate::module(name = "builtins")]
mod builtins {
    use super::*;
    use crate::convert::Variadic;
    use crate::function::Function;

    // ----------------------------------------------------------------
    // type(v) — returns the type name as a string.
    // Renamed because `type` is a Rust keyword.
    // ----------------------------------------------------------------
    #[function(rename = "type")]
    fn lua_type(v: Value) -> &'static str {
        match v {
            Value::Nil => "nil",
            Value::Boolean(_) => "boolean",
            Value::Integer(_) | Value::Float(_) => "number",
            Value::String(_) => "string",
            Value::Table(_) => "table",
            Value::Function(_) => "function",
            Value::Userdata(_) => "userdata",
        }
    }

    // ----------------------------------------------------------------
    // typeof(v) — LuaU extension.
    //
    // Behaves like `type()` for primitive values.  For userdata it
    // returns the host-defined `Userdata::type_name()` string.  For
    // tables (and userdata) with a `__type` metafield whose value is a
    // string, that string is returned instead — matching LuaU's
    // `luaT_objtypename` behaviour.
    // Renamed because `typeof` is a reserved keyword in Rust.
    // ----------------------------------------------------------------
    #[function(rename = "typeof")]
    fn lua_typeof(v: Value) -> Bytes {
        match &v {
            Value::Nil => Bytes::from("nil"),
            Value::Boolean(_) => Bytes::from("boolean"),
            Value::Integer(_) | Value::Float(_) => Bytes::from("number"),
            Value::String(_) => Bytes::from("string"),
            Value::Function(_) => Bytes::from("function"),
            Value::Table(t) => match t.get_metamethod("__type") {
                Some(Value::String(s)) => s,
                _ => Bytes::from("table"),
            },
            Value::Userdata(ud) => Bytes::from(ud.type_name().as_bytes()),
        }
    }

    // ----------------------------------------------------------------
    // rawget(table, key)
    // ----------------------------------------------------------------
    #[function]
    fn rawget(table: Table, key: Value) -> Result<Value, VmError> {
        table.raw_get(&key)
    }

    // ----------------------------------------------------------------
    // rawset(table, key, value) — returns the table.
    // ----------------------------------------------------------------
    #[function]
    fn rawset(table: Table, key: Value, val: Value) -> Result<Table, VmError> {
        table.raw_set(key, val)?;
        Ok(table)
    }

    // ----------------------------------------------------------------
    // rawequal(v1, v2) — equality without metamethods.
    // ----------------------------------------------------------------
    #[function]
    fn rawequal(v1: Value, v2: Value) -> bool {
        v1 == v2
    }

    // ----------------------------------------------------------------
    // rawlen(v) — length without metamethods.  Accepts tables and strings.
    // ----------------------------------------------------------------
    #[function]
    fn rawlen(v: Value) -> Result<i64, VmError> {
        match &v {
            Value::Table(t) => Ok(t.raw_len()),
            Value::String(s) => Ok(s.len() as i64),
            _ => Err(VmError::BadArgument {
                position: 1,
                function: "rawlen".to_string(),
                expected: "table or string".to_string(),
                got: v.type_name().to_string(),
            }),
        }
    }

    // ----------------------------------------------------------------
    // tonumber(v [, base]))
    // ----------------------------------------------------------------
    #[function]
    fn tonumber(v: Value, base: Option<Value>) -> Option<crate::Number> {
        match base {
            Some(Value::Integer(b)) if b >= 2 && b <= 36 => {
                let s = match &v {
                    Value::String(s) => s.clone(),
                    _ => return None,
                };
                let s_str = String::from_utf8_lossy(&s);
                match i64::from_str_radix(s_str.trim(), b as u32) {
                    Ok(n) => Some(crate::Number::Integer(n)),
                    Err(_) => None,
                }
            }
            None | Some(Value::Nil) => match &v {
                Value::Integer(n) => Some(crate::Number::Integer(*n)),
                Value::Float(f) => Some(crate::Number::Float(*f)),
                Value::String(s) => {
                    let trimmed = String::from_utf8_lossy(s);
                    let trimmed = trimmed.trim();
                    if let Ok(n) = trimmed.parse::<i64>() {
                        Some(crate::Number::Integer(n))
                    } else if let Some(n) = parse_hex_integer(trimmed) {
                        Some(crate::Number::Integer(n))
                    } else if let Some(f) = crate::string_lib::lua_str_to_float(trimmed) {
                        Some(crate::Number::Float(f))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    // ----------------------------------------------------------------
    // tostring(v) — respects __tostring metamethod.
    // ----------------------------------------------------------------
    #[function]
    async fn tostring(ctx: CallContext, v: Value) -> Result<Bytes, VmError> {
        Ok(Bytes::from(value_tostring(&ctx, v).await?))
    }

    // ----------------------------------------------------------------
    // next(table [, key]))
    // ----------------------------------------------------------------
    #[function]
    fn next(table: Table, key: Option<Value>) -> Result<NextResult, VmError> {
        let key = key.unwrap_or(Value::Nil);
        match table.next(&key)? {
            Some((k, v)) => Ok(NextResult::Pair(k, v)),
            None => Ok(NextResult::End),
        }
    }

    // ----------------------------------------------------------------
    // getmetatable(object)
    // Respects __metatable field (Lua 5.2+ protection).
    //
    // Strings expose the shared `string` metatable installed by
    // `register_libs` (Lua 5.4 §6.4) so user code can install custom
    // operator metamethods, e.g. `__band` for bitwise coercion.
    // ----------------------------------------------------------------
    #[function]
    fn getmetatable(ctx: CallContext, obj: Value) -> Value {
        match obj {
            Value::Table(t) => match t.get_metamethod("__metatable") {
                Some(guard) => guard,
                None => match t.get_metatable() {
                    Some(mt) => Value::Table(mt),
                    None => Value::Nil,
                },
            },
            Value::String(_) => match ctx.global.get_string_metatable() {
                Some(mt) => match mt.get_metamethod("__metatable") {
                    Some(guard) => guard,
                    None => Value::Table(mt),
                },
                None => Value::Nil,
            },
            _ => Value::Nil,
        }
    }

    // ----------------------------------------------------------------
    // setmetatable(table, metatable)
    // Respects `__metatable` protection (Lua 5.2+): if the current
    // metatable has a non-nil `__metatable` field, the caller cannot
    // replace it.  The check sits in the builtin rather than in
    // `Table::set_metatable` so the VM and `debug.setmetatable` can
    // still bypass it.
    // ----------------------------------------------------------------
    #[function]
    fn setmetatable(table: Table, mt: Option<Table>) -> Result<Table, VmError> {
        if table.get_metamethod("__metatable").is_some() {
            let msg = "cannot change a protected metatable".to_owned();
            return Err(VmError::LuaError {
                display: msg.clone(),
                value: Value::string(msg),
            });
        }
        table.set_metatable(mt)?;
        Ok(table)
    }

    // ----------------------------------------------------------------
    // select(index, ...)
    // ----------------------------------------------------------------
    #[function]
    fn select(index: super::SelectIndex, rest: Variadic) -> Result<Variadic, VmError> {
        let rest = rest.0;
        match index {
            super::SelectIndex::Hash(s) if s.as_ref() == b"#" => {
                Ok(Variadic(valuevec![Value::Integer(rest.len() as i64)]))
            }
            super::SelectIndex::Hash(_) => Err(VmError::BadArgument {
                position: 1,
                function: "select".to_owned(),
                expected: "number or string \"#\"".to_owned(),
                got: "string".to_owned(),
            }),
            super::SelectIndex::Num(n) => {
                let len = rest.len() as i64;
                let idx = if n < 0 {
                    (len + n).max(0) as usize
                } else if n >= 1 {
                    (n - 1) as usize
                } else {
                    return Err(VmError::BadArgument {
                        position: 1,
                        function: "select".to_owned(),
                        expected: "index out of range".to_owned(),
                        got: "0".to_owned(),
                    });
                };
                Ok(Variadic(rest.into_iter().skip(idx).collect()))
            }
        }
    }

    // ----------------------------------------------------------------
    // error([msg [, level]])
    // level 1 (default) = position of the caller; 2 = caller's caller;
    // 0 = no position info.
    //
    // `msg` is optional: `error()` with no arguments propagates `nil`
    // as the error value (Lua 5.4 semantics — the location prefix is
    // only added when `msg` is a string).
    // ----------------------------------------------------------------
    #[function]
    fn error(
        ctx: CallContext,
        msg: Option<Value>,
        level_val: Option<Value>,
    ) -> Result<crate::Never, VmError> {
        let msg = msg.unwrap_or(Value::Nil);
        let level = match level_val {
            Some(Value::Integer(n)) => n as usize,
            Some(Value::Float(f)) => f as usize,
            _ => 1,
        };

        // Prepend "source:line: " to string messages when level > 0.
        let (display, value) = if level > 0 {
            if let Value::String(ref s) = msg {
                let stack = ctx.call_stack();
                // Level 1 = last Lua frame in the stack.
                let lua_frames: Vec<_> = stack
                    .frames_bottom_up()
                    .into_iter()
                    .filter(|f| matches!(f, StackFrame::Lua { .. }))
                    .collect();
                let loc = lua_frames
                    .len()
                    .checked_sub(level)
                    .and_then(|i| lua_frames[i].source_location());
                if let Some(loc) = loc {
                    let prefixed = Bytes::from(format!(
                        "{}:{}: {}",
                        crate::proto::format_source_name(&loc.source_name),
                        loc.line,
                        String::from_utf8_lossy(s.as_ref())
                    ));
                    let display = String::from_utf8_lossy(&prefixed).into_owned();
                    let value = Value::String(prefixed);
                    (display, value)
                } else {
                    (value_to_error_string(&msg), msg)
                }
            } else {
                (value_to_error_string(&msg), msg)
            }
        } else {
            (value_to_error_string(&msg), msg)
        };
        Err(VmError::LuaError { display, value })
    }

    // ----------------------------------------------------------------
    // assert(v [, msg, ...]))
    // Returns all arguments on success; raises an error on failure.
    // ----------------------------------------------------------------
    #[function]
    fn assert(args: Variadic) -> Result<Variadic, VmError> {
        let args = args.0;
        let v = args.first().cloned().unwrap_or(Value::Nil);
        if v.is_truthy() {
            // Return all arguments on success.
            Ok(Variadic(args))
        } else {
            let msg = args
                .into_iter()
                .nth(1)
                .unwrap_or_else(|| Value::string("assertion failed!"));
            let display = value_to_error_string(&msg);
            Err(VmError::LuaError {
                display,
                value: msg,
            })
        }
    }

    // ----------------------------------------------------------------
    // pairs(table)
    // Returns (next, table, nil) for use with generic for.
    // Respects __pairs metamethod (Lua 5.2).
    // ----------------------------------------------------------------
    #[function]
    async fn pairs(ctx: CallContext, table: Table) -> Result<super::PairsResult, VmError> {
        use super::PairsResult;
        // Lua 5.2: if __pairs is defined on the table's metatable,
        // call it with the table and return its results directly.
        if let Some(Value::Function(mm)) = table.get_metamethod("__pairs") {
            let results = ctx
                .call_function(mm, valuevec![Value::Table(table)])
                .await?;
            return Ok(PairsResult::Metamethod(Variadic(results)));
        }
        let next_fn = match ctx.global.get_global("next") {
            Some(Value::Function(f)) => f,
            _ => {
                return Err(VmError::LuaError {
                    display: "'next' is not a function".into(),
                    value: Value::string("'next' is not a function"),
                })
            }
        };
        Ok(PairsResult::Standard(next_fn, table))
    }

    // ----------------------------------------------------------------
    // ipairs(table)
    // Returns (iter, table, 0) for sequential integer-keyed iteration.
    // Respects __ipairs metamethod (Lua 5.2).
    // In Lua 5.3+ __ipairs was removed; instead ipairs uses __index, so
    // the inner iterator goes through ctx.call_function which dispatches
    // __index at the VM level.
    // ----------------------------------------------------------------
    #[function]
    async fn ipairs(ctx: CallContext, table: Table) -> Result<super::IpairsResult, VmError> {
        use super::{IpairsIterResult, IpairsResult};
        // Lua 5.2: if __ipairs is defined, delegate entirely.
        // The metamethod can return arbitrary values (e.g. nil as
        // the control variable), so we use Variadic for the return.
        if let Some(Value::Function(mm)) = table.get_metamethod("__ipairs") {
            let results = ctx
                .call_function(mm, valuevec![Value::Table(table)])
                .await?;
            return Ok(IpairsResult::Metamethod(Variadic(results)));
        }
        // Lua 5.3+: the iterator uses raw table access (integer keys
        // only); __index is not consulted during ipairs iteration per
        // the 5.3 spec.  We use raw_get here to match that behaviour.
        // Return the same stateless iterator function each time (Lua 5.4
        // conformance: `ipairs{} == ipairs{}` is true).
        use std::sync::LazyLock;
        static IPAIRS_ITER: LazyLock<Function> = LazyLock::new(|| {
            Function::wrap(
                "ipairs_iter",
                |tab: Table, idx: i64| -> Result<IpairsIterResult, VmError> {
                    // Wrap on overflow per Lua 5.4: from `maxinteger`
                    // the next key is `mininteger`.  Termination is
                    // still well-defined because the loop ends when
                    // the next index is absent from the table.
                    let idx = idx.wrapping_add(1);
                    let v = tab.raw_get(&Value::Integer(idx))?;
                    if v.is_nil() {
                        Ok(IpairsIterResult::End)
                    } else {
                        Ok(IpairsIterResult::Item(idx, v))
                    }
                },
            )
        });
        Ok(IpairsResult::Standard(IPAIRS_ITER.clone(), table, 0))
    }

    // ----------------------------------------------------------------
    // print(...)
    // Calls tostring() on each argument (respecting __tostring),
    // writes them tab-separated to stdout, followed by a newline.
    // ----------------------------------------------------------------
    #[function]
    async fn print(ctx: CallContext, args: Variadic) -> Result<(), VmError> {
        let mut parts = Vec::with_capacity(args.0.len());
        for v in args.0 {
            let s = value_tostring(&ctx, v).await?;
            parts.push(s);
        }
        let line = parts.join("\t");
        println!("{}", line);
        Ok(())
    }

    // ----------------------------------------------------------------
    // collectgarbage([opt [, arg]]))
    // ----------------------------------------------------------------
    #[function]
    async fn collectgarbage(
        ctx: CallContext,
        opt: Option<Bytes>,
    ) -> Result<super::CollectGarbageResult, VmError> {
        use super::CollectGarbageResult;
        let opt = opt.unwrap_or_else(|| Bytes::from("collect"));
        match opt.as_ref() {
            b"collect" => {
                // Synchronous mark-and-sweep.
                ctx.global.collect_cycles();
                // Run any __gc finalizers found during sweep.
                let queue = ctx.global.take_pending_finalizers();
                for (table, gc_fn) in queue {
                    let _ = ctx
                        .call_function(gc_fn, valuevec![Value::Table(table)])
                        .await;
                }
                Ok(CollectGarbageResult::Integer(0))
            }
            b"count" => Ok(CollectGarbageResult::Count(0.0, 0.0)),
            b"isrunning" => Ok(CollectGarbageResult::Running(true)),
            // "stop", "restart", "step", "setpause",
            // "setstepmul", "incremental", "generational" → 0
            _ => Ok(CollectGarbageResult::Integer(0)),
        }
    }
}

fn parse_hex_integer(s: &str) -> Option<i64> {
    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };
    let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    // Only pure hex digits (no dot or exponent)
    if hex.contains('.') || hex.contains('p') || hex.contains('P') {
        return None;
    }
    // Wrap modularly per Lua 5.4 §3.1: hex literals (and `tonumber`
    // on hex strings) yield an i64 even when the value exceeds the
    // signed range.
    let n = shingetsu_vm::Number::parse_hex_integer_wrapping(hex)?;
    Some(n.wrapping_mul(if negative { -1 } else { 1 }))
}

/// Install the macro-generated builtins and sandbox-safe standard library
/// modules (math, string, table, utf8) as globals on `env`.
///
/// This does **not** register `os` or `io` — call [`crate::os_lib::register`],
/// [`crate::io_lib::register`], etc. separately for those.
pub fn register_sandboxed(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = builtins::build_module_table(env)?;
    env.register_from_table(&table)?;

    // Sandbox-safe standard library modules.
    crate::math_lib::register(env)?;
    crate::string_lib::register(env)?;
    crate::table_lib::register(env)?;
    crate::utf8_lib::register(env)?;

    // Populate the `loaded` cache so that `require("math")` etc. works.
    for name in ["math", "string", "table", "utf8"] {
        if let Some(v) = env.get_global(name) {
            env.set_loaded(name, v);
        }
    }

    Ok(())
}

/// First argument to `load`: either a source string or a reader function.
#[derive(crate::FromLua, crate::LuaTyped)]
enum LoadChunk {
    Source(Bytes),
    Reader(crate::function::Function),
}

/// Return type for `load`: `(function)` on success, `(nil, errmsg)` on
/// failure.
#[derive(crate::IntoLuaMulti)]
enum LoadResult {
    Ok(crate::function::Function),
    Err(Value, Bytes),
}

impl LoadResult {
    fn error(msg: impl Into<Bytes>) -> Self {
        LoadResult::Err(Value::Nil, msg.into())
    }
}

impl LoadChunk {
    /// Collect source text and a default chunkname from this chunk.
    async fn into_source(self, ctx: &CallContext) -> Result<(String, String), LoadResult> {
        match self {
            LoadChunk::Source(s) => {
                let source = String::from_utf8(s.to_vec())
                    .map_err(|_| LoadResult::error("load: chunk is not valid UTF-8"))?;
                let default_name = source.clone();
                Ok((source, default_name))
            }
            LoadChunk::Reader(reader) => {
                let mut buf = Vec::new();
                loop {
                    let results = ctx
                        .call_function(reader.clone(), valuevec![])
                        .await
                        .map_err(|re| LoadResult::error(re.error.to_string()))?;
                    match results.into_iter().next() {
                        Some(Value::String(s)) if !s.is_empty() => {
                            buf.extend_from_slice(&s);
                        }
                        _ => break,
                    }
                }
                let source = String::from_utf8(buf)
                    .map_err(|_| LoadResult::error("load: chunk is not valid UTF-8"))?;
                Ok((source, "=(load)".to_owned()))
            }
        }
    }
}

/// Read source text from a file.  Returns an error if `filename` is `None`.
async fn read_file_source(filename: Option<&[u8]>) -> Result<(String, String), String> {
    match filename {
        Some(name) => {
            let path =
                crate::io_lib::bytes_to_path(name).map_err(|e| format!("cannot open file: {e}"))?;
            let display = path.display().to_string();
            let source = tokio::fs::read_to_string(&path).await.map_err(|e| {
                let desc = shingetsu_vm::error::portable_io_error_description(&e);
                format!("cannot open {display}: {desc}")
            })?;
            let chunkname = format!("@{display}");
            Ok((source, chunkname))
        }
        None => Err("filename required".to_owned()),
    }
}

/// Format a `CompileError` for inclusion in a runtime error message,
/// preserving its `help:` text on a second line.  Used by `load`,
/// `loadfile`, and `dofile`, where the error surfaces via the runtime
/// diagnostic path rather than `render_compile_error`.
fn format_compile_error(err: &shingetsu_compiler::CompileError) -> String {
    let base = err.to_string();
    match err {
        shingetsu_compiler::CompileError::Semantic {
            help: Some(help), ..
        } => format!("{base}\nhelp: {help}"),
        _ => base,
    }
}

/// Shared compile-and-wrap logic used by `load`, `loadfile`, and `dofile`.
async fn compile_chunk(
    ctx: &CallContext,
    source: String,
    chunkname: String,
    mode: Option<Bytes>,
    env_table: Option<Table>,
) -> Result<LoadResult, VmError> {
    let mode = mode
        .map(|s| String::from_utf8_lossy(&s).into_owned())
        .unwrap_or_else(|| "t".to_owned());

    if !mode.contains('t') {
        return Ok(LoadResult::error(format!(
            "attempt to load a text chunk (mode is '{mode}')"
        )));
    }

    let opts = shingetsu_compiler::CompileOptions {
        debug_info: true,
        source_name: Arc::new(chunkname),
        type_check: false,
    };
    let compiler = shingetsu_compiler::Compiler::new(opts, ctx.global.global_type_map());
    let bc = match compiler.compile(&source).await {
        Ok(bc) => bc,
        Err(e) => return Ok(LoadResult::error(format_compile_error(&e))),
    };

    // Use `lua_with_env` unconditionally so the closure's `_ENV`
    // upvalue is initialised from the start.  Without an explicit
    // env arg, default to the host's `_G` so the loaded chunk shares
    // the caller's globals — matching Lua 5.4 semantics for `load`.
    let env_tbl = env_table.unwrap_or_else(|| ctx.global.env_table());
    let func = crate::function::Function::lua_with_env(bc.top_level, vec![], env_tbl);

    Ok(LoadResult::Ok(func))
}

#[crate::module(name = "load_mod")]
mod load_mod {
    use super::*;

    /// Compiles a chunk of Lua source and returns it as a callable function.
    ///
    /// ```lua
    /// local f, err = load(chunk [, chunkname [, mode [, env]]])
    /// ```
    ///
    /// `chunk` is either a string containing source code or a function that
    /// is called repeatedly to produce the source piece by piece.  When
    /// `chunk` is a function, it is called with no arguments and must return
    /// a string; an empty string, `nil`, or a non-string value signals the
    /// end of the source.  All returned pieces are concatenated, so a
    /// multi-byte UTF-8 sequence may safely span two consecutive pieces.
    /// The final assembled source must be valid UTF-8.
    ///
    /// `chunkname` names the chunk for use in error messages and
    /// `debug.getinfo`.  It follows Lua 5.4's source-name conventions:
    ///
    /// * **`@path`** — a file path.  The leading `@` is stripped for
    ///   display; long paths are truncated from the front (e.g.
    ///   `...ong/path/to/file.lua`).
    /// * **`=label`** — an embedder-defined label.  The leading `=` is
    ///   stripped for display; long labels are truncated from the end.
    /// * **anything else** — treated as literal source text.  Displayed
    ///   as `[string "first line..."]`, truncated to 60 characters.
    ///
    /// When `chunkname` is omitted the default depends on `chunk`:
    /// for a string chunk, the source text itself (shown in
    /// `[string "..."]` form); for a reader function, `=(load)`.
    ///
    /// `mode` controls what kind of chunks are accepted.  It must contain
    /// the letter `t` (text) to accept source code.  Binary chunks are not
    /// currently supported, so a mode of `"b"` alone will always fail.
    /// Defaults to `"t"` when omitted.
    ///
    /// `env` sets the `_ENV` table for the loaded chunk, controlling where
    /// global variable reads and writes go.  When omitted, the chunk uses
    /// the caller's global environment.  Closures defined inside the loaded
    /// chunk inherit the same `env`.
    ///
    /// On success, returns the compiled function.  On failure (syntax error,
    /// invalid mode, invalid UTF-8), returns `nil` plus an error message.
    ///
    /// Gated behind `Libraries::LOAD`; not available in `Libraries::SANDBOXED`.
    #[function]
    async fn load(
        ctx: CallContext,
        chunk: super::LoadChunk,
        chunkname: Option<Bytes>,
        mode: Option<Bytes>,
        env_table: Option<Table>,
    ) -> Result<super::LoadResult, VmError> {
        let (source, default_name) = match chunk.into_source(&ctx).await {
            Ok(pair) => pair,
            Err(lr) => return Ok(lr),
        };

        let chunkname = chunkname
            .map(|s| String::from_utf8_lossy(&s).into_owned())
            .unwrap_or(default_name);

        super::compile_chunk(&ctx, source, chunkname, mode, env_table).await
    }

    /// Compiles a Lua source file and returns it as a callable function.
    ///
    /// ```lua
    /// local f, err = loadfile([filename [, mode [, env]]])
    /// ```
    ///
    /// Reads the contents of `filename` and compiles them as a Lua chunk.
    /// The file path is used as the chunk name with an `@` prefix, so
    /// error messages display the file path directly.
    ///
    /// `filename` is required; omitting it raises an error.
    ///
    /// `mode` controls what kind of chunks are accepted (see `load()`).
    /// Defaults to `"t"` (text only).
    ///
    /// `env` sets the `_ENV` table for the loaded chunk (see `load()`).
    ///
    /// On success, returns the compiled function.  On failure (I/O error,
    /// syntax error, invalid mode), returns `nil` plus an error message.
    ///
    /// Gated behind `Libraries::LOAD`; not available in `Libraries::SANDBOXED`.
    #[function]
    async fn loadfile(
        ctx: CallContext,
        filename: Option<Bytes>,
        mode: Option<Bytes>,
        env_table: Option<Table>,
    ) -> Result<super::LoadResult, VmError> {
        let (source, chunkname) = match super::read_file_source(filename.as_deref()).await {
            Ok(pair) => pair,
            Err(msg) => return Ok(super::LoadResult::error(msg)),
        };

        super::compile_chunk(&ctx, source, chunkname, mode, env_table).await
    }

    /// Executes a Lua source file and returns all its results.
    ///
    /// ```lua
    /// local results... = dofile([filename])
    /// ```
    ///
    /// Opens, compiles, and immediately executes the named file.  All
    /// values returned by the chunk are returned by `dofile`.  Errors
    /// during reading, compilation, or execution propagate to the caller
    /// (they are not caught).
    ///
    /// `filename` is required; omitting it raises an error.
    ///
    /// Gated behind `Libraries::LOAD`; not available in `Libraries::SANDBOXED`.
    #[function(variadic)]
    async fn dofile(
        ctx: CallContext,
        filename: Option<Bytes>,
    ) -> Result<shingetsu_vm::Variadic, VmError> {
        let (source, chunkname) =
            super::read_file_source(filename.as_deref())
                .await
                .map_err(|msg| VmError::LuaError {
                    display: msg.clone(),
                    value: Value::string(msg),
                })?;

        let func = match super::compile_chunk(&ctx, source, chunkname, None, None).await? {
            super::LoadResult::Ok(f) => f,
            super::LoadResult::Err(_, msg) => {
                let display = String::from_utf8_lossy(&msg).into_owned();
                return Err(VmError::LuaError {
                    display: display.clone(),
                    value: Value::String(msg),
                });
            }
        };

        let results = ctx
            .call_function(func, valuevec![])
            .await
            .map_err(|re| re.error)?;
        Ok(shingetsu_vm::Variadic(results))
    }
}
///
/// Gated behind [`Libraries::LOAD`] because it can execute arbitrary
/// code from untrusted strings (excluded from sandboxed mode,
/// following Luau convention).
pub fn register_load(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = load_mod::build_module_table(env)?;
    env.register_from_table(&table)
}

/// Install all builtins and standard library modules as globals on `env`.
///
/// This is a convenience that calls [`register_sandboxed`] plus
/// [`crate::os_lib::register`].
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    register_sandboxed(env)?;
    crate::os_lib::register(env)?;

    // Populate `loaded` for non-sandboxed libraries.
    for name in ["os", "io", "coroutine", "debug", "package"] {
        if let Some(v) = env.get_global(name) {
            env.set_loaded(name, v);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shingetsu_compiler::{CompileError, SourceLocation};
    use std::sync::Arc;

    fn loc(line: u32, column: u32, byte_len: u32) -> SourceLocation {
        SourceLocation {
            source_name: Arc::new("chunk.lua".to_string()),
            line,
            column,
            byte_offset: 0,
            byte_len,
        }
    }

    #[test]
    fn format_compile_error_appends_help() {
        let err = CompileError::Semantic {
            location: loc(65537, 3, 5),
            message: "too many constants in chunk (limit: 65535)".to_string(),
            help: Some("split large literal table constructors".to_string()),
        };
        k9::assert_equal!(
            format_compile_error(&err),
            "[string \"chunk.lua\"]:65537:3: too many constants in chunk (limit: 65535)\n\
             help: split large literal table constructors"
        );
    }

    #[test]
    fn format_compile_error_no_help_unchanged() {
        let err = CompileError::Parse {
            location: loc(1, 1, 1),
            message: "unexpected token".to_string(),
        };
        k9::assert_equal!(
            format_compile_error(&err),
            "[string \"chunk.lua\"]:1:1: unexpected token"
        );
    }
}
