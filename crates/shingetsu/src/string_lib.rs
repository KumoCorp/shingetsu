//! Lua `string` standard library.

use crate::valuevec;
use shingetsu::Bytes;

use crate::convert::Variadic;
use crate::lua_pattern::{Capture, Match, Pattern};
use crate::table::Table;
use crate::{Function, VmError};

use crate::value::{Value, ValueVec};

/// Return type for `string.find`: `(start, end, ...captures)` or `nil`.
#[derive(crate::IntoLuaMulti)]
enum FindResult {
    Match(i64, i64, Variadic),
    NotFound,
}

/// Return type for `string.match`: captures or `nil`.
#[derive(crate::IntoLuaMulti)]
enum StringMatchResult {
    Captures(Variadic),
    NotFound,
}

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
        Capture::Span { start, end } => Value::string(&haystack[start..end]),
        Capture::Position(p) => Value::Integer(p as i64 + 1),
    }
}

/// Extract the "captures list" from a match for callers like `match`,
/// `gmatch`, and `gsub`.  If the pattern has no explicit captures, the
/// whole match is returned as a single string capture (mirroring the
/// reference Lua behaviour).
fn extract_captures(m: &Match, haystack: &[u8]) -> Vec<Value> {
    if m.captures.is_empty() {
        vec![Value::string(&haystack[m.start..m.end])]
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
                "invalid replacement value (a {})",
                other.type_name()
            ))
            .with_arg_position(3)
            .with_hint(
                "the third argument to `string.gsub` must produce a \
                 string or a number per match; convert other types via \
                 `tostring` first",
            ));
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
    // When there are no explicit captures, %1 refers to the whole match
    // (same as %0), matching Lua 5.4's push_onecapture behavior.
    if m.captures.is_empty() {
        if idx == 1 {
            result.extend_from_slice(&haystack[m.start..m.end]);
            return Ok(());
        } else {
            return Err(runtime_error(format!("invalid capture index %{}", idx)));
        }
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
        None => Value::string(&haystack[m.start..m.end]),
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
                let ret = ctx
                    .call_function(f, valuevec![Value::Table(tab), key])
                    .await?;
                return Ok(ret.into_iter().next().unwrap_or(Value::Nil));
            }
            Some(_other) => return Ok(Value::Nil),
        }
    }
    Err(
        runtime_error("'__index' chain too long".to_owned()).with_hint(
            "the `__index` metamethod chain hit the recursion guard; \
         this usually means a metatable cycle, or a `__index` \
         that always returns another table whose own `__index` \
         keeps redirecting",
        ),
    )
}

