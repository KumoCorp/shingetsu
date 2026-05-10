//! Core Lua built-in functions.

use crate::valuevec;
use std::sync::Arc;

use shingetsu::Bytes;

use crate::call_context::CallContext;
use crate::call_stack::StackFrame;
use crate::global_env::value_to_error_string;
use crate::table::Table;
use crate::value::Value;
use crate::VmError;
use shingetsu_vm::error::VmResultExt;

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
    Standard(crate::Function, crate::table::Table),
    Metamethod(crate::convert::Variadic),
}

/// Return type for `ipairs`: `(iter_fn, table, 0)` or metamethod results.
#[derive(crate::IntoLuaMulti)]
enum IpairsResult {
    Standard(crate::Function, crate::table::Table, i64),
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

/// Global functions that are always available without requiring a module.
///
/// These are the values bound directly into `_G`. They cover type
/// inspection (`type`, `typeof`), table primitives that bypass
/// metamethods (`rawget`, `rawset`, `rawequal`, `rawlen`), conversion
/// (`tonumber`, `tostring`), iteration (`next`, `pairs`, `ipairs`,
/// `select`), error handling (`error`, `assert`), metatable manipulation
/// (`getmetatable`, `setmetatable`), I/O (`print`), and garbage-collector
/// control (`collectgarbage`).
///
/// The chunk-loading builtins (`load`, `loadfile`, `dofile`) live in a
/// separate module gated behind `Libraries::LOAD`; the protected-call
/// builtins (`pcall`, `xpcall`) and `require` are implemented at the VM
/// level rather than via this macro, but appear alongside these in `_G`.
#[crate::module(name = "builtins")]
mod builtins {
    use super::*;
    use crate::convert::Variadic;
    use crate::Function;

    /// Returns the basic Lua type of `v` as a string.
    ///
    /// One of `"nil"`, `"boolean"`, `"number"`, `"string"`, `"table"`,
    /// `"function"`, or `"userdata"`. Both integers and floats are
    /// reported as `"number"`. This is unaffected by metatables — use
    /// `typeof` if you want LuaU-style `__type` overrides.
    ///
    /// # Parameters
    /// - `v` (any): the value to inspect.
    ///
    /// # Returns
    /// (string): the type name.
    ///
    /// # Examples
    /// ```lua
    /// assert(type(nil) == "nil")
    /// assert(type(true) == "boolean")
    /// assert(type(42) == "number")
    /// assert(type(3.14) == "number")
    /// assert(type("hi") == "string")
    /// assert(type({}) == "table")
    /// assert(type(print) == "function")
    /// ```
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

    /// Returns the type of `v`, honouring `__type` metafield overrides.
    ///
    /// Behaves like `type` for primitive values. For userdata, returns
    /// the host-defined type name. For tables (or userdata) whose
    /// metatable has a string `__type` field, that string is returned
    /// instead — useful for class-like patterns where you want
    /// `typeof(point) == "Point"` rather than `"table"`.
    ///
    /// `typeof` is a LuaU extension; standard Lua only provides `type`.
    ///
    /// # Parameters
    /// - `v` (any): the value to inspect.
    ///
    /// # Returns
    /// (string): the type name, possibly overridden via `__type`.
    ///
    /// # Examples
    /// ```lua
    /// assert(typeof(42) == "number")
    /// assert(typeof("hi") == "string")
    ///
    /// local Point = setmetatable({}, { __type = "Point" })
    /// assert(type(Point) == "table")
    /// assert(typeof(Point) == "Point")
    /// ```
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

    /// Reads `table[key]` without invoking the `__index` metamethod.
    ///
    /// # Parameters
    /// - `table` (table): the table to read from.
    /// - `key` (any): the key to look up.
    ///
    /// # Returns
    /// (any): the raw value stored under `key`, or `nil` if absent.
    ///
    /// # Examples
    /// ```lua
    /// local t = setmetatable({ x = 1 }, { __index = function() return 99 end })
    /// assert(t.missing == 99)         -- via __index
    /// assert(rawget(t, "missing") == nil) -- bypasses __index
    /// assert(rawget(t, "x") == 1)
    /// ```
    #[function]
    fn rawget(table: Table, key: Value) -> Result<Value, VmError> {
        table.raw_get(&key)
    }

