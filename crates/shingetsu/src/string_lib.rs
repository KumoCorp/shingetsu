//! Lua `string` standard library.
//!
//! Registered as a global `string` table and set as the `__index` of the
//! shared string metatable so that `("hello"):upper()` works.

use bytes::Bytes;

use crate::convert::Variadic;
use crate::error::VmError;
use crate::function::Function;
use crate::lua_pattern::{Capture, Match, Pattern};
use crate::table::Table;

use crate::value::Value;

/// Clamp a 1-based Lua string index into a 0-based Rust byte offset.
/// Negative indices count from the end.  Out-of-range values are clamped
/// to `[0, len]`.
fn lua_index(i: i64, len: usize) -> usize {
    if i >= 0 {
        ((i as usize).saturating_sub(1)).min(len)
    } else {
        // Negative: -1 == last byte, -len == first byte.
        let pos = len as i64 + i;
        if pos < 0 {
            0
        } else {
            pos as usize
        }
    }
}

/// Same as `lua_index` but the result is *inclusive* (suitable for end
/// positions like the `j` in `string.sub`).
fn lua_index_end(j: i64, len: usize) -> usize {
    if j >= 0 {
        (j as usize).min(len)
    } else {
        let pos = len as i64 + j + 1;
        if pos < 0 {
            0
        } else {
            pos as usize
        }
    }
}

/// Create a `VmError` for runtime errors (uses the `LuaError` variant).
fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::string(msg),
    }
}

/// Compile a Lua pattern from bytes, returning a `VmError` on
/// malformed patterns.  Wraps the in-house byte-level pattern matcher.
fn compile_pattern(pat: &[u8]) -> Result<Pattern, VmError> {
    Pattern::compile(pat).map_err(|e| runtime_error(e.message))
}

/// Execute `Pattern::find` and convert runtime pattern errors into
/// `VmError`s.
fn pattern_find(pat: &Pattern, haystack: &[u8], init: usize) -> Result<Option<Match>, VmError> {
    pat.find(haystack, init)
        .map_err(|e| runtime_error(e.message))
}

/// Convert a single `Capture` into a Lua `Value` given the haystack it
/// refers to.
fn capture_to_value(cap: &Capture, haystack: &[u8]) -> Value {
    match *cap {
        Capture::Span { start, end } => {
            Value::String(Bytes::copy_from_slice(&haystack[start..end]))
        }
        Capture::Position(p) => Value::Integer(p as i64 + 1),
    }
}

/// Extract the "captures list" from a match for callers like `match`,
/// `gmatch`, and `gsub`.  If the pattern has no explicit captures, the
/// whole match is returned as a single string capture (mirroring the
/// reference Lua behaviour).
fn extract_captures(m: &Match, haystack: &[u8]) -> Vec<Value> {
    if m.captures.is_empty() {
        vec![Value::String(Bytes::copy_from_slice(
            &haystack[m.start..m.end],
        ))]
    } else {
        m.captures
            .iter()
            .map(|c| capture_to_value(c, haystack))
            .collect()
    }
}

/// Apply a replacement value from table lookup or function call in `gsub`.
/// If the value is `nil` or `false`, the original match (`original`) is kept.
/// Strings and numbers are used as the replacement text; other types are an error.
fn gsub_apply_replacement(
    result: &mut Vec<u8>,
    original: &[u8],
    replacement: &Value,
) -> Result<(), VmError> {
    match replacement {
        Value::Nil | Value::Boolean(false) => {
            result.extend_from_slice(original);
        }
        Value::String(rs) => result.extend_from_slice(rs),
        Value::Integer(n) => {
            result.extend_from_slice(n.to_string().as_bytes());
        }
        Value::Float(f) => {
            result.extend_from_slice(f.to_string().as_bytes());
        }
        other => {
            return Err(runtime_error(format!(
                "invalid replacement value (a {} value)",
                other.type_name()
            )));
        }
    }
    Ok(())
}

/// Append the bytes of capture index `idx` (where `0` is the whole match
/// and `1..=n_captures` are explicit captures) to `result`.  Position
/// captures are formatted as their 1-based decimal representation,
/// mirroring Lua's `add_s` / `get_onecapture` behaviour.
fn append_replacement_capture(
    result: &mut Vec<u8>,
    m: &Match,
    haystack: &[u8],
    idx: usize,
) -> Result<(), VmError> {
    if idx == 0 {
        result.extend_from_slice(&haystack[m.start..m.end]);
        return Ok(());
    }
    match m.captures.get(idx - 1) {
        Some(Capture::Span { start, end }) => result.extend_from_slice(&haystack[*start..*end]),
        Some(Capture::Position(p)) => {
            result.extend_from_slice((*p as i64 + 1).to_string().as_bytes());
        }
        None => {
            return Err(runtime_error(format!("invalid capture index %{}", idx)));
        }
    }
    Ok(())
}