/// Byte-oriented string manipulation: length, slicing, case conversion,
/// pattern matching, and `printf`-style formatting.
///
/// Lua strings are sequences of arbitrary bytes — they are not required to
/// be valid UTF-8. The functions in this module operate on bytes, with
/// 1-based indexing where negative indices count from the end of the
/// string. For multi-byte text aware operations, see the `utf8` library.
///
/// All functions are also accessible through method-call syntax on string
/// values, since the string metatable's `__index` points at this module:
/// `("hello"):upper()` is equivalent to `string.upper("hello")`.
///
/// ## Pattern syntax
///
/// `string.find`, `string.match`, `string.gmatch`, and `string.gsub`
/// accept a *pattern* — a small string-matching language inspired by
/// regular expressions but considerably simpler.  The grammar below
/// covers everything Shingetsu's pattern engine recognises.
///
/// ### Character classes
///
/// A class matches one byte from a defined set.  Lowercase forms list
/// the set; uppercase forms are the complement of the lowercase form.
///
/// | Class | Matches                                            |
/// | ----- | -------------------------------------------------- |
/// | `.`   | any byte                                           |
/// | `%a`  | ASCII letter (`A-Z`, `a-z`)                        |
/// | `%c`  | ASCII control byte                                 |
/// | `%d`  | ASCII digit (`0-9`)                                |
/// | `%g`  | printable, non-space (`0x21`–`0x7E`)               |
/// | `%l`  | ASCII lowercase letter                             |
/// | `%p`  | ASCII punctuation                                  |
/// | `%s`  | whitespace (space, tab, CR, LF, VT, FF)            |
/// | `%u`  | ASCII uppercase letter                             |
/// | `%w`  | ASCII alphanumeric                                 |
/// | `%x`  | ASCII hex digit (`0-9`, `A-F`, `a-f`)              |
/// | `%z`  | the NUL byte (retained for compatibility)          |
///
/// `%A`, `%C`, `%D`, … are the complements: `%D` matches any byte that
/// is not a digit, and so on.
///
/// A literal `%` is written `%%`.  The pattern engine works on bytes,
/// not characters; classes only know about ASCII.  For UTF-8-aware
/// matching see the `utf8` library.
///
/// ### Sets
///
/// Square brackets define a set of bytes:
///
/// - `[abc]` — any of `a`, `b`, or `c`.
/// - `[a-z]` — any byte in the byte range from `a` to `z` inclusive.
/// - `[%a%d]` — character classes are allowed inside; this matches a
///   letter or a digit.
/// - `[^abc]` — a leading `^` complements the set: any byte *not* in
///   `{a, b, c}`.
///
/// Inside a set, `%` still introduces a class; the other magic
/// characters lose their special meaning.
///
/// ### Anchors
///
/// - `^pat` at the start of a pattern anchors the match to the start
///   of the string.  At any other position `^` is literal.
/// - `pat$` at the end anchors the match to the end of the string.
///   At any other position `$` is literal.
///
/// ### Quantifiers
///
/// A quantifier follows a single class, set, or literal byte and
/// repeats it:
///
/// | Quantifier | Repetitions                                |
/// | ---------- | ------------------------------------------ |
/// | `*`        | zero or more, *as many as possible*        |
/// | `+`        | one or more, *as many as possible*         |
/// | `-`        | zero or more, *as few as possible*         |
/// | `?`        | zero or one                                |
///
/// The `-` form is the lazy counterpart of `*`: when the rest of the
/// pattern still has work to do, `-` prefers a shorter run while `*`
/// prefers a longer one.
///
/// ### Captures
///
/// Parentheses around a sub-pattern mark a *capture* — the matching
/// substring becomes one of the function's return values.  An empty
/// pair `()` records the current 1-based byte position instead of a
/// substring.  Captures are numbered left-to-right starting at `1`.
///
/// Inside a pattern, `%1`–`%9` *back-references* a previously captured
/// substring: `(%a)%1` matches a letter immediately followed by the
/// same letter.  In a `string.gsub` replacement string, `%0` refers
/// to the entire match and `%1`–`%9` to the captured substrings.
///
/// ### Balanced match: `%b`
///
/// `%bxy` matches a *balanced* substring that starts with byte `x`
/// and ends with byte `y`, with nested `x`/`y` pairs counted: `%b()`
/// over `"f(g(h)i)j"` matches `"(g(h)i)"`.
///
/// ### Frontier: `%f`
///
/// `%f[set]` matches the empty string at a position where the
/// previous byte is not in `set` and the next byte is.  Useful for
/// matching word boundaries: `%f[%a]` matches the start of any run
/// of letters.
///
/// ### Escaping magic characters
///
/// The metacharacters `( ) . % + - * ? [ ] ^ $` lose their special
/// meaning when prefixed with `%`.  To match a literal `(`, write
/// `%(`.
///
/// ### Differences from full regular expressions
///
/// Shingetsu's patterns intentionally lack alternation (no `|`),
/// general grouping repetition (no `(...)+`), and brace quantifiers
/// (no `{n,m}`).  When you need those, do the work in pieces with
/// several calls or build the result with explicit code.  In
/// exchange the pattern matcher is small, allocation-free, and
/// behaves predictably without the catastrophic-backtracking traps
/// that more powerful regex engines can fall into.
#[crate::module(name = "string")]
pub mod string_mod {
    use super::*;

    /// Returns the length of `s` in bytes.
    ///
    /// Equivalent to the `#` length operator on a string. Because Lua
    /// strings are byte sequences, this counts bytes, not characters: a
    /// UTF-8 string containing multi-byte codepoints will have a length
    /// greater than the number of visible characters. Use `utf8.len` for
    /// codepoint counting.
    ///
    /// # Parameters
    /// - `s` (string): the string to measure.
    ///
    /// # Returns
    /// (integer): the number of bytes in `s`.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.len("hello") == 5)
    /// assert(string.len("") == 0)
    /// -- Multi-byte UTF-8: "é" is two bytes
    /// assert(string.len("é") == 2)
    /// ```
    #[function]
    fn len(s: Bytes) -> i64 {
        s.len() as i64
    }

