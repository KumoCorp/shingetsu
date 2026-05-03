//! Lua 5.4 pattern matcher — a direct byte-level port of
//! `lstrlib.c`'s `match()` routine.
//!
//! Lua patterns are *not* regular expressions.  They have a distinct
//! syntax — `%d` for digits, `-` as a lazy `*`, no alternation — and,
//! crucially, operate on **bytes**, not Unicode scalar values.  The
//! previous version of this module translated Lua patterns to a
//! `regex::bytes::Regex` string, which lost byte-level fidelity when
//! the pattern or haystack contained non-ASCII bytes (the `.` class
//! under `regex`'s default Unicode mode matched whole UTF-8 codepoints
//! rather than individual bytes, and non-ASCII pattern literals were
//! UTF-8-re-encoded in the regex source).
//!
//! This implementation mirrors the reference C code closely: a
//! `MatchState` struct holds the subject, pattern, and capture slots;
//! `do_match()` walks the pattern byte-by-byte with recursion for
//! alternatives and a tail-call `continue`-loop optimization.
//!
//! Reference: <https://www.lua.org/manual/5.4/manual.html#6.4.1>
//! Reference implementation: `lstrlib.c` in the Lua 5.4 source tree.

/// Maximum number of capture groups per pattern, matching Lua's
/// `LUA_MAXCAPTURES`.
const MAX_CAPTURES: usize = 32;

/// Maximum recursion depth inside the matcher, matching Lua's
/// `MAXCCALLS`.  Exceeding this limit reports "pattern too complex".
const MAX_CALLS: usize = 200;

/// The Lua pattern escape byte.
const L_ESC: u8 = b'%';

/// Errors raised while compiling or executing a pattern.
#[derive(Debug)]
pub(crate) struct PatternError {
    pub message: String,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PatternError {}

impl From<&str> for PatternError {
    fn from(message: &str) -> Self {
        Self {
            message: message.to_owned(),
        }
    }
}

impl From<String> for PatternError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

/// Result type for the inner matcher: `Ok(Some(end))` on a successful
/// match ending at byte index `end`, `Ok(None)` on match failure, or
/// `Err` on a malformed pattern / depth overflow.
type MatchResult = Result<Option<usize>, PatternError>;

/// A single capture produced by a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    /// A substring capture: `haystack[start..end]`.
    Span { start: usize, end: usize },
    /// A position capture (`()`): 0-based byte index into the haystack.
    /// Lua exposes this as a 1-based integer; callers convert.
    Position(usize),
}

/// The result of a successful match.
#[derive(Debug, Clone)]
pub(crate) struct Match {
    /// Inclusive start index of the whole match.
    pub start: usize,
    /// Exclusive end index of the whole match.
    pub end: usize,
    /// Explicit captures from `(...)` groups, in order.
    pub captures: Vec<Capture>,
}

/// A validated Lua pattern ready to be matched against a haystack.
///
/// Lua doesn't actually pre-compile patterns — the reference matcher
/// walks the raw bytes on every attempt — but we perform a one-time
/// validation pass to surface malformed patterns before the first
/// match and to record the number of captures up front.
#[derive(Debug, Clone)]
pub(crate) struct Pattern {
    pat: Vec<u8>,
    #[cfg_attr(not(test), allow(dead_code))]
    n_captures: usize,
}

impl Pattern {
    /// Validate a Lua pattern and count its explicit captures.
    pub fn compile(pat: &[u8]) -> Result<Self, PatternError> {
        let n_captures = validate(pat)?;
        Ok(Self {
            pat: pat.to_vec(),
            n_captures,
        })
    }

    /// Number of explicit `(...)` capture groups in the pattern.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn n_captures(&self) -> usize {
        self.n_captures
    }

    /// Whether the pattern begins with a `^` anchor.  Callers that
    /// iterate (e.g. `gsub`, `gmatch`) need to stop after the first
    /// match for anchored patterns, since the anchor binds to the
    /// start of the subject string.
    pub fn is_anchored(&self) -> bool {
        self.pat.first() == Some(&b'^')
    }

    /// Search `haystack` starting at byte offset `init`.
    ///
    /// Honours a leading `^` anchor: if anchored, only the position at
    /// `init` is tried; otherwise every position from `init` to the
    /// end of the haystack is tried in order.
    pub fn find(&self, haystack: &[u8], init: usize) -> Result<Option<Match>, PatternError> {
        let init = init.min(haystack.len());
        let anchored = self.is_anchored();
        let pat = if anchored {
            &self.pat[1..]
        } else {
            &self.pat[..]
        };

        let mut s_idx = init;
        loop {
            let mut ms = MatchState {
                src: haystack,
                pat,
                level: 0,
                captures: [CaptureSlot::Unfinished { start: 0 }; MAX_CAPTURES],
                depth: MAX_CALLS,
            };
            if let Some(end) = ms.do_match(s_idx, 0)? {
                let captures = ms.collect_captures();
                return Ok(Some(Match {
                    start: s_idx,
                    end,
                    captures,
                }));
            }
            if anchored || s_idx >= haystack.len() {
                return Ok(None);
            }
            s_idx += 1;
        }
    }
}

// =========================================================================
// Validation
// =========================================================================

