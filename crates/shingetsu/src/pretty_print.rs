use std::collections::HashSet;
use std::fmt::Write as _;

use bstr::ByteSlice as _;

use crate::value::Value;

/// Controls how deeply and how broadly [`pretty_print`] renders tables.
///
/// Implements `LuaTable` so it can be passed directly from Lua as
/// the `options` argument to `debug.pretty_print`.
#[derive(Clone, Debug, crate::LuaTable)]
pub struct PrettyPrintConfig {
    /// Maximum recursion depth for nested tables. At the cap, `{...}` is emitted.
    #[lua(default = 4)]
    pub max_depth: usize,
    /// Maximum number of table entries rendered before truncating with `, …`.
    #[lua(default = 32)]
    pub max_entries: usize,
    /// Threshold beyond which a table is rendered with one entry per
    /// line instead of inline.  Compared against the rendered
    /// width of the table including the surrounding `{ }` braces.
    /// Defaults to `60`.
    #[lua(default = 60)]
    pub wrap_width: usize,
    /// Number of spaces of indentation per nesting level when a
    /// table renders multi-line.  Defaults to `2`.
    #[lua(default = 2)]
    pub indent: usize,
}

impl Default for PrettyPrintConfig {
    fn default() -> Self {
        Self {
            max_depth: 4,
            max_entries: 32,
            wrap_width: 60,
            indent: 2,
        }
    }
}

/// Pretty-print a [`Value`] as a human-readable string.
///
/// Tables are rendered recursively up to `config.max_depth` levels deep and
/// `config.max_entries` entries wide. Cycles are detected and rendered as
/// `<cycle>`.
pub fn pretty_print(value: &Value, config: &PrettyPrintConfig) -> String {
    let mut seen = HashSet::new();
    render(value, config, 0, &mut seen)
}

fn render(
    value: &Value,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
) -> String {
    match value {
        Value::Nil => "nil".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => {
            // Match Lua's display convention: whole finite floats get ".0".
            if f.is_nan() {
                "nan".to_string()
            } else if f.fract() == 0.0 && f.is_finite() {
                format!("{f:.1}")
            } else {
                format!("{f}")
            }
        }
        Value::String(s) => {
            let raw: &[u8] = s.as_ref();
            let mut out = String::with_capacity(raw.len() + 2);
            out.push('"');
            let mut repl_buf = [0u8; 4];
            let repl_bytes = char::REPLACEMENT_CHARACTER
                .encode_utf8(&mut repl_buf)
                .as_bytes();
            for (start, end, c) in raw.char_indices() {
                if c == char::REPLACEMENT_CHARACTER && &raw[start..end] != repl_bytes {
                    // Invalid UTF-8: emit a hex escape for each byte in the run.
                    for &b in &raw[start..end] {
                        write!(out, "\\x{b:02x}").ok();
                    }
                } else {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if c.is_control() => {
                            write!(out, "\\u{{{:x}}}", c as u32).ok();
                        }
                        c => out.push(c),
                    }
                }
            }
            out.push('"');
            out
        }
        Value::Function(_) => format!("function: {:p}", value.to_pointer()),
        Value::Userdata(_) => format!("userdata: {:p}", value.to_pointer()),
        Value::Table(t) => {
            let ptr = value.to_pointer();
            if seen.contains(&ptr) {
                return "<cycle>".to_string();
            }
            if depth >= config.max_depth {
                return "{...}".to_string();
            }
            seen.insert(ptr);
            let result = render_table(t, config, depth, seen);
            seen.remove(&ptr);
            result
        }
    }
}