    /// Returns the integer byte values of `s[i]` through `s[j]`, inclusive.
    ///
    /// Indices are 1-based; negative values count from the end. When `i`
    /// is omitted it defaults to `1`; when `j` is omitted it defaults to
    /// `i` (so `string.byte(s)` returns just the first byte). Out-of-range
    /// indices simply yield no values rather than erroring.
    ///
    /// # Parameters
    /// - `s` (string): the source string.
    /// - `i` (integer, optional): starting index (default `1`).
    /// - `j` (integer, optional): ending index, inclusive (default `i`).
    ///
    /// # Returns
    /// (integer...): zero or more byte values in the range `0..=255`.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.byte("A") == 65)
    /// assert(string.byte("hello", 2) == 101) -- 'e'
    /// local h, e, l, l2, o = string.byte("hello", 1, -1)
    /// assert(h == 104 and e == 101 and l == 108 and l2 == 108 and o == 111)
    /// ```
    #[function]
    fn byte(s: Bytes, i: Option<i64>, j: Option<i64>) -> crate::convert::TypedVariadic<i64> {
        let len = s.len();
        let i = i.unwrap_or(1);
        let j = j.unwrap_or(i);
        let start = lua_index(i, len);
        let end = lua_index_end(j, len);
        if start >= end {
            return crate::convert::TypedVariadic(vec![]);
        }
        crate::convert::TypedVariadic(s[start..end].iter().map(|&b| b as i64).collect())
    }

    /// Returns a string composed of the given byte values.
    ///
    /// Each argument must be an integer (or float-with-no-fractional-part)
    /// in the range `0..=255`. Out-of-range values raise a bad-argument
    /// error. The result is the concatenation of those bytes — no
    /// codepoint or character interpretation is applied.
    ///
    /// # Parameters
    /// - `...` (integer): zero or more byte values.
    ///
    /// # Returns
    /// (string): a string with one byte per argument.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.char(65, 66, 67) == "ABC")
    /// assert(string.char() == "")
    /// assert(string.char(0x68, 0x69) == "hi")
    /// ```
    #[function]
    fn char(args: Variadic) -> Result<Bytes, VmError> {
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
        Ok(Bytes::from(buf))
    }

    /// Returns a copy of `s` with ASCII lowercase letters mapped to
    /// uppercase. Non-ASCII bytes are passed through unchanged — locale
    /// rules and Unicode case folding are not applied.
    ///
    /// # Parameters
    /// - `s` (string): the source string.
    ///
    /// # Returns
    /// (string): the uppercased copy.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.upper("Hello") == "HELLO")
    /// assert(string.upper("abc123") == "ABC123")
    /// ```
    #[function]
    fn upper(s: Bytes) -> Bytes {
        Bytes::from(s.to_ascii_uppercase())
    }

    /// Returns a copy of `s` with ASCII uppercase letters mapped to
    /// lowercase. Non-ASCII bytes are passed through unchanged — locale
    /// rules and Unicode case folding are not applied.
    ///
    /// # Parameters
    /// - `s` (string): the source string.
    ///
    /// # Returns
    /// (string): the lowercased copy.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.lower("Hello") == "hello")
    /// assert(string.lower("ABC123") == "abc123")
    /// ```
    #[function]
    fn lower(s: Bytes) -> Bytes {
        Bytes::from(s.to_ascii_lowercase())
    }

    /// Returns `s` with its bytes in reverse order.
    ///
    /// This reverses bytes, not characters: a multi-byte UTF-8 string will
    /// not produce a meaningful reversal of the visible text.
    ///
    /// # Parameters
    /// - `s` (string): the string to reverse.
    ///
    /// # Returns
    /// (string): the byte-reversed string.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.reverse("hello") == "olleh")
    /// assert(string.reverse("") == "")
    /// ```
    #[function]
    fn reverse(s: Bytes) -> Bytes {
        let mut v: Vec<u8> = s.to_vec();
        v.reverse();
        Bytes::from(v)
    }

    /// Returns the substring of `s` from index `i` to index `j`, inclusive.
    ///
    /// Indices are 1-based bytes; negative values count from the end (so
    /// `-1` is the last byte). When `j` is omitted it defaults to `-1`,
    /// returning the rest of the string. Indices are clamped to the
    /// string boundaries — passing values outside `[-#s, #s]` is not an
    /// error and returns an empty string when the range is empty.
    ///
    /// # Parameters
    /// - `s` (string): the source string.
    /// - `i` (integer): starting index.
    /// - `j` (integer, optional): ending index, inclusive (default `-1`).
    ///
    /// # Returns
    /// (string): the substring, possibly empty.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.sub("hello world", 1, 5) == "hello")
    /// assert(string.sub("hello world", 7) == "world")
    /// assert(string.sub("hello world", -5) == "world")
    /// assert(string.sub("hello", 2, -2) == "ell")
    /// ```
    #[function]
    fn sub(s: Bytes, i: i64, j: Option<i64>) -> Bytes {
        let len = s.len();
        let j = j.unwrap_or(-1);
        let start = lua_index(i, len);
        let end = lua_index_end(j, len);
        if start >= end {
            Bytes::default()
        } else {
            Bytes::from(&s[start..end])
        }
    }