/// The key used when `gsub`'s replacement argument is a table: the
/// first explicit capture if one exists, otherwise the whole match.
/// Position captures become integer keys.
fn gsub_table_key(m: &Match, haystack: &[u8]) -> Value {
    match m.captures.first() {
        Some(cap) => capture_to_value(cap, haystack),
        None => Value::String(Bytes::copy_from_slice(&haystack[m.start..m.end])),
    }
}

/// Look up `key` in `tab` using Lua's full `__index` semantics: raw
/// lookup first, then follow the `__index` metamethod chain.  A table
/// `__index` is chased iteratively; a function `__index` is dispatched
/// via `ctx.call_function` so its side effects are visible to the
/// caller, matching the reference `lua_gettable`.
async fn gsub_table_lookup(
    ctx: &crate::CallContext,
    mut tab: Table,
    key: Value,
) -> Result<Value, VmError> {
    // Cap the chain depth to match the task-level limit; a pathological
    // cycle would otherwise loop forever.
    for _ in 0..100 {
        let v = tab.raw_get(&key)?;
        if !v.is_nil() {
            return Ok(v);
        }
        match tab.get_metamethod("__index") {
            None => return Ok(Value::Nil),
            Some(Value::Table(next)) => tab = next,
            Some(Value::Function(f)) => {
                let ret = ctx.call_function(f, vec![Value::Table(tab), key]).await?;
                return Ok(ret.into_iter().next().unwrap_or(Value::Nil));
            }
            Some(_other) => return Ok(Value::Nil),
        }
    }
    Err(runtime_error("'__index' chain too long".to_owned()))
}

#[crate::module(name = "string")]
pub mod string_mod {
    use super::*;

    // ----------------------------------------------------------------
    // string.len(s)
    // ----------------------------------------------------------------
    #[function]
    fn len(s: Bytes) -> i64 {
        s.len() as i64
    }

    // ----------------------------------------------------------------
    // string.byte(s [, i [, j]])
    // Returns the byte values of s[i] through s[j].
    // ----------------------------------------------------------------
    #[function]
    fn byte(s: Bytes, i: Option<i64>, j: Option<i64>) -> Variadic {
        let len = s.len();
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(i);
        let start = lua_index(i, len);
        let end = lua_index_end(j, len);
        if start >= end {
            return Variadic(vec![]);
        }
        Variadic(
            s[start..end]
                .iter()
                .map(|&b| Value::Integer(b as i64))
                .collect(),
        )
    }

    // ----------------------------------------------------------------
    // string.char(...)
    // Returns a string from the given byte values.
    // ----------------------------------------------------------------
    #[function]
    fn char(args: Variadic) -> Result<Value, VmError> {
        let mut buf = Vec::with_capacity(args.0.len());
        for (i, v) in args.0.iter().enumerate() {
            match v {
                Value::Integer(n) if *n >= 0 && *n <= 255 => buf.push(*n as u8),
                Value::Float(f) if *f >= 0.0 && *f <= 255.0 => buf.push(*f as u8),
                _ => {
                    return Err(VmError::BadArgument {
                        position: i + 1,
                        function: "char".to_owned(),
                        expected: "integer in [0,255]".to_owned(),
                        got: v.type_name().to_owned(),
                    });
                }
            }
        }
        Ok(Value::string(buf))
    }

    // ----------------------------------------------------------------
    // string.upper(s)
    // ----------------------------------------------------------------
    #[function]
    fn upper(s: Bytes) -> Bytes {
        Bytes::from(s.to_ascii_uppercase())
    }

    // ----------------------------------------------------------------
    // string.lower(s)
    // ----------------------------------------------------------------
    #[function]
    fn lower(s: Bytes) -> Bytes {
        Bytes::from(s.to_ascii_lowercase())
    }

    // ----------------------------------------------------------------
    // string.reverse(s)
    // ----------------------------------------------------------------
    #[function]
    fn reverse(s: Bytes) -> Bytes {
        let mut v: Vec<u8> = s.to_vec();
        v.reverse();
        Bytes::from(v)
    }

    // ----------------------------------------------------------------
    // string.sub(s, i [, j])
    // ----------------------------------------------------------------
    #[function]
    fn sub(s: Bytes, i: i64, j: Option<i64>) -> Bytes {
        let len = s.len();
        let j = j.unwrap_or(-1);
        let start = lua_index(i, len);
        let end = lua_index_end(j, len);
        if start >= end {
            Bytes::new()
        } else {
            s.slice(start..end)
        }
    }

