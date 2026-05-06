use std::sync::LazyLock;

use full_moon::tokenizer::{
    InterpolatedStringKind, Lexer, LexerResult, StringLiteralQuoteType, Symbol, TokenType,
};
use termwiz::cell::{AttributeChange, Intensity};
use termwiz::color::{ColorAttribute, SrgbaTuple};
use termwiz::surface::change::Change;

// ---------------------------------------------------------------------------
// TokenStyle
// ---------------------------------------------------------------------------

/// Rendering attributes for a single token class.
#[derive(Clone, Debug, Default)]
pub struct TokenStyle {
    pub color: ColorAttribute,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
}

impl TokenStyle {
    /// Emit termwiz `Change` values that apply this style.
    pub fn to_changes(&self) -> Vec<Change> {
        let intensity = if self.bold {
            Intensity::Bold
        } else if self.dim {
            Intensity::Half
        } else {
            Intensity::Normal
        };
        vec![
            Change::Attribute(AttributeChange::Foreground(self.color)),
            Change::Attribute(AttributeChange::Intensity(intensity)),
            Change::Attribute(AttributeChange::Italic(self.italic)),
        ]
    }
}

// ---------------------------------------------------------------------------
// TokenClass
// ---------------------------------------------------------------------------

/// The semantic class of a Lua token, independent of rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenClass {
    Keyword,
    StringLiteral,
    Number,
    Comment,
    Operator,
    Punctuation,
    Whitespace,
    Other,
}

/// Classify a `full_moon` token type into a [`TokenClass`].
pub fn classify_token(tt: &TokenType) -> TokenClass {
    match tt {
        TokenType::Symbol { symbol } => classify_symbol(symbol),
        TokenType::StringLiteral { .. } | TokenType::InterpolatedString { .. } => {
            TokenClass::StringLiteral
        }
        TokenType::Number { .. } => TokenClass::Number,
        TokenType::SingleLineComment { .. } | TokenType::MultiLineComment { .. } => {
            TokenClass::Comment
        }
        TokenType::Whitespace { .. } => TokenClass::Whitespace,
        _ => TokenClass::Other,
    }
}

fn classify_symbol(s: &Symbol) -> TokenClass {
    match s {
        Symbol::And
        | Symbol::Break
        | Symbol::Do
        | Symbol::Else
        | Symbol::ElseIf
        | Symbol::End
        | Symbol::False
        | Symbol::For
        | Symbol::Function
        | Symbol::Goto
        | Symbol::If
        | Symbol::In
        | Symbol::Local
        | Symbol::Nil
        | Symbol::Not
        | Symbol::Or
        | Symbol::Repeat
        | Symbol::Return
        | Symbol::Then
        | Symbol::True
        | Symbol::Until
        | Symbol::While => TokenClass::Keyword,

        Symbol::LeftParen
        | Symbol::RightParen
        | Symbol::LeftBracket
        | Symbol::RightBracket
        | Symbol::LeftBrace
        | Symbol::RightBrace
        | Symbol::Comma
        | Symbol::Semicolon
        | Symbol::Dot
        | Symbol::Colon => TokenClass::Punctuation,

        _ => TokenClass::Operator,
    }
}

// ---------------------------------------------------------------------------
// HighlightTheme
// ---------------------------------------------------------------------------

/// A set of [`TokenStyle`] values that control how each token class is rendered.
#[derive(Clone, Debug)]
pub struct HighlightTheme {
    pub keyword: TokenStyle,
    pub string: TokenStyle,
    pub escape: TokenStyle,
    pub number: TokenStyle,
    pub comment: TokenStyle,
    pub operator: TokenStyle,
    pub punctuation: TokenStyle,
}

impl Default for HighlightTheme {
    fn default() -> Self {
        Self::named("dark").expect("dark theme is always present")
    }
}

/// Parse a `#RRGGBB` or other `SrgbaTuple`-supported color string into a
/// `ColorAttribute`. Panics if `s` is not a valid color string — only called
/// with compile-time literals inside `THEMES`.
fn tc(s: &str) -> ColorAttribute {
    let color: SrgbaTuple = s.parse().expect("valid color string");
    ColorAttribute::TrueColorWithDefaultFallback(color)
}

fn palette(idx: u8) -> ColorAttribute {
    ColorAttribute::PaletteIndex(idx)
}