    /// Returns `s` repeated `n` times, optionally with `sep` between copies.
    ///
    /// When `n <= 0` the result is the empty string. The separator only
    /// appears between copies, not at the start or end, so
    /// `string.rep("a", 3, ",")` is `"a,a,a"` (not `"a,a,a,"`).
    ///
    /// # Parameters
    /// - `s` (string): the string to repeat.
    /// - `n` (integer): number of repetitions.
    /// - `sep` (string, optional): separator placed between copies.
    ///
    /// # Returns
    /// (string): the repeated string.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.rep("ab", 3) == "ababab")
    /// assert(string.rep("ab", 3, "-") == "ab-ab-ab")
    /// assert(string.rep("x", 0) == "")
    /// ```
    #[function]
    fn rep(s: Bytes, n: i64, sep: Option<Bytes>) -> Bytes {
        if n <= 0 {
            return Bytes::default();
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

    /// Searches `s` for the first match of `pattern` and returns its bounds.
    ///
    /// On success returns the 1-based start and end byte indices of the
    /// match, followed by any captures produced by the pattern. On
    /// failure returns `nil`.
    ///
    /// The `pattern` argument uses the syntax described in the
    /// [pattern reference](index.md#pattern-syntax).
    ///
    /// When `plain` is `true`, `pattern` is treated as a literal byte
    /// sequence rather than a Lua pattern — no metacharacters are
    /// interpreted, and no captures are produced. This is faster than
    /// pattern matching when you don't need wildcards.
    ///
    /// `init` selects the byte offset to start searching from; it follows
    /// the same 1-based / negative-from-end conventions as `string.sub`.
    ///
    /// See also `string.match` for capture-only results, and
    /// `string.gmatch` for iterating all matches.
    ///
    /// # Parameters
    /// - `s` (string): the string to search.
    /// - `pattern` (string): the Lua pattern (or plain substring).
    /// - `init` (integer, optional): byte index to start at (default `1`).
    /// - `plain` (boolean, optional): treat `pattern` as a literal (default `false`).
    ///
    /// # Returns
    /// On match: `(integer, integer, ...)` — start index, end index, and
    /// any captures. On no match: `nil`.
    ///
    /// # Examples
    /// ```lua
    /// local s, e = string.find("hello world", "world")
    /// assert(s == 7 and e == 11)
    ///
    /// -- Plain search ignores pattern metacharacters
    /// local ps, pe = string.find("a.b.c", ".", 1, true)
    /// assert(ps == 2 and pe == 2)
    ///
    /// -- Captures are returned after the bounds
    /// local s2, e2, year, month = string.find("date: 2025-01", "(%d+)-(%d+)")
    /// assert(year == "2025" and month == "01")
    ///
    /// assert(string.find("hello", "xyz") == nil)
    /// ```
    #[function]
    fn find(
        s: Bytes,
        pattern: Bytes,
        init: Option<i64>,
        plain: Option<bool>,
    ) -> Result<FindResult, VmError> {
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
                return Ok(FindResult::Match(
                    lua_start,
                    start as i64,
                    Variadic(valuevec![]),
                ));
            }
            if let Some(pos) = haystack
                .windows(pattern.len())
                .position(|w| w == &pattern[..])
            {
                let lua_start = (start + pos + 1) as i64;
                let lua_end = (start + pos + pattern.len()) as i64;
                Ok(FindResult::Match(lua_start, lua_end, Variadic(valuevec![])))
            } else {
                Ok(FindResult::NotFound)
            }
        } else {
            let pat = compile_pattern(&pattern)?;
            if let Some(m) = pattern_find(&pat, haystack, 0)? {
                let lua_start = (start + m.start + 1) as i64;
                let lua_end = (start + m.end) as i64;
                let captures: Vec<Value> = m
                    .captures
                    .iter()
                    .map(|c| capture_to_value(c, haystack))
                    .collect();
                Ok(FindResult::Match(
                    lua_start,
                    lua_end,
                    Variadic(captures.into()),
                ))
            } else {
                Ok(FindResult::NotFound)
            }
        }
    }