    // ----------------------------------------------------------------
    // string.rep(s, n [, sep])
    // ----------------------------------------------------------------
    #[function]
    fn rep(s: Bytes, n: i64, sep: Option<Bytes>) -> Bytes {
        if n <= 0 {
            return Bytes::new();
        }
        let n = n as usize;
        let sep = sep.unwrap_or_default();
        let cap = s.len() * n + sep.len() * n.saturating_sub(1);
        let mut buf = Vec::with_capacity(cap);
        for i in 0..n {
            if i > 0 && !sep.is_empty() {
                buf.extend_from_slice(&sep);
            }
            buf.extend_from_slice(&s);
        }
        Bytes::from(buf)
    }

    // ----------------------------------------------------------------
    // string.find(s, pattern [, init [, plain]])
    // Returns `(start, end, ...captures)` (1-based) on match, or `nil`.
    // ----------------------------------------------------------------
    #[function]
    fn find(
        s: Bytes,
        pattern: Bytes,
        init: Option<i64>,
        plain: Option<bool>,
    ) -> Result<Variadic, VmError> {
        let len = s.len();
        let start = if let Some(i) = init {
            lua_index(i, len)
        } else {
            0
        };
        let haystack = &s[start..];

        if plain.unwrap_or(false) {
            // Plain substring search.
            if pattern.is_empty() {
                let lua_start = (start + 1) as i64;
                return Ok(Variadic(vec![
                    Value::Integer(lua_start),
                    Value::Integer(start as i64),
                ]));
            }
            if let Some(pos) = haystack
                .windows(pattern.len())
                .position(|w| w == &pattern[..])
            {
                let lua_start = (start + pos + 1) as i64;
                let lua_end = (start + pos + pattern.len()) as i64;
                Ok(Variadic(vec![
                    Value::Integer(lua_start),
                    Value::Integer(lua_end),
                ]))
            } else {
                Ok(Variadic(vec![Value::Nil]))
            }
        } else {
            let pat = compile_pattern(&pattern)?;
            if let Some(m) = pattern_find(&pat, haystack, 0)? {
                let lua_start = (start + m.start + 1) as i64;
                let lua_end = (start + m.end) as i64;
                let mut result = vec![Value::Integer(lua_start), Value::Integer(lua_end)];
                for cap in &m.captures {
                    result.push(capture_to_value(cap, haystack));
                }
                Ok(Variadic(result))
            } else {
                Ok(Variadic(vec![Value::Nil]))
            }
        }
    }

    // ----------------------------------------------------------------
    // string.match(s, pattern [, init])
    // Returns the captures from the first match, or `nil`.
    // ----------------------------------------------------------------
    #[function(rename = "match")]
    fn string_match(s: Bytes, pattern: Bytes, init: Option<i64>) -> Result<Variadic, VmError> {
        let len = s.len();
        let start = if let Some(i) = init {
            lua_index(i, len)
        } else {
            0
        };
        let haystack = &s[start..];

        let pat = compile_pattern(&pattern)?;
        if let Some(m) = pattern_find(&pat, haystack, 0)? {
            Ok(Variadic(extract_captures(&m, haystack)))
        } else {
            Ok(Variadic(vec![Value::Nil]))
        }
    }

