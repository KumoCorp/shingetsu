//! Tests for the non-standard `string.*` extensions:
//! trim / starts_with / ends_with / strip_prefix / strip_suffix /
//! trim_prefix / trim_suffix / split_once / rsplit_once / truncate /
//! dedent / indent.

mod common;

use common::{run_all, run_one};
use shingetsu::valuevec;
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// trim / trim_start / trim_end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trim_basic() {
    k9::assert_equal!(
        run_one("return string.trim('  hello  ')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn trim_all_ascii_whitespace_chars() {
    // space, tab, newline, vertical-tab, form-feed, carriage-return
    k9::assert_equal!(
        run_one("return string.trim('\\t\\n\\x0B\\x0C\\r foo \\t\\n\\x0B\\x0C\\r ')").await,
        Value::string("foo")
    );
}

#[tokio::test]
async fn trim_empty_string() {
    k9::assert_equal!(run_one("return string.trim('')").await, Value::string(""));
}

#[tokio::test]
async fn trim_only_whitespace() {
    k9::assert_equal!(
        run_one("return string.trim('   \\t\\n   ')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn trim_no_whitespace() {
    k9::assert_equal!(
        run_one("return string.trim('hello')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn trim_preserves_internal_whitespace() {
    k9::assert_equal!(
        run_one("return string.trim('  hello  world  ')").await,
        Value::string("hello  world")
    );
}

#[tokio::test]
async fn trim_does_not_strip_non_ascii_whitespace() {
    // U+00A0 NBSP encoded as \xC2\xA0 -- must not be trimmed.
    k9::assert_equal!(
        run_one("return string.trim('\\xC2\\xA0hello\\xC2\\xA0')").await,
        Value::string("\u{00A0}hello\u{00A0}")
    );
}

#[tokio::test]
async fn trim_start_basic() {
    k9::assert_equal!(
        run_one("return string.trim_start('  hello  ')").await,
        Value::string("hello  ")
    );
}

#[tokio::test]
async fn trim_start_only_whitespace() {
    k9::assert_equal!(
        run_one("return string.trim_start('   ')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn trim_end_basic() {
    k9::assert_equal!(
        run_one("return string.trim_end('  hello  ')").await,
        Value::string("  hello")
    );
}

#[tokio::test]
async fn trim_method_syntax() {
    k9::assert_equal!(
        run_one("return ('  hi  '):trim()").await,
        Value::string("hi")
    );
}

// ---------------------------------------------------------------------------
// starts_with / ends_with
// ---------------------------------------------------------------------------

#[tokio::test]
async fn starts_with_true() {
    k9::assert_equal!(
        run_one("return string.starts_with('hello', 'he')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn starts_with_false() {
    k9::assert_equal!(
        run_one("return string.starts_with('hello', 'world')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn starts_with_empty_prefix_is_true() {
    k9::assert_equal!(
        run_one("return string.starts_with('hello', '')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn starts_with_empty_string_empty_prefix() {
    k9::assert_equal!(
        run_one("return string.starts_with('', '')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn starts_with_empty_string_non_empty_prefix() {
    k9::assert_equal!(
        run_one("return string.starts_with('', 'x')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn starts_with_prefix_longer_than_string() {
    k9::assert_equal!(
        run_one("return string.starts_with('hi', 'hello')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn starts_with_exact_match() {
    k9::assert_equal!(
        run_one("return string.starts_with('hello', 'hello')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn starts_with_binary_bytes() {
    // No UTF-8 assumptions.
    k9::assert_equal!(
        run_one("return string.starts_with('\\xFF\\xFE\\x00abc', '\\xFF\\xFE')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn ends_with_true() {
    k9::assert_equal!(
        run_one("return string.ends_with('hello', 'lo')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn ends_with_false() {
    k9::assert_equal!(
        run_one("return string.ends_with('hello', 'world')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn ends_with_empty_suffix_is_true() {
    k9::assert_equal!(
        run_one("return string.ends_with('hello', '')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn ends_with_suffix_longer_than_string() {
    k9::assert_equal!(
        run_one("return string.ends_with('hi', 'hello')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn starts_ends_with_method_syntax() {
    k9::assert_equal!(
        run_all("return ('hello'):starts_with('he'), ('hello'):ends_with('lo')").await,
        valuevec![Value::Boolean(true), Value::Boolean(true)]
    );
}

// ---------------------------------------------------------------------------
// strip_prefix / strip_suffix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strip_prefix_match() {
    k9::assert_equal!(
        run_one("return string.strip_prefix('hello.lua', 'hello')").await,
        Value::string(".lua")
    );
}

#[tokio::test]
async fn strip_prefix_no_match_returns_nil() {
    k9::assert_equal!(
        run_one("return string.strip_prefix('hello', 'world')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn strip_prefix_empty_prefix_returns_whole_string() {
    k9::assert_equal!(
        run_one("return string.strip_prefix('hello', '')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn strip_prefix_exact_match_returns_empty() {
    k9::assert_equal!(
        run_one("return string.strip_prefix('hello', 'hello')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn strip_prefix_longer_than_string() {
    k9::assert_equal!(
        run_one("return string.strip_prefix('hi', 'hello')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn strip_suffix_match() {
    k9::assert_equal!(
        run_one("return string.strip_suffix('hello.lua', '.lua')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn strip_suffix_no_match_returns_nil() {
    k9::assert_equal!(
        run_one("return string.strip_suffix('hello', 'world')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn strip_suffix_empty_suffix() {
    k9::assert_equal!(
        run_one("return string.strip_suffix('hello', '')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn strip_suffix_exact_match() {
    k9::assert_equal!(
        run_one("return string.strip_suffix('hello', 'hello')").await,
        Value::string("")
    );
}

// ---------------------------------------------------------------------------
// trim_prefix / trim_suffix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trim_prefix_match() {
    k9::assert_equal!(
        run_one("return string.trim_prefix('hello.lua', 'hello')").await,
        Value::string(".lua")
    );
}

#[tokio::test]
async fn trim_prefix_no_match_returns_input() {
    k9::assert_equal!(
        run_one("return string.trim_prefix('hello', 'world')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn trim_suffix_match() {
    k9::assert_equal!(
        run_one("return string.trim_suffix('hello.lua', '.lua')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn trim_suffix_no_match_returns_input() {
    k9::assert_equal!(
        run_one("return string.trim_suffix('hello', 'world')").await,
        Value::string("hello")
    );
}

// ---------------------------------------------------------------------------
// split_once / rsplit_once
// ---------------------------------------------------------------------------

#[tokio::test]
async fn split_once_basic() {
    k9::assert_equal!(
        run_all("return string.split_once('key=value', '=')").await,
        valuevec![Value::string("key"), Value::string("value")]
    );
}

#[tokio::test]
async fn split_once_uses_first_occurrence() {
    k9::assert_equal!(
        run_all("return string.split_once('a=b=c', '=')").await,
        valuevec![Value::string("a"), Value::string("b=c")]
    );
}

#[tokio::test]
async fn split_once_no_match_returns_nil() {
    k9::assert_equal!(
        run_one("return string.split_once('hello', 'x')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn split_once_at_start() {
    k9::assert_equal!(
        run_all("return string.split_once('=value', '=')").await,
        valuevec![Value::string(""), Value::string("value")]
    );
}

#[tokio::test]
async fn split_once_at_end() {
    k9::assert_equal!(
        run_all("return string.split_once('key=', '=')").await,
        valuevec![Value::string("key"), Value::string("")]
    );
}

#[tokio::test]
async fn split_once_empty_sep_matches_at_start() {
    k9::assert_equal!(
        run_all("return string.split_once('abc', '')").await,
        valuevec![Value::string(""), Value::string("abc")]
    );
}

#[tokio::test]
async fn split_once_empty_string_empty_sep() {
    k9::assert_equal!(
        run_all("return string.split_once('', '')").await,
        valuevec![Value::string(""), Value::string("")]
    );
}

#[tokio::test]
async fn split_once_empty_string_non_empty_sep() {
    k9::assert_equal!(
        run_one("return string.split_once('', 'x')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn split_once_multi_byte_separator() {
    k9::assert_equal!(
        run_all("return string.split_once('foo::bar::baz', '::')").await,
        valuevec![Value::string("foo"), Value::string("bar::baz")]
    );
}

#[tokio::test]
async fn rsplit_once_basic() {
    k9::assert_equal!(
        run_all("return string.rsplit_once('a/b/c.lua', '/')").await,
        valuevec![Value::string("a/b"), Value::string("c.lua")]
    );
}

#[tokio::test]
async fn rsplit_once_no_match_returns_nil() {
    k9::assert_equal!(
        run_one("return string.rsplit_once('hello', 'x')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn rsplit_once_empty_sep_matches_at_end() {
    k9::assert_equal!(
        run_all("return string.rsplit_once('abc', '')").await,
        valuevec![Value::string("abc"), Value::string("")]
    );
}

#[tokio::test]
async fn rsplit_once_multi_byte_separator() {
    k9::assert_equal!(
        run_all("return string.rsplit_once('foo::bar::baz', '::')").await,
        valuevec![Value::string("foo::bar"), Value::string("baz")]
    );
}

// ---------------------------------------------------------------------------
// truncate (byte-oriented)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn truncate_shorter_than_max_returns_unchanged() {
    k9::assert_equal!(
        run_one("return string.truncate('hello', 100)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn truncate_equal_length_returns_unchanged() {
    k9::assert_equal!(
        run_one("return string.truncate('hello', 5)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn truncate_cuts_at_max_bytes() {
    k9::assert_equal!(
        run_one("return string.truncate('hello world', 5)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn truncate_zero_bytes() {
    k9::assert_equal!(
        run_one("return string.truncate('hello', 0)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn truncate_zero_bytes_with_ellipsis() {
    // Ellipsis longer than budget -> byte-truncate ellipsis itself.
    k9::assert_equal!(
        run_one("return string.truncate('hello', 0, '...')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn truncate_empty_string() {
    k9::assert_equal!(
        run_one("return string.truncate('', 5)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn truncate_empty_string_zero_bytes() {
    k9::assert_equal!(
        run_one("return string.truncate('', 0)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn truncate_with_ellipsis() {
    k9::assert_equal!(
        run_one("return string.truncate('hello world', 8, '...')").await,
        Value::string("hello...")
    );
}

#[tokio::test]
async fn truncate_no_truncation_does_not_append_ellipsis() {
    k9::assert_equal!(
        run_one("return string.truncate('hi', 100, '...')").await,
        Value::string("hi")
    );
}

#[tokio::test]
async fn truncate_no_truncation_at_boundary() {
    k9::assert_equal!(
        run_one("return string.truncate('hello', 5, '...')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn truncate_ellipsis_longer_than_budget() {
    k9::assert_equal!(
        run_one("return string.truncate('hello', 2, '...')").await,
        Value::string("..")
    );
}

#[tokio::test]
async fn truncate_ellipsis_equal_to_budget() {
    // Ellipsis len == max_bytes: per the `>=` rule, emit just the
    // ellipsis itself, byte-truncated to max_bytes (no change since
    // they are equal).
    k9::assert_equal!(
        run_one("return string.truncate('hello', 3, '...')").await,
        Value::string("...")
    );
}

#[tokio::test]
async fn truncate_cuts_mid_utf8_sequence() {
    // Pure byte truncation may slice through a multi-byte char.
    // "héllo" is h \xC3 \xA9 l l o -- 6 bytes.  Cutting to 2 bytes
    // yields h \xC3 (invalid UTF-8) -- which is allowed for the
    // byte-oriented string.truncate.
    let v = run_one("return string.truncate('h\\xC3\\xA9llo', 2)").await;
    match v {
        Value::String(s) => {
            k9::assert_equal!(s.as_ref(), b"h\xC3");
        }
        other => panic!("expected string, got {other:?}"),
    }
}

#[tokio::test]
async fn truncate_negative_max_bytes_errors() {
    common::assert_runtime_error!(
        "return string.truncate('hello', -1)",
        "\
error: bad argument #2 to 'truncate' (non-negative integer expected, got -1)
 --> test.lua:1:33
  |
1 | return string.truncate('hello', -1)
  |                                 ^^ bad argument #2 to 'truncate' (non-negative integer expected, got -1)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

// ---------------------------------------------------------------------------
// dedent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dedent_basic_uniform_indent() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("    hello\n    world\n")"#).await,
        Value::string("hello\nworld\n")
    );
}

#[tokio::test]
async fn dedent_no_common_indent() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("hello\nworld\n")"#).await,
        Value::string("hello\nworld\n")
    );
}

#[tokio::test]
async fn dedent_partial_common_indent() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("    a\n      b\n")"#).await,
        Value::string("a\n  b\n")
    );
}

#[tokio::test]
async fn dedent_mixed_tab_and_space_collapses_to_empty_prefix() {
    // Tab and space are literal-byte different -- no common prefix.
    k9::assert_equal!(
        run_one(r#"return string.dedent("\tfoo\n  bar\n")"#).await,
        Value::string("\tfoo\n  bar\n")
    );
}

#[tokio::test]
async fn dedent_whitespace_only_lines_normalized() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("  a\n   \n  b\n")"#).await,
        Value::string("a\n\nb\n")
    );
}

#[tokio::test]
async fn dedent_whitespace_only_lines_do_not_constrain_prefix() {
    // The whitespace-only line "  " is shorter than the actual
    // indent ("    "), but it must not pull the common prefix down.
    k9::assert_equal!(
        run_one(r#"return string.dedent("    a\n  \n    b\n")"#).await,
        Value::string("a\n\nb\n")
    );
}

#[tokio::test]
async fn dedent_empty_string() {
    k9::assert_equal!(run_one("return string.dedent('')").await, Value::string(""));
}

#[tokio::test]
async fn dedent_single_line_no_terminator() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("    hello")"#).await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn dedent_preserves_crlf_terminators() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("  a\r\n  b\r\n")"#).await,
        Value::string("a\r\nb\r\n")
    );
}

#[tokio::test]
async fn dedent_only_newlines() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("\n\n\n")"#).await,
        Value::string("\n\n\n")
    );
}

#[tokio::test]
async fn dedent_trailing_no_newline() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("  a\n  b")"#).await,
        Value::string("a\nb")
    );
}

// ---------------------------------------------------------------------------
// indent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn indent_basic() {
    k9::assert_equal!(
        run_one(r#"return string.indent("a\nb\n", "> ")"#).await,
        Value::string("> a\n> b\n")
    );
}

#[tokio::test]
async fn indent_skips_blank_lines() {
    k9::assert_equal!(
        run_one(r#"return string.indent("a\n\nb\n", "> ")"#).await,
        Value::string("> a\n\n> b\n")
    );
}

#[tokio::test]
async fn indent_skips_whitespace_only_lines() {
    k9::assert_equal!(
        run_one(r#"return string.indent("a\n   \nb\n", "> ")"#).await,
        Value::string("> a\n   \n> b\n")
    );
}

#[tokio::test]
async fn indent_empty_prefix_is_identity() {
    k9::assert_equal!(
        run_one(r#"return string.indent("a\nb\n", "")"#).await,
        Value::string("a\nb\n")
    );
}

#[tokio::test]
async fn indent_empty_string() {
    k9::assert_equal!(
        run_one(r#"return string.indent("", "> ")"#).await,
        Value::string("")
    );
}

#[tokio::test]
async fn indent_no_trailing_newline() {
    k9::assert_equal!(
        run_one(r#"return string.indent("a\nb", "> ")"#).await,
        Value::string("> a\n> b")
    );
}

#[tokio::test]
async fn indent_preserves_crlf() {
    k9::assert_equal!(
        run_one(r#"return string.indent("a\r\nb\r\n", "> ")"#).await,
        Value::string("> a\r\n> b\r\n")
    );
}

#[tokio::test]
async fn indent_does_not_add_phantom_line_after_trailing_newline() {
    // A trailing "\n" should not produce an extra "> " line.
    let v = run_one(r#"return string.indent("a\n", "> ")"#).await;
    k9::assert_equal!(v, Value::string("> a\n"));
}

// ---------------------------------------------------------------------------
// Panic-safety smoke tests: feed slicing-heavy functions a variety of
// edge inputs and ensure the VM does not crash and returns a string.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn truncate_panic_safety_at_every_boundary() {
    // Run truncate at every byte length from 0..=#s and check we
    // never panic.  The exact bytes are checked by the dedicated
    // tests above; this is purely a panic-safety sweep.
    let src = r#"
        local s = "h\xC3\xA9llo"  -- 6 bytes, valid UTF-8
        local out = {}
        for n = 0, #s + 3 do
            out[#out+1] = string.truncate(s, n)
        end
        return #out
    "#;
    k9::assert_equal!(run_one(src).await, Value::Integer(10));
}

#[tokio::test]
async fn split_once_panic_safety_every_position() {
    // Walk the separator across every position of the haystack and
    // confirm no panic.
    let src = r#"
        local s = "abcdef"
        for i = 1, #s do
            local sep = string.sub(s, i, i)
            local a, b = string.split_once(s, sep)
            assert(a ~= nil and b ~= nil)
            assert(a .. sep .. b == s)
        end
        return "ok"
    "#;
    k9::assert_equal!(run_one(src).await, Value::string("ok"));
}

// ---------------------------------------------------------------------------
// Method-call syntax: confirms the string metatable installs every
// extension as a method on string values.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_syntax_trim_start_end() {
    k9::assert_equal!(
        run_all("return ('  hi  '):trim_start(), ('  hi  '):trim_end()").await,
        valuevec![Value::string("hi  "), Value::string("  hi")]
    );
}

#[tokio::test]
async fn method_syntax_strip_and_trim_prefix_suffix() {
    k9::assert_equal!(
        run_all(
            "return ('hi.lua'):strip_prefix('hi'), ('hi.lua'):strip_suffix('.lua'), \
             ('hi'):trim_prefix('x'), ('hi'):trim_suffix('x')"
        )
        .await,
        valuevec![
            Value::string(".lua"),
            Value::string("hi"),
            Value::string("hi"),
            Value::string("hi"),
        ]
    );
}

#[tokio::test]
async fn method_syntax_split_once_and_rsplit_once() {
    k9::assert_equal!(
        run_all("return ('a=b=c'):split_once('=')").await,
        valuevec![Value::string("a"), Value::string("b=c")]
    );
    k9::assert_equal!(
        run_all("return ('a/b/c'):rsplit_once('/')").await,
        valuevec![Value::string("a/b"), Value::string("c")]
    );
}

#[tokio::test]
async fn method_syntax_truncate_dedent_indent() {
    k9::assert_equal!(
        run_all(
            r#"return ("hello world"):truncate(5),
               ("    a\n    b\n"):dedent(),
               ("a\nb\n"):indent("> ")"#
        )
        .await,
        valuevec![
            Value::string("hello"),
            Value::string("a\nb\n"),
            Value::string("> a\n> b\n"),
        ]
    );
}

#[tokio::test]
async fn method_chaining_extensions() {
    k9::assert_equal!(
        run_one("return ('  hello  '):trim():upper()").await,
        Value::string("HELLO")
    );
}

// ---------------------------------------------------------------------------
// Extra edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn truncate_i64_min_errors() {
    // i64::MIN is negative and must not overflow during the negative
    // check (we compare before any cast to usize).
    common::assert_runtime_error!(
        "return string.truncate('hello', -9223372036854775808)",
        "\
error: bad argument #2 to 'truncate' (non-negative integer expected, got -9223372036854775808)
 --> test.lua:1:33
  |
1 | return string.truncate('hello', -9223372036854775808)
  |                                 ^^^^^^^^^^^^^^^^^^^^ bad argument #2 to 'truncate' (non-negative integer expected, got -9223372036854775808)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn truncate_huge_max_bytes_returns_unchanged() {
    // Very large max_bytes must not panic from a vec capacity hint.
    k9::assert_equal!(
        run_one("return string.truncate('hello', 9223372036854775807)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn dedent_only_whitespace_lines() {
    // No contributing line -> common prefix is empty; every line
    // gets normalised to empty.
    k9::assert_equal!(
        run_one(r#"return string.dedent("   \n  \n    \n")"#).await,
        Value::string("\n\n\n")
    );
}

#[tokio::test]
async fn dedent_only_whitespace_no_terminator() {
    k9::assert_equal!(
        run_one(r#"return string.dedent("   ")"#).await,
        Value::string("")
    );
}

#[tokio::test]
async fn indent_prefix_with_tab() {
    k9::assert_equal!(
        run_one(r#"return string.indent("a\nb\n", "\t")"#).await,
        Value::string("\ta\n\tb\n")
    );
}

#[tokio::test]
async fn split_once_separator_at_exact_end() {
    k9::assert_equal!(
        run_all("return string.split_once('abc=', '=')").await,
        valuevec![Value::string("abc"), Value::string("")]
    );
}

#[tokio::test]
async fn rsplit_once_separator_at_exact_start() {
    k9::assert_equal!(
        run_all("return string.rsplit_once('=abc', '=')").await,
        valuevec![Value::string(""), Value::string("abc")]
    );
}

#[tokio::test]
async fn strip_prefix_binary_bytes() {
    k9::assert_equal!(
        run_one("return string.strip_prefix('\\xFF\\xFEdata', '\\xFF\\xFE')").await,
        Value::string("data")
    );
}

#[tokio::test]
async fn trim_only_internal_whitespace_returns_unchanged() {
    // No leading or trailing whitespace -> input returned unchanged
    // (this exercises the no-allocation short-circuit; correctness
    // is observable, the optimisation is not).
    k9::assert_equal!(
        run_one("return string.trim('a  b')").await,
        Value::string("a  b")
    );
}

#[tokio::test]
async fn truncate_ellipsis_at_max_bytes_exact_boundary() {
    // s.len() == max_bytes: no truncation, ellipsis ignored.
    k9::assert_equal!(
        run_one("return string.truncate('hello', 5, '...')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn dedent_partial_overlap_tab_then_spaces() {
    // "\t  a" and "\t   b" -> common is "\t  "
    k9::assert_equal!(
        run_one(r#"return string.dedent("\t  a\n\t   b\n")"#).await,
        Value::string("a\n b\n")
    );
}

#[tokio::test]
async fn dedent_panic_safety_random_indents() {
    // Mix of empty lines, ws-only lines, varying indents, no trailing newline.
    let src = r#"
        local cases = {
            "",
            "\n",
            "\n\n",
            "a",
            "  a",
            "  a\n  b",
            "  a\n  b\n",
            "  a\n    b\n  c\n",
            "  \n  a\n  ",
            "\ta\n\tb",
            "\ta\n  b",
            "\r\n",
            "  a\r\n  b\r\n",
        }
        for _, c in ipairs(cases) do
            local _ = string.dedent(c)
        end
        return #cases
    "#;
    k9::assert_equal!(run_one(src).await, Value::Integer(13));
}