    /// Returns the captures from the first match of `pattern` in `s`.
    ///
    /// If the pattern contains explicit captures, returns one value per
    /// capture. If the pattern has no captures, returns the whole match
    /// as a single string. Returns `nil` when there is no match.
    ///
    /// The `pattern` argument uses the syntax described in the
    /// [pattern reference](index.md#pattern-syntax).
    ///
    /// Use this when you only want the matched text, not the bounds —
    /// see `string.find` if you also need the byte indices.
    ///
    /// # Parameters
    /// - `s` (string): the string to search.
    /// - `pattern` (string): the Lua pattern.
    /// - `init` (integer, optional): byte index to start at (default `1`).
    ///
    /// # Returns
    /// On match: one value per capture (or the whole match if there are no
    /// captures). On no match: `nil`.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.match("hello world", "%a+") == "hello")
    ///
    /// local year, month, day = string.match("2025-01-15", "(%d+)-(%d+)-(%d+)")
    /// assert(year == "2025" and month == "01" and day == "15")
    ///
    /// assert(string.match("abc", "%d+") == nil)
    /// ```
    #[function(rename = "match")]
    fn string_match(
        s: Bytes,
        pattern: Bytes,
        init: Option<i64>,
    ) -> Result<StringMatchResult, VmError> {
        let len = s.len();
        let start = if let Some(i) = init {
            lua_index(i, len)
        } else {
            0
        };
        let haystack = &s[start..];

        let pat = compile_pattern(&pattern)?;
        if let Some(m) = pattern_find(&pat, haystack, 0)? {
            Ok(StringMatchResult::Captures(Variadic(
                extract_captures(&m, haystack).into(),
            )))
        } else {
            Ok(StringMatchResult::NotFound)
        }
    }

