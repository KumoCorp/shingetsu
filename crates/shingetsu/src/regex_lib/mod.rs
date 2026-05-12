//! Implementation of the `regex` standard library module.
//!
//! Two engines are exposed side-by-side:
//!
//! * `regex.compile` builds a [`fancy_regex::Regex`] (UTF-8 input,
//!   supports lookaround / backreferences, subroutines).  Haystacks
//!   that are not valid UTF-8 are rejected at the call boundary.
//!   See [`fancy`] for the userdata implementation.
//! * `regex.compile_bytes` builds a [`regex::bytes::Regex`] (byte-
//!   oriented input, linear-time matching, no backreferences or
//!   lookaround).  See [`bytes`] for the userdata implementation.
//!
//! Both expose the same method surface.  Captures are returned as a
//! userdata with index `0` = the whole match and indices `1..n` =
//! explicit groups, matching standard regex APIs.

use std::sync::Arc;

use bstr::ByteSlice as _;

use crate::{valuevec, CallContext, Ud, Value, VmError};
use shingetsu::Bytes;

mod bytes;
mod fancy;

pub use bytes::{LuaBytesCaptures, LuaBytesRegex};
pub use fancy::{LuaCaptures, LuaRegex};

// =========================================================================
// Helpers shared by both engines
// =========================================================================

/// Build a Lua-side runtime error with a string message.
pub(super) fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::string(msg),
    }
}

/// Convert a `Bytes` haystack to `&str`, raising a structured
/// [`VmError::BadArgument`] when the bytes are not valid UTF-8.
///
/// `visible_position` is the 1-based position counting only the
/// explicit Lua arguments at the call site (e.g. `1` for the first
/// explicit argument on a colon-call).  Internally this is shifted
/// by `+1` to account for the implicit `self` receiver, matching
/// Lua's convention that `self` is argument #1.
pub(super) fn require_utf8<'a>(
    bytes: &'a Bytes,
    function: &str,
    visible_position: usize,
) -> Result<&'a str, VmError> {
    bytes.to_str().map_err(|e| VmError::BadArgument {
        position: visible_position + 1,
        function: function.to_owned(),
        expected: "valid UTF-8 string".to_owned(),
        got: format!("invalid UTF-8 at byte {}", e.valid_up_to() + 1),
    })
}

/// Convert a 1-based Lua `init` argument to a 0-based byte offset,
/// clamped into `[0, len]`.  Negative values count back from the
/// end, matching `string.find`.
pub(super) fn init_to_offset(init: Option<i64>, len: usize) -> usize {
    let i = init.unwrap_or(1);
    if i > 0 {
        ((i as usize).saturating_sub(1)).min(len)
    } else if i == 0 || i < -(len as i64) {
        0
    } else {
        (len as i64 + i) as usize
    }
}

#[derive(Clone, Copy)]
enum GroupRef<'a> {
    Index(usize),
    Name(&'a [u8]),
}