    /// Writes `value` into `table[key]` without invoking `__newindex`.
    ///
    /// Returns the table so calls can be chained.
    ///
    /// # Parameters
    /// - `table` (table): the table to modify.
    /// - `key` (any): the key to set (must not be `nil` or `NaN`).
    /// - `value` (any): the value to store; `nil` removes the entry.
    ///
    /// # Returns
    /// (table): the same table.
    ///
    /// # Examples
    /// ```lua
    /// local t = setmetatable({}, { __newindex = function() error("frozen") end })
    /// rawset(t, "x", 1) -- bypasses __newindex
    /// assert(rawget(t, "x") == 1)
    /// ```
    #[function]
    fn rawset(table: Table, key: Value, val: Value) -> Result<Table, VmError> {
        table.raw_set(key, val)?;
        Ok(table)
    }

    /// Compares two values for equality without invoking `__eq`.
    ///
    /// Two values are raw-equal when they are the same primitive value
    /// or refer to the exact same object. Tables, functions, and
    /// userdata compare by identity.
    ///
    /// # Parameters
    /// - `v1` (any): the first value.
    /// - `v2` (any): the second value.
    ///
    /// # Returns
    /// (boolean): `true` if raw-equal, otherwise `false`.
    ///
    /// # Examples
    /// ```lua
    /// assert(rawequal(1, 1))
    /// assert(rawequal("hi", "hi"))
    /// local a, b = {}, {}
    /// assert(rawequal(a, a))
    /// assert(not rawequal(a, b))
    /// ```
    #[function]
    fn rawequal(v1: Value, v2: Value) -> bool {
        v1 == v2
    }

    /// Returns the length of a table or string without invoking `__len`.
    ///
    /// For strings, the result is the byte count. For tables, the result
    /// is the array-part border length. Other types raise an error.
    ///
    /// # Parameters
    /// - `v` (table | string): the value to measure.
    ///
    /// # Returns
    /// (integer): the raw length.
    ///
    /// # Examples
    /// ```lua
    /// assert(rawlen("hello") == 5)
    /// assert(rawlen({ 10, 20, 30 }) == 3)
    /// local t = setmetatable({1, 2}, { __len = function() return 99 end })
    /// assert(#t == 99)
    /// assert(rawlen(t) == 2) -- ignores __len
    /// ```
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

    /// Converts a value to a number, returning `nil` if it cannot.
    ///
    /// Without `base`, accepts:
    /// - integers and floats (returned unchanged),
    /// - strings holding a decimal integer, decimal float, or hex
    ///   integer literal (`0x...` / `0X...`), with optional sign and
    ///   surrounding whitespace.
    ///
    /// With `base` (2..=36), `v` must be a string of digits valid in
    /// that base; the result is always an integer.
    ///
    /// `"inf"`, `"nan"`, and similar are rejected, matching reference
    /// Lua's `lua_stringtonumber`.
    ///
    /// # Parameters
    /// - `v` (any): the value to convert.
    /// - `base` (integer, optional): radix in `2..=36` for string parsing.
    ///
    /// # Returns
    /// (number | nil): the converted number, or `nil` on failure.
    ///
    /// # Examples
    /// ```lua
    /// assert(tonumber("42") == 42)
    /// assert(tonumber("3.14") == 3.14)
    /// assert(tonumber("0xff") == 255)
    /// assert(tonumber("  -7  ") == -7)
    /// assert(tonumber("ff", 16) == 255)
    /// assert(tonumber("101", 2) == 5)
    /// assert(tonumber("abc") == nil)
    /// ```
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

    /// Converts any value to a string for display.
    ///
    /// Honours the `__tostring` metamethod on tables and userdata,
    /// allowing custom string representations. For values without a
    /// metamethod, returns a default rendering: numbers in their natural
    /// form, booleans as `"true"` / `"false"`, `nil` as `"nil"`, and
    /// non-printable values as a type name plus identity (e.g.
    /// `"table: 0x..."`).
    ///
    /// # Parameters
    /// - `v` (any): the value to convert.
    ///
    /// # Returns
    /// (string): the textual representation.
    ///
    /// # Examples
    /// ```lua
    /// assert(tostring(42) == "42")
    /// assert(tostring(true) == "true")
    /// assert(tostring(nil) == "nil")
    ///
    /// local p = setmetatable({ x = 1, y = 2 }, {
    ///     __tostring = function(self) return "("..self.x..","..self.y..")" end
    /// })
    /// assert(tostring(p) == "(1,2)")
    /// ```
    #[function]
    async fn tostring(ctx: CallContext, v: Value) -> Result<Bytes, VmError> {
        Ok(Bytes::from(value_tostring(&ctx, v).await?))
    }

