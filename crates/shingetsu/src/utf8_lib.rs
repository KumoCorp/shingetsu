//! Lua `utf8` standard library.
//!
//! Provides Unicode-aware operations on UTF-8 encoded strings:
//! `utf8.char`, `utf8.codes`, `utf8.codepoint`, `utf8.len`, `utf8.offset`,
//! and the `utf8.charpattern` constant.

use bytes::Bytes;

use crate::call_context::CallContext;
use crate::convert::Variadic;

/// Return type for `utf8.len`: character count or `(nil, errpos)`.
enum Utf8LenResult {
    Count(i64),
    /// Invalid UTF-8 detected at the given 1-based byte position.
    Invalid(i64),
}

impl crate::convert::IntoLuaMulti for Utf8LenResult {
    fn into_lua_multi(self) -> Vec<Value> {
        match self {
            Utf8LenResult::Count(n) => vec![Value::Integer(n)],
            Utf8LenResult::Invalid(errpos) => vec![Value::Nil, Value::Integer(errpos)],
        }
    }
}

impl crate::convert::LuaTypedMulti for Utf8LenResult {
    fn lua_types() -> Vec<crate::types::LuaType> {
        use crate::types::LuaType;
        // Count(i64) → integer | Invalid(i64) → (nil, integer)
        vec![LuaType::Union(vec![
            LuaType::Integer,
            LuaType::Tuple(vec![LuaType::Nil, LuaType::Integer]),
        ])]
    }
}
use crate::error::{VmError, VmResultExt};
use crate::function::Function;
use crate::value::Value;

/// Return type for the `utf8.codes` iterator: `(byte_pos, codepoint)` or end.
#[derive(crate::IntoLuaMulti)]
enum Utf8CodesIterResult {
    Char(i64, i64),
    End,
}

/// Build the utf8 library table and register it as the `utf8` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = utf8_mod::build_module_table(env)?;

    // utf8.charpattern — a pattern that matches one UTF-8 byte sequence.
    // Lua 5.4 defines this as "[\0-\x7F\xC2-\xFD][\x80-\xBF]*".
    table.raw_set(
        Value::string("charpattern"),
        Value::String(Bytes::from_static(b"[\0-\x7F\xC2-\xFD][\x80-\xBF]*")),
    )?;

    env.set_global("utf8", Value::Table(table));
    Ok(())
}

#[crate::module(name = "utf8")]
mod utf8_mod {
    use super::*;

