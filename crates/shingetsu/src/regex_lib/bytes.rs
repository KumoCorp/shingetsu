//! Bytes-regex backend: arbitrary `&[u8]` input, linear-time
//! matching, no backreferences or lookaround.  Pairs with
//! [`super::LuaBytesCaptures`] for match introspection.

use std::sync::Arc;

use crate::convert::Variadic;
use crate::{valuevec, CallContext, Function, Ud, Value, VmError};
use shingetsu::Bytes;

use super::{apply_replacement, init_to_offset, BytesRegexOpts, CapturesData, FindResult};

/// Compile a regex::bytes pattern, applying options.
pub(super) fn compile_bytes_pat(
    pattern: &str,
    opts: BytesRegexOpts,
) -> Result<regex::bytes::Regex, VmError> {
    let mut b = regex::bytes::RegexBuilder::new(pattern);
    b.case_insensitive(opts.case_insensitive);
    b.multi_line(opts.multi_line);
    b.dot_matches_new_line(opts.dot_matches_new_line);
    b.ignore_whitespace(opts.ignore_whitespace);
    b.unicode(opts.unicode);
    b.crlf(opts.crlf);
    b.octal(opts.octal);
    if let Some(n) = opts.size_limit {
        b.size_limit(n.max(0) as usize);
    }
    if let Some(n) = opts.dfa_size_limit {
        b.dfa_size_limit(n.max(0) as usize);
    }
    if let Some(n) = opts.nest_limit {
        b.nest_limit(n.max(0) as u32);
    }
    b.build().map_err(super::compile_error_bytes)
}

/// Compiled regex (regex::bytes backend).  Returned by
/// `regex.compile_bytes`.  Accepts arbitrary byte haystacks.
pub struct LuaBytesRegex {
    pub(super) inner: regex::bytes::Regex,
    pub(super) names: Arc<[Option<Bytes>]>,
}

impl LuaBytesRegex {
    pub(super) fn build_names(re: &regex::bytes::Regex) -> Arc<[Option<Bytes>]> {
        re.capture_names()
            .map(|opt| opt.map(Bytes::from))
            .collect::<Vec<_>>()
            .into()
    }

    fn build_captures(
        &self,
        haystack: &Bytes,
        caps: regex::bytes::Captures<'_>,
    ) -> Ud<LuaBytesCaptures> {
        let groups = (0..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect();
        Ud(Arc::new(LuaBytesCaptures {
            data: CapturesData {
                haystack: haystack.clone(),
                groups,
                names: Arc::clone(&self.names),
            },
        }))
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "BytesRegex", index_fallback = "nil")]
impl LuaBytesRegex {
    /// The pattern source as supplied to `regex.compile_bytes`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("\\d+")
    /// assert(re:pattern() == "\\d+")
    /// ```
    #[lua_method]
    fn pattern(self: Arc<Self>) -> Bytes {
        Bytes::from(self.inner.as_str())
    }

    /// Returns `true` if the regex matches anywhere in `haystack`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("\\xff")
    /// assert(re:is_match("\xff binary data"))
    /// assert(not re:is_match("plain ascii"))
    /// ```
    #[lua_method]
    fn is_match(self: Arc<Self>, haystack: Bytes) -> bool {
        self.inner.is_match(&haystack)
    }

    /// Returns `(start, end, match_str)` for the first match at or
    /// after the 1-based byte offset `init` (default 1), or `nil`
    /// when there is no match.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("\\d+")
    /// local s, e, m = re:find("port=8080")
    /// assert(s == 6 and e == 9 and m == "8080")
    /// ```
    #[lua_method]
    fn find(self: Arc<Self>, haystack: Bytes, init: Option<i64>) -> FindResult {
        let off = init_to_offset(init, haystack.len());
        match self.inner.find_at(&haystack, off) {
            Some(m) => FindResult::Match(
                (m.start() + 1) as i64,
                m.end() as i64,
                Bytes::from(m.as_bytes()),
            ),
            None => FindResult::NotFound,
        }
    }