    // ----------------------------------------------------------------
    // string.gsub(s, pattern, repl [, n])
    // Replaces occurrences of `pattern` in `s`.
    // When `repl` is a function, it is called with the captures for
    // each match; its return value becomes the replacement.
    // ----------------------------------------------------------------
    #[function]
    async fn gsub(
        ctx: crate::CallContext,
        s: Bytes,
        pattern: Bytes,
        repl: Value,
        max_n: Option<i64>,
    ) -> Result<(Value, i64), VmError> {
        let max_n = max_n.map(|n| n.max(0) as usize).unwrap_or(usize::MAX);
        let pat = compile_pattern(&pattern)?;

        let mut result = Vec::with_capacity(s.len());
        let mut count: usize = 0;
        let mut offset: usize = 0;
        // Track the end of the previous match so that an empty match
        // landing at the same position twice in a row is treated as a
        // no-op (and we advance one byte instead) — matching Lua's
        // `lastmatch` check in `str_gsub`.
        let mut last_match_end: Option<usize> = None;

        while count < max_n {
            let m = pattern_find(&pat, &s, offset)?;
            let Some(m) = m else { break };

            // Empty match that ends at the same place as the previous
            // match is a degenerate duplicate; skip one byte instead.
            if m.start == m.end && Some(m.end) == last_match_end {
                if offset < s.len() {
                    result.push(s[offset]);
                    offset += 1;
                    continue;
                } else {
                    break;
                }
            }

            // Append everything between the previous offset and this match.
            result.extend_from_slice(&s[offset..m.start]);
            let match_bytes = &s[m.start..m.end];

            // Build the replacement.
            match &repl {
                Value::String(repl_str) => {
                    // Process `%0`..`%9` capture references and `%%` → `%`.
                    let mut i = 0;
                    let rb = repl_str.as_ref();
                    while i < rb.len() {
                        if rb[i] == b'%' {
                            if i + 1 >= rb.len() {
                                return Err(runtime_error(
                                    "invalid use of '%' in replacement string".to_owned(),
                                ));
                            }
                            let next = rb[i + 1];
                            if next == b'%' {
                                result.push(b'%');
                            } else if next.is_ascii_digit() {
                                let idx = (next - b'0') as usize;
                                append_replacement_capture(&mut result, &m, &s, idx)?;
                            } else {
                                return Err(runtime_error(
                                    "invalid use of '%' in replacement string".to_owned(),
                                ));
                            }
                            i += 2;
                        } else {
                            result.push(rb[i]);
                            i += 1;
                        }
                    }
                }
                Value::Table(tab) => {
                    let key = gsub_table_key(&m, &s);
                    let replacement = gsub_table_lookup(&ctx, tab.clone(), key).await?;
                    gsub_apply_replacement(&mut result, match_bytes, &replacement)?;
                }
                Value::Function(func) => {
                    // Call the function with the captures as arguments.
                    let call_args = extract_captures(&m, &s);
                    let ret = ctx.call_function(func.clone(), call_args).await?;
                    let replacement = ret.into_iter().next().unwrap_or(Value::Nil);
                    gsub_apply_replacement(&mut result, match_bytes, &replacement)?;
                }
                _ => {
                    return Err(VmError::BadArgument {
                        position: 3,
                        function: "gsub".to_owned(),
                        expected: "string, table, or function".to_owned(),
                        got: repl.type_name().to_owned(),
                    });
                }
            }

            count += 1;
            last_match_end = Some(m.end);
            offset = m.end;

            // Reference Lua's `str_gsub` exits the loop after the
            // first successful match for anchored patterns (the `^`
            // binds to the start of the subject and cannot match
            // again).  `string.gsub("aaa", "^a", "X", 5)` returns
            // `("Xaa", 1)` even when `max_n` would allow more.
            if pat.is_anchored() {
                break;
            }
        }

        // Append the remainder.
        if offset < s.len() {
            result.extend_from_slice(&s[offset..]);
        }

        Ok((Value::string(result), count as i64))
    }

    // ----------------------------------------------------------------
    // string.format(fmt, ...)
    // A subset of C `sprintf`-style formatting.
    // ----------------------------------------------------------------
    #[function]
    fn format(fmt: Bytes, args: Variadic) -> Result<Value, VmError> {
        string_format_impl(&fmt, &args.0)
    }

    // ----------------------------------------------------------------
    // string.pack(fmt, v1, v2, ...)
    // ----------------------------------------------------------------
    #[function]
    fn pack(fmt: Bytes, args: Variadic) -> Result<Value, VmError> {
        let data = crate::string_pack::string_pack(&fmt, &args.0)?;
        Ok(Value::string(data))
    }

    // ----------------------------------------------------------------
    // string.unpack(fmt, s [, pos])
    // ----------------------------------------------------------------
    #[function]
    fn unpack(fmt: Bytes, s: Bytes, pos: Option<Value>) -> Result<Variadic, VmError> {
        // Lua coerces `pos` through its standard number rules: numeric
        // strings are accepted, fractional floats are rejected with
        // "number has no integer representation".  Negative and
        // below-1 values are handled inside `string_unpack`.
        let init_pos = match pos {
            None => 1,
            Some(v) => coerce_to_integer(&v, 3, "unpack")?,
        };
        let vals = crate::string_pack::string_unpack(&fmt, &s, init_pos)?;
        Ok(Variadic(vals))
    }

    // ----------------------------------------------------------------
    // string.packsize(fmt)
    // ----------------------------------------------------------------
    #[function]
    fn packsize(fmt: Bytes) -> Result<Value, VmError> {
        let size = crate::string_pack::string_packsize(&fmt)?;
        Ok(Value::Integer(size))
    }

    // ----------------------------------------------------------------
    // string.split(s [, sep])  (LuaU extension)
    //
    // Splits `s` on each occurrence of the literal byte sequence `sep`
    // (default `","`) and returns the pieces as an array-style table.
    // `sep` is a plain string, not a Lua pattern.  An empty `sep`
    // splits `s` into its individual bytes; `string.split("", "")`
    // returns an empty table, matching LuaU.
    // ----------------------------------------------------------------
    #[function]
    fn split(s: Bytes, sep: Option<Bytes>) -> Result<Table, VmError> {
        let sep = sep.unwrap_or_else(|| Bytes::from_static(b","));
        let t = Table::new();
        let mut idx: i64 = 1;

        if sep.is_empty() {
            // Empty separator: emit one element per byte.  LuaU handles
            // this differently from a generic substring search — every
            // byte becomes its own piece, and "" split by "" yields an
            // empty table.  `memmem` would instead match at every offset
            // (including `s.len()`), so we short-circuit here.
            for i in 0..s.len() {
                t.raw_set(Value::Integer(idx), Value::String(s.slice(i..i + 1)))?;
                idx += 1;
            }
            return Ok(t);
        }

        // `memmem::find_iter` yields non-overlapping match positions
        // using SIMD / Two-Way under the hood.
        let sep_len = sep.len();
        let mut span_start = 0usize;
        for pos in memchr::memmem::find_iter(&s, &sep) {
            t.raw_set(Value::Integer(idx), Value::String(s.slice(span_start..pos)))?;
            idx += 1;
            span_start = pos + sep_len;
        }
        // Push the trailing span (always, even when empty).
        t.raw_set(
            Value::Integer(idx),
            Value::String(s.slice(span_start..s.len())),
        )?;
        Ok(t)
    }
}

