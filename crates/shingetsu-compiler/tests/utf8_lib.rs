mod common;

use bytes::Bytes;
use common::{run_all, run_err, run_one};
use shingetsu_vm::Value;

// utf8 library
// ===========================================================================

#[test]
fn utf8_char_basic() {
    k9::assert_equal!(
        run_one("return utf8.char(72, 101, 108, 108, 111)"),
        Value::string("Hello")
    );
}

#[test]
fn utf8_char_unicode() {
    // U+2603 = ☃ (snowman)
    k9::assert_equal!(run_one("return utf8.char(0x2603)"), Value::string("☃"));
}

#[test]
fn utf8_char_empty() {
    k9::assert_equal!(run_one("return utf8.char()"), Value::string(""));
}

#[test]
fn utf8_char_multibyte() {
    // U+1F600 = 😀
    k9::assert_equal!(run_one("return utf8.char(0x1F600)"), Value::string("😀"));
}

#[test]
fn utf8_char_invalid_codepoint() {
    k9::assert_equal!(
        run_err("utf8.char(0x110000)"),
        "bad argument #1 to 'utf8.char' (valid Unicode codepoint expected, got 1114112)"
    );
}

#[test]
fn utf8_len_ascii() {
    k9::assert_equal!(run_one("return utf8.len('Hello')"), Value::Integer(5));
}

#[test]
fn utf8_len_unicode() {
    // "☃" is 3 bytes, 1 character.
    k9::assert_equal!(run_one("return utf8.len('☃')"), Value::Integer(1));
}

#[test]
fn utf8_len_mixed() {
    // "a☃b" = 1 + 3 + 1 = 5 bytes, 3 characters.
    k9::assert_equal!(run_one("return utf8.len('a☃b')"), Value::Integer(3));
}

#[test]
fn utf8_len_empty() {
    k9::assert_equal!(run_one("return utf8.len('')"), Value::Integer(0));
}

#[test]
fn utf8_len_range() {
    // utf8.len("Hello", 2, 4) = characters in bytes 2..4 = "ell" = 3
    k9::assert_equal!(run_one("return utf8.len('Hello', 2, 4)"), Value::Integer(3));
}

#[test]
fn utf8_len_invalid_returns_nil() {
    // Invalid UTF-8: \xff
    let results = run_all("return utf8.len('abc\\xff')");
    k9::assert_equal!(results[0], Value::Nil);
    k9::assert_equal!(results[1], Value::Integer(4));
}

#[test]
fn utf8_codepoint_single() {
    // 'A' = 65
    k9::assert_equal!(run_one("return utf8.codepoint('A')"), Value::Integer(65));
}

#[test]
fn utf8_codepoint_unicode() {
    // ☃ = U+2603
    k9::assert_equal!(
        run_one("return utf8.codepoint('☃')"),
        Value::Integer(0x2603)
    );
}

#[test]
fn utf8_codepoint_range() {
    // "Hello" codepoints at bytes 1..3 = H, e, l
    let results = run_all("return utf8.codepoint('Hello', 1, 3)");
    k9::assert_equal!(results[0], Value::Integer(72)); // H
    k9::assert_equal!(results[1], Value::Integer(101)); // e
    k9::assert_equal!(results[2], Value::Integer(108)); // l
}

#[test]
fn utf8_offset_forward() {
    // "aéb": a=1byte, é=2bytes, b=1byte
    // offset(s, 1) = 1 (byte pos of 1st char)
    // offset(s, 2) = 2 (byte pos of 2nd char)
    // offset(s, 3) = 4 (byte pos of 3rd char, after 2-byte é)
    k9::assert_equal!(run_one("return utf8.offset('aéb', 1)"), Value::Integer(1));
    k9::assert_equal!(run_one("return utf8.offset('aéb', 2)"), Value::Integer(2));
    k9::assert_equal!(run_one("return utf8.offset('aéb', 3)"), Value::Integer(4));
}

#[test]
fn utf8_offset_negative() {
    // offset(s, -1) from end = byte pos of last char
    // "aéb" (4 bytes): last char 'b' is at byte 4
    k9::assert_equal!(run_one("return utf8.offset('aéb', -1)"), Value::Integer(4));
}

#[test]
fn utf8_codes_basic() {
    let results = run_all(
        "local r = {}\n\
         for p, c in utf8.codes('aé') do\n\
           r[#r+1] = p\n\
           r[#r+1] = c\n\
         end\n\
         return r[1], r[2], r[3], r[4]",
    );
    k9::assert_equal!(results[0], Value::Integer(1)); // byte pos of 'a'
    k9::assert_equal!(results[1], Value::Integer(97)); // codepoint 'a'
    k9::assert_equal!(results[2], Value::Integer(2)); // byte pos of 'é'
    k9::assert_equal!(results[3], Value::Integer(233)); // codepoint 'é'
}

#[test]
fn utf8_codes_empty() {
    k9::assert_equal!(
        run_one("local n = 0; for _ in utf8.codes('') do n = n + 1 end; return n"),
        Value::Integer(0)
    );
}

#[test]
fn utf8_charpattern_exists() {
    k9::assert_equal!(
        run_one("return type(utf8.charpattern)"),
        Value::string("string")
    );
}