    /// Returns a stateful iterator yielding `(start, end, match_str)`
    /// for every non-overlapping match.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("\\d+")
    /// local nums = {}
    /// for _, _, m in re:find_iter("a=1 b=22 c=333") do
    ///     table.insert(nums, m)
    /// end
    /// assert(nums[1] == "1" and nums[2] == "22" and nums[3] == "333")
    /// ```
    #[lua_method]
    fn find_iter(self: Arc<Self>, haystack: Bytes) -> Function {
        let re = self.inner.clone();
        let mut offset = 0usize;
        let mut last_end: Option<usize> = None;
        let iter = std::iter::from_fn(move || -> Option<(i64, i64, Bytes)> {
            if offset > haystack.len() {
                return None;
            }
            let m = re.find_at(&haystack, offset)?;
            if m.start() == m.end() && Some(m.end()) == last_end {
                offset += 1;
                return Some((
                    (m.start() + 1) as i64,
                    m.end() as i64,
                    Bytes::from(m.as_bytes()),
                ));
            }
            last_end = Some(m.end());
            offset = if m.start() == m.end() {
                m.end() + 1
            } else {
                m.end()
            };
            Some((
                (m.start() + 1) as i64,
                m.end() as i64,
                Bytes::from(m.as_bytes()),
            ))
        });
        Function::from_iter("BytesRegex:find_iter", iter)
    }

    /// Returns a `BytesCaptures` userdata for the first match, or
    /// `nil` when there is no match.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(\\w+)=(\\d+)")
    /// local c = re:captures("port=8080")
    /// assert(c:get(0) == "port=8080")
    /// assert(c:get(1) == "port" and c:get(2) == "8080")
    /// ```
    #[lua_method]
    fn captures(
        self: Arc<Self>,
        haystack: Bytes,
        init: Option<i64>,
    ) -> Option<Ud<LuaBytesCaptures>> {
        let off = init_to_offset(init, haystack.len());
        let caps = self.inner.captures_at(&haystack, off)?;
        Some(self.build_captures(&haystack, caps))
    }

    /// Returns a stateful iterator yielding `BytesCaptures` userdata
    /// per non-overlapping match.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(\\w+)=(\\d+)")
    /// local pairs_found = {}
    /// for c in re:captures_iter("a=1 b=22") do
    ///     pairs_found[c:get(1)] = c:get(2)
    /// end
    /// assert(pairs_found.a == "1" and pairs_found.b == "22")
    /// ```
    #[lua_method]
    fn captures_iter(self: Arc<Self>, haystack: Bytes) -> Function {
        let this = self.clone();
        let mut offset = 0usize;
        let mut last_end: Option<usize> = None;
        let iter = std::iter::from_fn(move || -> Option<Ud<LuaBytesCaptures>> {
            if offset > haystack.len() {
                return None;
            }
            let caps = this.inner.captures_at(&haystack, offset)?;
            let m = caps
                .get(0)
                .expect("group 0 always set on a successful match");
            if m.start() == m.end() && Some(m.end()) == last_end {
                offset += 1;
                return Some(this.build_captures(&haystack, caps));
            }
            last_end = Some(m.end());
            offset = if m.start() == m.end() {
                m.end() + 1
            } else {
                m.end()
            };
            Some(this.build_captures(&haystack, caps))
        });
        Function::from_iter("BytesRegex:captures_iter", iter)
    }

    /// Replaces up to `n` non-overlapping matches.  `n` defaults to
    /// `1`.  See `Regex:replace` for the replacement semantics.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("foo")
    /// assert(re:replace("foo foo foo", "BAR") == "BAR foo foo")
    /// assert(re:replace("foo foo foo", "BAR", 2) == "BAR BAR foo")
    /// ```
    #[lua_method]
    async fn replace(
        self: Arc<Self>,
        ctx: CallContext,
        haystack: Bytes,
        repl: Value,
        n: Option<i64>,
    ) -> Result<Bytes, VmError> {
        let limit = n.map(|v| v.max(0) as usize).unwrap_or(1);
        bytes_replace(self, &ctx, haystack, repl, limit).await
    }