// =========================================================================
// string.format implementation (kept outside the module for readability)
// =========================================================================

/// `string.format(fmt, ...)`
///
/// A subset of C `sprintf`-style formatting.  Supports `%d`, `%i`, `%u`,
/// `%f`, `%e`, `%g`, `%x`, `%X`, `%o`, `%s`, `%c`, `%q`, and `%%`.
fn string_format_impl(fmt: &[u8], args: &[Value]) -> Result<Value, VmError> {
    let mut result = Vec::with_capacity(fmt.len());
    let mut arg_idx: usize = 0;
    let mut i = 0;

    while i < fmt.len() {
        if fmt[i] != b'%' {
            result.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1; // skip '%'
        if i >= fmt.len() {
            return Err(runtime_error(
                "invalid format string (ends with '%')".to_owned(),
            ));
        }

        // Literal %%
        if fmt[i] == b'%' {
            result.push(b'%');
            i += 1;
            continue;
        }

        // Parse flags.
        let spec_start = i - 1; // include the '%'
        while i < fmt.len() && b"-+ #0".contains(&fmt[i]) {
            i += 1;
        }
        // Parse width.
        while i < fmt.len() && fmt[i].is_ascii_digit() {
            i += 1;
        }
        // Parse precision.
        if i < fmt.len() && fmt[i] == b'.' {
            i += 1;
            while i < fmt.len() && fmt[i].is_ascii_digit() {
                i += 1;
            }
        }
        if i >= fmt.len() {
            return Err(runtime_error(
                "invalid format string (missing conversion specifier)".to_owned(),
            ));
        }

        let conv = fmt[i];
        i += 1;
        let spec_str = std::str::from_utf8(&fmt[spec_start..i]).unwrap_or("%?");

        if arg_idx >= args.len() {
            // Arg positions are 1-based and include the format string as
            // #1, so the first value arg is #2.
            return Err(runtime_error(format!(
                "bad argument #{} to 'format' (no value)",
                arg_idx + 2
            )));
        }
        let arg = &args[arg_idx];
        arg_idx += 1;
        // 1-based Lua position of the current value arg (fmt is #1).
        let lua_pos = arg_idx + 1;

        match conv {
            b'd' | b'i' => {
                let n = coerce_to_integer(arg, lua_pos, "format")?;
                let formatted = c_format_int(spec_str, n);
                result.extend_from_slice(formatted.as_bytes());
            }
            b'u' => {
                let n = coerce_to_integer(arg, lua_pos, "format")?;
                let formatted = c_format_uint(spec_str, n as u64);
                result.extend_from_slice(formatted.as_bytes());
            }
            b'f' | b'e' | b'E' | b'g' | b'G' => {
                let f = coerce_to_float(arg, lua_pos, "format")?;
                let formatted = c_format_float(spec_str, f, conv);
                result.extend_from_slice(formatted.as_bytes());
            }
            b'x' | b'X' => {
                let n = coerce_to_integer(arg, lua_pos, "format")?;
                let formatted = c_format_hex(spec_str, n, conv == b'X');
                result.extend_from_slice(formatted.as_bytes());
            }
            b'o' => {
                let n = coerce_to_integer(arg, lua_pos, "format")?;
                let formatted = c_format_oct(spec_str, n);
                result.extend_from_slice(formatted.as_bytes());
            }
            b's' => {
                let s = coerce_to_string(arg)?;
                // Check if there's a precision that truncates.
                if let Some(dot_pos) = spec_str.find('.') {
                    let prec_str = &spec_str[dot_pos + 1..spec_str.len() - 1];
                    if let Ok(prec) = prec_str.parse::<usize>() {
                        let truncated: String = s.chars().take(prec).collect();
                        let info = parse_format_spec(spec_str);
                        let padded = apply_padding(&truncated, &info);
                        result.extend_from_slice(padded.as_bytes());
                        continue;
                    }
                }
                let info = parse_format_spec(spec_str);
                let padded = apply_padding(&s, &info);
                result.extend_from_slice(padded.as_bytes());
            }
            b'c' => {
                let n = coerce_to_integer(arg, lua_pos, "format")?;
                result.push((n & 0xFF) as u8);
            }
            b'q' => {
                // Quoted string — surround with double quotes, escaping
                // special characters.
                let s = coerce_to_string(arg)?;
                result.push(b'"');
                for &b in s.as_bytes() {
                    match b {
                        b'\\' => result.extend_from_slice(b"\\\\"),
                        b'"' => result.extend_from_slice(b"\\\""),
                        b'\n' => result.extend_from_slice(b"\\n"),
                        b'\r' => result.extend_from_slice(b"\\r"),
                        b'\0' => result.extend_from_slice(b"\\0"),
                        b'\x1a' => result.extend_from_slice(b"\\26"),
                        _ => result.push(b),
                    }
                }
                result.push(b'"');
            }
            _ => {
                return Err(runtime_error(format!(
                    "invalid format string (invalid conversion specifier '%{}')",
                    conv as char
                )));
            }
        }
    }

    Ok(Value::string(result))
}

// -------------------------------------------------------------------------
// string.format helpers
// -------------------------------------------------------------------------

pub(crate) fn coerce_to_integer(v: &Value, pos: usize, func: &str) -> Result<i64, VmError> {
    match v {
        Value::Integer(n) => Ok(*n),
        Value::Float(f) => float_to_integer(*f).ok_or_else(|| no_int_rep_error(pos, func)),
        Value::String(s) => match parse_numeric_string(s) {
            ParsedNumeric::Integer(n) => Ok(n),
            ParsedNumeric::Float(f) => {
                float_to_integer(f).ok_or_else(|| no_int_rep_error(pos, func))
            }
            ParsedNumeric::NotNumeric => Err(VmError::BadArgument {
                position: pos,
                function: func.to_owned(),
                expected: "number".to_owned(),
                got: "string".to_owned(),
            }),
        },
        _ => Err(VmError::BadArgument {
            position: pos,
            function: func.to_owned(),
            expected: "number".to_owned(),
            got: v.type_name().to_owned(),
        }),
    }
}

/// Lua's float-to-integer conversion for contexts that require an exact
/// integer (`string.format "%d"`, `string.pack "i"`, etc.).  Matches the
/// `lua_numbertointeger` macro: the value must be finite, must fit in the
/// `i64` range `[i64::MIN, 2^63)`, and must equal its own truncation.
fn float_to_integer(f: f64) -> Option<i64> {
    if !f.is_finite() {
        return None;
    }
    // `i64::MIN as f64` is exact (-2^63 is a power of 2); likewise 2^63.
    if f < i64::MIN as f64 || f >= -(i64::MIN as f64) {
        return None;
    }
    if f.trunc() != f {
        return None;
    }
    Some(f as i64)
}

/// Build the `bad argument #N to 'F' (number has no integer representation)`
/// error that Lua raises when a non-integer float reaches an integer slot.
fn no_int_rep_error(pos: usize, func: &str) -> VmError {
    VmError::ArgError {
        position: pos,
        function: func.to_owned(),
        msg: "number has no integer representation".to_owned(),
    }
}

/// Outcome of parsing a Lua numeric string.  Distinguishes "not a number
/// at all" from "parsed as a float" so callers expecting an integer can
/// raise the distinct `number has no integer representation` error when
/// the float isn't exactly integral.
enum ParsedNumeric {
    Integer(i64),
    Float(f64),
    NotNumeric,
}

/// Parse a Lua numeric string.  Matches `lua_stringtonumber`: accepts
/// decimal integers, decimal floats, and `0x`/`0X`-prefixed hex integer
/// literals (with optional leading `+`/`-` sign, trimming surrounding
/// whitespace).
fn parse_numeric_string(s: &[u8]) -> ParsedNumeric {
    let text = String::from_utf8_lossy(s);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ParsedNumeric::NotNumeric;
    }
    // Hex literal: optional sign, then `0x`/`0X` prefix.
    let (neg, rest) = match trimmed.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if let Some(hex) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        if let Ok(v) = u64::from_str_radix(hex, 16) {
            // Interpret as unsigned then cast; Lua allows the full u64 range.
            let signed = v as i64;
            return ParsedNumeric::Integer(if neg { signed.wrapping_neg() } else { signed });
        }
        return ParsedNumeric::NotNumeric;
    }
    // Plain integer first — preserves the i64 value exactly.
    if let Ok(n) = trimmed.parse::<i64>() {
        return ParsedNumeric::Integer(n);
    }
    // Otherwise try as a float; caller applies the integer-representation
    // check if they need an integer.
    if let Some(f) = lua_str_to_float(trimmed) {
        return ParsedNumeric::Float(f);
    }
    ParsedNumeric::NotNumeric
}