    /// Replaces occurrences of `pattern` in `s` and returns the result.
    ///
    /// The `pattern` argument uses the syntax described in the
    /// [pattern reference](index.md#pattern-syntax).
    ///
    /// `repl` controls what each match is replaced with:
    /// - **string**: the replacement text. `%0` refers to the whole match,
    ///   `%1`..`%9` to the corresponding capture, and `%%` to a literal
    ///   `%`. Any other `%`-escape is an error.
    /// - **table**: the first capture (or whole match if none) is used as
    ///   a key; the looked-up value (string or number) is the replacement.
    ///   `nil` or `false` keeps the original match unchanged. The
    ///   `__index` metamethod is honoured.
    /// - **function**: called with the captures as arguments for each
    ///   match. Its first return value (string or number) is the
    ///   replacement; `nil` or `false` keeps the original match.
    ///
    /// When `n` is given, only the first `n` matches are replaced.
    ///
    /// # Parameters
    /// - `s` (string): the string to operate on.
    /// - `pattern` (string): the Lua pattern.
    /// - `repl` (string | table | function): the replacement.
    /// - `n` (integer, optional): maximum number of replacements.
    ///
    /// # Returns
    /// (string, integer): the substituted string, and the number of
    /// replacements actually performed.
    ///
    /// # Examples
    /// ```lua
    /// local out, n = string.gsub("hello world", "o", "0")
    /// assert(out == "hell0 w0rld" and n == 2)
    ///
    /// -- Capture references in replacement string
    /// local swapped = string.gsub("Smith, John", "(%w+), (%w+)", "%2 %1")
    /// assert(swapped == "John Smith")
    ///
    /// -- Limit replacements
    /// local out2 = string.gsub("aaaa", "a", "b", 2)
    /// assert(out2 == "bbaa")
    ///
    /// -- Function replacement
    /// local shouted = string.gsub("hi there", "%a+", function(w)
    ///     return string.upper(w)
    /// end)
    /// assert(shouted == "HI THERE")
    ///
    /// -- Table replacement
    /// local expanded = string.gsub("$name is $age", "%$(%w+)", {
    ///     name = "Alice", age = "30",
    /// })
    /// assert(expanded == "Alice is 30")
    /// ```
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
                                )
                                .with_hint(
                                    "every `%` in a replacement string \
                                     must be followed by a digit (`%0` is \
                                     the whole match, `%1`..`%9` are the \
                                     captures) or another `%` for a \
                                     literal `%`",
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
                                )
                                .with_hint(
                                    "every `%` in a replacement string \
                                     must be followed by a digit (`%0` is \
                                     the whole match, `%1`..`%9` are the \
                                     captures) or another `%` for a \
                                     literal `%`",
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
                    let call_args: ValueVec = extract_captures(&m, &s).into();
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

    /// Formats values into a string using `printf`-style directives.
    ///
    /// Each `%`-directive in `fmt` consumes one argument. Supported
    /// conversions:
    ///
    /// - `%d`, `%i` — signed decimal integer
    /// - `%u` — unsigned decimal integer
    /// - `%x`, `%X` — lowercase / uppercase hexadecimal integer
    /// - `%o` — octal integer
    /// - `%c` — single byte (low 8 bits of integer argument)
    /// - `%f`, `%e`, `%E`, `%g`, `%G` — floating-point
    /// - `%s` — string (any value is coerced via `tostring`-like rules)
    /// - `%q` — quoted string with escapes (suitable for re-reading)
    /// - `%p` — pointer-style identity (for tables, functions, userdata)
    /// - `%%` — a literal `%`
    ///
    /// Standard width, precision, and the `-`, `+`, ` `, `#`, `0` flags
    /// are supported. Numeric arguments are coerced from numeric strings
    /// when the directive expects a number.
    ///
    /// # Parameters
    /// - `fmt` (string): the format string.
    /// - `...`: values to format, one per directive.
    ///
    /// # Returns
    /// (string): the formatted result.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.format("%d items", 42) == "42 items")
    /// assert(string.format("%05d", 7) == "00007")
    /// assert(string.format("%.2f", 3.14159) == "3.14")
    /// assert(string.format("%-10s|", "hi") == "hi        |")
    /// assert(string.format("%x %X", 255, 255) == "ff FF")
    /// assert(string.format("%q", 'he said "hi"') == [["he said \"hi\""]])
    /// assert(string.format("%s = %d", "answer", 42) == "answer = 42")
    /// ```
    #[function]
    fn format(fmt: Bytes, args: Variadic) -> Result<Bytes, VmError> {
        string_format_impl(&fmt, &args.0)
    }

    /// Packs values into a binary string according to a format directive.
    ///
    /// `fmt` is a sequence of single-character directives describing how
    /// each argument is encoded. Common directives include:
    ///
    /// - `b` / `B` — signed / unsigned byte
    /// - `h` / `H` — signed / unsigned 16-bit
    /// - `i[n]` / `I[n]` — signed / unsigned integer (default 4 bytes; `n`
    ///   selects 1..16)
    /// - `l` / `L` / `j` / `J` — long / Lua-integer-sized integer
    /// - `f`, `d` — 32-bit / 64-bit IEEE float
    /// - `s[n]` — length-prefixed string (length stored as `n`-byte int)
    /// - `z` — zero-terminated string
    /// - `c[n]` — fixed-size string of exactly `n` bytes
    /// - `<`, `>`, `=` — set little-endian, big-endian, native byte order
    /// - `!n` — set max alignment to `n`
    /// - `x` — one byte of padding
    ///
    /// Use `string.unpack` to read packed data, and `string.packsize` to
    /// compute the byte size of a fixed format ahead of time.
    ///
    /// # Parameters
    /// - `fmt` (string): the format directive sequence.
    /// - `...`: values to pack, one per data directive.
    ///
    /// # Returns
    /// (string): the packed bytes.
    ///
    /// # Examples
    /// ```lua
    /// local bytes = string.pack("<I4", 0x01020304)
    /// assert(bytes == "\x04\x03\x02\x01")
    ///
    /// local rec = string.pack("BBH", 1, 2, 0x0304)
    /// assert(#rec == 4)
    /// ```
    #[function]
    fn pack(fmt: Bytes, args: Variadic) -> Result<Bytes, VmError> {
        let data = crate::string_pack::string_pack(&fmt, &args.0)?;
        Ok(Bytes::from(data))
    }

    /// Unpacks values from a binary string according to a format directive.
    ///
    /// Reads `s` starting at byte position `pos` (1-based, default `1`)
    /// using the same format-directive language as `string.pack`. Returns
    /// the unpacked values followed by the position one past the last
    /// byte read — useful for chaining successive `unpack` calls over a
    /// composite buffer.
    ///
    /// # Parameters
    /// - `fmt` (string): the format directive sequence.
    /// - `s` (string): the source bytes.
    /// - `pos` (integer, optional): 1-based byte offset to start at (default `1`).
    ///
    /// # Returns
    /// (...): the unpacked values, followed by the next byte position.
    ///
    /// # Examples
    /// ```lua
    /// local n, next_pos = string.unpack("<I4", "\x04\x03\x02\x01")
    /// assert(n == 0x01020304 and next_pos == 5)
    ///
    /// local a, b, c, np = string.unpack("BBH", string.pack("BBH", 1, 2, 300))
    /// assert(a == 1 and b == 2 and c == 300 and np == 5)
    /// ```
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
        Ok(Variadic(vals.into()))
    }

    /// Returns the size in bytes that `fmt` would produce when packed.
    ///
    /// Only fixed-size formats are supported: variable-length directives
    /// (`s` length-prefixed strings and `z` zero-terminated strings) raise
    /// an error since their size depends on the value being packed.
    ///
    /// # Parameters
    /// - `fmt` (string): the format directive sequence.
    ///
    /// # Returns
    /// (integer): the number of bytes a `string.pack(fmt, ...)` would emit.
    ///
    /// # Examples
    /// ```lua
    /// assert(string.packsize("BBH") == 4)
    /// assert(string.packsize("<I8") == 8)
    /// assert(string.packsize("c10") == 10)
    /// ```
    #[function]
    fn packsize(fmt: Bytes) -> Result<i64, VmError> {
        crate::string_pack::string_packsize(&fmt)
    }

    /// Splits `s` on each occurrence of the literal byte sequence `sep`.
    ///
    /// Returns the pieces as an array-style table indexed from `1`. `sep`
    /// is matched as a plain byte sequence — Lua pattern metacharacters
    /// are not interpreted. Successive separators yield empty pieces, and
    /// a trailing separator yields a trailing empty piece.
    ///
    /// When `sep` is the empty string, each byte becomes its own piece;
    /// splitting an empty string by an empty separator yields an empty
    /// table. (This is a LuaU extension and matches LuaU's behaviour.)
    ///
    /// # Parameters
    /// - `s` (string): the string to split.
    /// - `sep` (string, optional): the separator (default `","`).
    ///
    /// # Returns
    /// (table): array of resulting pieces.
    ///
    /// # Examples
    /// ```lua
    /// local parts = string.split("a,b,c")
    /// assert(#parts == 3 and parts[1] == "a" and parts[2] == "b" and parts[3] == "c")
    ///
    /// local words = string.split("hello world foo", " ")
    /// assert(#words == 3 and words[2] == "world")
    ///
    /// -- Empty trailing piece preserved
    /// local trailing = string.split("a,", ",")
    /// assert(#trailing == 2 and trailing[2] == "")
    ///
    /// -- Empty separator: byte-per-element
    /// local bytes = string.split("abc", "")
    /// assert(#bytes == 3 and bytes[1] == "a" and bytes[2] == "b" and bytes[3] == "c")
    /// ```
    #[function]
    fn split(s: Bytes, sep: Option<Bytes>) -> Result<Table, VmError> {
        let sep = sep.unwrap_or_else(|| Bytes::from(","));
        let t = Table::new();
        let mut idx: i64 = 1;

        if sep.is_empty() {
            // Empty separator: emit one element per byte.  LuaU handles
            // this differently from a generic substring search — every
            // byte becomes its own piece, and "" split by "" yields an
            // empty table.  `memmem` would instead match at every offset
            // (including `s.len()`), so we short-circuit here.
            for i in 0..s.len() {
                t.raw_set(Value::Integer(idx), Value::string(&s[i..i + 1]))?;
                idx += 1;
            }
            return Ok(t);
        }

        // `memmem::find_iter` yields non-overlapping match positions
        // using SIMD / Two-Way under the hood.
        let sep_len = sep.len();
        let mut span_start = 0usize;
        for pos in memchr::memmem::find_iter(&s, &sep) {
            t.raw_set(Value::Integer(idx), Value::string(&s[span_start..pos]))?;
            idx += 1;
            span_start = pos + sep_len;
        }
        // Push the trailing span (always, even when empty).
        t.raw_set(Value::Integer(idx), Value::string(&s[span_start..s.len()]))?;
        Ok(t)
    }

    // ----------------------------------------------------------------
    // string.gmatch(s, pattern [, init])
    // ----------------------------------------------------------------

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
                return Some(Variadic(captures.into()));
            }
        }
    }

    /// Returns an iterator over all matches of `pattern` in `s`.
    ///
    /// The `pattern` argument uses the syntax described in the
    /// [pattern reference](index.md#pattern-syntax).
    ///
    /// Each call to the returned iterator advances to the next match and
    /// yields its captures: one value per explicit capture, or the whole
    /// match if the pattern has none. Designed for use in a `for` loop:
    ///
    /// ```lua
    /// for word in string.gmatch("the quick brown fox", "%a+") do
    ///     -- word is "the", "quick", "brown", "fox" in turn
    /// end
    /// ```
    ///
    /// `init` selects a 1-based byte offset to begin from. Pattern
    /// compilation errors are reported eagerly when `gmatch` is called,
    /// not lazily during iteration.
    ///
    /// # Parameters
    /// - `s` (string): the string to search.
    /// - `pattern` (string): the Lua pattern.
    /// - `init` (integer, optional): byte index to start at (default `1`).
    ///
    /// # Returns
    /// (function): an iterator returning each match's captures, or no
    /// values when exhausted.
    ///
    /// # Examples
    /// ```lua
    /// local words = {}
    /// for w in string.gmatch("the quick brown fox", "%a+") do
    ///     table.insert(words, w)
    /// end
    /// assert(#words == 4 and words[1] == "the" and words[4] == "fox")
    ///
    /// -- Multiple captures per match
    /// local pairs_found = {}
    /// for k, v in string.gmatch("a=1, b=2, c=3", "(%a)=(%d)") do
    ///     pairs_found[k] = v
    /// end
    /// assert(pairs_found.a == "1" and pairs_found.b == "2" and pairs_found.c == "3")
    /// ```
    #[function]
    fn gmatch(s: Bytes, pattern: Bytes, init: Option<i64>) -> Result<Value, VmError> {
        // Compile eagerly to catch pattern errors.
        let pat = compile_pattern(&pattern)?;

        // Convert 1-based Lua init to a 0-based byte offset, mirroring
        // Lua's `posrelatI` semantics: positive values pass through;
        // zero and very-negative values clip to the start; negative
        // values in range count from the end.  We deliberately do NOT
        // clamp positive values to the string length — an init past
        // `#s + 1` must yield zero matches (matching `string.find`'s
        // behaviour); the iterator's `offset > s.len()` guard handles
        // the resulting overshoot.
        let init = init.unwrap_or(1);
        let offset = if init > 0 {
            (init - 1) as usize
        } else if init == 0 || init < -(s.len() as i64) {
            0
        } else {
            (s.len() as i64 + init) as usize
        };

        let iter = GmatchIter {
            s,
            pat,
            offset,
            last_match_end: None,
        };

        Ok(Value::Function(Function::from_iter(
            "gmatch_iterator",
            iter,
        )))
    }
}