    /// Returns the next key-value pair after `key` in iteration order.
    ///
    /// With `key` omitted or `nil`, returns the first pair. When there
    /// are no more pairs, returns `nil`. The traversal order is
    /// implementation-defined and stable only as long as the table is
    /// not mutated mid-iteration. Setting an existing key to `nil`
    /// during traversal is allowed; adding new keys is not.
    ///
    /// `next` is the primitive that `pairs` is built on — most code
    /// should use `for k, v in pairs(t) do` instead of calling `next`
    /// directly.
    ///
    /// # Parameters
    /// - `table` (table): the table to traverse.
    /// - `key` (any, optional): the previous key (default `nil`).
    ///
    /// # Returns
    /// On a pair: `(key, value)`. At the end: `nil`.
    ///
    /// # Examples
    /// ```lua
    /// local t = { x = 10 }
    /// local k, v = next(t)
    /// assert(k == "x" and v == 10)
    /// assert(next(t, k) == nil)
    /// ```
    #[function]
    fn next(table: Table, key: Option<Value>) -> Result<NextResult, VmError> {
        let key = key.unwrap_or(Value::Nil);
        match table.next(&key)? {
            Some((k, v)) => Ok(NextResult::Pair(k, v)),
            None => Ok(NextResult::End),
        }
    }

    /// Returns the metatable of `obj`, or `nil` when none is set.
    ///
    /// If the object's metatable has a `__metatable` field, that field's
    /// value is returned instead of the actual metatable — the standard
    /// way to make a metatable opaque to user code. Strings share a
    /// single metatable installed by the runtime, exposed here so user
    /// code can register custom operator metamethods on it.
    ///
    /// # Parameters
    /// - `obj` (any): the object whose metatable to fetch.
    ///
    /// # Returns
    /// (table | any | nil): the metatable, the `__metatable` guard
    /// value, or `nil` if `obj` has no metatable.
    ///
    /// # Examples
    /// ```lua
    /// local t = setmetatable({}, { __index = function() return 0 end })
    /// assert(type(getmetatable(t)) == "table")
    ///
    /// local guarded = setmetatable({}, { __metatable = "locked" })
    /// assert(getmetatable(guarded) == "locked")
    ///
    /// assert(getmetatable({}) == nil)
    /// ```
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

    /// Installs `mt` as the metatable of `table` and returns `table`.
    ///
    /// Passing `nil` for `mt` removes the existing metatable. If the
    /// current metatable has a `__metatable` field, the call raises an
    /// error ("cannot change a protected metatable"). The VM and
    /// `debug.setmetatable` bypass this guard.
    ///
    /// # Parameters
    /// - `table` (table): the table to modify.
    /// - `mt` (table, optional): the new metatable, or `nil` to clear.
    ///
    /// # Returns
    /// (table): the same `table` (so the call can be chained).
    ///
    /// # Examples
    /// ```lua
    /// local t = setmetatable({}, {
    ///     __index = function(_, k) return "missing:"..k end,
    /// })
    /// assert(t.foo == "missing:foo")
    ///
    /// -- Removing a metatable
    /// setmetatable(t, nil)
    /// assert(t.foo == nil)
    /// ```
    #[function]
    fn setmetatable(table: Table, mt: Option<Table>) -> Result<Table, VmError> {
        if table.get_metamethod("__metatable").is_some() {
            let msg = "cannot change a protected metatable".to_owned();
            return Err(VmError::LuaError {
                display: msg.clone(),
                value: Value::string(msg),
            }
            .with_hint(
                "the table's metatable defines a `__metatable` field; \
                 the table can no longer be re-metatabled (this is by \
                 design — the original author opted out)",
            )
            .with_arg_position(1));
        }
        table.set_metatable(mt).with_arg_position(1)?;
        Ok(table)
    }