/// Lua-compatible string-to-float conversion.  Matches `l_str2d` in
/// `lua.c`: rejects any input containing `n` or `N` so that `"nan"`,
/// `"inf"`, `"Inf"`, etc. are not accepted as numbers.  `strtod`-style
/// implementations (including Rust's `f64::parse`) otherwise accept
/// these and diverge from reference Lua's `tonumber`/`luaL_checknumber`.
pub(crate) fn lua_str_to_float(s: &str) -> Option<f64> {
    if s.bytes().any(|b| b == b'n' || b == b'N') {
        return None;
    }
    s.parse::<f64>().ok()
}

pub(crate) fn coerce_to_float(v: &Value, pos: usize, func: &str) -> Result<f64, VmError> {
    match v {
        Value::Integer(n) => Ok(*n as f64),
        Value::Float(f) => Ok(*f),
        Value::String(s) => {
            let text = String::from_utf8_lossy(s);
            lua_str_to_float(text.trim()).ok_or_else(|| VmError::BadArgument {
                position: pos,
                function: func.to_owned(),
                expected: "number".to_owned(),
                got: "string".to_owned(),
            })
        }
        _ => Err(VmError::BadArgument {
            position: pos,
            function: func.to_owned(),
            expected: "number".to_owned(),
            got: v.type_name().to_owned(),
        }),
    }
}

