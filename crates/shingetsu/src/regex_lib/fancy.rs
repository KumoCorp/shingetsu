//! Fancy-regex backend: UTF-8 input, lookaround, backreferences,
//! subroutines.  Pairs with [`super::LuaCaptures`] for match
//! introspection.  Haystacks passed to these methods must be valid
//! UTF-8; non-UTF-8 input is rejected with a `BadArgument` error at
//! the call boundary.

use std::sync::Arc;

use crate::convert::Variadic;
use crate::{valuevec, CallContext, Function, Ud, Value, VmError};
use shingetsu::Bytes;

use super::{
    apply_replacement, init_to_offset, require_utf8, runtime_error, CapturesData, FindResult,
    RegexOpts,
};

/// Compile a fancy-regex pattern, applying options.
pub(super) fn compile_fancy(pattern: &str, opts: RegexOpts) -> Result<fancy_regex::Regex, VmError> {
    let mut b = fancy_regex::RegexBuilder::new(pattern);
    b.case_insensitive(opts.case_insensitive);
    b.multi_line(opts.multi_line);
    b.dot_matches_new_line(opts.dot_matches_new_line);
    b.ignore_whitespace(opts.ignore_whitespace);
    b.unicode_mode(opts.unicode);
    b.crlf(opts.crlf);
    b.oniguruma_mode(opts.oniguruma);
    b.verbose_mode(opts.verbose);
    b.find_not_empty(opts.find_not_empty);
    if let Some(n) = opts.backtrack_limit {
        b.backtrack_limit(n.max(0) as usize);
    }
    if let Some(n) = opts.delegate_size_limit {
        b.delegate_size_limit(n.max(0) as usize);
    }
    if let Some(n) = opts.delegate_dfa_size_limit {
        b.delegate_dfa_size_limit(n.max(0) as usize);
    }
    b.build().map_err(super::compile_error_fancy)
}

/// Compiled regex (fancy-regex backend).  Returned by
/// `regex.compile`.
pub struct LuaRegex {
    pub(super) inner: fancy_regex::Regex,
    pub(super) names: Arc<[Option<Bytes>]>,
}

impl LuaRegex {
    pub(super) fn build_names(re: &fancy_regex::Regex) -> Arc<[Option<Bytes>]> {
        re.capture_names()
            .map(|opt| opt.map(Bytes::from))
            .collect::<Vec<_>>()
            .into()
    }

    fn build_captures(&self, haystack: &Bytes, caps: fancy_regex::Captures<'_>) -> Ud<LuaCaptures> {
        let groups = (0..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect();
        Ud(Arc::new(LuaCaptures {
            data: CapturesData {
                haystack: haystack.clone(),
                groups,
                names: Arc::clone(&self.names),
            },
        }))
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "Regex", index_fallback = "nil")]
impl LuaRegex {
    /// The pattern source as supplied to `regex.compile`.
    #[lua_method]
    fn pattern(self: Arc<Self>) -> Bytes {
        Bytes::from(self.inner.as_str())
    }

    /// Returns `true` if the regex matches anywhere in `haystack`.
    #[lua_method]
    fn is_match(self: Arc<Self>, haystack: Bytes) -> Result<bool, VmError> {
        let s = require_utf8(&haystack, "Regex:is_match", 1)?;
        self.inner
            .is_match(s)
            .map_err(|e| runtime_error(e.to_string()))
    }

    /// Returns `(start, end, match_str)` for the first match at or
    /// after the 1-based byte offset `init` (default 1), or `nil`
    /// when there is no match.  `start` and `end` are 1-based byte
    /// offsets; `s..e` (inclusive) is the matched substring, mirroring
    /// `string.find`.
    #[lua_method]
    fn find(self: Arc<Self>, haystack: Bytes, init: Option<i64>) -> Result<FindResult, VmError> {
        let s = require_utf8(&haystack, "Regex:find", 1)?;
        let off = init_to_offset(init, s.len());
        match self
            .inner
            .find_from_pos(s, off)
            .map_err(|e| runtime_error(e.to_string()))?
        {
            Some(m) => Ok(FindResult::Match(
                (m.start() + 1) as i64,
                m.end() as i64,
                Bytes::from(m.as_str()),
            )),
            None => Ok(FindResult::NotFound),
        }
    }

    /// Returns a stateful iterator yielding `(start, end, match_str)`
    /// for every non-overlapping match in `haystack`.  Designed for
    /// use as the iterator in a generic `for` loop.
    #[lua_method]
    fn find_iter(self: Arc<Self>, haystack: Bytes) -> Result<Function, VmError> {
        let _ = require_utf8(&haystack, "Regex:find_iter", 1)?;
        let re = self.inner.clone();
        let mut offset = 0usize;
        let mut last_end: Option<usize> = None;
        let iter = std::iter::from_fn(move || -> Option<Result<(i64, i64, Bytes), VmError>> {
            let s = std::str::from_utf8(&haystack).ok()?;
            if offset > s.len() {
                return None;
            }
            let m = match re.find_from_pos(s, offset) {
                Ok(Some(m)) => m,
                Ok(None) => return None,
                Err(e) => return Some(Err(runtime_error(e.to_string()))),
            };
            if m.start() == m.end() && Some(m.end()) == last_end {
                offset += 1;
                return Some(Ok((
                    (m.start() + 1) as i64,
                    m.end() as i64,
                    Bytes::from(m.as_str()),
                )));
            }
            last_end = Some(m.end());
            offset = if m.start() == m.end() {
                m.end() + 1
            } else {
                m.end()
            };
            Some(Ok((
                (m.start() + 1) as i64,
                m.end() as i64,
                Bytes::from(m.as_str()),
            )))
        });
        Ok(Function::from_iter("Regex:find_iter", iter))
    }