    /// Selects values from a variadic argument list by position or count.
    ///
    /// When `index` is the string `"#"`, returns the number of remaining
    /// arguments. When `index` is a positive integer `n`, returns the
    /// `n`-th argument and everything after it. When `index` is
    /// negative, counts from the end (`-1` is the last). `index == 0`
    /// is an error.
    ///
    /// Commonly used inside variadic functions to pick out specific
    /// arguments without packing them into a table.
    ///
    /// # Parameters
    /// - `index` (integer | string): position or `"#"` for count.
    /// - `...` (any): the value list to select from.
    ///
    /// # Returns
    /// (...): the selected suffix of the list, or the count.
    ///
    /// # Examples
    /// ```lua
    /// assert(select("#", "a", "b", "c") == 3)
    ///
    /// local a, b = select(2, "x", "y", "z")
    /// assert(a == "y" and b == "z")
    ///
    /// -- Negative index counts from the end
    /// assert(select(-1, 10, 20, 30) == 30)
    /// ```
    #[function]
    fn select(index: super::SelectIndex, rest: Variadic) -> Result<Variadic, VmError> {
        let rest = rest.0;
        match index {
            super::SelectIndex::Hash(s) if s == "#" => {
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

    /// Raises an error with `msg` as the error value.
    ///
    /// When `msg` is a string and `level > 0`, a `source:line:` prefix
    /// is prepended pointing to the chosen call-stack frame. `level == 1`
    /// (the default) reports the function that called `error`;
    /// `level == 2` reports its caller; `level == 0` adds no prefix.
    /// For non-string `msg`, the value is propagated unchanged so callers
    /// can match on structured error values via `pcall`.
    ///
    /// `error()` with no arguments propagates `nil` as the error value.
    ///
    /// # Parameters
    /// - `msg` (any, optional): the error value (default `nil`).
    /// - `level` (integer, optional): call-stack level for the prefix
    ///   (default `1`).
    ///
    /// # Returns
    /// Never returns — unwinds to the nearest `pcall`/`xpcall`.
    ///
    /// # Examples
    /// ```lua
    /// local ok, err = pcall(function()
    ///     error("something went wrong")
    /// end)
    /// assert(ok == false)
    /// assert(string.find(err, "something went wrong", 1, true) ~= nil)
    ///
    /// -- Structured error value, no prefix
    /// local ok2, err2 = pcall(function()
    ///     error({ code = 42, what = "oops" }, 0)
    /// end)
    /// assert(err2.code == 42)
    /// ```
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

    /// Raises an error if `v` is `nil` or `false`; otherwise returns all arguments.
    ///
    /// When `v` is truthy, returns every argument unchanged so `assert`
    /// can be used inline, e.g. `local f = assert(io.open(path))`. When
    /// `v` is falsy, raises an error: the second argument (or
    /// `"assertion failed!"` if absent) is used as the error value.
    /// Non-string error values are passed through unchanged.
    ///
    /// # Parameters
    /// - `v` (any): the value to test for truthiness.
    /// - `msg` (any, optional): error value used when `v` is falsy.
    /// - `...` (any): additional values returned on success.
    ///
    /// # Returns
    /// On success: every argument. On failure: never returns.
    ///
    /// # Examples
    /// ```lua
    /// local x = assert(42, "unreachable")
    /// assert(x == 42)
    ///
    /// -- Inline pattern: capture both return values from a function
    /// local function lookup(k)
    ///     if k == "yes" then return "hit" end
    ///     return nil, "missing key: "..k
    /// end
    /// assert(lookup("yes") == "hit")
    ///
    /// local ok, err = pcall(function() assert(false, "boom") end)
    /// assert(not ok)
    /// assert(string.find(err, "boom", 1, true) ~= nil)
    /// ```
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

    /// Returns an iterator over every key-value pair in `table`.
    ///
    /// Designed for use in a generic `for` loop. If the value's
    /// metatable defines a `__pairs` metamethod, that metamethod is
    /// called with the value and its return values are forwarded
    /// directly — letting you customise iteration for table-like
    /// objects.  Userdata with a `__pairs` metamethod is iterable
    /// the same way.
    ///
    /// Iteration order is implementation-defined. Mutating existing
    /// keys during traversal is allowed; adding new keys is not.
    ///
    /// # Parameters
    /// - `target` (table or userdata): the value to iterate.
    ///
    /// # Returns
    /// `(iterator, target, nil)` suitable for
    /// `for k, v in pairs(t) do`.
    ///
    /// # Examples
    /// ```lua
    /// local t = { a = 1, b = 2, c = 3 }
    /// local seen = {}
    /// for k, v in pairs(t) do
    ///     seen[k] = v
    /// end
    /// assert(seen.a == 1 and seen.b == 2 and seen.c == 3)
    /// ```
    #[function]
    async fn pairs(ctx: CallContext, target: Value) -> Result<super::PairsResult, VmError> {
        use super::PairsResult;
        // Userdata: dispatch the `__pairs` metamethod (if any).  The
        // userdata is passed as the sole argument; the metamethod
        // returns the `(iter_fn, state, control)` triple directly,
        // matching Lua 5.2's semantics for table `__pairs` and the
        // mlua convention used by kumomta and wezterm.
        if let Value::Userdata(ud) = &target {
            let ud = ::std::sync::Arc::clone(ud);
            let results = ud
                .dispatch(ctx.clone(), "__pairs", valuevec![target.clone()])
                .await?;
            return Ok(PairsResult::Metamethod(Variadic(results)));
        }
        let table = match target {
            Value::Table(t) => t,
            other => {
                return Err(VmError::BadArgument {
                    position: 1,
                    function: "pairs".to_owned(),
                    expected: "table or userdata".to_owned(),
                    got: other.type_name().to_owned(),
                });
            }
        };
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
                }
                .with_hint(
                    "`pairs` falls back on the global `next` to drive \
                     iteration; restore the standard `next` or define a \
                     `__pairs` metamethod on the table",
                ))
            }
        };
        Ok(PairsResult::Standard(next_fn, table))
    }

