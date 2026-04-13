//! Translate Lua pattern syntax into a `regex::bytes::Regex`.
//!
//! Lua patterns are *not* regular expressions — they have a different
//! syntax for character classes (`%d` instead of `\d`), a lazy quantifier
//! (`-` instead of `*?`), and no alternation.  This module converts a Lua
//! pattern byte string into an equivalent regex string that can be compiled
//! with `regex::bytes::Regex`.
//!
//! Reference: <https://www.lua.org/manual/5.4/manual.html#6.4.1>

use regex::bytes::Regex;

/// Errors that can occur while translating a Lua pattern.
#[derive(Debug)]
pub struct PatternError {
    pub message: String,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Translate a Lua pattern (as raw bytes) to a compiled `Regex`.
///
/// The pattern is translated to a regex string and then compiled.
/// Returns an error if the pattern is malformed or uses an unsupported
/// feature (e.g. `%bxy` balanced match).
pub fn compile(pattern: &[u8]) -> Result<Regex, PatternError> {
    let regex_str = translate(pattern)?;
    Regex::new(&regex_str).map_err(|e| PatternError {
        message: format!("regex compilation error: {}", e),
    })
}

/// Translate a Lua pattern to a regex string.
fn translate(pattern: &[u8]) -> Result<String, PatternError> {
    let mut out = String::with_capacity(pattern.len() * 2);
    let mut i = 0;
    let len = pattern.len();

    // Handle leading anchor.
    let anchored_start = len > 0 && pattern[0] == b'^';
    if anchored_start {
        out.push('^');
        i = 1;
    }

    while i < len {
        // Handle trailing `$` anchor.
        if pattern[i] == b'$' && i + 1 == len {
            out.push('$');
            i += 1;
            continue;
        }

        let (class_regex, next_i, quantifiable) = translate_class(pattern, i)?;

        // Check for a quantifier following a quantifiable class.
        // Quantifiers only apply to single character classes, escapes,
        // sets, and `.` — not to `(` or `)`.
        if quantifiable && next_i < len {
            match pattern[next_i] {
                b'*' => {
                    out.push_str(&class_regex);
                    out.push('*');
                    i = next_i + 1;
                }
                b'+' => {
                    out.push_str(&class_regex);
                    out.push('+');
                    i = next_i + 1;
                }
                b'-' => {
                    // Lua `-` is a lazy `*` (match as few as possible).
                    out.push_str(&class_regex);
                    out.push_str("*?");
                    i = next_i + 1;
                }
                b'?' => {
                    out.push_str(&class_regex);
                    out.push('?');
                    i = next_i + 1;
                }
                _ => {
                    out.push_str(&class_regex);
                    i = next_i;
                }
            }
        } else {
            out.push_str(&class_regex);
            i = next_i;
        }
    }

    Ok(out)
}

/// Translate a single "class" element at position `i` in the pattern.
///
/// A class is one of:
/// - A `%x` escape (character class or literal)
/// - A `[set]` or `[^set]`
/// - A `(` or `)` (capture group delimiters)
/// - `.` (any byte)
/// - A literal character
///
/// Returns `(regex_fragment, next_index, quantifiable)`.  The third
/// element is `true` when the element can be followed by a Lua
/// quantifier (`*`, `+`, `-`, `?`).
fn translate_class(pattern: &[u8], i: usize) -> Result<(String, usize, bool), PatternError> {
    let len = pattern.len();
    match pattern[i] {
        b'%' => {
            if i + 1 >= len {
                return Err(PatternError {
                    message: "malformed pattern (ends with '%')".to_owned(),
                });
            }
            let next = pattern[i + 1];
            match next {
                // Balanced match — not supported.
                b'b' => Err(PatternError {
                    message: "balanced match (%b) is not supported".to_owned(),
                }),
                // Frontier pattern — not supported.
                b'f' => Err(PatternError {
                    message: "frontier pattern (%f) is not supported".to_owned(),
                }),
                _ => {
                    let frag = percent_class(next);
                    Ok((frag, i + 2, true))
                }
            }
        }
        b'[' => {
            let (frag, next) = translate_set(pattern, i)?;
            Ok((frag, next, true))
        }
        // Capture group delimiters are NOT quantifiable.
        b'(' => Ok(("(".to_owned(), i + 1, false)),
        b')' => Ok((")".to_owned(), i + 1, false)),
        b'.' => Ok(("(?s:.)".to_owned(), i + 1, true)),
        // Characters that are special in regex and need escaping.
        ch if is_regex_meta(ch) => {
            let mut s = String::with_capacity(2);
            s.push('\\');
            s.push(ch as char);
            Ok((s, i + 1, true))
        }
        ch => Ok(((ch as char).to_string(), i + 1, true)),
    }
}

/// Translate a `[set]` or `[^set]` at position `i`.
fn translate_set(pattern: &[u8], i: usize) -> Result<(String, usize), PatternError> {
    let len = pattern.len();
    let mut j = i + 1; // skip '['
    let mut out = String::from("[");

    // Handle complemented set `[^...]`.
    if j < len && pattern[j] == b'^' {
        out.push('^');
        j += 1;
    }

    // A `]` right after `[` or `[^` is treated as a literal.
    if j < len && pattern[j] == b']' {
        out.push_str("\\]");
        j += 1;
    }

    while j < len && pattern[j] != b']' {
        if pattern[j] == b'%' {
            if j + 1 >= len {
                return Err(PatternError {
                    message: "malformed pattern (ends with '%' inside set)".to_owned(),
                });
            }
            let next = pattern[j + 1];
            out.push_str(&percent_class_in_set(next));
            j += 2;
        } else if j + 2 < len && pattern[j + 1] == b'-' && pattern[j + 2] != b']' {
            // Range: `a-z`.
            let lo = pattern[j];
            let hi = pattern[j + 2];
            if is_regex_meta_in_set(lo) {
                out.push('\\');
            }
            out.push(lo as char);
            out.push('-');
            if is_regex_meta_in_set(hi) {
                out.push('\\');
            }
            out.push(hi as char);
            j += 3;
        } else {
            let ch = pattern[j];
            if is_regex_meta_in_set(ch) {
                out.push('\\');
            }
            out.push(ch as char);
            j += 1;
        }
    }

    if j >= len {
        return Err(PatternError {
            message: "malformed pattern (missing ']')".to_owned(),
        });
    }

    out.push(']');
    Ok((out, j + 1)) // +1 to skip ']'
}

/// Convert a `%x` escape to a regex fragment (outside a character set).
fn percent_class(ch: u8) -> String {
    match ch {
        // Predefined character classes.
        b'a' => "[a-zA-Z]".to_owned(),
        b'A' => "[^a-zA-Z]".to_owned(),
        b'd' => "[0-9]".to_owned(),
        b'D' => "[^0-9]".to_owned(),
        b'l' => "[a-z]".to_owned(),
        b'L' => "[^a-z]".to_owned(),
        b'u' => "[A-Z]".to_owned(),
        b'U' => "[^A-Z]".to_owned(),
        b'w' => "[a-zA-Z0-9]".to_owned(),
        b'W' => "[^a-zA-Z0-9]".to_owned(),
        b's' => "[ \\t\\n\\r\\x0B\\x0C]".to_owned(),
        b'S' => "[^ \\t\\n\\r\\x0B\\x0C]".to_owned(),
        b'p' => "[!-/:-@\\[-`{-~]".to_owned(),
        b'P' => "[^!-/:-@\\[-`{-~]".to_owned(),
        b'c' => "[\\x00-\\x1F\\x7F]".to_owned(),
        b'C' => "[^\\x00-\\x1F\\x7F]".to_owned(),
        // `%x` where x is not alphanumeric is a literal escape.
        _ => {
            let c = ch as char;
            if is_regex_meta(ch) {
                format!("\\{}", c)
            } else {
                c.to_string()
            }
        }
    }
}

/// Convert a `%x` escape to a regex fragment inside a `[set]`.
fn percent_class_in_set(ch: u8) -> String {
    match ch {
        b'a' => "a-zA-Z".to_owned(),
        b'A' => "^a-zA-Z".to_owned(), // Note: `^` inside [] after position 0 is literal in regex
        b'd' => "0-9".to_owned(),
        b'D' => "^0-9".to_owned(),
        b'l' => "a-z".to_owned(),
        b'L' => "^a-z".to_owned(),
        b'u' => "A-Z".to_owned(),
        b'U' => "^A-Z".to_owned(),
        b'w' => "a-zA-Z0-9".to_owned(),
        b'W' => "^a-zA-Z0-9".to_owned(),
        b's' => " \\t\\n\\r\\x0B\\x0C".to_owned(),
        b'S' => "^ \\t\\n\\r\\x0B\\x0C".to_owned(),
        b'p' => "!-/:-@\\[-`{-~".to_owned(),
        b'P' => "^!-/:-@\\[-`{-~".to_owned(),
        b'c' => "\\x00-\\x1F\\x7F".to_owned(),
        b'C' => "^\\x00-\\x1F\\x7F".to_owned(),
        _ => {
            let c = ch as char;
            if is_regex_meta_in_set(ch) {
                format!("\\{}", c)
            } else {
                c.to_string()
            }
        }
    }
}

/// Is this byte a regex metacharacter (outside `[...]`)?
fn is_regex_meta(ch: u8) -> bool {
    matches!(
        ch,
        b'\\'
            | b'.'
            | b'^'
            | b'$'
            | b'*'
            | b'+'
            | b'?'
            | b'('
            | b')'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'|'
    )
}

/// Is this byte a regex metacharacter inside `[...]`?
fn is_regex_meta_in_set(ch: u8) -> bool {
    matches!(ch, b'\\' | b']' | b'^' | b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: translate a Lua pattern and return the regex string.
    fn tr(pat: &str) -> String {
        translate(pat.as_bytes()).expect("pattern should translate")
    }

    #[test]
    fn literal() {
        assert_eq!(tr("hello"), "hello");
    }

    #[test]
    fn anchors() {
        assert_eq!(tr("^hello$"), "^hello$");
    }

    #[test]
    fn percent_classes() {
        assert_eq!(tr("%d+"), "[0-9]+");
        assert_eq!(tr("%a"), "[a-zA-Z]");
        assert_eq!(tr("%S+"), "[^ \\t\\n\\r\\x0B\\x0C]+");
    }

    #[test]
    fn dot() {
        assert_eq!(tr(".+"), "(?s:.)+");
    }

    #[test]
    fn lazy_quantifier() {
        assert_eq!(tr(".-"), "(?s:.)*?");
    }

    #[test]
    fn captures() {
        assert_eq!(tr("(%d+)"), "([0-9]+)");
        assert_eq!(
            tr("(%a+)%s+(%d+)"),
            "([a-zA-Z]+)[ \\t\\n\\r\\x0B\\x0C]+([0-9]+)"
        );
    }

    #[test]
    fn set() {
        assert_eq!(tr("[abc]"), "[abc]");
        assert_eq!(tr("[^abc]"), "[^abc]");
        assert_eq!(tr("[a-z0-9]"), "[a-z0-9]");
    }

    #[test]
    fn set_with_class() {
        assert_eq!(tr("[%d%a]"), "[0-9a-zA-Z]");
    }

    #[test]
    fn escaped_special() {
        assert_eq!(tr("%."), "\\.");
        assert_eq!(tr("%%"), "%");
        assert_eq!(tr("%["), "\\[");
    }

    #[test]
    fn literal_close_bracket_in_set() {
        assert_eq!(tr("[]abc]"), "[\\]abc]");
    }

    #[test]
    fn regex_compiles() {
        compile(b"%d+").expect("should compile");
        compile(b"(%a+)%s*=%s*(%d+)").expect("should compile");
        compile(b"[%w_]+").expect("should compile");
    }

    #[test]
    fn date_pattern_captures() {
        let regex = tr("(%d+)-(%d+)-(%d+)");
        assert_eq!(regex, "([0-9]+)-([0-9]+)-([0-9]+)");
        let re = compile(b"(%d+)-(%d+)-(%d+)").unwrap();
        let m = re.captures(b"2025-04-13").unwrap();
        assert_eq!(m.get(0).unwrap().as_bytes(), b"2025-04-13");
        assert_eq!(m.get(1).unwrap().as_bytes(), b"2025");
        assert_eq!(m.get(2).unwrap().as_bytes(), b"04");
        assert_eq!(m.get(3).unwrap().as_bytes(), b"13");
    }

    #[test]
    fn balanced_unsupported() {
        let err = compile(b"%bxy").unwrap_err();
        assert!(err.message.contains("balanced match"));
    }

    #[test]
    fn frontier_unsupported() {
        let err = compile(b"%f[%a]").unwrap_err();
        assert!(err.message.contains("frontier pattern"));
    }

    #[test]
    fn malformed_trailing_percent() {
        let err = compile(b"hello%").unwrap_err();
        assert!(err.message.contains("ends with '%'"));
    }

    #[test]
    fn malformed_missing_bracket() {
        let err = compile(b"[abc").unwrap_err();
        assert!(err.message.contains("missing ']'"));
    }

    #[test]
    fn malformed_percent_in_set() {
        let err = compile(b"[%").unwrap_err();
        assert!(err.message.contains("ends with '%' inside set"));
    }

    #[test]
    fn percent_class_lowercase() {
        assert_eq!(tr("%l+"), "[a-z]+");
        assert_eq!(tr("%L"), "[^a-z]");
    }

    #[test]
    fn percent_class_uppercase() {
        assert_eq!(tr("%u+"), "[A-Z]+");
        assert_eq!(tr("%U"), "[^A-Z]");
    }

    #[test]
    fn percent_class_alphanumeric() {
        assert_eq!(tr("%w+"), "[a-zA-Z0-9]+");
        assert_eq!(tr("%W"), "[^a-zA-Z0-9]");
    }

    #[test]
    fn percent_class_punctuation() {
        assert_eq!(tr("%p"), "[!-/:-@\\[-`{-~]");
        assert_eq!(tr("%P"), "[^!-/:-@\\[-`{-~]");
        // Verify it actually matches punctuation.
        let re = compile(b"%p+").unwrap();
        assert!(re.is_match(b"!@#"));
        assert!(!re.is_match(b"abc"));
    }

    #[test]
    fn percent_class_control() {
        assert_eq!(tr("%c"), "[\\x00-\\x1F\\x7F]");
        assert_eq!(tr("%C"), "[^\\x00-\\x1F\\x7F]");
        // Verify it matches control characters.
        let re = compile(b"%c+").unwrap();
        assert!(re.is_match(b"\x01\x1f"));
        assert!(!re.is_match(b"abc"));
    }

    #[test]
    fn quantifier_question_mark() {
        assert_eq!(tr("%d?"), "[0-9]?");
    }

    #[test]
    fn quantifier_plus() {
        assert_eq!(tr("%a+"), "[a-zA-Z]+");
    }

    #[test]
    fn quantifier_star() {
        assert_eq!(tr("%s*"), "[ \\t\\n\\r\\x0B\\x0C]*");
    }

    #[test]
    fn complemented_set_with_class() {
        assert_eq!(tr("[^%d]"), "[^0-9]");
        let re = compile(b"[^%d]+").unwrap();
        assert!(re.is_match(b"abc"));
        let m = re.find(b"abc123").unwrap();
        assert_eq!(m.as_bytes(), b"abc");
    }

    #[test]
    fn set_with_range_involving_meta() {
        // Range from hyphen: `[%-/]` should match `-` and `/`.
        let re = compile(b"[%-/]+").unwrap();
        assert!(re.is_match(b"-./"));
        assert!(!re.is_match(b"abc"));
    }
}
