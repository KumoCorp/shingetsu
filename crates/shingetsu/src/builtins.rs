//! Core Lua built-in functions expressed via the `#[module]` proc macro.
//!
//! Call [`register`] to install these into a [`GlobalEnv`].  The VM-level
//! builtins that cannot be expressed through the macro (`pcall`, `xpcall`,
//! `require`) are registered separately by `GlobalEnv::register_builtins`.

use std::sync::Arc;

use bytes::Bytes;

use crate::call_context::{CallContext, StackFrame};
use crate::error::VmError;
use crate::global_env::value_to_error_string;
use crate::table::Table;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a value to its string representation, respecting `__tostring`.
async fn value_tostring(ctx: &CallContext, v: Value) -> Result<String, VmError> {
    // Check __tostring metamethod on tables.
    if let Value::Table(ref t) = v {
        if let Some(Value::Function(mm)) = t.get_metamethod("__tostring") {
            let results = ctx.call_function(mm, vec![v]).await?;
            let s = results.into_iter().next().unwrap_or(Value::Nil);
            return Ok(s.to_string());
        }
    }
    // Dispatch __tostring on userdata via its dispatch mechanism.
    if let Value::Userdata(ref ud) = v {
        let results = Arc::clone(ud)
            .dispatch(ctx.clone(), "__tostring", vec![v])
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
        match &v {
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
    fn rawlen(v: Value) -> Result<Value, VmError> {
        match &v {
            Value::Table(t) => Ok(Value::Integer(t.raw_len())),
            Value::String(s) => Ok(Value::Integer(s.len() as i64)),
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
    fn tonumber(v: Value, base: Option<Value>) -> Value {
        match base {
            Some(Value::Integer(b)) if b >= 2 && b <= 36 => {
                let s = match &v {
                    Value::String(s) => s.clone(),
                    _ => return Value::Nil,
                };
                let s_str = String::from_utf8_lossy(&s);
                match i64::from_str_radix(s_str.trim(), b as u32) {
                    Ok(n) => Value::Integer(n),
                    Err(_) => Value::Nil,
                }
            }
            None | Some(Value::Nil) => match &v {
                Value::Integer(n) => Value::Integer(*n),
                Value::Float(f) => Value::Float(*f),
                Value::String(s) => {
                    let trimmed = String::from_utf8_lossy(s);
                    let trimmed = trimmed.trim();
                    if let Ok(n) = trimmed.parse::<i64>() {
                        Value::Integer(n)
                    } else if let Some(f) = crate::string_lib::lua_str_to_float(trimmed) {
                        Value::Float(f)
                    } else {
                        Value::Nil
                    }
                }
                _ => Value::Nil,
            },
            _ => Value::Nil,
        }
    }

    // ----------------------------------------------------------------
    // tostring(v) — respects __tostring metamethod.
    // ----------------------------------------------------------------
    #[function]
    async fn tostring(ctx: CallContext, v: Value) -> Result<Value, VmError> {
        Ok(Value::String(Bytes::from(value_tostring(&ctx, v).await?)))
    }

    // ----------------------------------------------------------------
    // next(table [, key]))
    // ----------------------------------------------------------------
    #[function]
    fn next(table: Table, key: Option<Value>) -> Result<Variadic, VmError> {
        let key = key.unwrap_or(Value::Nil);
        match table.next(&key)? {
            Some((k, v)) => Ok(Variadic(vec![k, v])),
            None => Ok(Variadic(vec![Value::Nil])),
        }
    }

    // ----------------------------------------------------------------
    // getmetatable(object)
    // Respects __metatable field (Lua 5.2+ protection).
    // ----------------------------------------------------------------
    #[function]
    fn getmetatable(obj: Value) -> Value {
        match obj {
            Value::Table(t) => match t.get_metamethod("__metatable") {
                Some(guard) => guard,
                None => match t.get_metatable() {
                    Some(mt) => Value::Table(mt),
                    None => Value::Nil,
                },
            },
            _ => Value::Nil,
        }
    }

    // ----------------------------------------------------------------
    // setmetatable(table, metatable)
    // ----------------------------------------------------------------
    #[function]
    fn setmetatable(table: Table, mt: Option<Table>) -> Table {
        table.set_metatable(mt);
        table
    }

    // ----------------------------------------------------------------
    // select(index, ...)
    // ----------------------------------------------------------------
    #[function]
    fn select(index: Value, rest: Variadic) -> Result<Variadic, VmError> {
        let rest = rest.0;
        match index {
            Value::String(s) if s.as_ref() == b"#" => {
                Ok(Variadic(vec![Value::Integer(rest.len() as i64)]))
            }
            Value::Integer(n) => {
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
            other => Err(VmError::BadArgument {
                position: 1,
                function: "select".to_owned(),
                expected: "number or string \"#\"".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }

    // ----------------------------------------------------------------
    // error(msg [, level]))
    // level 1 (default) = position of the caller; 2 = caller's caller;
    // 0 = no position info.
    // ----------------------------------------------------------------
    #[function]
    fn error(ctx: CallContext, msg: Value, level_val: Option<Value>) -> Result<Value, VmError> {
        let level = match level_val {
            Some(Value::Integer(n)) => n as usize,
            Some(Value::Float(f)) => f as usize,
            _ => 1,
        };

        // Prepend "source:line: " to string messages when level > 0.
        let (display, value) = if level > 0 {
            if let Value::String(ref s) = msg {
                let stack = &ctx.call_stack;
                // Level 1 = last Lua frame in the stack.
                let lua_frames: Vec<_> = stack
                    .iter()
                    .filter(|f| matches!(f, StackFrame::Lua { .. }))
                    .collect();
                let loc = lua_frames.len().checked_sub(level).and_then(|i| {
                    if let StackFrame::Lua {
                        source_location, ..
                    } = lua_frames[i]
                    {
                        source_location.as_ref()
                    } else {
                        None
                    }
                });
                if let Some(loc) = loc {
                    let prefixed = Bytes::from(format!(
                        "{}:{}: {}",
                        loc.source_name,
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
                .unwrap_or_else(|| Value::String(Bytes::from_static(b"assertion failed!")));
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
    async fn pairs(ctx: CallContext, table: Table) -> Result<Variadic, VmError> {
        // Lua 5.2: if __pairs is defined on the table's metatable,
        // call it with the table and return its results directly.
        if let Some(Value::Function(mm)) = table.get_metamethod("__pairs") {
            let results = ctx.call_function(mm, vec![Value::Table(table)]).await?;
            return Ok(Variadic(results));
        }
        let next_fn = ctx.global.get_global("next").unwrap_or(Value::Nil);
        Ok(Variadic(vec![next_fn, Value::Table(table), Value::Nil]))
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
    async fn ipairs(ctx: CallContext, table: Table) -> Result<Variadic, VmError> {
        // Lua 5.2: if __ipairs is defined, delegate entirely.
        // The metamethod can return arbitrary values (e.g. nil as
        // the control variable), so we use Variadic for the return.
        if let Some(Value::Function(mm)) = table.get_metamethod("__ipairs") {
            let results = ctx.call_function(mm, vec![Value::Table(table)]).await?;
            return Ok(Variadic(results));
        }
        // Lua 5.3+: the iterator uses raw table access (integer keys
        // only); __index is not consulted during ipairs iteration per
        // the 5.3 spec.  We use raw_get here to match that behaviour.
        let iter_fn = Function::wrap(
            "ipairs_iter",
            |tab: Table, idx: i64| -> Result<Variadic, VmError> {
                let idx = idx + 1;
                let v = tab.raw_get(&Value::Integer(idx))?;
                if v.is_nil() {
                    Ok(Variadic(vec![Value::Nil]))
                } else {
                    Ok(Variadic(vec![Value::Integer(idx), v]))
                }
            },
        );
        Ok(Variadic(vec![
            Value::Function(iter_fn),
            Value::Table(table),
            Value::Integer(0),
        ]))
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
    async fn collectgarbage(ctx: CallContext, opt: Option<Value>) -> Result<Variadic, VmError> {
        let opt = opt.unwrap_or_else(|| Value::String(Bytes::from_static(b"collect")));
        match &opt {
            Value::String(s) => match s.as_ref() {
                b"collect" => {
                    // Synchronous mark-and-sweep.
                    ctx.global.collect_cycles();
                    // Run any __gc finalizers found during sweep.
                    let queue = ctx.global.take_pending_finalizers();
                    for (table, gc_fn) in queue {
                        let _ = ctx.call_function(gc_fn, vec![Value::Table(table)]).await;
                    }
                    Ok(Variadic(vec![Value::Integer(0)]))
                }
                b"count" => Ok(Variadic(vec![Value::Float(0.0), Value::Float(0.0)])),
                b"isrunning" => Ok(Variadic(vec![Value::Boolean(true)])),
                // "stop", "restart", "step", "setpause",
                // "setstepmul", "incremental", "generational" → 0
                _ => Ok(Variadic(vec![Value::Integer(0)])),
            },
            _ => Ok(Variadic(vec![Value::Integer(0)])),
        }
    }
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

    Ok(())
}

/// Install all builtins and standard library modules as globals on `env`.
///
/// This is a convenience that calls [`register_sandboxed`] plus
/// [`crate::os_lib::register`].
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    register_sandboxed(env)?;
    crate::os_lib::register(env)?;

    Ok(())
}