    /// Returns a `Captures` userdata for the first match at or after
    /// the 1-based byte offset `init` (default 1), or `nil` when
    /// there is no match.
    #[lua_method]
    fn captures(
        self: Arc<Self>,
        haystack: Bytes,
        init: Option<i64>,
    ) -> Result<Option<Ud<LuaCaptures>>, VmError> {
        let s = require_utf8(&haystack, "Regex:captures", 1)?;
        let off = init_to_offset(init, s.len());
        let caps = self
            .inner
            .captures_from_pos(s, off)
            .map_err(|e| runtime_error(e.to_string()))?;
        Ok(caps.map(|c| self.build_captures(&haystack, c)))
    }

    /// Returns a stateful iterator yielding a `Captures` userdata
    /// per non-overlapping match in `haystack`.
    #[lua_method]
    fn captures_iter(self: Arc<Self>, haystack: Bytes) -> Result<Function, VmError> {
        let _ = require_utf8(&haystack, "Regex:captures_iter", 1)?;
        let this = self.clone();
        let mut offset = 0usize;
        let mut last_end: Option<usize> = None;
        let iter = std::iter::from_fn(move || -> Option<Result<Ud<LuaCaptures>, VmError>> {
            let s = std::str::from_utf8(&haystack).ok()?;
            if offset > s.len() {
                return None;
            }
            let caps = match this.inner.captures_from_pos(s, offset) {
                Ok(Some(c)) => c,
                Ok(None) => return None,
                Err(e) => return Some(Err(runtime_error(e.to_string()))),
            };
            let m = caps
                .get(0)
                .expect("group 0 always set on a successful match");
            if m.start() == m.end() && Some(m.end()) == last_end {
                offset += 1;
                return Some(Ok(this.build_captures(&haystack, caps)));
            }
            last_end = Some(m.end());
            offset = if m.start() == m.end() {
                m.end() + 1
            } else {
                m.end()
            };
            Some(Ok(this.build_captures(&haystack, caps)))
        });
        Ok(Function::from_iter("Regex:captures_iter", iter))
    }

    /// Replaces up to `n` non-overlapping matches in `haystack`.
    /// `n` defaults to `1`; use `replace_all` for unlimited
    /// replacements.  `repl` may be a string (with `$N` / `${name}`
    /// substitution), a function called with the `Captures`
    /// userdata, or a table looked up by the whole match.
    #[lua_method]
    async fn replace(
        self: Arc<Self>,
        ctx: CallContext,
        haystack: Bytes,
        repl: Value,
        n: Option<i64>,
    ) -> Result<Bytes, VmError> {
        let limit = n.map(|v| v.max(0) as usize).unwrap_or(1);
        fancy_replace(self, &ctx, haystack, repl, limit).await
    }

    /// Replaces every non-overlapping match in `haystack`.  See
    /// `replace` for the replacement-value semantics.
    #[lua_method]
    async fn replace_all(
        self: Arc<Self>,
        ctx: CallContext,
        haystack: Bytes,
        repl: Value,
    ) -> Result<Bytes, VmError> {
        fancy_replace(self, &ctx, haystack, repl, usize::MAX).await
    }

    /// Splits `haystack` on matches of the regex.  `limit` (if > 0)
    /// caps the number of splits; the last element of the returned
    /// array holds the unsplit remainder.  Default is unlimited.
    #[lua_method]
    fn split(self: Arc<Self>, haystack: Bytes, limit: Option<i64>) -> Result<Vec<Bytes>, VmError> {
        let s = require_utf8(&haystack, "Regex:split", 1)?;
        let limit = limit.map(|v| v.max(0) as usize).unwrap_or(0);
        let mut out: Vec<Bytes> = Vec::new();
        let mut last = 0usize;
        let mut count = 0usize;
        let mut offset = 0usize;
        loop {
            if limit != 0 && count + 1 >= limit {
                break;
            }
            let m = match self
                .inner
                .find_from_pos(s, offset)
                .map_err(|e| runtime_error(e.to_string()))?
            {
                Some(m) => m,
                None => break,
            };
            out.push(Bytes::from(&s.as_bytes()[last..m.start()]));
            last = m.end();
            offset = if m.start() == m.end() {
                m.end() + 1
            } else {
                m.end()
            };
            count += 1;
            if offset > s.len() {
                break;
            }
        }
        out.push(Bytes::from(&s.as_bytes()[last..]));
        Ok(out)
    }