// =========================================================================
// string.format implementation (kept outside the module for readability)
// =========================================================================

/// `string.format(fmt, ...)`
///
/// A subset of C `sprintf`-style formatting.  Supports `%d`, `%i`, `%u`,
/// `%f`, `%e`, `%g`, `%x`, `%X`, `%o`, `%s`, `%c`, `%q`, and `%%`.
fn string_format_impl(fmt: &[u8], args: &[Value]) -> Result<Bytes, VmError> {
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
            return Err(
                runtime_error("invalid format string (ends with '%')".to_owned()).with_hint(
                    "every `%` must be followed by a conversion specifier; \
                 use `%%` to insert a literal `%`",
                ),
            );
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
            )
            .with_hint(
                "the `%` was followed only by flags, width, or precision; \
                 add a conversion specifier (one of `d`, `i`, `u`, `o`, \
                 `x`, `X`, `c`, `e`, `E`, `f`, `g`, `G`, `s`, `q`, `%`)",
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
            b'p' => {
                let ptr = arg.to_pointer();
                let formatted = format!("{ptr:p}");
                result.extend_from_slice(formatted.as_bytes());
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
                ))
                .or_suggest(
                    (conv as char).to_string(),
                    "specifier",
                    &[
                        b"d", b"i", b"u", b"o", b"x", b"X", b"c", b"e", b"E", b"f", b"g", b"G",
                        b"s", b"q", b"%",
                    ],
                    "valid specifiers are `d`, `i`, `u`, `o`, `x`, \
                     `X`, `c`, `e`, `E`, `f`, `g`, `G`, `s`, `q`, and \
                     `%` for a literal `%`",
                ));
            }
        }
    }

    Ok(Bytes::from(result))
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
    .with_hint(
        "floor, round, or truncate the value first (e.g. via \
         `math.floor`, `math.tointeger`, or `//1`)",
    )
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
    s.parse::<f64>()
        .ok()
        .or_else(|| crate::Number::parse_hex_float(s))
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
    if f.is_nan() {
        return "nan".to_string();
    }
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
// Registration
// =========================================================================

/// Build the string library table, register it as the `string` global, and
/// install a string metatable so method-call syntax works on string values.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = string_mod::build_module_table(env)?;

    // Set the string module as a global.
    env.set_global("string", Value::Table(table.clone()));
    env.register_module_type("string", string_mod::module_type());

    // Build a metatable whose __index points to the string table,
    // then install it as the shared string metatable.
    let mt = Table::new();
    mt.raw_set(Value::string("__index"), Value::Table(table))?;
    env.set_string_metatable(mt);

    Ok(())
}
