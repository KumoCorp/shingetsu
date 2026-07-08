use std::collections::HashSet;
use std::fmt::Write as _;

use bstr::ByteSlice as _;

use crate::value::Value;
use crate::GlobalEnv;
use shingetsu_vm::PrettyShape;

/// Controls how deeply and how broadly [`pretty_print`] renders tables.
///
/// Implements `LuaRepr` so it can be passed directly from Lua as
/// the `options` argument to `debug.pretty_print`.
#[derive(Clone, Debug, crate::LuaRepr)]
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
    /// Sort table entries by key before rendering.  Lua's iteration
    /// order is unspecified; sorting produces a deterministic
    /// rendering that is stable across runs and across changes to
    /// the table implementation.  Defaults to `true`.  Disable
    /// when you want to see the table's own iteration order.
    #[lua(default = true)]
    pub sort_keys: bool,
}

impl Default for PrettyPrintConfig {
    fn default() -> Self {
        Self {
            max_depth: 4,
            max_entries: 32,
            wrap_width: 60,
            indent: 2,
            sort_keys: true,
        }
    }
}

/// Pretty-print a [`Value`] as a human-readable string.
///
/// Tables are rendered recursively up to `config.max_depth` levels deep and
/// `config.max_entries` entries wide. Cycles are detected and rendered as
/// `<cycle>`.
///
/// `env` is used to lazily rebuild userdata that opts into structured
/// rendering via [`shingetsu_vm::Userdata::pretty_entries`] (e.g. the
/// snapshot-table proxies).
pub fn pretty_print(value: &Value, config: &PrettyPrintConfig, env: &GlobalEnv) -> String {
    let mut seen = HashSet::new();
    render(value, config, 0, &mut seen, env)
}

fn render(
    value: &Value,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
    env: &GlobalEnv,
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
        Value::Userdata(ud) => {
            let ptr = value.to_pointer();
            if seen.contains(&ptr) {
                return "<cycle>".to_string();
            }
            let type_label = ud.type_name();
            match ud.pretty_entries(env) {
                None => format!("{type_label}: {ptr:p}"),
                Some(Err(e)) => format!("<error rendering {type_label}: {e}>"),
                Some(Ok(shape)) => {
                    if depth >= config.max_depth {
                        return format!("{type_label} {{...}}");
                    }
                    seen.insert(ptr);
                    let body = match shape {
                        PrettyShape::Map(iter) => {
                            render_userdata_map(iter, config, depth, seen, env)
                        }
                        PrettyShape::Vec(iter) => {
                            render_userdata_vec(iter, config, depth, seen, env)
                        }
                    };
                    seen.remove(&ptr);
                    format!("{type_label} {body}")
                }
            }
        }
        Value::Table(t) => {
            let ptr = value.to_pointer();
            if seen.contains(&ptr) {
                return "<cycle>".to_string();
            }
            if depth >= config.max_depth {
                return "{...}".to_string();
            }
            seen.insert(ptr);
            let result = render_table(t, config, depth, seen, env);
            seen.remove(&ptr);
            result
        }
    }
}

fn render_userdata_map(
    iter: Box<dyn Iterator<Item = Result<(Value, Value), shingetsu_vm::VmError>> + '_>,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
    env: &GlobalEnv,
) -> String {
    let mut entries: Vec<(Value, Value)> = Vec::new();
    let mut truncated = false;
    for (i, item) in iter.enumerate() {
        if i >= config.max_entries {
            truncated = true;
            break;
        }
        match item {
            Ok(pair) => entries.push(pair),
            Err(e) => return format!("<error rendering entries: {e}>"),
        }
    }
    if entries.is_empty() {
        return if truncated {
            "{ … }".to_string()
        } else {
            "{}".to_string()
        };
    }
    if config.sort_keys {
        entries.sort_by(|(a, _), (b, _)| compare_keys(a, b));
    }
    render_entries(&entries, false, config, depth, seen, env, truncated)
}