fn render_table(
    table: &crate::table::Table,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
) -> String {
    // Collect all entries via table.next().
    let mut entries: Vec<(Value, Value)> = Vec::new();
    let mut key = Value::Nil;
    loop {
        match table.next(&key) {
            Ok(Some((k, v))) => {
                key = k.clone();
                entries.push((k, v));
            }
            Ok(None) => break,
            // On error (shouldn't happen for well-formed tables), stop.
            Err(_) => break,
        }
    }

    if entries.is_empty() {
        return "{}".to_string();
    }

    // Detect whether all keys form a dense integer sequence 1..N.
    let is_array = entries
        .iter()
        .enumerate()
        .all(|(i, (k, _))| matches!(k, Value::Integer(n) if *n == i as i64 + 1));

    let truncated = entries.len() > config.max_entries;
    let to_render = &entries[..entries.len().min(config.max_entries)];

    // Pre-render each entry as `key = value` (or just `value` for
    // arrays) once.  Each rendered entry may itself be multi-line
    // when a nested table wrapped.
    let rendered_entries: Vec<String> = to_render
        .iter()
        .map(|(k, v)| render_entry(k, v, is_array, config, depth, seen))
        .collect();

    let trailing = if truncated { ", …" } else { "" };

    // Try compact (single-line) form first.
    let compact = format!("{{ {}{} }}", rendered_entries.join(", "), trailing);
    if !compact.contains('\n') && compact.len() <= config.wrap_width {
        return compact;
    }

    // Multi-line form: one entry per line, indented one step from
    // the table's opening brace.
    let entry_indent = " ".repeat(config.indent);
    let mut out = String::from("{\n");
    for entry in &rendered_entries {
        out.push_str(&entry_indent);
        out.push_str(&reindent(entry, &entry_indent));
        out.push_str(",\n");
    }
    if truncated {
        out.push_str(&entry_indent);
        out.push_str("…\n");
    }
    out.push('}');
    out
}

/// Render one `key = value` (or just `value` for arrays) entry.
fn render_entry(
    k: &Value,
    v: &Value,
    is_array: bool,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
) -> String {
    let value_str = render(v, config, depth + 1, seen);
    if is_array {
        return value_str;
    }
    match k {
        Value::String(s) if is_bare_key(s.as_ref()) => {
            // Safe: is_bare_key only passes ASCII identifier bytes.
            let ks = std::str::from_utf8(s.as_ref()).unwrap_or("?");
            format!("{ks} = {value_str}")
        }
        _ => {
            let key_str = render(k, config, depth + 1, seen);
            format!("[{key_str}] = {value_str}")
        }
    }
}

/// Add `prefix` to the start of every line after the first.  Used
/// to keep multi-line nested values aligned with their parent
/// entry's indentation.
fn reindent(text: &str, prefix: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(prefix);
        }
        out.push_str(line);
    }
    out
}

