//! Integration tests for the `regex` standard library.
//!
//! Doc-example execution covers the success-path basics
//! (`regex.compile`, `regex.compile_bytes`, `regex.escape`); this
//! file exercises the surface area that isn't expressible in a
//! single doc snippet (iterators, callback replacements, options
//! plumbing) and the negative paths (non-UTF-8 haystacks, malformed
//! patterns, unsupported features per engine).

mod common;

use common::run_with;
use shingetsu::{Libraries, Value};

const LIBS: Libraries = Libraries::SANDBOXED;

// ===========================================================================
// Fancy engine — captures, options, errors
// ===========================================================================

#[tokio::test]
async fn captures_returns_whole_match_at_index_zero() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(\\d{4})-(\\d{2})")
        local c = re:captures("date: 2024-08")
        return c:get(0), c:get(1), c:get(2), c:len()
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::string("2024-08"),
            Value::string("2024"),
            Value::string("08"),
            Value::Integer(3),
        ]
    );
}

#[tokio::test]
async fn captures_offsets_are_one_based_inclusive_end() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(\\d+)")
        local c = re:captures("abc 42 xyz")
        return c:start(0), c:end_(0), c:start(1), c:end_(1)
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::Integer(5),
            Value::Integer(6),
            Value::Integer(5),
            Value::Integer(6),
        ]
    );
}

#[tokio::test]
async fn captures_named_group_lookup() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(?<year>\\d{4})-(?<month>\\d{2})")
        local c = re:captures("2024-08")
        return c:by_name("year"), c:by_name("month"), c:name(1), c:name(2), c:by_name("missing")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::string("2024"),
            Value::string("08"),
            Value::string("year"),
            Value::string("month"),
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn captures_returns_nil_when_no_match() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("\\d+")
        return re:captures("no numbers here")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(result.into_iter().collect::<Vec<_>>(), vec![Value::Nil]);
}

#[tokio::test]
async fn capture_names_and_count() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(?<a>\\d+)-(\\w+)-(?<c>\\S+)")
        local names = re:capture_names()
        return re:capture_count(), names[1], names[2], names[3]
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::Integer(3),
            Value::string("a"),
            Value::Nil,
            Value::string("c"),
        ]
    );
}

#[tokio::test]
async fn find_iter_yields_every_match() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("\\w+")
        local words = {}
        for s, e, w in re:find_iter("the quick brown fox") do
            table.insert(words, string.format("%d:%d:%s", s, e, w))
        end
        return table.concat(words, "|")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("1:3:the|5:9:quick|11:15:brown|17:19:fox")]
    );
}

#[tokio::test]
async fn captures_iter_yields_userdata_per_match() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(\\w+)=(\\d+)")
        local out = {}
        for c in re:captures_iter("a=1 b=22 c=333") do
            table.insert(out, c:get(1) .. ":" .. c:get(2))
        end
        return table.concat(out, ",")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("a:1,b:22,c:333")]
    );
}

// ===========================================================================
// Replacement paths
// ===========================================================================

#[tokio::test]
async fn replace_string_template_expands_groups() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(\\w+) (\\w+)")
        return re:replace_all("alice bob carol dave", "$2 $1")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("bob alice dave carol")]
    );
}

#[tokio::test]
async fn replace_string_template_handles_named_and_dollar_literal() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(?<name>\\w+)")
        return re:replace("hello world", "$$${name}!")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("$hello! world")]
    );
}

#[tokio::test]
async fn replace_default_n_is_one() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("foo")
        return re:replace("foo foo foo", "BAR")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("BAR foo foo")]
    );
}

#[tokio::test]
async fn replace_with_function_callback() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("\\d+")
        return re:replace_all("a=1 b=22 c=333", function(c)
            return tostring(tonumber(c:get(0)) * 10)
        end)
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("a=10 b=220 c=3330")]
    );
}

#[tokio::test]
async fn replace_with_table_lookup() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("\\w+")
        return re:replace_all("alpha beta gamma", {
            alpha = "A", beta = "B", gamma = "G",
        })
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("A B G")]
    );
}

#[tokio::test]
async fn replace_function_returning_nil_keeps_original() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("\\d+")
        return re:replace_all("a=1 b=22", function(c)
            local n = tonumber(c:get(0))
            if n > 10 then return tostring(n * 2) end
            return nil
        end)
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("a=1 b=44")]
    );
}

// ===========================================================================
// Split
// ===========================================================================

#[tokio::test]
async fn split_breaks_on_pattern() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("[ \\t]+")
        local parts = re:split("a b \t  c\td    e")
        return parts[1], parts[2], parts[3], parts[4], parts[5], #parts
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
            Value::string("d"),
            Value::string("e"),
            Value::Integer(5),
        ]
    );
}

#[tokio::test]
async fn split_with_limit_keeps_remainder_in_last_element() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile(",")
        local parts = re:split("a,b,c,d,e", 3)
        return parts[1], parts[2], parts[3], #parts
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::string("a"),
            Value::string("b"),
            Value::string("c,d,e"),
            Value::Integer(3),
        ]
    );
}

// ===========================================================================
// Options
// ===========================================================================