fn render_userdata_vec(
    iter: Box<dyn Iterator<Item = Result<Value, shingetsu_vm::VmError>> + '_>,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
    env: &GlobalEnv,
) -> String {
    let mut entries: Vec<(Value, Value)> = Vec::new();
    let mut truncated = false;
    for (i, item) in iter.enumerate() {
        if i >= config.max_entries {
            truncated = true;
            break;
        }
        match item {
            Ok(v) => entries.push((Value::Integer((i + 1) as i64), v)),
            Err(e) => return format!("<error rendering entries: {e}>"),
        }
    }
    if entries.is_empty() {
        return if truncated {
            "{ … }".to_string()
        } else {
            "{}".to_string()
        };
    }
    render_entries(&entries, true, config, depth, seen, env, truncated)
}

/// Shared rendering tail for `(entries, is_array)` shape, used by
/// both `render_table` and the userdata renderers.  Produces the
/// final `{ ... }` or multi-line form.
fn render_entries(
    entries: &[(Value, Value)],
    is_array: bool,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
    env: &GlobalEnv,
    truncated: bool,
) -> String {
    let rendered_entries: Vec<String> = entries
        .iter()
        .map(|(k, v)| render_entry(k, v, is_array, config, depth, seen, env))
        .collect();

    let trailing = if truncated { ", …" } else { "" };

    let compact = format!("{{ {}{} }}", rendered_entries.join(", "), trailing);
    if !compact.contains('\n') && compact.len() <= config.wrap_width {
        return compact;
    }

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

fn render_table(
    table: &crate::table::Table,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
    env: &GlobalEnv,
) -> String {
    use shingetsu_vm::TableShape;

    let is_array = matches!(table.detect_shape(), Ok(TableShape::Vec { .. }));

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

    // Optionally sort entries by key for deterministic output.
    // Arrays already iterate in ascending integer order, so sorting
    // is a no-op for them; the path matters for record-style and
    // mixed-key tables.
    if config.sort_keys && !is_array {
        entries.sort_by(|(a, _), (b, _)| compare_keys(a, b));
    }

    let truncated = entries.len() > config.max_entries;
    let to_render = &entries[..entries.len().min(config.max_entries)];

    render_entries(to_render, is_array, config, depth, seen, env, truncated)
}

/// Render one `key = value` (or just `value` for arrays) entry.
fn render_entry(
    k: &Value,
    v: &Value,
    is_array: bool,
    config: &PrettyPrintConfig,
    depth: usize,
    seen: &mut HashSet<*const ()>,
    env: &GlobalEnv,
) -> String {
    let value_str = render(v, config, depth + 1, seen, env);
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
            let key_str = render(k, config, depth + 1, seen, env);
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

/// Compare two table keys for deterministic sort order.  Numbers
/// sort before strings sort before everything else; within a type
/// keys sort by their natural order (numeric, bytewise, then
/// pretty-printed form as a fallback for booleans / tables /
/// functions).
fn compare_keys(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn rank(v: &Value) -> u8 {
        match v {
            Value::Integer(_) | Value::Float(_) => 0,
            Value::String(_) => 1,
            _ => 2,
        }
    }
    let r = rank(a).cmp(&rank(b));
    if r != Ordering::Equal {
        return r;
    }
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (Value::Integer(x), Value::Float(y)) => {
            (*x as f64).partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Value::Float(x), Value::Integer(y)) => {
            x.partial_cmp(&(*y as f64)).unwrap_or(Ordering::Equal)
        }
        (Value::String(x), Value::String(y)) => x.as_ref().cmp(y.as_ref()),
        // Same-rank "other" keys: fall back to a stable but
        // arbitrary ordering by pointer identity.
        _ => (a.to_pointer() as usize).cmp(&(b.to_pointer() as usize)),
    }
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
        let env = GlobalEnv::new();
        pretty_print(value, &PrettyPrintConfig::default(), &env)
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
        k9::assert_equal!(pp(&Value::Float(1.42)), "1.42");
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
        let config = PrettyPrintConfig::default();
        let env = GlobalEnv::new();
        let out = pretty_print(&Value::Table(t), &config, &env);
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