static THEMES: LazyLock<Vec<(&'static str, HighlightTheme)>> = LazyLock::new(|| {
    vec![
        (
            "plain",
            HighlightTheme {
                keyword: TokenStyle::default(),
                string: TokenStyle::default(),
                escape: TokenStyle::default(),
                number: TokenStyle::default(),
                comment: TokenStyle::default(),
                operator: TokenStyle::default(),
                punctuation: TokenStyle::default(),
            },
        ),
        (
            "basic",
            HighlightTheme {
                keyword: TokenStyle {
                    bold: true,
                    ..Default::default()
                },
                string: TokenStyle {
                    italic: true,
                    ..Default::default()
                },
                escape: TokenStyle {
                    bold: true,
                    italic: true,
                    ..Default::default()
                },
                number: TokenStyle::default(),
                comment: TokenStyle {
                    dim: true,
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle::default(),
                punctuation: TokenStyle::default(),
            },
        ),
        (
            "dark",
            HighlightTheme {
                keyword: TokenStyle {
                    color: palette(3),
                    bold: true,
                    ..Default::default()
                },
                string: TokenStyle {
                    color: palette(2),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#E8A45A"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: palette(6),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: palette(8),
                    italic: true,
                    dim: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: palette(7),
                    ..Default::default()
                },
                punctuation: TokenStyle::default(),
            },
        ),
        (
            "light",
            HighlightTheme {
                keyword: TokenStyle {
                    color: palette(4),
                    bold: true,
                    ..Default::default()
                },
                string: TokenStyle {
                    color: palette(2),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#D06000"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: palette(5),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: palette(8),
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: palette(0),
                    ..Default::default()
                },
                punctuation: TokenStyle::default(),
            },
        ),
        (
            "monokai",
            HighlightTheme {
                keyword: TokenStyle {
                    color: tc("#F92672"),
                    ..Default::default()
                },
                string: TokenStyle {
                    color: tc("#E6DB74"),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#AE81FF"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: tc("#AE81FF"),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: tc("#75715E"),
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: tc("#F92672"),
                    ..Default::default()
                },
                punctuation: TokenStyle::default(),
            },
        ),
        (
            "nord",
            HighlightTheme {
                keyword: TokenStyle {
                    color: tc("#81A1C1"),
                    ..Default::default()
                },
                string: TokenStyle {
                    color: tc("#A3BE8C"),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#EBCB8B"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: tc("#B48EAD"),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: tc("#4C566A"),
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: tc("#81A1C1"),
                    ..Default::default()
                },
                punctuation: TokenStyle {
                    color: tc("#88C0D0"),
                    ..Default::default()
                },
            },
        ),
        (
            "one-dark",
            HighlightTheme {
                keyword: TokenStyle {
                    color: tc("#C678DD"),
                    ..Default::default()
                },
                string: TokenStyle {
                    color: tc("#98C379"),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#E5C07B"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: tc("#D19A66"),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: tc("#5C6370"),
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: tc("#56B6C2"),
                    ..Default::default()
                },
                punctuation: TokenStyle {
                    color: tc("#ABB2BF"),
                    ..Default::default()
                },
            },
        ),
        (
            "dracula",
            HighlightTheme {
                keyword: TokenStyle {
                    color: tc("#FF79C6"),
                    ..Default::default()
                },
                string: TokenStyle {
                    color: tc("#F1FA8C"),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#FFB86C"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: tc("#BD93F9"),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: tc("#6272A4"),
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: tc("#FF79C6"),
                    ..Default::default()
                },
                punctuation: TokenStyle {
                    color: tc("#F8F8F2"),
                    ..Default::default()
                },
            },
        ),
        (
            "solarized-dark",
            HighlightTheme {
                keyword: TokenStyle {
                    color: tc("#268BD2"),
                    ..Default::default()
                },
                string: TokenStyle {
                    color: tc("#2AA198"),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#CB4B16"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: tc("#D33682"),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: tc("#586E75"),
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: tc("#859900"),
                    ..Default::default()
                },
                punctuation: TokenStyle {
                    color: tc("#839496"),
                    ..Default::default()
                },
            },
        ),
        (
            "solarized-light",
            HighlightTheme {
                keyword: TokenStyle {
                    color: tc("#268BD2"),
                    ..Default::default()
                },
                string: TokenStyle {
                    color: tc("#2AA198"),
                    ..Default::default()
                },
                escape: TokenStyle {
                    color: tc("#CB4B16"),
                    ..Default::default()
                },
                number: TokenStyle {
                    color: tc("#D33682"),
                    ..Default::default()
                },
                comment: TokenStyle {
                    color: tc("#93A1A1"),
                    italic: true,
                    ..Default::default()
                },
                operator: TokenStyle {
                    color: tc("#859900"),
                    ..Default::default()
                },
                punctuation: TokenStyle {
                    color: tc("#657B83"),
                    ..Default::default()
                },
            },
        ),
    ]
});