#[tokio::test]
async fn opts_case_insensitive_matches_mixed_case() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("HELLO", { case_insensitive = true })
        return re:is_match("hello world"), re:is_match("HELLO WORLD")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::Boolean(true), Value::Boolean(true)]
    );
}

#[tokio::test]
async fn opts_multi_line_anchors_per_line() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("^a", { multi_line = true })
        local count = 0
        for _ in re:find_iter("a\nb\na\nc") do count = count + 1 end
        return count
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::Integer(2)]
    );
}

// ===========================================================================
// Errors
// ===========================================================================

#[tokio::test]
async fn compile_rejects_malformed_pattern() {
    common::assert_runtime_error_with_env!(
        common::build_env(LIBS),
        r#"return regex.compile("(unclosed")"#,
        "\
error: bad argument #1 to 'regex.compile' (a valid regex pattern expected, got Parsing error at position 9: Opening parenthesis without closing parenthesis)
 --> test.lua:1:22
  |
1 | return regex.compile(\"(unclosed\")
  |                      ^^^^^^^^^^^ bad argument #1 to 'regex.compile' (a valid regex pattern expected, got Parsing error at position 9: Opening parenthesis without closing parenthesis)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn fancy_haystack_must_be_utf8() {
    common::assert_runtime_error_with_env!(
        common::build_env(LIBS),
        "local re = regex.compile(\"\\\\w+\")\nreturn re:is_match(\"\\xff\\xfe\")",
        "\
error: bad argument #2 to 'Regex:is_match' (valid UTF-8 string expected, got invalid UTF-8 at byte 1)
 --> test.lua:2:20
  |
2 | return re:is_match(\"\\xff\\xfe\")
  |                    ^^^^^^^^^^ bad argument #2 to 'Regex:is_match' (valid UTF-8 string expected, got invalid UTF-8 at byte 1)
stack traceback:
\ttest.lua:2: in main chunk",
    );
}

#[tokio::test]
async fn replace_rejects_non_string_callback_return() {
    common::assert_runtime_error_with_env!(
        common::build_env(LIBS),
        "local re = regex.compile(\"\\\\d+\")\nreturn re:replace_all(\"1 2 3\", function() return {} end)",
        "\
error: bad argument #3 to 'Regex:replace' (string, number, false, or nil expected, got table)
 --> test.lua:2:32
  |
2 | return re:replace_all(\"1 2 3\", function() return {} end)
  |                                ^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #3 to 'Regex:replace' (string, number, false, or nil expected, got table)
stack traceback:
\ttest.lua:2: in main chunk",
    );
}

// ===========================================================================
// Bytes engine
// ===========================================================================

#[tokio::test]
async fn bytes_engine_accepts_arbitrary_bytes() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile_bytes("\\xff(\\xfe+)")
        local c = re:captures("\xff\xfe\xfe text")
        return c:get(0), c:get(1)
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::string(b"\xff\xfe\xfe".as_slice()),
            Value::string(b"\xfe\xfe".as_slice()),
        ]
    );
}

#[tokio::test]
async fn bytes_engine_rejects_backreferences() {
    common::assert_runtime_error_with_env!(
        common::build_env(LIBS),
        r#"return regex.compile_bytes("(\\w+)\\1")"#,
        "\
error: bad argument #1 to 'regex.compile_bytes' (a valid regex pattern expected, got regex parse error:
           (\\w+)\\1
                ^^
       error: backreferences are not supported)
 --> test.lua:1:28
  |
1 | return regex.compile_bytes(\"(\\\\w+)\\\\1\")
  |                            ^^^^^^^^^^^ bad argument #1 to 'regex.compile_bytes' (a valid regex pattern expected, got regex parse error: ...
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn bytes_engine_replace_and_split() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile_bytes("\\d+")
        local replaced = re:replace_all("a=1 b=22 c=333", "N")
        local parts = re:split("a=1,b=22,c=333", 2)
        return replaced, parts[1], parts[2], #parts
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::string("a=N b=N c=N"),
            Value::string("a="),
            Value::string(",b=22,c=333"),
            Value::Integer(2),
        ]
    );
}

// ===========================================================================
// escape, is_valid
// ===========================================================================

#[tokio::test]
async fn escape_makes_a_pattern_match_literally() {
    let result = run_with(
        LIBS,
        r#"
        local pat = regex.escape("1.2+3")
        local re = regex.compile(pat)
        return re:is_match("1.2+3"), re:is_match("1a2b3"), pat
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::string("1\\.2\\+3"),
        ]
    );
}

// ===========================================================================
// Captures:expand
// ===========================================================================

#[tokio::test]
async fn captures_expand_substitutes_groups() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(?<first>\\w+) (?<last>\\w+)")
        local c = re:captures("alice smith")
        return c:expand("${last}, ${first} ($0)")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("smith, alice (alice smith)")]
    );
}

#[tokio::test]
async fn captures_expand_unknown_name_is_empty_string() {
    let result = run_with(
        LIBS,
        r#"
        local re = regex.compile("(\\w+)")
        local c = re:captures("hello")
        return c:expand("[$1]<${missing}>")
        "#,
        |_| {},
    )
    .await
    .expect("run");
    k9::assert_equal!(
        result.into_iter().collect::<Vec<_>>(),
        vec![Value::string("[hello]<>")]
    );
}