/// Expand a `$N` / `${name}` / `$$` template against a lookup
/// function.  `$$` emits a literal `$`.  `$N` and `${N}` look up
/// group `N` (the longest valid numeric prefix is consumed).
/// `$name` and `${name}` look up a named group; the unbraced form
/// consumes ASCII letters, digits, and underscores.  Unknown names
/// or missing groups expand to the empty string, matching the regex
/// crate's `Replacer` behaviour.
fn expand_template<F>(template: &[u8], lookup: F) -> Vec<u8>
where
    F: Fn(GroupRef<'_>) -> Option<Bytes>,
{
    let mut out = Vec::with_capacity(template.len());
    let mut i = 0;
    while i < template.len() {
        if template[i] != b'$' {
            out.push(template[i]);
            i += 1;
            continue;
        }
        if i + 1 >= template.len() {
            out.push(b'$');
            i += 1;
            continue;
        }
        if template[i + 1] == b'$' {
            out.push(b'$');
            i += 2;
            continue;
        }
        if template[i + 1] == b'{' {
            if let Some(close) = template[i + 2..].iter().position(|&b| b == b'}') {
                let name = &template[i + 2..i + 2 + close];
                if let Some(val) = lookup_name_or_index(name, &lookup) {
                    out.extend_from_slice(&val);
                }
                i += 2 + close + 1;
                continue;
            }
            // Unterminated `${...` — emit literally and keep going.
            out.push(b'$');
            i += 1;
            continue;
        }
        // Bare `$N` or `$name`: consume the longest run of name-chars.
        let start = i + 1;
        let mut end = start;
        while end < template.len() && is_name_char(template[end]) {
            end += 1;
        }
        if start == end {
            out.push(b'$');
            i += 1;
            continue;
        }
        let name = &template[start..end];
        if let Some(val) = lookup_name_or_index(name, &lookup) {
            out.extend_from_slice(&val);
        }
        i = end;
    }
    out
}

fn lookup_name_or_index<F>(name: &[u8], lookup: &F) -> Option<Bytes>
where
    F: Fn(GroupRef<'_>) -> Option<Bytes>,
{
    if !name.is_empty() && name.iter().all(|b| b.is_ascii_digit()) {
        if let Ok(s) = std::str::from_utf8(name) {
            if let Ok(idx) = s.parse::<usize>() {
                return lookup(GroupRef::Index(idx));
            }
        }
    }
    lookup(GroupRef::Name(name))
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Pull a `Bytes` replacement out of a `Value` produced by a
/// user-supplied callback or table lookup.  Nil and `false` are
/// treated as "keep the original match".  Other non-stringish
/// values raise a [`VmError::BadArgument`] attributed to the
/// replacement argument (position 3 on a colon-call: receiver,
/// haystack, repl).
fn replacement_from_value(v: Value, function: &str, original: &[u8]) -> Result<Bytes, VmError> {
    match v {
        Value::Nil | Value::Boolean(false) => Ok(Bytes::from(original)),
        Value::String(s) => Ok(s),
        Value::Integer(n) => Ok(Bytes::from(n.to_string())),
        Value::Float(f) => Ok(Bytes::from(f.to_string())),
        other => Err(VmError::BadArgument {
            position: 3,
            function: function.to_owned(),
            expected: "string, number, false, or nil".to_owned(),
            got: other.type_name().to_owned(),
        }),
    }
}

// =========================================================================
// CapturesData: shared internal representation for both engines
// =========================================================================

/// Internal storage shared by both `Captures` and `BytesCaptures`
/// userdata.  Group 0 is the whole match; groups 1..n are the
/// explicit capture groups.  Names are aligned with the group
/// indices (entry `i` is the name of group `i`, or `None` for
/// unnamed groups; entry `0` is always `None`).
pub(super) struct CapturesData {
    haystack: Bytes,
    groups: Vec<Option<(usize, usize)>>,
    names: Arc<[Option<Bytes>]>,
}

impl CapturesData {
    pub(super) fn get_span(&self, i: usize) -> Option<(usize, usize)> {
        self.groups.get(i).copied().flatten()
    }

    pub(super) fn get_bytes(&self, i: usize) -> Option<Bytes> {
        let (s, e) = self.get_span(i)?;
        Some(Bytes::from(&self.haystack[s..e]))
    }

    pub(super) fn index_of_name(&self, name: &[u8]) -> Option<usize> {
        self.names
            .iter()
            .position(|n| n.as_deref().map(|b| b == name).unwrap_or(false))
    }

    fn lookup(&self, r: GroupRef<'_>) -> Option<Bytes> {
        match r {
            GroupRef::Index(i) => self.get_bytes(i),
            GroupRef::Name(n) => self.index_of_name(n).and_then(|i| self.get_bytes(i)),
        }
    }

    pub(super) fn expand(&self, template: &[u8]) -> Bytes {
        Bytes::from(expand_template(template, |r| self.lookup(r)))
    }

    fn clone_data(&self) -> CapturesData {
        CapturesData {
            haystack: self.haystack.clone(),
            groups: self.groups.clone(),
            names: Arc::clone(&self.names),
        }
    }

    pub(super) fn tostring(&self, type_name: &str) -> String {
        let mut s = format!("{type_name}(");
        for (i, g) in self.groups.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            match g {
                Some((start, end)) => {
                    let slice = &self.haystack[*start..*end];
                    let _ = std::fmt::Write::write_fmt(
                        &mut s,
                        format_args!("{i}={:?}", bstr::BStr::new(slice)),
                    );
                }
                None => {
                    let _ = std::fmt::Write::write_fmt(&mut s, format_args!("{i}=nil"));
                }
            }
        }
        s.push(')');
        s
    }
}

/// Bridge that lets [`apply_replacement`] construct the right
/// userdata kind for each engine's `Captures`.
pub(super) trait WrapCaptures {
    fn wrap(data: CapturesData) -> Value;
}

/// Resolve a `repl` value (string template, function, or table)
/// into a literal `Bytes` replacement for one match.  Used by both
/// engines' `replace` implementations.
pub(super) async fn apply_replacement<C: WrapCaptures>(
    ctx: &CallContext,
    data: &CapturesData,
    function: &str,
    repl: &Value,
) -> Result<Bytes, VmError> {
    let whole = data.get_bytes(0).unwrap_or_default();
    match repl {
        Value::String(template) => Ok(data.expand(template.as_ref())),
        Value::Function(f) => {
            let captures_val = C::wrap(data.clone_data());
            let rets = ctx
                .call_function(f.clone(), valuevec![captures_val])
                .await
                .map_err(|re| re.error)?;
            let v = rets.into_iter().next().unwrap_or(Value::Nil);
            replacement_from_value(v, function, &whole)
        }
        Value::Table(tab) => {
            let key = Value::String(whole.clone());
            let v = tab.get(&key, ctx).await?;
            replacement_from_value(v, function, &whole)
        }
        other => Err(VmError::BadArgument {
            position: 3,
            function: function.to_owned(),
            expected: "string, function, or table".to_owned(),
            got: other.type_name().to_owned(),
        }),
    }
}

// =========================================================================
// Options structs (shared with the submodule builders)
// =========================================================================

/// Builder options for `regex.compile` (fancy-regex backend).
///
/// All flags default to off except `unicode`, which mirrors
/// fancy-regex's default.  `backtrack_limit`, `delegate_size_limit`,
/// and `delegate_dfa_size_limit` are passed through unchanged when
/// provided; absent values keep fancy-regex's built-in defaults.
#[derive(Clone, Debug, crate::LuaTable)]
pub(super) struct RegexOpts {
    /// Match without regard to ASCII letter case.  `(?i)` inline.
    #[lua(default = false)]
    pub(super) case_insensitive: bool,
    /// Treat `^` and `$` as matching at every line boundary instead
    /// of only at the start and end of the haystack.  `(?m)` inline.
    #[lua(default = false)]
    pub(super) multi_line: bool,
    /// Let `.` match newline (`\n`) characters.  `(?s)` inline.
    #[lua(default = false)]
    pub(super) dot_matches_new_line: bool,
    /// Ignore unescaped whitespace and `# ...` line comments in the
    /// pattern, for readability of complex expressions.  `(?x)`
    /// inline.
    #[lua(default = false)]
    pub(super) ignore_whitespace: bool,
    /// Treat character classes like `\w`, `\b`, `.`, and `[a-z]` as
    /// Unicode-aware.  Default `true`; set to `false` for byte-class
    /// semantics.
    #[lua(default = true)]
    pub(super) unicode: bool,
    /// Treat `\r\n` as an atomic line terminator so `^`, `$`, and
    /// `.` honour CRLF line breaks.
    #[lua(default = false)]
    pub(super) crlf: bool,
    /// Prefer Oniguruma parsing rules where they differ from the
    /// `regex` crate's syntax.  Defaults off.
    #[lua(default = false)]
    pub(super) oniguruma: bool,
    /// Synonym for `ignore_whitespace`; provided for parity with
    /// callers that spell it "verbose" (Python style).
    #[lua(default = false)]
    pub(super) verbose: bool,
    /// Reject matches that consume zero characters.  Useful when
    /// driving the regex from a `find_iter`-style loop where empty
    /// matches would otherwise stall progress.
    #[lua(default = false)]
    pub(super) find_not_empty: bool,
    /// Maximum number of backtracking steps before the match fails
    /// with a runtime error.  Defaults to fancy-regex's built-in
    /// limit (currently 1,000,000).  Lower this to bound worst-case
    /// time on adversarial input.
    pub(super) backtrack_limit: Option<i64>,
    /// Maximum compiled size, in bytes, of the underlying NFA
    /// delegate used by fancy-regex for non-fancy sub-patterns.
    /// Defaults to the regex crate's built-in cap.
    pub(super) delegate_size_limit: Option<i64>,
    /// Maximum compiled size, in bytes, of the delegate's lazy DFA
    /// cache.  Defaults to the regex crate's built-in cap.
    pub(super) delegate_dfa_size_limit: Option<i64>,
}

impl Default for RegexOpts {
    fn default() -> Self {
        Self {
            case_insensitive: false,
            multi_line: false,
            dot_matches_new_line: false,
            ignore_whitespace: false,
            unicode: true,
            crlf: false,
            oniguruma: false,
            verbose: false,
            find_not_empty: false,
            backtrack_limit: None,
            delegate_size_limit: None,
            delegate_dfa_size_limit: None,
        }
    }
}

/// Builder options for `regex.compile_bytes` (regex crate's bytes
/// backend).  All boolean flags default to `false`.  Size and nest
/// limits default to the `regex` crate's built-in values when
/// absent.
#[derive(Clone, Debug, crate::LuaTable)]
pub(super) struct BytesRegexOpts {
    /// Match without regard to ASCII letter case.  `(?i)` inline.
    #[lua(default = false)]
    pub(super) case_insensitive: bool,
    /// Treat `^` and `$` as matching at every line boundary instead
    /// of only at the start and end of the haystack.  `(?m)` inline.
    #[lua(default = false)]
    pub(super) multi_line: bool,
    /// Let `.` match newline (`\n`) characters.  `(?s)` inline.
    #[lua(default = false)]
    pub(super) dot_matches_new_line: bool,
    /// Ignore unescaped whitespace and `# ...` line comments in
    /// the pattern.  `(?x)` inline.
    #[lua(default = false)]
    pub(super) ignore_whitespace: bool,
    /// Treat character classes like `\w`, `\b`, `.`, and `[a-z]` as
    /// Unicode-aware.  Defaults to `false` for this engine because
    /// the byte backend's purpose is matching arbitrary bytes; opt
    /// in to codepoint-aware classes by setting `unicode = true`.
    /// Note that with `unicode = true` patterns containing raw
    /// non-UTF-8 byte literals will fail to compile.
    #[lua(default = false)]
    pub(super) unicode: bool,
    /// Treat `\r\n` as an atomic line terminator so `^`, `$`, and
    /// `.` honour CRLF line breaks.
    #[lua(default = false)]
    pub(super) crlf: bool,
    /// Allow `\NNN` to be interpreted as a literal octal byte value
    /// instead of a backreference.  Off by default; only useful when
    /// porting patterns from systems that use octal escapes.
    #[lua(default = false)]
    pub(super) octal: bool,
    /// Maximum compiled size, in bytes, of the compiled program.
    /// Defaults to the `regex` crate's built-in cap.
    pub(super) size_limit: Option<i64>,
    /// Maximum compiled size, in bytes, of the lazy DFA cache used
    /// during matching.  Defaults to the `regex` crate's built-in
    /// cap.
    pub(super) dfa_size_limit: Option<i64>,
    /// Maximum depth of nested groups allowed in the pattern.
    /// Defaults to the `regex` crate's built-in limit.
    pub(super) nest_limit: Option<i64>,
}

impl Default for BytesRegexOpts {
    fn default() -> Self {
        Self {
            case_insensitive: false,
            multi_line: false,
            dot_matches_new_line: false,
            ignore_whitespace: false,
            unicode: false,
            crlf: false,
            octal: false,
            size_limit: None,
            dfa_size_limit: None,
            nest_limit: None,
        }
    }
}

// =========================================================================
// FindResult and compile-error builders
// =========================================================================

/// Return type for `Regex:find` and `BytesRegex:find`.
#[derive(crate::IntoLuaMulti)]
pub(super) enum FindResult {
    Match(i64, i64, Bytes),
    NotFound,
}

/// Build a `BadArgument` for a fancy-regex compile failure.  Using
/// `BadArgument` (with `position=1`) lets the diagnostic renderer
/// point the caret at the pattern argument rather than at the
/// `regex.compile` call expression.
pub(super) fn compile_error_fancy(e: fancy_regex::Error) -> VmError {
    VmError::BadArgument {
        position: 1,
        function: "regex.compile".to_owned(),
        expected: "a valid regex pattern".to_owned(),
        got: e.to_string(),
    }
}

/// `BadArgument` variant for a regex::bytes compile failure.
pub(super) fn compile_error_bytes(e: regex::Error) -> VmError {
    VmError::BadArgument {
        position: 1,
        function: "regex.compile_bytes".to_owned(),
        expected: "a valid regex pattern".to_owned(),
        got: e.to_string(),
    }
}

// =========================================================================
// Lua module
// =========================================================================

/// Regular-expression matching with two interchangeable engines.
///
/// * `regex.compile` builds a regex that accepts UTF-8 input and
///   supports the full fancy syntax — backreferences (`\1`),
///   lookaround (`(?=...)`, `(?<=...)`), subroutines (`\g<name>`),
///   and conditionals.  Haystacks must be valid UTF-8.  Pattern
///   syntax is documented at
///   <https://docs.rs/fancy-regex/latest/fancy_regex/#syntax>.
/// * `regex.compile_bytes` builds a regex backed by the `regex`
///   crate's bytes API.  It accepts arbitrary byte haystacks and
///   guarantees linear-time matching, but does not support
///   backreferences or lookaround.  Pattern syntax is documented
///   at
///   <https://docs.rs/regex/latest/regex/bytes/index.html#syntax>.
///
/// Pick the byte engine for protocol parsing, mixed-encoding text,
/// or any workload where guaranteed linear time matters; pick the
/// fancy engine when the pattern needs lookaround or backrefs.
///
/// Returned `Captures` / `BytesCaptures` userdata use index `0` for
/// the whole match and `1..n` for the explicit groups, matching
/// standard regex APIs.  Byte offsets returned from `find` and
/// `Captures:start` / `Captures:end_` are 1-based, matching
/// `string.find`.
#[crate::module(name = "regex")]
mod regex_mod {
    use super::*;

    /// Compile a regex with the `fancy-regex` backend.  Returns a
    /// `Regex` userdata.  Raises on a malformed pattern.
    ///
    /// The pattern syntax is fancy-regex's superset of the standard
    /// `regex` crate's syntax — it adds backreferences,
    /// lookaround, subroutines, conditionals, and Oniguruma-style
    /// constructs.  The full grammar is at
    /// <https://docs.rs/fancy-regex/latest/fancy_regex/#syntax>.
    ///
    /// Haystacks passed to methods on the returned `Regex` must be
    /// valid UTF-8; non-UTF-8 input is rejected with a `BadArgument`
    /// error at the call boundary.
    ///
    /// # Parameters
    ///
    /// - `pattern` — the regex source.
    /// - `opts` — optional table of builder flags.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile("(?<year>\\d{4})-(?<month>\\d{2})")
    /// local caps = re:captures("2024-08")
    /// assert(caps:by_name("year") == "2024")
    /// assert(caps:by_name("month") == "08")
    /// ```
    ///
    /// Lookaround and backreferences:
    ///
    /// ```lua
    /// -- Match a word that is repeated immediately, using a backref.
    /// local re = regex.compile("(\\w+) \\1")
    /// assert(re:is_match("hello hello world"))
    /// assert(not re:is_match("hello world"))
    /// ```
    #[function]
    fn compile(pattern: Bytes, opts: Option<RegexOpts>) -> Result<Ud<LuaRegex>, VmError> {
        let pat = pattern.to_str().map_err(|e| VmError::BadArgument {
            position: 1,
            function: "regex.compile".to_owned(),
            expected: "valid UTF-8 pattern".to_owned(),
            got: format!("invalid UTF-8 at byte {}", e.valid_up_to() + 1),
        })?;
        let re = fancy::compile_fancy(pat, opts.unwrap_or_default())?;
        let names = LuaRegex::build_names(&re);
        Ok(Ud(Arc::new(LuaRegex { inner: re, names })))
    }

    /// Compile a regex with the `regex::bytes` backend.  Returns a
    /// `BytesRegex` userdata.  The pattern itself must be UTF-8 but
    /// the haystacks passed to its methods may be arbitrary bytes.
    ///
    /// Backreferences and lookaround are not supported by this
    /// backend.  Use `regex.compile` if you need them.  The full
    /// syntax is at
    /// <https://docs.rs/regex/latest/regex/bytes/index.html#syntax>.
    ///
    /// # Parameters
    ///
    /// - `pattern` — the regex source (UTF-8).
    /// - `opts` — optional table of builder flags.  See the
    ///   *Unicode mode* section below for the implications of
    ///   `unicode = true`.
    ///
    /// # Unicode mode
    ///
    /// `unicode` defaults to `false` for this engine, on the
    /// assumption that callers who reach for `compile_bytes` want
    /// byte-level matching.  In this mode `\w` means `[A-Za-z0-9_]`
    /// (ASCII), `.` matches any single byte (except newline), and
    /// byte literals like `\xff` work directly:
    ///
    /// ```lua
    /// local re = regex.compile_bytes("\\xff+")
    /// assert(re:is_match("\xff\xff\xff"))
    /// ```
    ///
    /// Pass `{ unicode = true }` (or use the inline flag `(?u)`) to
    /// get codepoint-aware classes that treat the haystack as UTF-8.
    /// In that mode patterns containing raw non-UTF-8 byte literals
    /// will fail to compile.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("[A-Z]+")
    /// assert(re:is_match("HELLO"))
    /// assert(not re:is_match("hello"))
    /// ```
    #[function]
    fn compile_bytes(
        pattern: Bytes,
        opts: Option<BytesRegexOpts>,
    ) -> Result<Ud<LuaBytesRegex>, VmError> {
        let pat = pattern.to_str().map_err(|e| VmError::BadArgument {
            position: 1,
            function: "regex.compile_bytes".to_owned(),
            expected: "valid UTF-8 pattern".to_owned(),
            got: format!("invalid UTF-8 at byte {}", e.valid_up_to() + 1),
        })?;
        let re = bytes::compile_bytes_pat(pat, opts.unwrap_or_default())?;
        let names = LuaBytesRegex::build_names(&re);
        Ok(Ud(Arc::new(LuaBytesRegex { inner: re, names })))
    }

    /// Escape regex metacharacters in `text` so that the result, used
    /// as a pattern, matches `text` literally.  The same escape set
    /// is correct for both engines.
    ///
    /// # Examples
    ///
    /// ```lua
    /// assert(regex.escape("1+1=2") == "1\\+1=2")
    /// ```
    #[function]
    fn escape(text: Bytes) -> Result<Bytes, VmError> {
        let s = text.to_str().map_err(|e| VmError::BadArgument {
            position: 1,
            function: "regex.escape".to_owned(),
            expected: "valid UTF-8 string".to_owned(),
            got: format!("invalid UTF-8 at byte {}", e.valid_up_to() + 1),
        })?;
        Ok(Bytes::from(fancy_regex::escape(s).into_owned()))
    }
}

// =========================================================================
// Registration
// =========================================================================

/// Register the `regex` module: install the global table, register
/// both userdata types and both capture types, and record the
/// module's type information for docgen.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    env.register_userdata_type(LuaRegex::userdata_type());
    env.register_userdata_type(LuaCaptures::userdata_type());
    env.register_userdata_type(LuaBytesRegex::userdata_type());
    env.register_userdata_type(LuaBytesCaptures::userdata_type());
    let table = regex_mod::build_module_table(env)?;
    env.set_global("regex", Value::Table(table));
    env.register_module_type("regex", regex_mod::module_type());
    Ok(())
}