impl HighlightTheme {
    /// Look up a theme by name. Returns `None` if the name is not recognised.
    pub fn named(name: &str) -> Option<Self> {
        THEMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| t.clone())
    }

    /// Iterate over all available theme names in declaration order.
    pub fn theme_names() -> impl Iterator<Item = &'static str> {
        THEMES.iter().map(|(n, _)| *n)
    }

    fn style_for(&self, class: TokenClass) -> Option<&TokenStyle> {
        match class {
            TokenClass::Keyword => Some(&self.keyword),
            TokenClass::StringLiteral => Some(&self.string),
            TokenClass::Number => Some(&self.number),
            TokenClass::Comment => Some(&self.comment),
            TokenClass::Operator => Some(&self.operator),
            TokenClass::Punctuation => Some(&self.punctuation),
            TokenClass::Whitespace | TokenClass::Other => None,
        }
    }
}

// ---------------------------------------------------------------------------
// highlight_lua helpers
// ---------------------------------------------------------------------------

/// Split a quoted string literal token (including surrounding quote chars)
/// into `(text, is_escape)` pairs. Long strings (`multi_line.is_some()`) do
/// not process escape sequences and should not be passed here.
///
/// Handles `\a \b \f \n \r \t \v \\ \' \"`, decimal `\ddd`,
/// hex `\xXX`, unicode `\u{...}`, and whitespace-skip `\z`.
fn split_string_escapes<'a>(text: &'a str) -> Vec<(&'a str, bool)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return vec![];
    }
    let quote = bytes[0];
    if quote != b'"' && quote != b'\'' {
        return vec![(text, false)];
    }
    // If the token has a matching closing quote, stop scanning before it so
    // the closing quote is always emitted as a non-escape segment.
    let content_end = if len > 1 && bytes[len - 1] == quote {
        len - 1
    } else {
        len
    };

    let mut result = Vec::new();
    let mut pos = 1; // skip opening quote
    let mut seg_start = 0; // include the opening quote in the first segment

    while pos < content_end {
        if bytes[pos] != b'\\' {
            pos += 1;
            continue;
        }
        // Flush the non-escape text seen so far (including opening quote on
        // the very first segment).
        if pos > seg_start {
            result.push((&text[seg_start..pos], false));
        }
        let esc_start = pos;
        pos += 1; // consume '\'
        if pos < content_end {
            match bytes[pos] {
                b'x' => {
                    // \xXX — exactly 2 hex digits
                    pos += 1;
                    let mut n = 0;
                    while pos < content_end && bytes[pos].is_ascii_hexdigit() && n < 2 {
                        pos += 1;
                        n += 1;
                    }
                }
                b'u' => {
                    // \u{XXXX}
                    pos += 1;
                    if pos < content_end && bytes[pos] == b'{' {
                        pos += 1;
                        while pos < content_end && bytes[pos] != b'}' {
                            pos += 1;
                        }
                        if pos < content_end {
                            pos += 1; // consume '}'
                        }
                    }
                }
                b'0'..=b'9' => {
                    // \ddd — up to 3 decimal digits
                    let mut n = 0;
                    while pos < content_end && bytes[pos].is_ascii_digit() && n < 3 {
                        pos += 1;
                        n += 1;
                    }
                }
                b'z' => {
                    // \z — skip following whitespace
                    pos += 1;
                    while pos < content_end && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
                        pos += 1;
                    }
                }
                _ => {
                    // Single-char: \n \t \r \\ \' \" etc.
                    pos += 1;
                }
            }
        }
        result.push((&text[esc_start..pos], true));
        seg_start = pos;
    }

    // Remaining text including the closing quote (if present).
    if seg_start < len {
        result.push((&text[seg_start..], false));
    }
    result
}