/// Walk the pattern once and surface syntax errors up front.  Returns
/// the number of `(` capture groups (excluding those nested inside a
/// `%` escape).  Does **not** execute the match.
fn validate(pat: &[u8]) -> Result<usize, PatternError> {
    let mut i = 0;
    let mut n_captures = 0;
    let mut open_captures: usize = 0;
    while i < pat.len() {
        match pat[i] {
            b'%' => {
                if i + 1 >= pat.len() {
                    return Err("malformed pattern (ends with '%')".into());
                }
                match pat[i + 1] {
                    b'b' => {
                        if i + 3 >= pat.len() {
                            return Err("malformed pattern (missing arguments to '%b')".into());
                        }
                        i += 4;
                    }
                    b'f' => {
                        if i + 2 >= pat.len() || pat[i + 2] != b'[' {
                            return Err("missing '[' after '%f' in pattern".into());
                        }
                        // Validate the set.
                        let set_end = class_end(pat, i + 2)?;
                        i = set_end;
                    }
                    _ => i += 2,
                }
            }
            b'[' => {
                let end = class_end(pat, i)?;
                i = end;
            }
            b'(' => {
                if n_captures >= MAX_CAPTURES {
                    return Err("too many captures".into());
                }
                n_captures += 1;
                open_captures += 1;
                i += 1;
            }
            b')' => {
                if open_captures == 0 {
                    return Err("invalid pattern capture".into());
                }
                open_captures -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if open_captures != 0 {
        return Err("unfinished capture".into());
    }
    Ok(n_captures)
}

/// Return the index one past the end of a single pattern element
/// starting at `p`.  Matches `classend` in the reference.
fn class_end(pat: &[u8], p: usize) -> Result<usize, PatternError> {
    let mut i = p;
    match pat.get(i).copied() {
        None => Err("malformed pattern".into()),
        Some(L_ESC) => {
            if i + 1 >= pat.len() {
                return Err("malformed pattern (ends with '%')".into());
            }
            Ok(i + 2)
        }
        Some(b'[') => {
            i += 1;
            if pat.get(i) == Some(&b'^') {
                i += 1;
            }
            // A `]` at the very start of the set is treated as literal.
            if pat.get(i) == Some(&b']') {
                i += 1;
            }
            while pat.get(i) != Some(&b']') {
                let Some(&c) = pat.get(i) else {
                    return Err("malformed pattern (missing ']')".into());
                };
                i += 1;
                if c == L_ESC && i < pat.len() {
                    i += 1; // skip escaped char (e.g. %])
                }
            }
            Ok(i + 1)
        }
        Some(_) => Ok(i + 1),
    }
}

// =========================================================================
// Character class predicates
// =========================================================================

/// Match a single byte `c` against the 1-byte class letter `cl`.
/// Uppercase letters are complements of their lowercase counterparts.
/// Non-class letters match themselves literally.
fn match_class(c: u8, cl: u8) -> bool {
    let lower = cl.to_ascii_lowercase();
    let res = match lower {
        b'a' => c.is_ascii_alphabetic(),
        b'c' => c.is_ascii_control(),
        b'd' => c.is_ascii_digit(),
        b'g' => (0x21..=0x7E).contains(&c), // printable, non-space
        b'l' => c.is_ascii_lowercase(),
        b'p' => c.is_ascii_punctuation(),
        b's' => c == b' ' || (0x09..=0x0D).contains(&c),
        b'u' => c.is_ascii_uppercase(),
        b'w' => c.is_ascii_alphanumeric(),
        b'x' => c.is_ascii_hexdigit(),
        // `%z` (match NUL byte) is undocumented in the 5.4 manual but
        // is retained in the reference implementation for backwards
        // compatibility with older Lua pattern code.  `%Z` is its
        // complement (any non-NUL byte).
        b'z' => c == 0,
        _ => return cl == c,
    };
    if cl.is_ascii_uppercase() {
        !res
    } else {
        res
    }
}

/// Match `c` against the bracket set `pat[p..=ec]` where `pat[p] == b'['`
/// and `pat[ec] == b']'`.
fn match_bracket_class(c: u8, pat: &[u8], p: usize, ec: usize) -> bool {
    let mut sig = true;
    let mut i = p + 1;
    if pat.get(i) == Some(&b'^') {
        sig = false;
        i += 1;
    }
    while i < ec {
        if pat[i] == L_ESC && i + 1 < ec {
            if match_class(c, pat[i + 1]) {
                return sig;
            }
            i += 2;
        } else if i + 2 < ec && pat[i + 1] == b'-' {
            if pat[i] <= c && c <= pat[i + 2] {
                return sig;
            }
            i += 3;
        } else {
            if pat[i] == c {
                return sig;
            }
            i += 1;
        }
    }
    !sig
}

// =========================================================================
// MatchState
// =========================================================================

#[derive(Debug, Clone, Copy)]
enum CaptureSlot {
    /// A capture that has begun but not yet closed; `start` is the
    /// byte index into the subject where it opened.
    Unfinished { start: usize },
    /// A position capture (`()`): a zero-length marker at `at`.
    Position { at: usize },
    /// A closed substring capture.
    Finished { start: usize, end: usize },
}

struct MatchState<'a> {
    src: &'a [u8],
    pat: &'a [u8],
    level: usize,
    captures: [CaptureSlot; MAX_CAPTURES],
    depth: usize,
}

impl<'a> MatchState<'a> {
    /// The core recursive matcher.  Returns `Ok(Some(end))` on success
    /// where `end` is the byte index one past the end of the match in
    /// `self.src`; `Ok(None)` on match failure; `Err` for malformed
    /// patterns or recursion-depth overflow.
    ///
    /// Thin wrapper that bookkeeps the recursion-depth counter; the
    /// actual matching loop lives in [`do_match_body`](Self::do_match_body)
    /// so early returns can use plain `return` instead of `break`.
    fn do_match(&mut self, s: usize, p: usize) -> MatchResult {
        if self.depth == 0 {
            return Err("pattern too complex".into());
        }
        self.depth -= 1;
        let result = self.do_match_body(s, p);
        self.depth += 1;
        result
    }

    /// The matching loop itself.  Mirrors `match()` in `lstrlib.c`,
    /// including the tail-recursion optimization implemented as a
    /// `continue` on a surrounding loop.
    fn do_match_body(&mut self, mut s: usize, mut p: usize) -> MatchResult {
        loop {
            if p >= self.pat.len() {
                return Ok(Some(s));
            }
            match self.pat[p] {
                b'(' => {
                    // Start capture.
                    return if self.pat.get(p + 1) == Some(&b')') {
                        // Position capture.
                        self.start_position_capture(s, p + 2)
                    } else {
                        self.start_capture(s, p + 1)
                    };
                }
                b')' => {
                    return self.end_capture(s, p + 1);
                }
                b'$' if p + 1 == self.pat.len() => {
                    return Ok(if s == self.src.len() { Some(s) } else { None });
                }
                L_ESC => {
                    let next =
                        self.pat.get(p + 1).copied().ok_or_else(|| {
                            PatternError::from("malformed pattern (ends with '%')")
                        })?;
                    match next {
                        b'b' => {
                            // Balanced match %bxy — no quantifier follows.
                            let Some(end) = self.match_balance(s, p + 2)? else {
                                return Ok(None);
                            };
                            s = end;
                            p += 4;
                            continue;
                        }
                        b'f' => {
                            // Frontier pattern %f[set].
                            if self.pat.get(p + 2) != Some(&b'[') {
                                return Err("missing '[' after '%f' in pattern".into());
                            }
                            let ep = class_end(self.pat, p + 2)?;
                            let previous = if s == 0 { 0u8 } else { self.src[s - 1] };
                            let current = self.src.get(s).copied().unwrap_or(0);
                            if !match_bracket_class(previous, self.pat, p + 2, ep - 1)
                                && match_bracket_class(current, self.pat, p + 2, ep - 1)
                            {
                                p = ep;
                                continue;
                            }
                            return Ok(None);
                        }
                        b'0'..=b'9' => {
                            // Back-reference %1..%9 (and %0 is not a backref).
                            let Some(end) = self.match_capture(s, next)? else {
                                return Ok(None);
                            };
                            s = end;
                            p += 2;
                            continue;
                        }
                        _ => {
                            // Fallthrough to default class-with-suffix handling.
                            match self.match_single_or_expand(s, p)? {
                                MatchStep::Done(end) => return Ok(end),
                                MatchStep::Tail { next_s, next_p } => {
                                    s = next_s;
                                    p = next_p;
                                }
                            }
                        }
                    }
                }
                _ => match self.match_single_or_expand(s, p)? {
                    MatchStep::Done(end) => return Ok(end),
                    MatchStep::Tail { next_s, next_p } => {
                        s = next_s;
                        p = next_p;
                    }
                },
            }
        }
    }

    /// Handle a "pattern class plus optional suffix" position — `.`, a
    /// literal, a `%x` escape that isn't `b`/`f`/digit, or a bracket
    /// set, possibly followed by `*`/`+`/`-`/`?`.
    ///
    /// Returns either a completed match result or a tail-call
    /// continuation to be resumed by the caller loop.
    fn match_single_or_expand(&mut self, s: usize, p: usize) -> Result<MatchStep, PatternError> {
        let ep = class_end(self.pat, p)?;
        let single_ok = self.singlematch(s, p, ep);
        let suffix = self.pat.get(ep).copied();

        if !single_ok {
            if matches!(suffix, Some(b'*') | Some(b'?') | Some(b'-')) {
                // Zero repetitions accepted — advance past the suffix
                // and continue matching the rest of the pattern.
                return Ok(MatchStep::Tail {
                    next_s: s,
                    next_p: ep + 1,
                });
            }
            return Ok(MatchStep::Done(None));
        }

        match suffix {
            Some(b'?') => {
                // Optional: try with one, fall back to zero.
                if let Some(end) = self.do_match(s + 1, ep + 1)? {
                    return Ok(MatchStep::Done(Some(end)));
                }
                Ok(MatchStep::Tail {
                    next_s: s,
                    next_p: ep + 1,
                })
            }
            Some(b'+') => Ok(MatchStep::Done(self.max_expand(s + 1, p, ep)?)),
            Some(b'*') => Ok(MatchStep::Done(self.max_expand(s, p, ep)?)),
            Some(b'-') => Ok(MatchStep::Done(self.min_expand(s, p, ep)?)),
            _ => Ok(MatchStep::Tail {
                next_s: s + 1,
                next_p: ep,
            }),
        }
    }

    /// Does a single-character class at `pat[p..ep]` match the byte at
    /// `self.src[s]`?
    fn singlematch(&self, s: usize, p: usize, ep: usize) -> bool {
        let Some(&c) = self.src.get(s) else {
            return false;
        };
        match self.pat[p] {
            b'.' => true,
            L_ESC => match_class(c, self.pat[p + 1]),
            b'[' => match_bracket_class(c, self.pat, p, ep - 1),
            byte => byte == c,
        }
    }

    /// Greedy repetition: consume as many matches as possible, then
    /// back off one at a time looking for a match of the rest of the
    /// pattern.
    fn max_expand(&mut self, s: usize, p: usize, ep: usize) -> MatchResult {
        let mut i = 0usize;
        while self.singlematch(s + i, p, ep) {
            i += 1;
        }
        // i now holds the maximum possible repetitions.
        loop {
            if let Some(end) = self.do_match(s + i, ep + 1)? {
                return Ok(Some(end));
            }
            if i == 0 {
                return Ok(None);
            }
            i -= 1;
        }
    }

    /// Lazy repetition: start with zero matches and consume one at a
    /// time until the rest of the pattern matches or the input is
    /// exhausted.
    fn min_expand(&mut self, mut s: usize, p: usize, ep: usize) -> MatchResult {
        loop {
            if let Some(end) = self.do_match(s, ep + 1)? {
                return Ok(Some(end));
            }
            if self.singlematch(s, p, ep) {
                s += 1;
            } else {
                return Ok(None);
            }
        }
    }

    /// Open a substring capture slot and recurse into the rest of the
    /// pattern.  If the recursion fails, the slot is rolled back.
    fn start_capture(&mut self, s: usize, p: usize) -> MatchResult {
        if self.level >= MAX_CAPTURES {
            return Err("too many captures".into());
        }
        self.captures[self.level] = CaptureSlot::Unfinished { start: s };
        self.level += 1;
        let res = self.do_match(s, p)?;
        if res.is_none() {
            self.level -= 1; // roll back
        }
        Ok(res)
    }

    /// Open a position capture `()` — a zero-length marker at `s`.
    fn start_position_capture(&mut self, s: usize, p: usize) -> MatchResult {
        if self.level >= MAX_CAPTURES {
            return Err("too many captures".into());
        }
        self.captures[self.level] = CaptureSlot::Position { at: s };
        self.level += 1;
        let res = self.do_match(s, p)?;
        if res.is_none() {
            self.level -= 1;
        }
        Ok(res)
    }

    /// Close the most recently opened unfinished capture.
    fn end_capture(&mut self, s: usize, p: usize) -> MatchResult {
        let idx = self.captures[..self.level]
            .iter()
            .rposition(|c| matches!(c, CaptureSlot::Unfinished { .. }))
            .ok_or_else(|| PatternError::from("invalid pattern capture"))?;
        let saved = self.captures[idx];
        let CaptureSlot::Unfinished { start } = saved else {
            unreachable!("rposition above guarantees Unfinished");
        };
        self.captures[idx] = CaptureSlot::Finished { start, end: s };
        let res = self.do_match(s, p)?;
        if res.is_none() {
            self.captures[idx] = saved; // roll back
        }
        Ok(res)
    }

    /// Resolve a `%N` backreference to the already-closed capture at
    /// index `N-1` and match its bytes at the current position.
    ///
    /// An unfinished capture (backref to a `(` that has not yet been
    /// closed) is a pattern error, matching the reference
    /// `check_capture` behaviour.  A position capture (`()`), however,
    /// matches silently-nothing: in the reference implementation the
    /// `CAP_POSITION` sentinel (-2) cast to `size_t` produces a huge
    /// value so the length comparison fails and `NULL` is returned.
    fn match_capture(&self, s: usize, digit: u8) -> MatchResult {
        let idx = (digit - b'0') as usize;
        if idx == 0 || idx > self.level {
            return Err(format!("invalid capture index %{}", idx).into());
        }
        match self.captures[idx - 1] {
            CaptureSlot::Finished { start, end } => {
                let cap_bytes = &self.src[start..end];
                if s + cap_bytes.len() <= self.src.len()
                    && &self.src[s..s + cap_bytes.len()] == cap_bytes
                {
                    Ok(Some(s + cap_bytes.len()))
                } else {
                    Ok(None)
                }
            }
            CaptureSlot::Position { .. } => Ok(None),
            CaptureSlot::Unfinished { .. } => Err(format!("invalid capture index %{}", idx).into()),
        }
    }

    /// Match a balanced-pair construct `%b<open><close>` starting at
    /// `self.pat[p]`.  Returns the index just past the closing byte.
    fn match_balance(&self, s: usize, p: usize) -> MatchResult {
        if p + 1 >= self.pat.len() {
            return Err("malformed pattern (missing arguments to '%b')".into());
        }
        let Some(&sc) = self.src.get(s) else {
            return Ok(None);
        };
        let open = self.pat[p];
        let close = self.pat[p + 1];
        if sc != open {
            return Ok(None);
        }
        let mut count: i32 = 1;
        let mut i = s + 1;
        while i < self.src.len() {
            let c = self.src[i];
            if c == close {
                count -= 1;
                if count == 0 {
                    return Ok(Some(i + 1));
                }
            } else if c == open {
                count += 1;
            }
            i += 1;
        }
        Ok(None) // unbalanced
    }

    /// Collect closed captures after a successful match.
    fn collect_captures(&self) -> Vec<Capture> {
        self.captures[..self.level]
            .iter()
            .map(|slot| match *slot {
                CaptureSlot::Finished { start, end } => Capture::Span { start, end },
                CaptureSlot::Position { at } => Capture::Position(at),
                CaptureSlot::Unfinished { .. } => {
                    // A successful match should not leave unfinished
                    // captures; this indicates a bug in the matcher.
                    Capture::Span { start: 0, end: 0 }
                }
            })
            .collect()
    }
}

/// Control flow for tail-recursive continuation in `do_match`.
enum MatchStep {
    /// The current pattern position has produced a final answer
    /// (either success with `Some(end)` or failure with `None`).
    Done(Option<usize>),
    /// Continue the match at `(next_s, next_p)` within the same frame
    /// — mirrors the C code's `goto init`.
    Tail { next_s: usize, next_p: usize },
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn find(pat: &str, hay: &[u8]) -> Option<Match> {
        Pattern::compile(pat.as_bytes())
            .expect("pattern compiles")
            .find(hay, 0)
            .expect("no runtime error")
    }

    fn cap_str<'a>(m: &Match, hay: &'a [u8], i: usize) -> &'a [u8] {
        match m.captures[i] {
            Capture::Span { start, end } => &hay[start..end],
            Capture::Position(_) => panic!("expected span capture"),
        }
    }

    fn annotated_captures<'a>(m: &Match, hay: &'a [u8]) -> Vec<(Capture, &'a str)> {
        m.captures
            .iter()
            .map(|c| match *c {
                Capture::Span { start, end } => {
                    (*c, std::str::from_utf8(&hay[start..end]).expect("utf8"))
                }
                Capture::Position(_) => (*c, ""),
            })
            .collect()
    }

    /// The matched substring of `hay` for the first match of `pat`.
    fn matched<'a>(pat: &str, hay: &'a [u8]) -> &'a [u8] {
        let m = find(pat, hay).expect("pattern should match");
        &hay[m.start..m.end]
    }

    #[test]
    fn literal_match() {
        let m = find("hello", b"say hello world").unwrap();
        k9::assert_equal!(m.start, 4);
        k9::assert_equal!(m.end, 9);
    }

    #[test]
    fn no_match() {
        k9::assert_equal!(find("xyz", b"abc").is_none(), true);
    }

    #[test]
    fn anchored_start() {
        k9::assert_equal!(find("^hello", b"say hello").is_none(), true);
        let m = find("^hello", b"hello world").unwrap();
        k9::assert_equal!(m.end, 5);
    }

    #[test]
    fn anchored_end() {
        let m = find("world$", b"hello world").unwrap();
        k9::assert_equal!(m.start, 6);
        k9::assert_equal!(m.end, 11);
        k9::assert_equal!(find("world$", b"hello world!").is_none(), true);
    }

    #[test]
    fn dot_matches_any_byte() {
        // Key regression: `.` must match single bytes, including
        // non-ASCII bytes that would be part of a UTF-8 sequence.
        let utf8 = "é".as_bytes(); // 0xc3 0xa9
        let m = find(".", utf8).unwrap();
        k9::assert_equal!(m.start, 0);
        k9::assert_equal!(m.end, 1);
    }

    #[test]
    fn digit_class() {
        let m = find("%d+", b"abc 12345 xyz").unwrap();
        k9::assert_equal!(&b"abc 12345 xyz"[m.start..m.end], b"12345");
    }

    #[test]
    fn alpha_class() {
        let m = find("%a+", b"123 hello 456").unwrap();
        k9::assert_equal!(&b"123 hello 456"[m.start..m.end], b"hello");
    }

    #[test]
    fn space_class() {
        let m = find("%s+", b"abc   def").unwrap();
        k9::assert_equal!(m.end - m.start, 3);
    }

    #[test]
    fn upper_class_complement() {
        let m = find("%A+", b"abc123def").unwrap();
        k9::assert_equal!(&b"abc123def"[m.start..m.end], b"123");
    }

    #[test]
    fn bracket_set() {
        let m = find("[aeiou]+", b"rhythm aeiou").unwrap();
        k9::assert_equal!(&b"rhythm aeiou"[m.start..m.end], b"aeiou");
    }

    #[test]
    fn bracket_set_complement() {
        let m = find("[^aeiou ]+", b" hello world").unwrap();
        k9::assert_equal!(&b" hello world"[m.start..m.end], b"h");
    }

    #[test]
    fn bracket_set_range() {
        let m = find("[a-z]+", b"ABCdef").unwrap();
        k9::assert_equal!(&b"ABCdef"[m.start..m.end], b"def");
    }

    #[test]
    fn bracket_set_with_class_inside() {
        let m = find("[%d%a]+", b"!!abc123!!").unwrap();
        k9::assert_equal!(&b"!!abc123!!"[m.start..m.end], b"abc123");
    }

    #[test]
    fn bracket_set_literal_close_at_start() {
        // `[]abc]` matches any of `]`, `a`, `b`, `c`.
        let m = find("[]abc]+", b"!!]abc!!").unwrap();
        k9::assert_equal!(&b"!!]abc!!"[m.start..m.end], b"]abc");
    }

    #[test]
    fn quantifier_star() {
        let m = find("a*", b"bbb").unwrap();
        k9::assert_equal!(m.start, 0);
        k9::assert_equal!(m.end, 0);
    }

    #[test]
    fn quantifier_plus_requires_one() {
        k9::assert_equal!(find("a+", b"bbb").is_none(), true);
    }

    #[test]
    fn quantifier_optional() {
        let m = find("colou?r", b"color").unwrap();
        k9::assert_equal!(m.end - m.start, 5);
        let m = find("colou?r", b"colour").unwrap();
        k9::assert_equal!(m.end - m.start, 6);
    }

    #[test]
    fn lazy_quantifier() {
        // Greedy would consume "abcXdefX"; lazy stops at first X.
        let m = find("a.-X", b"abcXdefX").unwrap();
        k9::assert_equal!(&b"abcXdefX"[m.start..m.end], b"abcX");
    }

    #[test]
    fn simple_capture() {
        let hay = b"abc 12345 xyz";
        let m = find("(%d+)", hay).unwrap();
        k9::assert_equal!(
            annotated_captures(&m, hay),
            vec![(Capture::Span { start: 4, end: 9 }, "12345")]
        );
    }

    #[test]
    fn multi_capture() {
        let hay = b"key=value";
        let m = find("(%a+)=(%a+)", hay).unwrap();
        k9::assert_equal!(cap_str(&m, hay, 0), b"key");
        k9::assert_equal!(cap_str(&m, hay, 1), b"value");
    }

    #[test]
    fn position_capture() {
        let hay = b"hello world";
        let m = find("hello ()world", hay).unwrap();
        k9::assert_equal!(
            annotated_captures(&m, hay),
            vec![(Capture::Position(6), "")]
        );
    }

    #[test]
    fn backref_in_pattern() {
        // Match pairs of identical letters: `(%a)%1` should match "bb".
        let hay = b"foobar bb zap";
        let m = find("(%a)%1", hay).unwrap();
        k9::assert_equal!(m.start, 1); // "foobar": matches "oo"
        k9::assert_equal!(cap_str(&m, hay, 0), b"o");
    }

    #[test]
    fn balanced_match() {
        let hay = b"foo(bar(baz)qux)end";
        let m = find("%b()", hay).unwrap();
        k9::assert_equal!(&hay[m.start..m.end], b"(bar(baz)qux)");
    }

    #[test]
    fn frontier_pattern() {
        // Find transitions from non-letter to letter: frontier after
        // the space before "THE".
        let hay = b"hello THE end";
        let m = find("%f[%u]", hay).unwrap();
        k9::assert_equal!(m.start, 6);
        k9::assert_equal!(m.end, 6);
    }

    #[test]
    fn frontier_at_start_of_string() {
        // Previous byte is the sentinel \0 (not in set), so a frontier
        // at position 0 should match if the first char is in the set.
        let m = find("%f[%a]", b"abc def").unwrap();
        k9::assert_equal!(m.start, 0);
    }

    #[test]
    fn pattern_with_escaped_special() {
        let m = find("%.", b"a.b").unwrap();
        k9::assert_equal!(m.start, 1);
        k9::assert_equal!(m.end, 2);
    }

    #[test]
    fn pattern_with_escaped_percent() {
        let m = find("%%", b"50%").unwrap();
        k9::assert_equal!(m.start, 2);
        k9::assert_equal!(m.end, 3);
    }

    #[test]
    fn malformed_trailing_percent() {
        let err = Pattern::compile(b"abc%").unwrap_err();
        k9::assert_equal!(err.message, "malformed pattern (ends with '%')".to_owned());
    }

    #[test]
    fn malformed_missing_bracket() {
        let err = Pattern::compile(b"[abc").unwrap_err();
        k9::assert_equal!(err.message, "malformed pattern (missing ']')".to_owned());
    }

    #[test]
    fn malformed_balanced_missing_args() {
        let err = Pattern::compile(b"%b").unwrap_err();
        k9::assert_equal!(
            err.message,
            "malformed pattern (missing arguments to '%b')".to_owned()
        );
    }

    #[test]
    fn malformed_frontier_missing_bracket() {
        let err = Pattern::compile(b"%f%a").unwrap_err();
        k9::assert_equal!(err.message, "missing '[' after '%f' in pattern".to_owned());
    }

    #[test]
    fn malformed_unfinished_capture() {
        let err = Pattern::compile(b"(abc").unwrap_err();
        k9::assert_equal!(err.message, "unfinished capture".to_owned());
    }

    #[test]
    fn malformed_unbalanced_paren() {
        let err = Pattern::compile(b"abc)").unwrap_err();
        k9::assert_equal!(err.message, "invalid pattern capture".to_owned());
    }

    #[test]
    fn too_many_captures_at_compile() {
        let mut pat = String::new();
        for _ in 0..33 {
            pat.push_str("(a)");
        }
        let err = Pattern::compile(pat.as_bytes()).unwrap_err();
        k9::assert_equal!(err.message, "too many captures".to_owned());
    }

    #[test]
    fn recursion_limit() {
        // Each `a?` pushes a new `do_match` frame when the optional
        // byte actually matches, so `N` of them in a row produces `N`
        // deep recursion before any tail call.  MAX_CALLS = 200 so
        // 201 copies must trip the guard.
        let pat: String = "a?".repeat(201);
        let hay = vec![b'a'; 210];
        let p = Pattern::compile(pat.as_bytes()).expect("compile ok");
        let err = p.find(&hay, 0).unwrap_err();
        k9::assert_equal!(err.message, "pattern too complex".to_owned());
    }

    #[test]
    fn n_captures_counted() {
        let p = Pattern::compile(b"(%a+)=(%d+)").unwrap();
        k9::assert_equal!(p.n_captures(), 2);
        let p = Pattern::compile(b"no captures here").unwrap();
        k9::assert_equal!(p.n_captures(), 0);
    }

    #[test]
    fn find_with_init_offset() {
        let hay = b"abc abc abc";
        let p = Pattern::compile(b"abc").unwrap();
        let m = p.find(hay, 4).unwrap().unwrap();
        k9::assert_equal!(m.start, 4);
    }

    #[test]
    fn date_pattern_captures() {
        let hay = b"2025-04-13";
        let m = find("(%d+)-(%d+)-(%d+)", hay).unwrap();
        k9::assert_equal!(
            annotated_captures(&m, hay),
            vec![
                (Capture::Span { start: 0, end: 4 }, "2025"),
                (Capture::Span { start: 5, end: 7 }, "04"),
                (Capture::Span { start: 8, end: 10 }, "13"),
            ]
        );
    }

    #[test]
    fn non_ascii_byte_literal_in_pattern() {
        // Pattern byte 0xc3 should match the byte 0xc3 in the
        // haystack.  Under the old regex-translator implementation
        // this was silently re-encoded as UTF-8 bytes 0xc3 0x83.
        let pat = [0xc3u8];
        let hay = [b'a', 0xc3, b'b'];
        let p = Pattern::compile(&pat).unwrap();
        let m = p.find(&hay, 0).unwrap().unwrap();
        k9::assert_equal!(m.start, 1);
        k9::assert_equal!(m.end, 2);
    }

    #[test]
    fn xdigit_class() {
        let m = find("%x+", b"gg FACE42 zz").unwrap();
        k9::assert_equal!(&b"gg FACE42 zz"[m.start..m.end], b"FACE42");
    }

    #[test]
    fn graph_class() {
        // `%g` is printable non-space.
        let m = find("%g+", b"   hello   ").unwrap();
        k9::assert_equal!(&b"   hello   "[m.start..m.end], b"hello");
    }

    // ---------------------------------------------------------------
    // Class complements and less-common classes.
    // ---------------------------------------------------------------

    #[test]
    fn digit_complement() {
        k9::assert_equal!(matched("%D+", b"abc 1 def"), b"abc ");
    }

    #[test]
    fn punct_class_and_complement() {
        k9::assert_equal!(matched("%p+", b"abc!!!def"), b"!!!");
        k9::assert_equal!(matched("%P+", b"!abc!"), b"abc");
    }

    #[test]
    fn alnum_complement() {
        k9::assert_equal!(matched("%W+", b"abc!!!def"), b"!!!");
    }

    #[test]
    fn lower_class_and_complement() {
        k9::assert_equal!(matched("%l+", b"ABCdef"), b"def");
        k9::assert_equal!(matched("%L+", b"abCDef"), b"CD");
    }

    #[test]
    fn upper_class_and_positive() {
        k9::assert_equal!(matched("%u+", b"abcDEFghi"), b"DEF");
        k9::assert_equal!(matched("%U+", b"abcDef"), b"abc");
    }

    #[test]
    fn graph_complement() {
        // `%G` = not (printable non-space) = control chars and space.
        k9::assert_equal!(matched("%G+", b"abc   def"), b"   ");
    }

    #[test]
    fn control_class_and_complement() {
        let hay = b"ab\x01\x02cd";
        k9::assert_equal!(matched("%c+", hay), b"\x01\x02");
        k9::assert_equal!(matched("%C+", hay), b"ab");
    }

    #[test]
    fn space_complement() {
        k9::assert_equal!(matched("%S+", b"  hello  world"), b"hello");
    }

    #[test]
    fn z_class_matches_only_nul() {
        // `%z` is undocumented in Lua 5.4 but the reference
        // implementation keeps it as "matches NUL".
        k9::assert_equal!(find("%z", b"z").is_none(), true);
        let hay = b"a\0b";
        let m = find("%z", hay).unwrap();
        k9::assert_equal!(m.start, 1);
        k9::assert_equal!(m.end, 2);
    }

    #[test]
    fn capital_z_class_is_non_nul() {
        let hay = b"\0\0abc\0";
        let m = find("%Z+", hay).unwrap();
        k9::assert_equal!(&hay[m.start..m.end], b"abc");
    }

    #[test]
    fn unknown_percent_letter_is_literal() {
        // `%q` is not a recognized class, so it matches literal `q`.
        let m = find("%q", b"aqb").unwrap();
        k9::assert_equal!(m.start, 1);
        k9::assert_equal!(m.end, 2);
    }

    // ---------------------------------------------------------------
    // Bracket set edge cases.
    // ---------------------------------------------------------------

    #[test]
    fn bracket_set_leading_dash_is_literal() {
        // `[-ab]` matches any of `-`, `a`, `b`.
        k9::assert_equal!(matched("[-ab]+", b"xxa-bxx"), b"a-b");
    }

    #[test]
    fn bracket_set_trailing_dash_is_literal() {
        k9::assert_equal!(matched("[ab-]+", b"xxa-bxx"), b"a-b");
    }

    #[test]
    fn bracket_set_escaped_close_bracket() {
        // `[%]]` is a set containing the escaped `]`.
        let m = find("[%]]", b"x]y").unwrap();
        k9::assert_equal!(m.start, 1);
        k9::assert_equal!(m.end, 2);
    }

    #[test]
    fn bracket_set_complement_range() {
        k9::assert_equal!(matched("[^a-z]+", b"abc123def"), b"123");
    }

    #[test]
    fn bracket_set_complement_class() {
        // `[%A]+` matches runs of non-alpha characters.
        k9::assert_equal!(matched("[%A]+", b"abc123!!!def"), b"123!!!");
    }

    #[test]
    fn bracket_set_high_byte_range() {
        // Byte ranges spanning the high-bit region must compare raw
        // byte values, not interpret them as UTF-8 code units.
        let hay = [0x40u8, 0x80, 0x90, 0xFF, 0x30];
        let pat: &[u8] = &[b'[', 0x80, b'-', 0xFF, b']', b'+'];
        let p = Pattern::compile(pat).unwrap();
        let m = p.find(&hay, 0).unwrap().unwrap();
        k9::assert_equal!(m.start, 1);
        k9::assert_equal!(m.end, 4);
    }

    // ---------------------------------------------------------------
    // Capture edge cases.
    // ---------------------------------------------------------------

    #[test]
    fn nested_captures() {
        let hay = b"ab";
        let m = find("((a)(b))", hay).unwrap();
        k9::assert_equal!(
            annotated_captures(&m, hay),
            vec![
                (Capture::Span { start: 0, end: 2 }, "ab"),
                (Capture::Span { start: 0, end: 1 }, "a"),
                (Capture::Span { start: 1, end: 2 }, "b"),
            ]
        );
    }

    #[test]
    fn position_and_span_captures_mixed() {
        let hay = b"abc XYZ def";
        let m = find("()(%u+)", hay).unwrap();
        k9::assert_equal!(
            annotated_captures(&m, hay),
            vec![
                (Capture::Position(4), ""),
                (Capture::Span { start: 4, end: 7 }, "XYZ"),
            ]
        );
    }

    #[test]
    fn backref_out_of_range_errors() {
        // Pattern references `%3` but only two captures exist.
        let p = Pattern::compile(b"(a)(b)%3").unwrap();
        let err = p.find(b"abc", 0).unwrap_err();
        k9::assert_equal!(err.message, "invalid capture index %3".to_owned());
    }

    #[test]
    fn backref_to_unfinished_capture_errors() {
        // `%1` inside its own capture references an unfinished slot.
        let p = Pattern::compile(b"(%1)").unwrap();
        let err = p.find(b"ab", 0).unwrap_err();
        k9::assert_equal!(err.message, "invalid capture index %1".to_owned());
    }

    // ---------------------------------------------------------------
    // Balanced and frontier edge cases.
    // ---------------------------------------------------------------

    #[test]
    fn balanced_brackets() {
        let hay = b"[[a]b]c";
        let m = find("%b[]", hay).unwrap();
        k9::assert_equal!(&hay[m.start..m.end], b"[[a]b]");
    }

    #[test]
    fn balanced_match_unbalanced_input_returns_none() {
        k9::assert_equal!(find("%b()", b"(foo").is_none(), true);
    }

    #[test]
    fn balanced_arg_too_short_errors_at_compile() {
        let err = Pattern::compile(b"%bx").unwrap_err();
        k9::assert_equal!(
            err.message,
            "malformed pattern (missing arguments to '%b')".to_owned()
        );
    }

    // ---------------------------------------------------------------
    // Quantifier backtracking.
    // ---------------------------------------------------------------

    #[test]
    fn greedy_star_backtracks_to_find_next() {
        // `.*X` must consume as much as possible then back off until
        // the remaining `X` finds its match.  On `aXbXc` the answer
        // is `aXbX` (greedy stops at the last X).
        k9::assert_equal!(matched(".*X", b"aXbXc"), b"aXbX");
    }

    #[test]
    fn greedy_plus_backtracks_with_optional() {
        // `a+b?a` on `aa`: `a+` first eats both, `b?` matches empty,
        // `a` needs a char but we're at end.  Back off `a+` to one
        // `a`, `b?` empty, final `a` matches.  Result: `aa`.
        k9::assert_equal!(matched("a+b?a", b"aa"), b"aa");
    }

    #[test]
    fn lazy_zero_length_match() {
        // `a.-b` on `ab` must take the zero-length lazy match.
        k9::assert_equal!(matched("a.-b", b"ab"), b"ab");
    }

    // ---------------------------------------------------------------
    // Edge cases and init offset handling.
    // ---------------------------------------------------------------

    #[test]
    fn empty_pattern_on_nonempty_string() {
        let m = find("", b"abc").unwrap();
        k9::assert_equal!(m.start, 0);
        k9::assert_equal!(m.end, 0);
    }

    #[test]
    fn empty_pattern_on_empty_string() {
        let m = find("", b"").unwrap();
        k9::assert_equal!(m.start, 0);
        k9::assert_equal!(m.end, 0);
    }

    #[test]
    fn find_with_init_at_end() {
        // Empty pattern should match at the one-past-end position.
        let p = Pattern::compile(b"").unwrap();
        let m = p.find(b"abc", 3).unwrap().unwrap();
        k9::assert_equal!(m.start, 3);
        k9::assert_equal!(m.end, 3);
    }

    #[test]
    fn find_with_init_past_end_clamps() {
        let p = Pattern::compile(b"").unwrap();
        let m = p.find(b"abc", 1000).unwrap().unwrap();
        k9::assert_equal!(m.start, 3);
        k9::assert_equal!(m.end, 3);
    }

    #[test]
    fn caret_not_at_start_is_literal() {
        // `^` only acts as an anchor at position 0 of the pattern.
        let m = find("a^b", b"xa^bx").unwrap();
        k9::assert_equal!(&b"xa^bx"[m.start..m.end], b"a^b");
    }

    #[test]
    fn dollar_not_at_end_is_literal() {
        let m = find("a$b", b"xa$bx").unwrap();
        k9::assert_equal!(&b"xa$bx"[m.start..m.end], b"a$b");
        // And `a$b` on input without literal `$` should not match.
        k9::assert_equal!(find("a$b", b"ab").is_none(), true);
    }

    // ---------------------------------------------------------------
    // Non-ASCII handling at the byte level.
    // ---------------------------------------------------------------

    #[test]
    fn high_bit_byte_is_not_alpha() {
        let hay = [0xFFu8];
        let p = Pattern::compile(b"%a").unwrap();
        k9::assert_equal!(p.find(&hay, 0).unwrap().is_none(), true);
    }

    #[test]
    fn high_bit_byte_is_not_space() {
        let hay = [0xA0u8]; // Latin-1 NBSP
        let p = Pattern::compile(b"%s").unwrap();
        k9::assert_equal!(p.find(&hay, 0).unwrap().is_none(), true);
    }

    // ---------------------------------------------------------------
    // Position-capture backrefs and the `is_anchored` helper.
    // ---------------------------------------------------------------

    #[test]
    fn position_capture_backref_fails_silently() {
        // In reference Lua, `()%1` treats the position capture's
        // length as a huge unsigned value so the compare fails and
        // the overall match returns no result (rather than raising
        // "invalid capture index").
        let p = Pattern::compile(b"()%1").unwrap();
        k9::assert_equal!(p.find(b"abc", 0).unwrap().is_none(), true);
    }

    #[test]
    fn is_anchored_detects_caret() {
        k9::assert_equal!(Pattern::compile(b"^abc").unwrap().is_anchored(), true);
        k9::assert_equal!(Pattern::compile(b"abc").unwrap().is_anchored(), false);
        // A `^` that's not at position 0 is a literal, not an anchor.
        k9::assert_equal!(Pattern::compile(b"a^b").unwrap().is_anchored(), false);
    }
}