    /// Returns an iterator over consecutive integer keys starting at `1`.
    ///
    /// Stops at the first absent key. Use `ipairs` for array-style
    /// iteration where ordering matters; use `pairs` to iterate every
    /// key (including non-integer ones).
    ///
    /// If the value's metatable defines `__ipairs` (a Lua 5.2 extension),
    /// that metamethod is called and its return values are forwarded
    /// directly. Otherwise the iterator uses raw integer-key access —
    /// it does *not* consult `__index`, matching the Lua 5.3+ spec.
    /// Userdata with an `__ipairs` metamethod is iterable the same
    /// way.
    ///
    /// # Parameters
    /// - `target` (table or userdata): the array-style value to iterate.
    ///
    /// # Returns
    /// `(iterator, target, 0)` suitable for
    /// `for i, v in ipairs(t) do`.
    ///
    /// # Examples
    /// ```lua
    /// local t = { "a", "b", "c" }
    /// local out = {}
    /// for i, v in ipairs(t) do
    ///     out[i] = v
    /// end
    /// assert(out[1] == "a" and out[2] == "b" and out[3] == "c")
    ///
    /// -- Stops at the first nil hole
    /// local sparse = { 1, 2, nil, 4 }
    /// local count = 0
    /// for _ in ipairs(sparse) do count = count + 1 end
    /// assert(count == 2)
    /// ```
    #[function]
    async fn ipairs(ctx: CallContext, target: Value) -> Result<super::IpairsResult, VmError> {
        use super::{IpairsIterResult, IpairsResult};
        // Userdata: dispatch the `__ipairs` metamethod (Lua 5.2
        // extension).  Same shape as `pairs`.
        if let Value::Userdata(ud) = &target {
            let ud = ::std::sync::Arc::clone(ud);
            let results = ud
                .dispatch(ctx.clone(), "__ipairs", valuevec![target.clone()])
                .await?;
            return Ok(IpairsResult::Metamethod(Variadic(results)));
        }
        let table = match target {
            Value::Table(t) => t,
            other => {
                return Err(VmError::BadArgument {
                    position: 1,
                    function: "ipairs".to_owned(),
                    expected: "table or userdata".to_owned(),
                    got: other.type_name().to_owned(),
                });
            }
        };
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

    /// Writes its arguments to standard output as a single tab-separated line.
    ///
    /// Each argument is converted through `tostring` (so `__tostring`
    /// metamethods are honoured), values are joined with tabs, and a
    /// trailing newline is added. Best for quick scripts and debugging —
    /// for structured output use `io.write` or `string.format`.
    ///
    /// # Parameters
    /// - `...` (any): zero or more values to print.
    ///
    /// # Returns
    /// Nothing.
    ///
    /// # Examples
    /// ```lua
    /// print("hello", "world")
    /// print(1, 2, 3)
    /// ```
    #[function]
    async fn print(ctx: CallContext, args: Variadic) -> Result<(), VmError> {
        let mut parts = Vec::with_capacity(args.0.len());
        for v in args.0 {
            let s = value_tostring(&ctx, v).await?;
            parts.push(s);
        }
        let line = parts.join("\t");
        // Tools that need to capture print output (e.g. the docgen
        // example validator) install a PrintCapture extension on
        // the env; otherwise print writes to process stdout.
        if let Some(capture) = ctx.global.extension::<crate::PrintCapture>() {
            capture.write_line(&line);
        } else {
            println!("{}", line);
        }
        Ok(())
    }

    /// Performs garbage-collection control operations.
    ///
    /// Recognised options:
    /// - `"collect"` (default) — run a full mark-and-sweep cycle,
    ///   invoke pending `__gc` finalizers, and return `0`.
    /// - `"count"` — return `(0, 0)` (memory accounting is not
    ///   implemented; the signature is preserved for compatibility).
    /// - `"isrunning"` — return `true`; the collector is always active.
    /// - any other option (`"stop"`, `"restart"`, `"step"`,
    ///   `"setpause"`, `"setstepmul"`, `"incremental"`,
    ///   `"generational"`) — accepted but a no-op, returning `0`.
    ///
    /// Shingetsu uses synchronous reference-counting plus an explicit
    /// cycle collector; most tuning knobs from reference Lua have no
    /// effect.
    ///
    /// # Parameters
    /// - `opt` (string, optional): the operation (default `"collect"`).
    ///
    /// # Returns
    /// Varies by `opt` — see above.
    ///
    /// # Examples
    /// ```lua
    /// assert(collectgarbage("collect") == 0)
    /// assert(collectgarbage("isrunning") == true)
    /// ```
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
/// This does **not** register `os` or `io` — call [`crate::os::register`],
/// [`crate::io::register`], etc. separately for those.
pub fn register_sandboxed(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = builtins::build_module_table(env)?;
    env.register_from_table(&table)?;
    env.register_module_type("builtins", builtins::module_type());

    // Sandbox-safe standard library modules.
    crate::bit32::register(env)?;
    crate::math_lib::register(env)?;
    crate::string_lib::register(env)?;
    crate::table_lib::register(env)?;
    crate::utf8_lib::register(env)?;

    // Populate the `loaded` cache so that `require("math")` etc. works.
    for name in ["bit32", "math", "string", "table", "utf8"] {
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
    Reader(crate::Function),
}

/// Return type for `load`: `(function)` on success, `(nil, errmsg)` on
/// failure.
#[derive(crate::IntoLuaMulti)]
enum LoadResult {
    Ok(crate::Function),
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
                crate::io::bytes_to_path(name).map_err(|e| format!("cannot open file: {e}"))?;
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
    let func = crate::Function::lua_with_env(bc.top_level, vec![], env_tbl);

    Ok(LoadResult::Ok(func))
}

/// Chunk-loading builtins gated behind `Libraries::LOAD`.
///
/// `load`, `loadfile`, and `dofile` can compile and execute arbitrary Lua
/// source at runtime, so they are excluded from the sandboxed library set
/// (matching the LuaU convention) and only registered when the embedder
/// explicitly enables `Libraries::LOAD`.
#[crate::module(name = "load_mod")]
mod load_mod {
    use super::*;

    /// Compiles a chunk of Lua source and returns it as a callable function.
    ///
    /// `chunk` is either a string of source code or a function that is
    /// called repeatedly to produce the source piece by piece. A reader
    /// function is called with no arguments and must return a string; an
    /// empty string, `nil`, or a non-string value signals end of input.
    /// All returned pieces are concatenated, so a multi-byte UTF-8
    /// sequence may safely span two consecutive pieces. The final
    /// assembled source must be valid UTF-8.
    ///
    /// `chunkname` names the chunk for use in error messages and
    /// `debug.getinfo`. It follows Lua 5.4's source-name conventions:
    ///
    /// - **`@path`** — a file path. The leading `@` is stripped for
    ///   display; long paths are truncated from the front.
    /// - **`=label`** — an embedder-defined label. The leading `=` is
    ///   stripped; long labels are truncated from the end.
    /// - **anything else** — treated as literal source text, displayed
    ///   as `[string "first line..."]`, truncated to 60 characters.
    ///
    /// When `chunkname` is omitted the default depends on `chunk`: for a
    /// string chunk, the source text itself (shown in `[string "..."]`
    /// form); for a reader function, `=(load)`.
    ///
    /// `mode` selects what kind of chunks are accepted: it must contain
    /// the letter `t` (text) to accept source code. Binary chunks are
    /// not supported, so a mode of `"b"` alone always fails. Defaults to
    /// `"t"` when omitted.
    ///
    /// `env` sets the `_ENV` table for the loaded chunk, controlling
    /// where global reads and writes go. When omitted, the chunk uses
    /// the caller's global environment. Closures defined inside the
    /// loaded chunk inherit the same `_ENV`.
    ///
    /// # Parameters
    /// - `chunk` (string | function): source string or reader function.
    /// - `chunkname` (string, optional): name shown in error messages.
    /// - `mode` (string, optional): allowed chunk kinds (default `"t"`).
    /// - `env` (table, optional): `_ENV` for the loaded chunk.
    ///
    /// # Returns
    /// On success: `(function)`. On failure: `(nil, errmsg)`.
    ///
    /// # Examples
    /// ```lua
    /// local f = assert(load("return 1 + 2"))
    /// assert(f() == 3)
    ///
    /// -- Custom _ENV: chunk reads/writes go to a sandbox table
    /// local sandbox = { x = 10 }
    /// local g = assert(load("return x * 2", "sandboxed", "t", sandbox))
    /// assert(g() == 20)
    ///
    /// -- Syntax error returns (nil, msg)
    /// local nope, err = load("this is not valid lua !")
    /// assert(nope == nil)
    /// assert(type(err) == "string")
    /// ```
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

    /// Reads a Lua source file and compiles it into a callable function.
    ///
    /// The file path is used as the chunk name with an `@` prefix, so
    /// error messages display the file path directly. `mode` and `env`
    /// behave as in `load`.
    ///
    /// # Parameters
    /// - `filename` (string): path to the source file.
    /// - `mode` (string, optional): allowed chunk kinds (default `"t"`).
    /// - `env` (table, optional): `_ENV` for the loaded chunk.
    ///
    /// # Returns
    /// On success: `(function)`. On failure: `(nil, errmsg)`.
    ///
    /// # Examples
    /// ```lua,no_run
    /// local f, err = loadfile("script.lua")
    /// if not f then
    ///     error(err)
    /// end
    /// f()
    /// ```
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

    /// Reads, compiles, and runs a Lua source file in one step.
    ///
    /// All values returned by the chunk are returned by `dofile`.
    /// Errors during reading, compilation, or execution propagate to the
    /// caller — wrap the call in `pcall` if you want to handle them.
    ///
    /// # Parameters
    /// - `filename` (string): path to the source file.
    ///
    /// # Returns
    /// (...): every value returned by the chunk.
    ///
    /// # Examples
    /// ```lua,no_run
    /// local result = dofile("compute.lua")
    /// print(result)
    /// ```
    #[function]
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
/// Gated behind [`crate::Libraries::LOAD`] because it can execute arbitrary
/// code from untrusted strings (excluded from sandboxed mode,
/// following Luau convention).
pub fn register_load(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = load_mod::build_module_table(env)?;
    env.register_from_table(&table)?;
    env.register_module_type("builtins", load_mod::module_type());
    Ok(())
}

/// Install all builtins and standard library modules as globals on `env`.
///
/// This is a convenience that calls [`register_sandboxed`] plus
/// [`crate::os::register`].
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    register_sandboxed(env)?;
    crate::os::register(env)?;

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