/// Emit changes for an interpolated-string token, splitting the embedded
/// `{` and `}` delimiters out so they can be colored separately from the
/// string content.
fn highlight_interpolated(
    kind: &InterpolatedStringKind,
    text: &str,
    theme: &HighlightTheme,
) -> Vec<Change> {
    let mut out = Vec::new();

    // Helper closures to keep the match arms concise.
    let push_str = |out: &mut Vec<Change>, s: &str| {
        out.extend(theme.string.to_changes());
        out.push(Change::Text(s.to_string()));
    };
    let push_delim = |out: &mut Vec<Change>, s: &str| {
        out.extend(theme.operator.to_changes());
        out.push(Change::Text(s.to_string()));
    };

    match kind {
        InterpolatedStringKind::Simple => {
            push_str(&mut out, text);
        }
        InterpolatedStringKind::Begin => {
            // e.g. "`hello {"  — trailing `{` is a delimiter
            match text.rfind('{') {
                Some(i) => {
                    push_str(&mut out, &text[..i]);
                    push_delim(&mut out, &text[i..]);
                }
                None => push_str(&mut out, text),
            }
        }
        InterpolatedStringKind::End => {
            // e.g. "}world`"  — leading `}` is a delimiter
            match text.find('}') {
                Some(i) => {
                    push_delim(&mut out, &text[..=i]);
                    push_str(&mut out, &text[i + 1..]);
                }
                None => push_str(&mut out, text),
            }
        }
        InterpolatedStringKind::Middle => {
            // e.g. "} mid {"  — leading `}`, string content, trailing `{`
            match text.find('}') {
                Some(close) => {
                    push_delim(&mut out, &text[..=close]);
                    let rest = &text[close + 1..];
                    match rest.rfind('{') {
                        Some(open) => {
                            push_str(&mut out, &rest[..open]);
                            push_delim(&mut out, &rest[open..]);
                        }
                        None => push_str(&mut out, rest),
                    }
                }
                None => push_str(&mut out, text),
            }
        }
    }

    out
}

// ---------------------------------------------------------------------------
// highlight_lua
// ---------------------------------------------------------------------------