    /// Replaces every non-overlapping match.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- String template with $N substitution.
    /// local re = regex.compile_bytes("(\\w+)\\s*=\\s*(\\d+)")
    /// assert(re:replace_all("a = 1, b=22", "$1:$2") == "a:1, b:22")
    /// ```
    ///
    /// ```lua
    /// -- Function callback receives a BytesCaptures userdata.
    /// local re = regex.compile_bytes("\\d+")
    /// local out = re:replace_all("a=1 b=22", function(c)
    ///     return tostring(tonumber(c:get(0)) * 10)
    /// end)
    /// assert(out == "a=10 b=220")
    /// ```
    #[lua_method]
    async fn replace_all(
        self: Arc<Self>,
        ctx: CallContext,
        haystack: Bytes,
        repl: Value,
    ) -> Result<Bytes, VmError> {
        bytes_replace(self, &ctx, haystack, repl, usize::MAX).await
    }

    /// Splits `haystack` on matches of the regex.  `limit` (if > 0)
    /// caps the number of splits.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes(",")
    /// local parts = re:split("a,b,c,d")
    /// assert(#parts == 4 and parts[1] == "a" and parts[4] == "d")
    /// ```
    #[lua_method]
    fn split(self: Arc<Self>, haystack: Bytes, limit: Option<i64>) -> Vec<Bytes> {
        let limit = limit.map(|v| v.max(0) as usize).unwrap_or(0);
        let mut out: Vec<Bytes> = Vec::new();
        let mut last = 0usize;
        let mut count = 0usize;
        let mut offset = 0usize;
        loop {
            if limit != 0 && count + 1 >= limit {
                break;
            }
            let m = match self.inner.find_at(&haystack, offset) {
                Some(m) => m,
                None => break,
            };
            out.push(Bytes::from(&haystack[last..m.start()]));
            last = m.end();
            offset = if m.start() == m.end() {
                m.end() + 1
            } else {
                m.end()
            };
            count += 1;
            if offset > haystack.len() {
                break;
            }
        }
        out.push(Bytes::from(&haystack[last..]));
        out
    }

    /// Returns the names of the explicit capture groups.  See
    /// `Regex:capture_names`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(?<key>\\w+)=(\\d+)")
    /// local names = re:capture_names()
    /// assert(names[1] == "key" and names[2] == nil)
    /// ```
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

    /// Number of explicit capture groups.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(\\w+)=(\\d+)")
    /// assert(re:capture_count() == 2)
    /// ```
    #[lua_method]
    fn capture_count(self: Arc<Self>) -> i64 {
        (self.inner.captures_len() as i64 - 1).max(0)
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = format!("BytesRegex({:?})", self.inner.as_str());
        Variadic(valuevec![Value::string(s)])
    }
}

/// Captures from a bytes-regex match.
pub struct LuaBytesCaptures {
    pub(super) data: CapturesData,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "BytesCaptures", index_fallback = "nil")]
impl LuaBytesCaptures {
    /// The matched substring for group `i`.  See `Captures:get`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(\\d+)-(\\d+)")
    /// local c = re:captures("42-99")
    /// assert(c:get(0) == "42-99" and c:get(1) == "42" and c:get(2) == "99")
    /// ```
    #[lua_method]
    fn get(self: Arc<Self>, i: i64) -> Option<Bytes> {
        if i < 0 {
            return None;
        }
        self.data.get_bytes(i as usize)
    }

    /// 1-based byte offset of the start of group `i`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(\\d+)")
    /// local c = re:captures("abc 42")
    /// assert(c:start(1) == 5)
    /// ```
    #[lua_method]
    fn start(self: Arc<Self>, i: i64) -> Option<i64> {
        if i < 0 {
            return None;
        }
        let (s, _) = self.data.get_span(i as usize)?;
        Some((s + 1) as i64)
    }