/// Returns true if `key` can be used as a bare Lua identifier in `key = val` syntax.
fn is_bare_key(key: &[u8]) -> bool {
    if key.is_empty() {
        return false;
    }
    let mut iter = key.iter();
    let first = *iter.next().unwrap();
    if !first.is_ascii_alphabetic() && first != b'_' {
        return false;
    }
    iter.all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::Table;

    fn pp(value: &Value) -> String {
        pretty_print(value, &PrettyPrintConfig::default())
    }

    #[test]
    fn nil_and_booleans() {
        k9::assert_equal!(pp(&Value::Nil), "nil");
        k9::assert_equal!(pp(&Value::Boolean(true)), "true");
        k9::assert_equal!(pp(&Value::Boolean(false)), "false");
    }

    #[test]
    fn integers() {
        k9::assert_equal!(pp(&Value::Integer(0)), "0");
        k9::assert_equal!(pp(&Value::Integer(-42)), "-42");
        k9::assert_equal!(pp(&Value::Integer(1_000_000)), "1000000");
    }

    #[test]
    fn floats() {
        k9::assert_equal!(pp(&Value::Float(1.0)), "1.0");
        k9::assert_equal!(pp(&Value::Float(3.14)), "3.14");
        k9::assert_equal!(pp(&Value::Float(f64::NAN)), "nan");
    }

    #[test]
    fn strings_simple() {
        k9::assert_equal!(pp(&Value::string("hello")), r#""hello""#);
    }

    #[test]
    fn strings_escape() {
        k9::assert_equal!(pp(&Value::string("a\"b\\c\nd")), r#""a\"b\\c\nd""#);
    }

    #[test]
    fn strings_invalid_utf8_hex_escape() {
        // \xa0 is a bare continuation byte - invalid UTF-8.
        let s = Value::string(vec![0xa0u8, b'h', b'e', b'l', b'l', b'o']);
        k9::assert_equal!(pp(&s), r#""\xa0hello""#);
    }

    #[test]
    fn strings_control_chars() {
        // \t, \n, \r get named escapes; other controls get \u{NN}.
        k9::assert_equal!(pp(&Value::string("\t")), r#""\t""#);
        k9::assert_equal!(pp(&Value::string("\x01")), r#""\u{1}""#);
        k9::assert_equal!(pp(&Value::string("\x7f")), r#""\u{7f}""#);
    }

    #[test]
    fn strings_valid_multibyte_utf8() {
        // Valid multi-byte UTF-8 passes through as the character.
        k9::assert_equal!(pp(&Value::string("caf\u{e9}")), "\"caf\u{e9}\"");
    }

    #[test]
    fn empty_table() {
        let t = Table::new();
        k9::assert_equal!(pp(&Value::Table(t)), "{}");
    }

    #[test]
    fn array_table() {
        let t = Table::new();
        t.raw_set(Value::Integer(1), Value::Integer(10)).unwrap();
        t.raw_set(Value::Integer(2), Value::Integer(20)).unwrap();
        t.raw_set(Value::Integer(3), Value::Integer(30)).unwrap();
        k9::assert_equal!(pp(&Value::Table(t)), "{ 10, 20, 30 }");
    }

    #[test]
    fn hash_table() {
        let t = Table::new();
        t.raw_set(Value::string("x"), Value::Integer(1)).unwrap();
        k9::assert_equal!(pp(&Value::Table(t)), r#"{ x = 1 }"#);
    }

    #[test]
    fn non_identifier_key() {
        let t = Table::new();
        t.raw_set(Value::string("hello world"), Value::Integer(1))
            .unwrap();
        k9::assert_equal!(pp(&Value::Table(t)), r#"{ ["hello world"] = 1 }"#);
    }

    #[test]
    fn nested_table() {
        let inner = Table::new();
        inner
            .raw_set(Value::string("a"), Value::Integer(1))
            .unwrap();
        let outer = Table::new();
        outer
            .raw_set(Value::string("inner"), Value::Table(inner))
            .unwrap();
        k9::assert_equal!(pp(&Value::Table(outer)), r#"{ inner = { a = 1 } }"#);
    }

    #[test]
    fn depth_cap() {
        // Build 5 levels of nesting; level 5 should be shown as {…}.
        let mut t = Table::new();
        for _ in 0..5 {
            let outer = Table::new();
            outer.raw_set(Value::string("n"), Value::Table(t)).unwrap();
            t = outer;
        }
        let out = pp(&Value::Table(t));
        // At depth 4 the innermost visible table should be collapsed.
        assert!(
            out.contains("{...}"),
            "expected depth cap marker, got: {out}"
        );
    }

    #[test]
    fn truncation() {
        let t = Table::new();
        for i in 1..=40i64 {
            t.raw_set(Value::Integer(i), Value::Integer(i)).unwrap();
        }
        let config = PrettyPrintConfig {
            max_depth: 4,
            max_entries: 32,
            wrap_width: 60,
            indent: 2,
        };
        let out = pretty_print(&Value::Table(t), &config);
        // Truncation marker: a `…` line at the end of the entry list,
        // immediately before the closing brace.  The exact line
        // content is asserted in full to follow the project's
        // "no partial-match assertions" rule — we check for the
        // last two lines of the rendering.
        let last_two = out.lines().rev().take(2).collect::<Vec<_>>();
        k9::assert_equal!(last_two, vec!["}", "  …"]);
    }

    #[test]
    fn cycle_detection() {
        let t = Table::new();
        // Point a key back to the table itself.
        t.raw_set(Value::string("self"), Value::Table(t.clone()))
            .unwrap();
        let out = pp(&Value::Table(t));
        assert!(out.contains("<cycle>"), "expected cycle marker, got: {out}");
    }
}