/// Syntax-highlight a single line of Lua source, returning termwiz `Change`s.
///
/// Only the current edit line is tokenised; multi-line strings or comments
/// that span continuation lines will not highlight correctly.
pub fn highlight_lua(line: &str, theme: &HighlightTheme) -> Vec<Change> {
    let lua_version = full_moon::LuaVersion::lua55().with_luau();
    let tokens = match Lexer::new(line, lua_version).collect() {
        LexerResult::Ok(tokens) | LexerResult::Recovered(tokens, _) => tokens,
        LexerResult::Fatal(_) => return vec![Change::Text(line.to_string())],
    };

    let mut changes = Vec::new();
    for token in &tokens {
        let text = token.to_string();
        if text.is_empty() {
            continue;
        }
        match token.token_type() {
            TokenType::InterpolatedString { kind, .. } => {
                changes.extend(highlight_interpolated(kind, &text, theme));
            }
            TokenType::StringLiteral { quote_type, .. } => {
                match *quote_type {
                    // Long strings ([[…]]) don't process escape sequences.
                    StringLiteralQuoteType::Brackets => {
                        changes.extend(theme.string.to_changes());
                        changes.push(Change::Text(text));
                    }
                    // Any quoted form (Single, Double, or future variants) may
                    // contain escape sequences.
                    StringLiteralQuoteType::Single | StringLiteralQuoteType::Double | _ => {
                        for (segment, is_escape) in split_string_escapes(&text) {
                            let style = if is_escape {
                                &theme.escape
                            } else {
                                &theme.string
                            };
                            changes.extend(style.to_changes());
                            changes.push(Change::Text(segment.to_string()));
                        }
                    }
                }
            }
            tt => {
                let class = classify_token(tt);
                match theme.style_for(class) {
                    Some(style) => changes.extend(style.to_changes()),
                    None => {
                        // Reset so unclassified tokens don't inherit the
                        // style of the preceding highlighted token.
                        changes.push(Change::Attribute(AttributeChange::Foreground(
                            ColorAttribute::Default,
                        )));
                        changes.push(Change::Attribute(AttributeChange::Intensity(
                            Intensity::Normal,
                        )));
                        changes.push(Change::Attribute(AttributeChange::Italic(false)));
                    }
                }
                changes.push(Change::Text(text));
            }
        }
    }

    // Reset attributes after the last token.
    changes.push(Change::Attribute(AttributeChange::Foreground(
        ColorAttribute::Default,
    )));
    changes.push(Change::Attribute(AttributeChange::Intensity(
        Intensity::Normal,
    )));
    changes.push(Change::Attribute(AttributeChange::Italic(false)));

    changes
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_tokens(source: &str) -> Vec<(String, TokenClass)> {
        let ver = full_moon::LuaVersion::lua55().with_luau();
        let tokens = match Lexer::new(source, ver).collect() {
            LexerResult::Ok(t) | LexerResult::Recovered(t, _) => t,
            LexerResult::Fatal(_) => return vec![],
        };
        tokens
            .into_iter()
            .filter_map(|t| {
                let tt = t.token_type();
                if matches!(tt, TokenType::Whitespace { .. } | TokenType::Eof) {
                    None
                } else {
                    Some((t.to_string(), classify_token(tt)))
                }
            })
            .collect()
    }

    #[test]
    fn classify_simple_assignment() {
        k9::assert_equal!(
            classify_tokens("local x = 42"),
            vec![
                ("local".to_string(), TokenClass::Keyword),
                ("x".to_string(), TokenClass::Other),
                ("=".to_string(), TokenClass::Operator),
                ("42".to_string(), TokenClass::Number),
            ]
        );
    }

    #[test]
    fn classify_function_definition() {
        k9::assert_equal!(
            classify_tokens("function add(a, b) return a + b end"),
            vec![
                ("function".to_string(), TokenClass::Keyword),
                ("add".to_string(), TokenClass::Other),
                ("(".to_string(), TokenClass::Punctuation),
                ("a".to_string(), TokenClass::Other),
                (",".to_string(), TokenClass::Punctuation),
                ("b".to_string(), TokenClass::Other),
                (")".to_string(), TokenClass::Punctuation),
                ("return".to_string(), TokenClass::Keyword),
                ("a".to_string(), TokenClass::Other),
                ("+".to_string(), TokenClass::Operator),
                ("b".to_string(), TokenClass::Other),
                ("end".to_string(), TokenClass::Keyword),
            ]
        );
    }

    #[test]
    fn classify_string_and_comment() {
        k9::assert_equal!(
            classify_tokens(r#"local s = "hello" -- greet"#),
            vec![
                ("local".to_string(), TokenClass::Keyword),
                ("s".to_string(), TokenClass::Other),
                ("=".to_string(), TokenClass::Operator),
                ("\"hello\"".to_string(), TokenClass::StringLiteral),
                ("-- greet".to_string(), TokenClass::Comment),
            ]
        );
    }

    #[test]
    fn classify_luau_interpolated_string() {
        // `Hello {name}!` — lexer splits into Begin / Identifier / End
        let tokens = classify_tokens("`Hello {name}!`");
        k9::assert_equal!(
            tokens,
            vec![
                ("`Hello {".to_string(), TokenClass::StringLiteral),
                ("name".to_string(), TokenClass::Other),
                ("}!`".to_string(), TokenClass::StringLiteral),
            ]
        );
    }

    #[test]
    fn split_escapes_basic() {
        // Lua source: "hello\nworld" (backslash-n is two chars in source)
        k9::assert_equal!(
            split_string_escapes("\"hello\\nworld\""),
            vec![("\"hello", false), ("\\n", true), ("world\"", false),]
        );
    }

    #[test]
    fn split_escapes_hex_and_unicode() {
        // Lua source: "\x41\u{1F600}"
        k9::assert_equal!(
            split_string_escapes("\"\\x41\\u{1F600}\""),
            vec![
                ("\"", false),
                ("\\x41", true),
                ("\\u{1F600}", true),
                ("\"", false),
            ]
        );
    }

    #[test]
    fn split_escapes_no_escapes() {
        k9::assert_equal!(
            split_string_escapes("\"plain\""),
            vec![("\"plain\"", false)]
        );
    }

    #[test]
    fn all_theme_names_resolve() {
        for name in HighlightTheme::theme_names() {
            assert!(
                HighlightTheme::named(name).is_some(),
                "theme {name:?} did not resolve"
            );
        }
    }

    #[test]
    fn default_theme_is_dark() {
        let default = HighlightTheme::default();
        // Dark theme uses palette(3) for keywords.
        k9::assert_equal!(default.keyword.color, ColorAttribute::PaletteIndex(3));
    }
}
