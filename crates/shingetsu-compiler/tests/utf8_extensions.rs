//! Tests for the non-standard `utf8.*` extensions:
//! lossy / truncate / sub / reverse.

mod common;

use bstr::ByteSlice;
use common::{run_err, run_one};
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// utf8.is_valid
// ---------------------------------------------------------------------------

#[tokio::test]
async fn is_valid_ascii() {
    k9::assert_equal!(
        run_one("return utf8.is_valid('hello')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn is_valid_empty_string() {
    k9::assert_equal!(
        run_one("return utf8.is_valid('')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn is_valid_multibyte_codepoints() {
    k9::assert_equal!(
        run_one("return utf8.is_valid('h\u{00E9}llo')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn is_valid_emoji() {
    k9::assert_equal!(
        run_one("return utf8.is_valid('\u{1F600}\u{1F601}')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn is_valid_lone_invalid_byte() {
    // \xFF is never valid as any UTF-8 byte position.
    k9::assert_equal!(
        run_one("return utf8.is_valid('a\\xFFb')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn is_valid_truncated_multibyte_sequence() {
    // \xC2 is a valid lead byte for a 2-byte sequence, but it must
    // be followed by a continuation byte; alone it is invalid.
    k9::assert_equal!(
        run_one("return utf8.is_valid('\\xC2')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn is_valid_orphan_continuation_byte() {
    // \x80 is a continuation byte (10xxxxxx) appearing without a
    // preceding lead byte.
    k9::assert_equal!(
        run_one("return utf8.is_valid('\\x80')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn is_valid_lossy_output_is_always_valid() {
    // Round-trip: any input through lossy must be valid.
    let src = r#"
        local inputs = { "hello", "", "\xFF\xFE", "a\xC2b", "\x80\x80", "h\u{00E9}llo" }
        for _, s in ipairs(inputs) do
            assert(utf8.is_valid(utf8.lossy(s)), "lossy output not valid for: "..s)
        end
        return "ok"
    "#;
    k9::assert_equal!(run_one(src).await, Value::string("ok"));
}

#[tokio::test]
async fn is_valid_overlong_encoding() {
    // \xC0\x80 is an overlong encoding of NUL; UTF-8 rejects all
    // overlong forms.
    k9::assert_equal!(
        run_one("return utf8.is_valid('\\xC0\\x80')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn is_valid_surrogate_codepoint() {
    // \xED\xA0\x80 encodes U+D800, a UTF-16 surrogate; surrogates
    // are forbidden in UTF-8.
    k9::assert_equal!(
        run_one("return utf8.is_valid('\\xED\\xA0\\x80')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn is_valid_truncated_4_byte_sequence() {
    // \xF0\x9F\x98 is the first three bytes of a 4-byte sequence;
    // missing the final continuation byte.
    k9::assert_equal!(
        run_one("return utf8.is_valid('\\xF0\\x9F\\x98')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn is_valid_codepoint_above_max() {
    // \xF5\x80\x80\x80 would encode U+140000, above U+10FFFF.
    k9::assert_equal!(
        run_one("return utf8.is_valid('\\xF5\\x80\\x80\\x80')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn is_valid_bom_is_valid() {
    // U+FEFF (BOM) encoded as \xEF\xBB\xBF is valid UTF-8.
    k9::assert_equal!(
        run_one("return utf8.is_valid('\\xEF\\xBB\\xBFhello')").await,
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// utf8.lossy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lossy_valid_utf8_returns_unchanged() {
    k9::assert_equal!(
        run_one("return utf8.lossy('h\u{00E9}llo')").await,
        Value::string("h\u{00E9}llo")
    );
}

#[tokio::test]
async fn lossy_empty_string() {
    k9::assert_equal!(run_one("return utf8.lossy('')").await, Value::string(""));
}

#[tokio::test]
async fn lossy_ascii_unchanged() {
    k9::assert_equal!(
        run_one("return utf8.lossy('hello')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn lossy_replaces_lone_invalid_byte_with_fffd() {
    // \xFF is invalid as a UTF-8 lead byte; should become U+FFFD.
    k9::assert_equal!(
        run_one("return utf8.lossy('a\\xFFb')").await,
        Value::string("a\u{FFFD}b")
    );
}

#[tokio::test]
async fn lossy_invalid_run_becomes_single_fffd_per_max_subpart() {
    // From_utf8_lossy emits one U+FFFD per maximal invalid sub-sequence
    // (W3C "substitution of maximal subparts" rule), so three lone
    // invalid lead bytes produce three U+FFFD codepoints.
    let v = run_one("return utf8.lossy('\\xFF\\xFE\\xFD')").await;
    let Value::String(bytes) = v else {
        panic!("expected string, got {v:?}");
    };
    let text = bytes
        .as_ref()
        .to_str()
        .expect("lossy output must be valid UTF-8");
    k9::assert_equal!(text, "\u{FFFD}\u{FFFD}\u{FFFD}");
}

#[tokio::test]
async fn lossy_invalid_at_very_end() {
    let v = run_one("return utf8.lossy('hello\\xFF')").await;
    let Value::String(bytes) = v else {
        panic!("expected string, got {v:?}");
    };
    let text = bytes.as_ref().to_str().expect("must be valid UTF-8");
    k9::assert_equal!(text, "hello\u{FFFD}");
}

#[tokio::test]
async fn lossy_invalid_at_very_start() {
    let v = run_one("return utf8.lossy('\\xFFhello')").await;
    let Value::String(bytes) = v else {
        panic!("expected string, got {v:?}");
    };
    let text = bytes.as_ref().to_str().expect("must be valid UTF-8");
    k9::assert_equal!(text, "\u{FFFD}hello");
}

#[tokio::test]
async fn lossy_truncated_multibyte_at_end() {
    // \xC2 alone is a truncated 2-byte sequence at end-of-string;
    // should yield a single U+FFFD.
    let v = run_one("return utf8.lossy('hi\\xC2')").await;
    let Value::String(bytes) = v else {
        panic!("expected string, got {v:?}");
    };
    let text = bytes.as_ref().to_str().expect("must be valid UTF-8");
    k9::assert_equal!(text, "hi\u{FFFD}");
}

#[tokio::test]
async fn lossy_output_is_valid_utf8() {
    let v = run_one("return utf8.lossy('a\\xFFb\\xC2c')").await;
    let Value::String(bytes) = v else {
        panic!("expected string, got {v:?}");
    };
    let text = bytes
        .as_ref()
        .to_str()
        .expect("lossy output must be valid UTF-8");
    k9::assert_equal!(text, "a\u{FFFD}b\u{FFFD}c");
}

#[tokio::test]
async fn lossy_pipes_into_strict_utf8_functions() {
    // utf8.sub would error on invalid input; lossy normalises first.
    let src = r#"
        local s = "a\xFFb"
        return utf8.sub(utf8.lossy(s), 1, 1)
    "#;
    k9::assert_equal!(run_one(src).await, Value::string("a"));
}

// ---------------------------------------------------------------------------
// utf8.truncate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn utf8_truncate_ascii() {
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 3)").await,
        Value::string("hel")
    );
}

#[tokio::test]
async fn utf8_truncate_under_budget_returns_unchanged() {
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 100)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn utf8_truncate_at_exact_budget() {
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 5)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn utf8_truncate_multibyte_codepoints() {
    // "héllo" has 5 codepoints; cutting to 3 yields "hél".
    k9::assert_equal!(
        run_one("return utf8.truncate('h\u{00E9}llo', 3)").await,
        Value::string("h\u{00E9}l")
    );
}

#[tokio::test]
async fn utf8_truncate_never_cuts_mid_codepoint() {
    // Slice an emoji string at every char index; each result must be
    // valid UTF-8 -- the strict utf8.* functions would error otherwise.
    let src = r#"
        local s = "\u{1F600}\u{1F601}\u{1F602}\u{1F603}\u{1F604}"
        local count = 0
        for n = 0, 10 do
            local out = utf8.truncate(s, n)
            assert(utf8.is_valid(out), "truncated to "..n.." is not valid UTF-8")
            count = count + 1
        end
        return count
    "#;
    k9::assert_equal!(run_one(src).await, Value::Integer(11));
}

#[tokio::test]
async fn utf8_truncate_zero_chars() {
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 0)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn utf8_truncate_empty_string() {
    k9::assert_equal!(
        run_one("return utf8.truncate('', 5)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn utf8_truncate_with_ellipsis() {
    k9::assert_equal!(
        run_one("return utf8.truncate('hello world', 6, '...')").await,
        Value::string("hel...")
    );
}

#[tokio::test]
async fn utf8_truncate_no_truncation_omits_ellipsis() {
    k9::assert_equal!(
        run_one("return utf8.truncate('hi', 100, '...')").await,
        Value::string("hi")
    );
}

#[tokio::test]
async fn utf8_truncate_ellipsis_with_multibyte_chars() {
    // The ellipsis is itself a multi-byte UTF-8 char (3 bytes,
    // but 1 codepoint).
    k9::assert_equal!(
        run_one("return utf8.truncate('hello world', 4, '\u{2026}')").await,
        Value::string("hel\u{2026}")
    );
}

#[tokio::test]
async fn utf8_truncate_ellipsis_equal_to_budget() {
    // 3-codepoint ellipsis, 3-char budget: emit just the ellipsis.
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 3, '...')").await,
        Value::string("...")
    );
}

#[tokio::test]
async fn utf8_truncate_ellipsis_longer_than_budget() {
    // 3-codepoint ellipsis, 2-char budget: ellipsis itself truncated.
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 2, '...')").await,
        Value::string("..")
    );
}

#[tokio::test]
async fn utf8_truncate_zero_chars_with_ellipsis() {
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 0, '...')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn utf8_truncate_negative_chars_errors() {
    let err = run_err("return utf8.truncate('hello', -1)").await;
    k9::assert_equal!(
        err,
        "\
error: bad argument #2 to 'utf8.truncate' (non-negative integer expected, got -1)
 --> test.lua:1:31
  |
1 | return utf8.truncate('hello', -1)
  |                               ^^ bad argument #2 to 'utf8.truncate' (non-negative integer expected, got -1)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn utf8_truncate_invalid_ellipsis_errors() {
    let err = run_err("return utf8.truncate('hello world', 5, 'a\\xFFb')").await;
    k9::assert_equal!(
        err,
        "\
error: bad argument #3 to 'utf8.truncate' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
 --> test.lua:1:40
  |
1 | return utf8.truncate('hello world', 5, 'a\\xFFb')
  |                                        ^^^^^^^^ bad argument #3 to 'utf8.truncate' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn utf8_truncate_huge_max_chars_returns_unchanged() {
    // i64::MAX must not overflow; result is the input unchanged.
    k9::assert_equal!(
        run_one("return utf8.truncate('hello', 9223372036854775807)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn utf8_truncate_multibyte_ellipsis_at_budget_boundary() {
    // 3-codepoint multi-byte ellipsis, 3-char budget: emit the
    // ellipsis only, nothing of `s`.
    k9::assert_equal!(
        run_one("return utf8.truncate('hello world', 3, '\u{2026}\u{2026}\u{2026}')").await,
        Value::string("\u{2026}\u{2026}\u{2026}")
    );
}

#[tokio::test]
async fn utf8_truncate_invalid_utf8_errors() {
    let err = run_err("return utf8.truncate('a\\xFFb', 5)").await;
    k9::assert_equal!(
        err,
        "\
error: bad argument #1 to 'utf8.truncate' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
 --> test.lua:1:22
  |
1 | return utf8.truncate('a\\xFFb', 5)
  |                      ^^^^^^^^ bad argument #1 to 'utf8.truncate' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// utf8.sub
// ---------------------------------------------------------------------------

#[tokio::test]
async fn utf8_sub_ascii_basic() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 2, 4)").await,
        Value::string("ell")
    );
}

#[tokio::test]
async fn utf8_sub_default_j_is_minus_one() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 2)").await,
        Value::string("ello")
    );
}

#[tokio::test]
async fn utf8_sub_default_i_is_one() {
    // No-args isn't possible (i is positional), but explicit nil works
    // depending on calling conventions; just test j-only via 1, j.
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 1, 3)").await,
        Value::string("hel")
    );
}

#[tokio::test]
async fn utf8_sub_negative_i() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', -2)").await,
        Value::string("lo")
    );
}

#[tokio::test]
async fn utf8_sub_negative_j() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 1, -2)").await,
        Value::string("hell")
    );
}

#[tokio::test]
async fn utf8_sub_negative_both() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', -3, -2)").await,
        Value::string("ll")
    );
}

#[tokio::test]
async fn utf8_sub_multibyte_codepoints() {
    // "héllo" -- 5 codepoints, é at index 2.
    k9::assert_equal!(
        run_one("return utf8.sub('h\u{00E9}llo', 2, 2)").await,
        Value::string("\u{00E9}")
    );
    k9::assert_equal!(
        run_one("return utf8.sub('h\u{00E9}llo', 2, 4)").await,
        Value::string("\u{00E9}ll")
    );
}

#[tokio::test]
async fn utf8_sub_emoji() {
    // 5 emoji codepoints, each 4 bytes.
    k9::assert_equal!(
        run_one("return utf8.sub('\u{1F600}\u{1F601}\u{1F602}\u{1F603}\u{1F604}', 2, 4)").await,
        Value::string("\u{1F601}\u{1F602}\u{1F603}")
    );
}

#[tokio::test]
async fn utf8_sub_i_greater_than_j_returns_empty() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 4, 2)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn utf8_sub_i_past_end_returns_empty() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 10, 20)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn utf8_sub_negative_i_clamps_to_start() {
    // -100 on a 5-char string clamps to 1.
    k9::assert_equal!(
        run_one("return utf8.sub('hello', -100, 3)").await,
        Value::string("hel")
    );
}

#[tokio::test]
async fn utf8_sub_j_past_end_clamps() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 1, 100)").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn utf8_sub_i_zero_treated_as_one() {
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 0, 3)").await,
        Value::string("hel")
    );
}

#[tokio::test]
async fn utf8_sub_empty_string() {
    k9::assert_equal!(
        run_one("return utf8.sub('', 1, 5)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn utf8_sub_huge_positive_indices() {
    // i64::MAX must not overflow; both clamp into range.
    k9::assert_equal!(
        run_one("return utf8.sub('hello', 9223372036854775807, 9223372036854775807)").await,
        Value::string("")
    );
}

#[tokio::test]
async fn utf8_sub_huge_negative_indices() {
    // i64::MIN should clamp to start without overflowing during
    // negative-index resolution.
    k9::assert_equal!(
        run_one("return utf8.sub('hello', -9223372036854775808, 3)").await,
        Value::string("hel")
    );
}

#[tokio::test]
async fn utf8_sub_invalid_utf8_errors() {
    let err = run_err("return utf8.sub('a\\xFFb', 1, 2)").await;
    k9::assert_equal!(
        err,
        "\
error: bad argument #1 to 'utf8.sub' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
 --> test.lua:1:17
  |
1 | return utf8.sub('a\\xFFb', 1, 2)
  |                 ^^^^^^^^ bad argument #1 to 'utf8.sub' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn utf8_sub_panic_safety_every_index_pair() {
    // Sweep i in [-7, 7] and j in [-7, 7] over a 5-codepoint string;
    // confirm no panics and that results are valid UTF-8.
    let src = r#"
        local s = "h\u{E9}llo"
        local count = 0
        for i = -7, 7 do
            for j = -7, 7 do
                local out = utf8.sub(s, i, j)
                assert(utf8.is_valid(out))
                count = count + 1
            end
        end
        return count
    "#;
    k9::assert_equal!(run_one(src).await, Value::Integer(15 * 15));
}

// ---------------------------------------------------------------------------
// utf8.reverse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn utf8_reverse_ascii() {
    k9::assert_equal!(
        run_one("return utf8.reverse('hello')").await,
        Value::string("olleh")
    );
}

#[tokio::test]
async fn utf8_reverse_empty() {
    k9::assert_equal!(run_one("return utf8.reverse('')").await, Value::string(""));
}

#[tokio::test]
async fn utf8_reverse_single_char() {
    k9::assert_equal!(
        run_one("return utf8.reverse('a')").await,
        Value::string("a")
    );
}

#[tokio::test]
async fn utf8_reverse_multibyte_keeps_codepoints_intact() {
    k9::assert_equal!(
        run_one("return utf8.reverse('h\u{00E9}llo')").await,
        Value::string("oll\u{00E9}h")
    );
}

#[tokio::test]
async fn utf8_reverse_emoji() {
    k9::assert_equal!(
        run_one("return utf8.reverse('\u{1F600}\u{1F601}\u{1F602}')").await,
        Value::string("\u{1F602}\u{1F601}\u{1F600}")
    );
}

#[tokio::test]
async fn utf8_reverse_output_is_valid_utf8() {
    // Regression check: byte reversal would corrupt multi-byte
    // sequences; codepoint reversal must not.
    k9::assert_equal!(
        run_one("return utf8.is_valid(utf8.reverse('\\u{1F600}h\\u{00E9}llo'))").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn utf8_reverse_invalid_utf8_errors() {
    let err = run_err("return utf8.reverse('a\\xFFb')").await;
    k9::assert_equal!(
        err,
        "\
error: bad argument #1 to 'utf8.reverse' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
 --> test.lua:1:21
  |
1 | return utf8.reverse('a\\xFFb')
  |                     ^^^^^^^^ bad argument #1 to 'utf8.reverse' (valid UTF-8 string expected, got invalid UTF-8 at byte 2)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn utf8_reverse_double_reverse_is_identity() {
    let src = r#"
        local original = "h\u{E9}llo \u{1F600}"
        return utf8.reverse(utf8.reverse(original)) == original
    "#;
    k9::assert_equal!(run_one(src).await, Value::Boolean(true));
}