    /// Returns the names of the explicit capture groups.  Entry `i`
    /// (1-based) corresponds to group `i`, with `nil` for unnamed
    /// groups.  Group 0 (the whole match) is excluded from the
    /// returned array.
    #[lua_method]
    fn capture_names(self: Arc<Self>) -> Vec<Value> {
        self.names
            .iter()
            .skip(1)
            .map(|n| match n {
                Some(b) => Value::String(b.clone()),
                None => Value::Nil,
            })
            .collect()
    }

    /// Number of explicit capture groups (i.e. not counting the
    /// implicit whole-match group at index 0).
    #[lua_method]
    fn capture_count(self: Arc<Self>) -> i64 {
        (self.inner.captures_len() as i64 - 1).max(0)
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = format!("Regex({:?})", self.inner.as_str());
        Variadic(valuevec![Value::string(s)])
    }
}

/// Captures from a fancy-regex match.
pub struct LuaCaptures {
    pub(super) data: CapturesData,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "Captures", index_fallback = "nil")]
impl LuaCaptures {
    /// The matched substring for group `i`.  `i = 0` returns the
    /// whole match; `i = 1..n` returns the explicit groups.
    /// Returns `nil` for an unmatched optional group or an
    /// out-of-range index.
    #[lua_method]
    fn get(self: Arc<Self>, i: i64) -> Option<Bytes> {
        if i < 0 {
            return None;
        }
        self.data.get_bytes(i as usize)
    }

    /// 1-based byte offset of the start of group `i`'s match, or
    /// `nil` when the group did not participate in the match.
    #[lua_method]
    fn start(self: Arc<Self>, i: i64) -> Option<i64> {
        if i < 0 {
            return None;
        }
        let (s, _) = self.data.get_span(i as usize)?;
        Some((s + 1) as i64)
    }

    /// 1-based byte offset of the last byte of group `i`'s match,
    /// inclusive (matching `string.find`).  `nil` when the group
    /// did not participate.
    #[lua_method]
    fn end_(self: Arc<Self>, i: i64) -> Option<i64> {
        if i < 0 {
            return None;
        }
        let (_, e) = self.data.get_span(i as usize)?;
        Some(e as i64)
    }

    /// The name of group `i`, or `nil` if the group is unnamed or
    /// the index is out of range.
    #[lua_method]
    fn name(self: Arc<Self>, i: i64) -> Option<Bytes> {
        if i < 0 {
            return None;
        }
        self.data.names.get(i as usize).and_then(|n| n.clone())
    }

    /// Looks up a named group's match.  Returns `nil` if the name
    /// is unknown or the group did not participate.
    #[lua_method]
    fn by_name(self: Arc<Self>, name: Bytes) -> Option<Bytes> {
        let i = self.data.index_of_name(&name)?;
        self.data.get_bytes(i)
    }

    /// Number of groups (the implicit whole match plus all
    /// explicit groups).  Always at least 1 on a match.
    #[lua_method]
    fn len(self: Arc<Self>) -> i64 {
        self.data.groups.len() as i64
    }

    /// Expand a `$N` / `${name}` / `$$` template against this
    /// match's captures.  Missing groups expand to the empty
    /// string.
    #[lua_method]
    fn expand(self: Arc<Self>, template: Bytes) -> Bytes {
        self.data.expand(&template)
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string(self.data.tostring("Captures"))])
    }
}

async fn fancy_replace(
    re: Arc<LuaRegex>,
    ctx: &CallContext,
    haystack: Bytes,
    repl: Value,
    limit: usize,
) -> Result<Bytes, VmError> {
    let s = require_utf8(&haystack, "Regex:replace", 1)?;
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut last = 0usize;
    let mut offset = 0usize;
    let mut count = 0usize;
    while count < limit {
        let caps = match re
            .inner
            .captures_from_pos(s, offset)
            .map_err(|e| runtime_error(e.to_string()))?
        {
            Some(c) => c,
            None => break,
        };
        let m0 = caps
            .get(0)
            .expect("group 0 always set on a successful match");
        let (mstart, mend) = (m0.start(), m0.end());
        out.extend_from_slice(&bytes[last..mstart]);
        let groups = (0..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect::<Vec<_>>();
        let data = CapturesData {
            haystack: haystack.clone(),
            groups,
            names: Arc::clone(&re.names),
        };
        let replacement =
            apply_replacement::<LuaCaptures>(ctx, &data, "Regex:replace", &repl).await?;
        out.extend_from_slice(&replacement);
        last = mend;
        offset = if mstart == mend { mend + 1 } else { mend };
        count += 1;
        if offset > bytes.len() {
            break;
        }
    }
    out.extend_from_slice(&bytes[last..]);
    Ok(Bytes::from(out))
}

impl super::WrapCaptures for LuaCaptures {
    fn wrap(data: CapturesData) -> Value {
        Value::from(Ud(Arc::new(Self { data })))
    }
}