    /// 1-based byte offset of the last byte of group `i` (inclusive).
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(\\d+)")
    /// local c = re:captures("abc 42")
    /// assert(c:end_(1) == 6)
    /// ```
    #[lua_method]
    fn end_(self: Arc<Self>, i: i64) -> Option<i64> {
        if i < 0 {
            return None;
        }
        let (_, e) = self.data.get_span(i as usize)?;
        Some(e as i64)
    }

    /// The name of group `i`, or `nil`.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(?<key>\\w+)=(\\d+)")
    /// local c = re:captures("port=8080")
    /// assert(c:name(1) == "key" and c:name(2) == nil)
    /// ```
    #[lua_method]
    fn name(self: Arc<Self>, i: i64) -> Option<Bytes> {
        if i < 0 {
            return None;
        }
        self.data.names.get(i as usize).and_then(|n| n.clone())
    }

    /// Looks up a named group's match.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(?<key>\\w+)=(?<val>\\d+)")
    /// local c = re:captures("port=8080")
    /// assert(c:by_name("key") == "port")
    /// assert(c:by_name("val") == "8080")
    /// ```
    #[lua_method]
    fn by_name(self: Arc<Self>, name: Bytes) -> Option<Bytes> {
        let i = self.data.index_of_name(&name)?;
        self.data.get_bytes(i)
    }

    /// Number of groups (whole match plus explicit groups).
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(\\w+)=(\\d+)")
    /// local c = re:captures("port=8080")
    /// assert(c:len() == 3)  -- whole match plus two groups
    /// ```
    #[lua_method]
    fn len(self: Arc<Self>) -> i64 {
        self.data.groups.len() as i64
    }

    /// Expand a `$N` / `${name}` / `$$` template.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local re = regex.compile_bytes("(?<first>\\w+) (?<last>\\w+)")
    /// local c = re:captures("alice smith")
    /// assert(c:expand("${last}, ${first}") == "smith, alice")
    /// ```
    #[lua_method]
    fn expand(self: Arc<Self>, template: Bytes) -> Bytes {
        self.data.expand(&template)
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string(
            self.data.tostring("BytesCaptures")
        )])
    }
}

async fn bytes_replace(
    re: Arc<LuaBytesRegex>,
    ctx: &CallContext,
    haystack: Bytes,
    repl: Value,
    limit: usize,
) -> Result<Bytes, VmError> {
    let mut out: Vec<u8> = Vec::with_capacity(haystack.len());
    let mut last = 0usize;
    let mut offset = 0usize;
    let mut count = 0usize;
    while count < limit {
        let caps = match re.inner.captures_at(&haystack, offset) {
            Some(c) => c,
            None => break,
        };
        let m0 = caps
            .get(0)
            .expect("group 0 always set on a successful match");
        let (mstart, mend) = (m0.start(), m0.end());
        out.extend_from_slice(&haystack[last..mstart]);
        let groups = (0..caps.len())
            .map(|i| caps.get(i).map(|m| (m.start(), m.end())))
            .collect::<Vec<_>>();
        let data = CapturesData {
            haystack: haystack.clone(),
            groups,
            names: Arc::clone(&re.names),
        };
        let replacement =
            apply_replacement::<LuaBytesCaptures>(ctx, &data, "BytesRegex:replace", &repl).await?;
        out.extend_from_slice(&replacement);
        last = mend;
        offset = if mstart == mend { mend + 1 } else { mend };
        count += 1;
        if offset > haystack.len() {
            break;
        }
    }
    out.extend_from_slice(&haystack[last..]);
    Ok(Bytes::from(out))
}

impl super::WrapCaptures for LuaBytesCaptures {
    fn wrap(data: CapturesData) -> Value {
        Value::from(Ud(Arc::new(Self { data })))
    }
}