    // -----------------------------------------------------------------
    // utf8.char(...)
    // Receives zero or more integers, converts each to its corresponding
    // UTF-8 byte sequence, and returns the concatenation.
    // -----------------------------------------------------------------
    #[function]
    fn char(args: Variadic) -> Result<Bytes, VmError> {
        let mut buf = String::new();
        for (i, v) in args.0.iter().enumerate() {
            let n = match v {
                Value::Integer(n) => *n,
                Value::Float(f) => *f as i64,
                _ => {
                    return Err(VmError::BadArgument {
                        position: i + 1,
                        function: "utf8.char".to_string(),
                        expected: "number".to_string(),
                        got: v.type_name().to_string(),
                    });
                }
            };
            let cp = u32::try_from(n)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| VmError::BadArgument {
                    position: i + 1,
                    function: "utf8.char".to_string(),
                    expected: "valid Unicode codepoint".to_string(),
                    got: format!("{}", n),
                })?;
            buf.push(cp);
        }
        Ok(Bytes::from(buf))
    }

    // -----------------------------------------------------------------
    // utf8.codes(s)
    // Returns (iterator_fn, s, 0) for the generic-for protocol.
    // The iterator function takes (s, byte_pos) where byte_pos is the
    // 1-based control variable (0 means start), decodes the character
    // at that position, and returns (next_byte_pos, codepoint) or nil.
    // -----------------------------------------------------------------
    #[function]
    fn codes(ctx: CallContext, s: Bytes) -> Result<(Function, Bytes, i64), VmError> {
        // Validate entire string upfront (Lua 5.4 behavior).
        if let Err(e) = std::str::from_utf8(&s) {
            return Err(VmError::BadArgument {
                position: 1,
                function: String::new(),
                expected: "valid UTF-8 string".to_string(),
                got: format!("invalid UTF-8 at byte {}", e.valid_up_to() + 1),
            })
            .with_call_context(1, &ctx);
        }

        // Stateless iterator: receives (string, last_byte_pos) and returns
        // (next_byte_pos, codepoint).  The control variable is a 1-based
        // byte position; 0 is the initial value meaning "start from the
        // beginning".
        let iter_fn = Function::wrap(
            "utf8.codes iterator",
            |s: Bytes, last_pos: i64| -> Result<Utf8CodesIterResult, VmError> {
                // Advance past the current character to find the next one.
                let start = if last_pos <= 0 {
                    0usize
                } else {
                    next_char_boundary(&s, (last_pos - 1) as usize)
                };

                if start >= s.len() {
                    return Ok(Utf8CodesIterResult::End);
                }

                // Decode the character at `start`.
                // The string was validated upfront, so this is safe.
                let text = std::str::from_utf8(&s[start..]).expect("pre-validated UTF-8");
                match text.chars().next() {
                    Some(ch) => Ok(Utf8CodesIterResult::Char(
                        start as i64 + 1,
                        ch as i64,
                    )),
                    None => Ok(Utf8CodesIterResult::End),
                }
            },
        );

        Ok((iter_fn, s, 0))
    }

    // -----------------------------------------------------------------
    // utf8.codepoint(s [, i [, j]])
    // Returns the codepoints (as integers) of all characters in s
    // whose starting byte position is between i and j (1-based,
    // inclusive).  Default i=1, j=i.
    // -----------------------------------------------------------------
    #[function]
    fn codepoint(
        s: Bytes,
        i: Option<i64>,
        j: Option<i64>,
    ) -> Result<crate::convert::TypedVariadic<i64>, VmError> {
        let len = s.len();
        let i_val = i.unwrap_or(1);
        let start = lua_byte_pos(i_val, len);
        // j defaults to i (single character).
        let end = lua_byte_pos_end(j.unwrap_or(i_val), len);

        let start = start.min(len);
        let end = end.min(len);
        if start >= end {
            return Ok(crate::convert::TypedVariadic(vec![]));
        }

        // Iterate characters in the full string, collecting those whose
        // starting byte offset falls within [start, end).
        let mut results = Vec::new();
        let slice = &s[start..];
        let text = std::str::from_utf8(slice).map_err(|e| VmError::BadArgument {
            position: 1,
            function: "utf8.codepoint".to_string(),
            expected: "valid UTF-8 string".to_string(),
            got: format!("invalid UTF-8 at byte {}", start + e.valid_up_to() + 1),
        })?;

        for (offset, ch) in text.char_indices() {
            let byte_pos = start + offset;
            if byte_pos >= end {
                break;
            }
            results.push(ch as i64);
        }
        Ok(crate::convert::TypedVariadic(results))
    }

    // -----------------------------------------------------------------
    // utf8.len(s [, i [, j]])
    // Returns the number of UTF-8 characters in s between byte
    // positions i and j (1-based, inclusive, default i=1 j=-1).
    // If it encounters invalid UTF-8, returns (nil, errpos).
    // -----------------------------------------------------------------
    #[function]
    fn len(s: Bytes, i: Option<i64>, j: Option<i64>) -> Utf8LenResult {
        let slen = s.len();
        let start = lua_byte_pos(i.unwrap_or(1), slen);
        let end = lua_byte_pos_end(j.unwrap_or(-1), slen);

        let start = start.min(slen);
        let end = end.min(slen);
        if start >= end {
            return Utf8LenResult::Count(0);
        }

        let slice = &s[start..end];
        match std::str::from_utf8(slice) {
            Ok(text) => Utf8LenResult::Count(text.chars().count() as i64),
            Err(e) => {
                // Return (nil, byte_position_of_error) — 1-based.
                let errpos = (start + e.valid_up_to() + 1) as i64;
                Utf8LenResult::Invalid(errpos)
            }
        }
    }

    // -----------------------------------------------------------------
    // utf8.offset(s, n [, i])
    // Returns the byte position (1-based) where the encoding of the
    // n-th character counting from position i starts.
    // n can be negative (count backwards).
    // Default i is 1 when n >= 0, or #s + 1 when n < 0.
    // -----------------------------------------------------------------
    #[function]
    fn offset(s: Bytes, n: i64, i: Option<i64>) -> Result<Option<i64>, VmError> {
        let slen = s.len();
        // Default starting position depends on direction.
        let start = match i {
            Some(pos) => {
                if pos >= 0 {
                    lua_byte_pos(pos, slen)
                } else {
                    // Negative: count from end, but result is 0-based index.
                    lua_byte_pos(pos, slen)
                }
            }
            None => {
                if n >= 0 {
                    0 // default to start of string
                } else {
                    slen // default to past end of string
                }
            }
        };

        if n == 0 {
            // n == 0: return the start of the character at position i.
            // Walk backward to find the start of the current character.
            let pos = find_char_start(&s, start);
            return Ok(Some(pos as i64 + 1));
        }

        if n > 0 {
            // Walk forward n characters from start.
            let mut pos = start;
            for _ in 1..n {
                if pos >= slen {
                    return Ok(None);
                }
                pos = next_char_boundary(&s, pos);
            }
            if pos > slen {
                return Ok(None);
            }
            Ok(Some(pos as i64 + 1))
        } else {
            // Walk backward |n| characters from start.
            let mut pos = start;
            for _ in 0..(-n) {
                if pos == 0 {
                    return Ok(None);
                }
                pos = prev_char_boundary(&s, pos);
            }
            Ok(Some(pos as i64 + 1))
        }
    }
}