fn coerce_to_string(v: &Value) -> Result<String, VmError> {
    match v {
        Value::String(s) => Ok(String::from_utf8_lossy(s).into_owned()),
        Value::Integer(n) => Ok(n.to_string()),
        Value::Float(f) => Ok(format!("{}", f)),
        Value::Nil => Ok("nil".to_owned()),
        Value::Boolean(b) => Ok(b.to_string()),
        _ => Ok(v.type_name().to_owned()),
    }
}

/// Format an integer using a C-style spec like `%05d`.
fn c_format_int(spec: &str, n: i64) -> String {
    let info = parse_format_spec(spec);
    let raw = format!("{}", n);
    apply_padding(&raw, &info)
}

fn c_format_uint(spec: &str, n: u64) -> String {
    let info = parse_format_spec(spec);
    let raw = format!("{}", n);
    apply_padding(&raw, &info)
}

fn c_format_hex(spec: &str, n: i64, upper: bool) -> String {
    let info = parse_format_spec(spec);
    let raw = if upper {
        format!("{:X}", n as u64)
    } else {
        format!("{:x}", n as u64)
    };
    let prefixed = if info.alt {
        if upper {
            format!("0X{}", raw)
        } else {
            format!("0x{}", raw)
        }
    } else {
        raw
    };
    apply_padding(&prefixed, &info)
}

fn c_format_oct(spec: &str, n: i64) -> String {
    let info = parse_format_spec(spec);
    let raw = format!("{:o}", n as u64);
    let prefixed = if info.alt && !raw.starts_with('0') {
        format!("0{}", raw)
    } else {
        raw
    };
    apply_padding(&prefixed, &info)
}

fn c_format_float(spec: &str, f: f64, conv: u8) -> String {
    let info = parse_format_spec(spec);
    let precision = info.precision.unwrap_or(6);
    let raw = match conv {
        b'f' => format!("{:.prec$}", f, prec = precision),
        b'e' => format!("{:.prec$e}", f, prec = precision),
        b'E' => format!("{:.prec$E}", f, prec = precision),
        b'g' | b'G' => {
            // %g uses the shorter of %e and %f.
            if f == 0.0 || (f.abs() >= 1e-4 && f.abs() < 10f64.powi(precision as i32)) {
                let s = format!("{:.prec$}", f, prec = precision);
                trim_trailing_zeros(&s)
            } else if conv == b'G' {
                format!("{:.prec$E}", f, prec = precision.saturating_sub(1))
            } else {
                format!("{:.prec$e}", f, prec = precision.saturating_sub(1))
            }
        }
        _ => format!("{}", f),
    };
    apply_padding(&raw, &info)
}

fn trim_trailing_zeros(s: &str) -> String {
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0');
        let trimmed = trimmed.trim_end_matches('.');
        trimmed.to_owned()
    } else {
        s.to_owned()
    }
}

struct FormatSpec {
    width: usize,
    left_align: bool,
    zero_pad: bool,
    plus: bool,
    space: bool,
    alt: bool,
    precision: Option<usize>,
}

