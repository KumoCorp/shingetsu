//! Implementation of the `utf8` standard library module.

use bstr::ByteSlice;

use crate::valuevec;
use shingetsu::Bytes;

use crate::call_context::CallContext;
use crate::convert::Variadic;

/// Return type for `utf8.len`: character count or `(nil, errpos)`.
enum Utf8LenResult {
    Count(i64),
    /// Invalid UTF-8 detected at the given 1-based byte position.
    Invalid(i64),
}

impl crate::convert::IntoLuaMulti for Utf8LenResult {
    fn into_lua_multi(self) -> crate::ValueVec {
        match self {
            Utf8LenResult::Count(n) => valuevec![Value::Integer(n)],
            Utf8LenResult::Invalid(errpos) => {
                valuevec![Value::Nil, Value::Integer(errpos)]
            }
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
use crate::value::Value;
use crate::{Function, VmError, VmResultExt};

/// Return type for the `utf8.codes` iterator: `(byte_pos, codepoint)` or end.
#[derive(crate::IntoLuaMulti)]
enum Utf8CodesIterResult {
    Char(i64, i64),
    End,
}

/// Build the utf8 library table and register it as the `utf8` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = utf8_mod::build_module_table(env)?;
    env.set_global("utf8", Value::Table(table));
    env.register_module_type("utf8", utf8_mod::module_type());
    Ok(())
}

/// Operations on UTF-8 encoded strings.
///
/// Lua strings are sequences of bytes with no built-in notion of
/// encoding, but most real-world strings are UTF-8 (a way of
/// representing Unicode text as bytes).  The functions in this
/// module treat their string arguments as UTF-8: they understand
/// multi-byte characters and can iterate, count, and slice on
/// character boundaries instead of byte boundaries.
///
/// All byte positions in arguments and return values are 1-based,
/// matching the rest of the Lua standard library.  When a function
/// encounters bytes that aren't valid UTF-8 it either raises an
/// error or returns `nil` together with the position of the bad
/// byte; each function's documentation states which.
#[crate::module(name = "utf8")]
mod utf8_mod {
    use super::*;

    /// A Lua pattern that matches one UTF-8 byte sequence.
    ///
    /// Use this with `string.find`, `string.gmatch`, and other
    /// pattern functions to walk a string character by character
    /// without having to spell out the byte ranges yourself.  The
    /// pattern matches one complete UTF-8 character per match,
    /// whether it occupies one byte (ASCII) or several.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local count = 0
    /// for _ in string.gmatch("héllo", utf8.charpattern) do
    ///     count = count + 1
    /// end
    /// assert(count == 5)
    /// ```
    #[field]
    fn charpattern() -> Bytes {
        Bytes::from(b"[\0-\x7F\xC2-\xFD][\x80-\xBF]*")
    }

    /// Build a string from one or more Unicode code points.
    ///
    /// Each argument is an integer code point in the range `0` to
    /// `0x10FFFF` (the full Unicode range), encoded as UTF-8 bytes;
    /// the results are concatenated into a single string.  Float
    /// arguments are accepted and truncated to integers.
    ///
    /// Raises an error when an argument is not a number or is
    /// outside the valid Unicode range.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more integer code points
    ///
    /// # Returns
    ///
    /// - the concatenated UTF-8 encoded string; the empty string
    ///   when called with no arguments
    ///
    /// # Examples
    ///
    /// ```lua
    /// local s = utf8.char(72, 105)
    /// assert(s == "Hi")
    /// ```
    ///
    /// ```lua
    /// -- 0x1F600 is the "grinning face" emoji.
    /// local emoji = utf8.char(0x1F600)
    /// assert(#emoji == 4) -- four UTF-8 bytes
    /// ```
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

    /// Iterate over the characters of a UTF-8 string.
    ///
    /// Returns the three values used by Lua's generic `for` loop, so
    /// the typical use is:
    ///
    /// ```lua
    /// for byte_pos, codepoint in utf8.codes(s) do
    ///     -- ...
    /// end
    /// ```
    ///
    /// Each iteration step yields the 1-based byte position where
    /// the character starts and the integer code point of that
    /// character.
    ///
    /// Raises an error if `s` contains invalid UTF-8.  The whole
    /// string is checked when `utf8.codes` is called, before any
    /// iteration begins, so an error here means "`s` is not valid
    /// UTF-8" rather than "the loop stopped partway through".
    ///
    /// # Parameters
    ///
    /// - `s` — a UTF-8 encoded string
    ///
    /// # Returns
    ///
    /// - an iterator function suitable for the generic `for` loop
    /// - the string `s`, passed back as the iterator state
    /// - the integer `0`, the initial byte position
    ///
    /// # Examples
    ///
    /// ```lua
    /// local positions = {}
    /// local codes = {}
    /// for byte_pos, cp in utf8.codes("hé!") do
    ///     table.insert(positions, byte_pos)
    ///     table.insert(codes, cp)
    /// end
    /// assert(positions[1] == 1 and codes[1] == 0x68) -- 'h' at byte 1
    /// assert(positions[2] == 2 and codes[2] == 0xE9) -- 'é' at byte 2
    /// assert(positions[3] == 4 and codes[3] == 0x21) -- '!' at byte 4
    /// ```
    #[function]
    fn codes(ctx: CallContext, s: Bytes) -> Result<(Function, Bytes, i64), VmError> {
        // Validate entire string upfront (Lua 5.4 behavior).
        if let Err(e) = s.to_str() {
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
                let text = s[start..].to_str().expect("pre-validated UTF-8");
                match text.chars().next() {
                    Some(ch) => Ok(Utf8CodesIterResult::Char(start as i64 + 1, ch as i64)),
                    None => Ok(Utf8CodesIterResult::End),
                }
            },
        );

        Ok((iter_fn, s, 0))
    }

    /// Read out the integer code points of a range of characters.
    ///
    /// Returns the code points of every character whose starting
    /// byte falls in the inclusive byte range `[i, j]`.  When `i` is
    /// omitted it defaults to `1`; when `j` is omitted it defaults
    /// to `i`, so calling `utf8.codepoint(s, n)` returns the single
    /// code point starting at byte `n`.
    ///
    /// Negative byte positions count back from the end of the
    /// string: `-1` is the last byte, `-2` the second-to-last, and
    /// so on.  When the resulting range is empty no values are
    /// returned.
    ///
    /// Raises an error if the byte range covers invalid UTF-8.
    ///
    /// # Parameters
    ///
    /// - `s` — a UTF-8 encoded string
    /// - `i` — starting byte position; defaults to `1`
    /// - `j` — ending byte position (inclusive); defaults to `i`
    ///
    /// # Returns
    ///
    /// - the code points of the matching characters as integers,
    ///   one per character, in source order
    ///
    /// # Examples
    ///
    /// ```lua
    /// local cp = utf8.codepoint("A")
    /// assert(cp == 65)
    /// ```
    ///
    /// ```lua
    /// local a, b, c = utf8.codepoint("abc", 1, 3)
    /// assert(a == 97 and b == 98 and c == 99)
    /// ```
    ///
    /// ```lua
    /// -- 'é' occupies bytes 2 and 3, so reading byte 2 returns
    /// -- the code point for 'é' (0xE9 = 233).
    /// local cp = utf8.codepoint("héllo", 2)
    /// assert(cp == 0xE9)
    /// ```
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
        let text = slice.to_str().map_err(|e| VmError::BadArgument {
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

    /// Count the number of UTF-8 characters in a byte range.
    ///
    /// Counts the characters whose starting byte falls in the
    /// inclusive byte range `[i, j]`.  Byte positions are 1-based;
    /// negative values count back from the end (`-1` is the last
    /// byte).  The defaults select the entire string.
    ///
    /// Returns the count on success.  When the range contains
    /// invalid UTF-8 returns `nil` plus the 1-based byte position
    /// of the first bad byte, so callers can locate and report
    /// encoding errors.
    ///
    /// Note that `#s` (the length operator on a string) returns the
    /// number of *bytes*, not characters.  Use `utf8.len` when you
    /// care about character count.
    ///
    /// # Parameters
    ///
    /// - `s` — a string to measure
    /// - `i` — starting byte position; defaults to `1`
    /// - `j` — ending byte position (inclusive); defaults to `-1`
    ///   (the last byte)
    ///
    /// # Returns
    ///
    /// - the character count when `s` is valid UTF-8 in the
    ///   selected range
    /// - `nil` plus the byte position of the first invalid byte
    ///   when the selected range contains invalid UTF-8
    ///
    /// # Examples
    ///
    /// ```lua
    /// local n = utf8.len("héllo")
    /// assert(n == 5)         -- 5 characters
    /// assert(#"héllo" == 6)  -- but 6 bytes
    /// ```
    ///
    /// ```lua
    /// local count, errpos = utf8.len("abc\xff")
    /// assert(count == nil)
    /// assert(errpos == 4)    -- byte 4 is the bad byte
    /// ```
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

    /// Convert a character index to a byte position.
    ///
    /// Returns the 1-based byte position where the `n`-th character
    /// starts, counting from byte position `i`.  When `n` is
    /// negative the count goes backwards.  When `n` is `0` the
    /// function returns the byte position where the character
    /// containing byte `i` starts; this is useful for snapping a
    /// byte position back to a character boundary.
    ///
    /// `i` defaults to `1` when `n` is non-negative, or to one past
    /// the end of the string (`#s + 1`) when `n` is negative.  This
    /// makes `utf8.offset(s, -1)` return the byte position of the
    /// last character.
    ///
    /// Returns `nil` when the requested character lies outside the
    /// string's bounds.
    ///
    /// # Parameters
    ///
    /// - `s` — a UTF-8 encoded string
    /// - `n` — character offset; positive counts forward, negative
    ///   counts backward, `0` snaps `i` to a character boundary
    /// - `i` — starting byte position; defaults to `1` for `n >= 0`
    ///   and `#s + 1` for `n < 0`
    ///
    /// # Returns
    ///
    /// - the byte position of the requested character
    /// - `nil` when the requested character is out of range
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- The 1st character of an ASCII string starts at byte 1.
    /// assert(utf8.offset("hello", 1) == 1)
    /// -- The 3rd character of "héllo" is the first 'l': 'é' occupies
    /// -- bytes 2 and 3, so 'l' starts at byte 4.
    /// assert(utf8.offset("héllo", 3) == 4)
    /// ```
    ///
    /// ```lua
    /// -- Last character of a string.
    /// local s = "café"
    /// assert(utf8.offset(s, -1) == 4)
    /// ```
    ///
    /// ```lua
    /// -- Snap a byte position to its enclosing character boundary.
    /// -- Byte 3 is the middle of 'é' (a 2-byte character starting at 2);
    /// -- offset 0 walks back to the start of that character.
    /// assert(utf8.offset("héllo", 0, 3) == 2)
    /// ```
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

    // ----------------------------------------------------------------
    // Extensions: lossy / truncate / sub / reverse.
    //
    // These are non-standard shingetsu additions.  All four operate
    // on codepoints (where the existing standard `utf8` functions
    // mostly use byte positions); `truncate`, `sub`, and `reverse`
    // raise an error on invalid UTF-8, matching `utf8.codepoint`.
    // Use `utf8.lossy` first to sanitise arbitrary bytes into valid
    // UTF-8 if you cannot rely on the input being well-formed.
    // ----------------------------------------------------------------

    /// Returns a copy of `s` in which every maximal run of invalid
    /// UTF-8 bytes is replaced with the Unicode replacement
    /// character (`U+FFFD`, encoded as `\xEF\xBF\xBD`).  The result
    /// is always valid UTF-8 and can be fed safely into the strict
    /// `utf8.*` functions.
    ///
    /// When `s` is already valid UTF-8 the original string is
    /// returned (no allocation).
    ///
    /// # Parameters
    /// - `s` (string): the bytes to sanitise.
    ///
    /// # Returns
    /// (string): a valid-UTF-8 copy of `s`.
    ///
    /// # Examples
    /// ```lua
    /// assert(utf8.lossy("hello") == "hello")
    /// -- \xFF is invalid as a UTF-8 lead byte; it becomes U+FFFD.
    /// local fixed = utf8.lossy("a\xFFb")
    /// assert(utf8.len(fixed) == 3)
    /// ```
    /// Returns true if `s` is well-formed UTF-8, false otherwise.
    ///
    /// This is the cheap "validate but do not process" check that pairs
    /// with the strict `utf8.*` functions: when `is_valid` returns
    /// false, `utf8.codepoint`, `utf8.sub`, `utf8.truncate`, and
    /// `utf8.reverse` would all raise on the same input.  Use
    /// `utf8.lossy` to coerce arbitrary bytes into valid UTF-8 if you
    /// need to process them downstream.
    ///
    /// # Parameters
    /// - `s` (string): the bytes to check.
    ///
    /// # Returns
    /// (boolean): true when `s` is valid UTF-8.
    ///
    /// # Examples
    /// ```lua
    /// assert(utf8.is_valid("hello"))
    /// assert(utf8.is_valid("h\u{00E9}llo"))
    /// assert(not utf8.is_valid("a\xFFb"))
    /// assert(utf8.is_valid(""))
    /// ```
    #[function]
    fn is_valid(s: Bytes) -> bool {
        s.to_str().is_ok()
    }

    #[function]
    fn lossy(s: Bytes) -> Bytes {
        match s.to_str() {
            Ok(_) => s,
            Err(_) => Bytes::from(s.to_str_lossy().into_owned()),
        }
    }

    /// Truncates `s` to at most `max_chars` Unicode codepoints.
    ///
    /// When `ellipsis` is given (and truncation actually occurs),
    /// the result ends with `ellipsis` and the total codepoint
    /// length remains at most `max_chars`.  If `max_chars` is too
    /// small to fit the ellipsis itself, the ellipsis is itself
    /// truncated by codepoints to `max_chars`.  The default
    /// `ellipsis` is the empty string.
    ///
    /// Raises an error when `max_chars` is negative, when `s`
    /// contains invalid UTF-8, or when `ellipsis` contains invalid
    /// UTF-8.
    ///
    /// # Parameters
    /// - `s` (string): the source string (must be valid UTF-8).
    /// - `max_chars` (integer): maximum codepoint length.
    /// - `ellipsis` (string, optional): marker appended on
    ///   truncation; defaults to the empty string.
    ///
    /// # Returns
    /// (string): the (possibly truncated) string.
    ///
    /// # Examples
    /// ```lua
    /// assert(utf8.truncate("héllo", 3) == "hél")
    /// assert(utf8.truncate("héllo", 100) == "héllo")
    /// assert(utf8.truncate("héllo world", 6, "...") == "hél...")
    /// ```
    #[function]
    fn truncate(s: Bytes, max_chars: i64, ellipsis: Option<Bytes>) -> Result<Bytes, VmError> {
        if max_chars < 0 {
            return Err(VmError::BadArgument {
                position: 2,
                function: "utf8.truncate".to_string(),
                expected: "non-negative integer".to_string(),
                got: format!("{max_chars}"),
            });
        }
        let max_chars = max_chars as usize;
        let text = std::str::from_utf8(&s)
            .map_err(|e| invalid_utf8_arg("utf8.truncate", 1, e.valid_up_to()))?;
        let ellipsis = ellipsis.unwrap_or_default();
        let ellipsis_text = std::str::from_utf8(&ellipsis)
            .map_err(|e| invalid_utf8_arg("utf8.truncate", 3, e.valid_up_to()))?;

        // Cheap upper-bound check: every codepoint is at least one
        // byte, so if there are fewer bytes than max_chars there
        // cannot be more codepoints than max_chars.
        if text.len() <= max_chars {
            // Still need precise check only if byte count exceeds.
            return Ok(s);
        }
        let total_chars = text.chars().count();
        if total_chars <= max_chars {
            return Ok(s);
        }

        // Truncation occurs.  If the ellipsis itself does not fit
        // within the codepoint budget, truncate the ellipsis to
        // max_chars codepoints (and emit nothing from `s`).
        let ellipsis_chars = ellipsis_text.chars().count();
        if ellipsis_chars >= max_chars {
            let cut = nth_char_byte(ellipsis_text, max_chars);
            return Ok(Bytes::from(ellipsis_text.as_bytes()[..cut].to_vec()));
        }
        let keep_chars = max_chars - ellipsis_chars;
        let cut = nth_char_byte(text, keep_chars);
        let mut out = Vec::with_capacity(cut + ellipsis_text.len());
        out.extend_from_slice(&text.as_bytes()[..cut]);
        out.extend_from_slice(ellipsis_text.as_bytes());
        Ok(Bytes::from(out))
    }

    /// Returns the substring of `s` between codepoint indices `i`
    /// and `j` (inclusive), 1-based, with negative values counting
    /// from the end in codepoints.
    ///
    /// Mirrors `string.sub` but operates on codepoints rather than
    /// bytes.  `i` defaults to `1` and `j` defaults to `-1`.  Out-
    /// of-range indices clamp to the available range and may yield
    /// the empty string.
    ///
    /// Raises an error when `s` contains invalid UTF-8.
    ///
    /// # Parameters
    /// - `s` (string): the source string (must be valid UTF-8).
    /// - `i` (integer, optional): starting codepoint index; defaults to 1.
    /// - `j` (integer, optional): ending codepoint index, inclusive;
    ///   defaults to -1.
    ///
    /// # Returns
    /// (string): the codepoint-indexed substring.
    ///
    /// # Examples
    /// ```lua
    /// assert(utf8.sub("héllo", 2, 3) == "él")
    /// assert(utf8.sub("héllo", -2) == "lo")
    /// assert(utf8.sub("héllo", 1, 1) == "h")
    /// -- Out-of-range yields empty string
    /// assert(utf8.sub("hi", 10, 20) == "")
    /// ```
    #[function]
    fn sub(s: Bytes, i: Option<i64>, j: Option<i64>) -> Result<Bytes, VmError> {
        let text = std::str::from_utf8(&s)
            .map_err(|e| invalid_utf8_arg("utf8.sub", 1, e.valid_up_to()))?;
        let total = text.chars().count() as i64;
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(-1);

        // Resolve to 1-based codepoint positions.  Negative indices
        // count from the end (-1 is the last codepoint).  Clamp the
        // start up to 1 and the end down to `total`.
        let start = if i < 0 {
            (total + i + 1).max(1)
        } else if i == 0 {
            1
        } else {
            i
        };
        let end = if j < 0 { total + j + 1 } else { j.min(total) };
        if start > end || start > total || end < 1 {
            return Ok(Bytes::default());
        }
        let start_idx = (start - 1) as usize;
        let end_idx = end as usize; // inclusive count -> exclusive char index
        let start_byte = nth_char_byte(text, start_idx);
        let end_byte = nth_char_byte(text, end_idx);
        Ok(Bytes::from(text.as_bytes()[start_byte..end_byte].to_vec()))
    }

    /// Returns `s` with its codepoints in reverse order.
    ///
    /// Unlike `string.reverse` (which reverses bytes and can produce
    /// invalid UTF-8 from non-ASCII input), `utf8.reverse` reverses
    /// whole codepoints.  Combining marks, ZWJ-joined emoji, and
    /// other multi-codepoint grapheme clusters are *not* preserved
    /// as units -- this is a codepoint reverse, not a grapheme
    /// reverse.
    ///
    /// Raises an error when `s` contains invalid UTF-8.
    ///
    /// # Parameters
    /// - `s` (string): the source string (must be valid UTF-8).
    ///
    /// # Returns
    /// (string): the codepoint-reversed string.
    ///
    /// # Examples
    /// ```lua
    /// assert(utf8.reverse("hello") == "olleh")
    /// assert(utf8.reverse("héllo") == "olléh")
    /// assert(utf8.reverse("") == "")
    /// ```
    #[function]
    fn reverse(s: Bytes) -> Result<Bytes, VmError> {
        let text = std::str::from_utf8(&s)
            .map_err(|e| invalid_utf8_arg("utf8.reverse", 1, e.valid_up_to()))?;
        let mut out = String::with_capacity(text.len());
        for c in text.chars().rev() {
            out.push(c);
        }
        Ok(Bytes::from(out))
    }
}

// Helpers used by the utf8 extensions.

/// Returns the byte offset of the `n`-th codepoint in `text`
/// (0-based: `nth_char_byte(text, 0)` is `0`, and
/// `nth_char_byte(text, char_count)` is `text.len()`).  When `n`
/// exceeds the codepoint count we return `text.len()`, which keeps
/// the result a valid slice boundary.
fn nth_char_byte(text: &str, n: usize) -> usize {
    text.char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

/// Builds a `BadArgument` matching the existing utf8 module's style
/// for invalid-UTF-8 inputs.  `prefix_len` is the byte offset
/// (relative to the argument) up to which the bytes were valid; the
/// reported position is 1-based, so we add 1.
fn invalid_utf8_arg(function: &str, position: usize, prefix_len: usize) -> VmError {
    VmError::BadArgument {
        position,
        function: function.to_string(),
        expected: "valid UTF-8 string".to_string(),
        got: format!("invalid UTF-8 at byte {}", prefix_len + 1),
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