// =====================================================================
// Helpers
// =====================================================================

/// Convert a 1-based Lua byte position to a 0-based Rust index.
/// Negative values count from the end. Clamps to [0, len].
fn lua_byte_pos(i: i64, len: usize) -> usize {
    if i >= 0 {
        ((i as usize).saturating_sub(1)).min(len)
    } else {
        let from_end = (-i) as usize;
        len.saturating_sub(from_end)
    }
}

/// Convert a 1-based Lua end position to a 0-based exclusive end.
/// Lua ranges are inclusive, so `j` maps to `j` in 0-based exclusive terms.
fn lua_byte_pos_end(j: i64, len: usize) -> usize {
    if j >= 0 {
        (j as usize).min(len)
    } else {
        let from_end = (-j) as usize;
        (len + 1).saturating_sub(from_end)
    }
}

/// Walk backward from `pos` to find the start of the UTF-8 character
/// containing (or just before) that byte position.
fn find_char_start(s: &[u8], mut pos: usize) -> usize {
    if pos >= s.len() {
        pos = s.len();
    }
    while pos > 0 && is_continuation_byte(s[pos.saturating_sub(1)]) {
        pos -= 1;
    }
    if pos > 0 {
        pos - 1
    } else {
        0
    }
}

/// Advance past the current UTF-8 character to the next character boundary.
fn next_char_boundary(s: &[u8], pos: usize) -> usize {
    if pos >= s.len() {
        return s.len() + 1;
    }
    let mut i = pos + 1;
    while i < s.len() && is_continuation_byte(s[i]) {
        i += 1;
    }
    i
}

/// Move backward to the previous character boundary.
fn prev_char_boundary(s: &[u8], pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut i = pos - 1;
    while i > 0 && is_continuation_byte(s[i]) {
        i -= 1;
    }
    i
}

/// Returns true if the byte is a UTF-8 continuation byte (10xxxxxx).
fn is_continuation_byte(b: u8) -> bool {
    (b & 0xC0) == 0x80
}