fn parse_format_spec(spec: &str) -> FormatSpec {
    let bytes = spec.as_bytes();
    let mut i = 1; // skip '%'
    let mut left_align = false;
    let mut zero_pad = false;
    let mut plus = false;
    let mut space = false;
    let mut alt = false;

    // Flags.
    while i < bytes.len() {
        match bytes[i] {
            b'-' => left_align = true,
            b'0' => zero_pad = true,
            b'+' => plus = true,
            b' ' => space = true,
            b'#' => alt = true,
            _ => break,
        }
        i += 1;
    }

    // Width.
    let mut width: usize = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        width = width * 10 + (bytes[i] - b'0') as usize;
        i += 1;
    }

    // Precision.
    let precision = if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let mut p: usize = 0;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            p = p * 10 + (bytes[i] - b'0') as usize;
            i += 1;
        }
        Some(p)
    } else {
        None
    };

    FormatSpec {
        width,
        left_align,
        zero_pad,
        plus,
        space,
        alt,
        precision,
    }
}

fn apply_padding(raw: &str, spec: &FormatSpec) -> String {
    let mut s = raw.to_owned();
    // Apply sign flags.
    if spec.plus && !s.starts_with('-') {
        s.insert(0, '+');
    } else if spec.space && !s.starts_with('-') && !s.starts_with('+') {
        s.insert(0, ' ');
    }
    // Pad to width.
    if s.len() < spec.width {
        let pad_len = spec.width - s.len();
        if spec.left_align {
            s.extend(std::iter::repeat(' ').take(pad_len));
        } else if spec.zero_pad {
            // Insert zeros after sign if present.
            let sign_len = if s.starts_with('-') || s.starts_with('+') || s.starts_with(' ') {
                1
            } else {
                0
            };
            let zeros: String = std::iter::repeat('0').take(pad_len).collect();
            s.insert_str(sign_len, &zeros);
        } else {
            let spaces: String = std::iter::repeat(' ').take(pad_len).collect();
            s.insert_str(0, &spaces);
        }
    }
    s
}

// =========================================================================
// string.gmatch — must stay outside the module because it returns a
// NativeFunction with captured state.
// =========================================================================

/// Iterator over successive pattern matches in a string, used by `string.gmatch`.
struct GmatchIter {
    s: Bytes,
    pat: Pattern,
    offset: usize,
    // End of the previous yielded match.  An empty match ending at
    // the same position is treated as a duplicate and skipped, just
    // like Lua's `lastmatch` check in `gmatch_aux`.
    last_match_end: Option<usize>,
}

// We track the offset manually rather than hooking into an external
// iterator: the `Pattern` owns its bytes, and manual offset tracking
// gives us precise control over the empty-match advance-by-one
// behaviour that Lua requires.
impl Iterator for GmatchIter {
    type Item = Variadic;

    fn next(&mut self) -> Option<Variadic> {
        loop {
            if self.offset > self.s.len() {
                return None;
            }
            // Pattern errors during gmatch are silently treated as
            // "no more matches" — any syntactic issue would already
            // have surfaced at compile time; runtime errors here
            // (e.g. depth overflow on a particular input) end the
            // iteration.
            let m = match self.pat.find(&self.s, self.offset) {
                Ok(Some(m)) => m,
                _ => {
                    self.offset = self.s.len() + 1;
                    return None;
                }
            };
            // Skip degenerate empty match whose end coincides with
            // the previous match's end.
            if m.start == m.end && Some(m.end) == self.last_match_end {
                self.offset += 1;
                continue;
            }
            let captures = extract_captures(&m, &self.s);
            self.last_match_end = Some(m.end);
            self.offset = if m.end == m.start { m.end + 1 } else { m.end };
            return Some(Variadic(captures));
        }
    }
}

/// `string.gmatch(s, pattern)`
///
/// Returns an iterator function that, each time it is called, returns the
/// next captures from `pattern` over `s`.
fn string_gmatch(s: Bytes, pattern: Bytes) -> Result<Value, VmError> {
    // Compile eagerly to catch pattern errors.
    let pat = compile_pattern(&pattern)?;

    let iter = GmatchIter {
        s,
        pat,
        offset: 0,
        last_match_end: None,
    };

    Ok(Value::Function(Function::from_iter(
        "gmatch_iterator",
        iter,
    )))
}

// =========================================================================
// Registration
// =========================================================================

/// Build the string library table, register it as the `string` global, and
/// install a string metatable so method-call syntax works on string values.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = string_mod::build_module_table(env)?;

    // gmatch stays as a manually-registered function because it returns
    // a NativeFunction with captured iterator state.
    table.raw_set(
        Value::string("gmatch"),
        Value::Function(Function::wrap("gmatch", |s: Bytes, pattern: Bytes| {
            string_gmatch(s, pattern)
        })),
    )?;

    // Set the string module as a global.
    env.set_global("string", Value::Table(table.clone()));

    // Build a metatable whose __index points to the string table,
    // then install it as the shared string metatable.
    let mt = Table::new();
    mt.raw_set(Value::string("__index"), Value::Table(table))?;
    env.set_string_metatable(mt);

    Ok(())
}
