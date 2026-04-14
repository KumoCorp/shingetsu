mod common;

use bytes::Bytes;
use common::{run_all, run_one};
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// string library
// ---------------------------------------------------------------------------

#[test]
fn string_lib_len() {
    k9::assert_equal!(run_one("return string.len('hello')"), Value::Integer(5));
    k9::assert_equal!(run_one("return string.len('')"), Value::Integer(0));
}

#[test]
fn string_lib_len_method_syntax() {
    // Method-call syntax on string values via the string metatable.
    k9::assert_equal!(run_one("return ('hello'):len()"), Value::Integer(5));
}

#[test]
fn string_lib_upper_lower() {
    k9::assert_equal!(
        run_one("return string.upper('hello')"),
        Value::String(Bytes::from("HELLO"))
    );
    k9::assert_equal!(
        run_one("return string.lower('HeLLo')"),
        Value::String(Bytes::from("hello"))
    );
}

#[test]
fn string_lib_upper_method_syntax() {
    k9::assert_equal!(
        run_one("return ('hello'):upper()"),
        Value::String(Bytes::from("HELLO"))
    );
}

#[test]
fn string_lib_reverse() {
    k9::assert_equal!(
        run_one("return string.reverse('abcd')"),
        Value::String(Bytes::from("dcba"))
    );
}

#[test]
fn string_lib_byte() {
    // Single byte at default position (first).
    k9::assert_equal!(run_one("return string.byte('A')"), Value::Integer(65));
    // Range: byte(s, 1, 3) returns three values.
    let res = run_all("return string.byte('ABC', 1, 3)");
    k9::assert_equal!(
        res,
        vec![Value::Integer(65), Value::Integer(66), Value::Integer(67)]
    );
    // Out-of-range returns nothing.
    let res = run_all("return string.byte('A', 5, 6)");
    k9::assert_equal!(res.len(), 0);
}

#[test]
fn string_lib_char() {
    k9::assert_equal!(
        run_one("return string.char(72, 101, 108, 108, 111)"),
        Value::String(Bytes::from("Hello"))
    );
}

#[test]
fn string_lib_sub() {
    k9::assert_equal!(
        run_one("return string.sub('Hello', 2, 4)"),
        Value::String(Bytes::from("ell"))
    );
    // Negative index: -3 = third from end.
    k9::assert_equal!(
        run_one("return string.sub('Hello', -3)"),
        Value::String(Bytes::from("llo"))
    );
}

#[test]
fn string_lib_rep() {
    k9::assert_equal!(
        run_one("return string.rep('ab', 3)"),
        Value::String(Bytes::from("ababab"))
    );
    // With separator.
    k9::assert_equal!(
        run_one("return string.rep('ab', 3, ',')"),
        Value::String(Bytes::from("ab,ab,ab"))
    );
    // Zero repetitions.
    k9::assert_equal!(
        run_one("return string.rep('x', 0)"),
        Value::String(Bytes::new())
    );
}

// ---------------------------------------------------------------------------
// string.find
// ---------------------------------------------------------------------------

#[test]
fn string_lib_find_plain() {
    let res = run_all("return string.find('hello world', 'world')");
    k9::assert_equal!(res, vec![Value::Integer(7), Value::Integer(11)]);
}

#[test]
fn string_lib_find_plain_flag() {
    // With plain=true, pattern chars are literal.
    let res = run_all("return string.find('100%', '%', 1, true)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Integer(4)]);
}

#[test]
fn string_lib_find_pattern() {
    let res = run_all("return string.find('hello 123 world', '(%d+)')");
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(7),
            Value::Integer(9),
            Value::String(Bytes::from("123"))
        ]
    );
}

#[test]
fn string_lib_find_no_match() {
    let res = run_all("return string.find('hello', 'xyz')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

#[test]
fn string_lib_find_with_init() {
    // Start search from position 6.
    let res = run_all("return string.find('abcabc', 'abc', 4)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Integer(6)]);
}

// ---------------------------------------------------------------------------
// string.match
// ---------------------------------------------------------------------------

#[test]
fn string_lib_match_captures() {
    let res = run_all("return string.match('2025-04-13', '(%d+)-(%d+)-(%d+)')");
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("2025")),
            Value::String(Bytes::from("04")),
            Value::String(Bytes::from("13")),
        ]
    );
}

#[test]
fn string_lib_match_whole() {
    // No explicit captures — returns the whole match.
    let res = run_all("return string.match('hello world', '%a+')");
    k9::assert_equal!(res, vec![Value::String(Bytes::from("hello"))]);
}

#[test]
fn string_lib_match_no_match() {
    let res = run_all("return string.match('hello', '%d+')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// string.gmatch
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gmatch_words() {
    let res = run_all(
        "\
        local t = {}
        for w in string.gmatch('one two three', '%a+') do
            t[#t+1] = w
        end
        return t[1], t[2], t[3]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("one")),
            Value::String(Bytes::from("two")),
            Value::String(Bytes::from("three")),
        ]
    );
}

#[test]
fn string_lib_gmatch_captures() {
    let res = run_all(
        "\
        local keys, vals = {}, {}
        for k, v in string.gmatch('a=1, b=2', '(%a+)=(%d+)') do
            keys[#keys+1] = k
            vals[#vals+1] = v
        end
        return keys[1], vals[1], keys[2], vals[2]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("a")),
            Value::String(Bytes::from("1")),
            Value::String(Bytes::from("b")),
            Value::String(Bytes::from("2")),
        ]
    );
}

// ---------------------------------------------------------------------------
// string.gsub
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gsub_string() {
    let res = run_all("return string.gsub('hello world', 'world', 'lua')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("hello lua")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_pattern() {
    let res = run_all("return string.gsub('abc 123 def 456', '%d+', 'NUM')");
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("abc NUM def NUM")),
            Value::Integer(2)
        ]
    );
}

#[test]
fn string_lib_gsub_capture_ref() {
    // %1 references the first capture.
    let res = run_all("return string.gsub('hello', '(%w+)', '[%1]')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("[hello]")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_max_n() {
    // Replace at most 1.
    let res = run_all("return string.gsub('aaa', 'a', 'b', 1)");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("baa")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_table() {
    let res = run_all(
        "\
        local t = { hello = 'HI', world = 'EARTH' }
        return string.gsub('hello world', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("HI EARTH")), Value::Integer(2)]
    );
}

// ---------------------------------------------------------------------------
// string.format
// ---------------------------------------------------------------------------

#[test]
fn string_lib_format_basic() {
    k9::assert_equal!(
        run_one("return string.format('%d + %d = %d', 1, 2, 3)"),
        Value::String(Bytes::from("1 + 2 = 3"))
    );
}

#[test]
fn string_lib_format_string() {
    k9::assert_equal!(
        run_one("return string.format('hello %s!', 'world')"),
        Value::String(Bytes::from("hello world!"))
    );
}

#[test]
fn string_lib_format_hex() {
    k9::assert_equal!(
        run_one("return string.format('%x', 255)"),
        Value::String(Bytes::from("ff"))
    );
    k9::assert_equal!(
        run_one("return string.format('%X', 255)"),
        Value::String(Bytes::from("FF"))
    );
}

#[test]
fn string_lib_format_float() {
    k9::assert_equal!(
        run_one("return string.format('%.2f', 3.14159)"),
        Value::String(Bytes::from("3.14"))
    );
}

#[test]
fn string_lib_format_padded() {
    k9::assert_equal!(
        run_one("return string.format('%05d', 42)"),
        Value::String(Bytes::from("00042"))
    );
}

#[test]
fn string_lib_format_quoted() {
    k9::assert_equal!(
        run_one("return string.format('%q', 'hello')"),
        Value::String(Bytes::from(r#""hello""#))
    );
}

#[test]
fn string_lib_format_percent() {
    k9::assert_equal!(
        run_one("return string.format('100%%')"),
        Value::String(Bytes::from("100%"))
    );
}

// ---------------------------------------------------------------------------
// string.find — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_find_anchored_start() {
    // `^` anchored pattern should only match at the start.
    let res = run_all("return string.find('hello world', '^hello')");
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(5)]);
}

#[test]
fn string_lib_find_anchored_start_no_match() {
    let res = run_all("return string.find('say hello', '^hello')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

#[test]
fn string_lib_find_anchored_end() {
    let res = run_all("return string.find('hello world', 'world$')");
    k9::assert_equal!(res, vec![Value::Integer(7), Value::Integer(11)]);
}

#[test]
fn string_lib_find_negative_init() {
    // Negative init counts from the end.
    let res = run_all("return string.find('abcabc', 'abc', -3)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Integer(6)]);
}

#[test]
fn string_lib_find_empty_pattern() {
    // Empty pattern matches at position 1.
    let res = run_all("return string.find('hello', '')");
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(0)]);
}

#[test]
fn string_lib_find_empty_haystack() {
    let res = run_all("return string.find('', 'anything')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

#[test]
fn string_lib_find_plain_empty_pattern() {
    let res = run_all("return string.find('hello', '', 1, true)");
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(0)]);
}

// ---------------------------------------------------------------------------
// string.match — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_match_with_init() {
    // Start matching from position 5.
    let res = run_all("return string.match('abc 123 def 456', '%d+', 10)");
    k9::assert_equal!(res, vec![Value::String(Bytes::from("456"))]);
}

#[test]
fn string_lib_match_anchored() {
    // `^%d+` only matches digits at the start.
    let res = run_all("return string.match('123abc', '^%d+')");
    k9::assert_equal!(res, vec![Value::String(Bytes::from("123"))]);
}

#[test]
fn string_lib_match_anchored_no_match() {
    let res = run_all("return string.match('abc123', '^%d+')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// string.gmatch — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gmatch_no_matches() {
    let res = run_one(
        "\
        local count = 0
        for w in string.gmatch('hello', '%d+') do
            count = count + 1
        end
        return count",
    );
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn string_lib_gmatch_empty_match() {
    // Empty pattern matches between every character; should not loop forever.
    let res = run_one(
        "\
        local t = {}
        for c in string.gmatch('ab', '.') do
            t[#t+1] = c
        end
        return #t",
    );
    k9::assert_equal!(res, Value::Integer(2));
}

// ---------------------------------------------------------------------------
// string.gsub — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gsub_capture_ref_zero() {
    // %0 references the whole match.
    let res = run_all("return string.gsub('hello', '%w+', '[%0]')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("[hello]")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_percent_literal_in_replacement() {
    // %% in replacement string produces a literal %.
    let res = run_all("return string.gsub('abc', 'abc', '100%%')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("100%")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_table_missing_key() {
    // When the table has no entry for a match, the original match is kept.
    let res = run_all(
        "\
        local t = { hello = 'HI' }
        return string.gsub('hello world', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("HI world")), Value::Integer(2)]
    );
}

#[test]
fn string_lib_gsub_table_false_value() {
    // If the table value is false, the original match is preserved.
    let res = run_all(
        "\
        local t = { hello = false }
        return string.gsub('hello', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("hello")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_table_numeric_value() {
    // Numeric table values are coerced to string.
    let res = run_all(
        "\
        local t = { hello = 42 }
        return string.gsub('hello', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("42")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_function_replacement() {
    // Function replacement: function is called with each match,
    // return value becomes the replacement.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m) return m:upper() end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("HELLO WORLD")));
}

#[test]
fn string_lib_gsub_function_with_captures() {
    // Function receives each capture group as a separate argument.
    let res = run_one(
        "\
        return string.gsub('2025-04-13', '(%d+)-(%d+)-(%d+)', function(y, m, d)
            return d .. '/' .. m .. '/' .. y
        end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("13/04/2025")));
}

#[test]
fn string_lib_gsub_function_nil_keeps_original() {
    // If the function returns nil, the original match is kept.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m)
            if m == 'hello' then return nil end
            return m:upper()
        end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("hello WORLD")));
}

#[test]
fn string_lib_gsub_function_false_keeps_original() {
    // If the function returns false, the original match is kept.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m)
            if m == 'world' then return false end
            return m:upper()
        end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("HELLO world")));
}

#[test]
fn string_lib_gsub_function_returns_number() {
    // If the function returns a number, it is coerced to a string.
    let res = run_one("return string.gsub('a b c', '%w+', function(m) return 42 end)");
    k9::assert_equal!(res, Value::String(Bytes::from("42 42 42")));
}

#[test]
fn string_lib_gsub_function_with_max_n() {
    // max_n limits the number of replacements.
    let res = run_all(
        "\
        return string.gsub('aaa', 'a', function() return 'b' end, 2)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("bba")), Value::Integer(2)]
    );
}

#[test]
fn string_lib_gsub_function_invalid_return() {
    // If the function returns a table (not string/number/nil/false), error.
    let res = run_one(
        "\
        local ok = pcall(string.gsub, 'hello', '%w+', function() return {} end)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_gsub_bad_replacement_type() {
    // Passing a boolean as replacement should error.
    let res = run_one(
        "\
        local ok, msg = pcall(string.gsub, 'hello', '%w+', true)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_gsub_anchored_pattern() {
    // `^%w+` should only replace the first word (anchored at start).
    let res = run_all("return string.gsub('hello world', '^%w+', 'BYE')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("BYE world")), Value::Integer(1)]
    );
}

// ---------------------------------------------------------------------------
// string.format — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_format_integer_i() {
    // %i is an alias for %d.
    k9::assert_equal!(
        run_one("return string.format('%i', 42)"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_unsigned() {
    k9::assert_equal!(
        run_one("return string.format('%u', 42)"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_octal() {
    k9::assert_equal!(
        run_one("return string.format('%o', 255)"),
        Value::String(Bytes::from("377"))
    );
}

#[test]
fn string_lib_format_octal_alt() {
    // `#` flag prepends a `0` for octal.
    k9::assert_equal!(
        run_one("return string.format('%#o', 255)"),
        Value::String(Bytes::from("0377"))
    );
}

#[test]
fn string_lib_format_scientific() {
    let res = run_one("return string.format('%.2e', 314.159)");
    k9::assert_equal!(res, Value::String(Bytes::from("3.14e2")));
}

#[test]
fn string_lib_format_scientific_upper() {
    let res = run_one("return string.format('%.2E', 314.159)");
    k9::assert_equal!(res, Value::String(Bytes::from("3.14E2")));
}

#[test]
fn string_lib_format_general_float() {
    // %g uses shorter of %e and %f.
    k9::assert_equal!(
        run_one("return string.format('%g', 100000.0)"),
        Value::String(Bytes::from("100000"))
    );
    k9::assert_equal!(
        run_one("return string.format('%g', 0.00123)"),
        Value::String(Bytes::from("0.00123"))
    );
}

#[test]
fn string_lib_format_char() {
    k9::assert_equal!(
        run_one("return string.format('%c', 65)"),
        Value::String(Bytes::from("A"))
    );
}

#[test]
fn string_lib_format_hex_alt() {
    // `#` flag prepends `0x` / `0X`.
    k9::assert_equal!(
        run_one("return string.format('%#x', 255)"),
        Value::String(Bytes::from("0xff"))
    );
    k9::assert_equal!(
        run_one("return string.format('%#X', 255)"),
        Value::String(Bytes::from("0XFF"))
    );
}

#[test]
fn string_lib_format_plus_flag() {
    k9::assert_equal!(
        run_one("return string.format('%+d', 42)"),
        Value::String(Bytes::from("+42"))
    );
    k9::assert_equal!(
        run_one("return string.format('%+d', -42)"),
        Value::String(Bytes::from("-42"))
    );
}

#[test]
fn string_lib_format_space_flag() {
    k9::assert_equal!(
        run_one("return string.format('% d', 42)"),
        Value::String(Bytes::from(" 42"))
    );
    k9::assert_equal!(
        run_one("return string.format('% d', -42)"),
        Value::String(Bytes::from("-42"))
    );
}

#[test]
fn string_lib_format_left_align() {
    k9::assert_equal!(
        run_one("return string.format('%-10d|', 42)"),
        Value::String(Bytes::from("42        |"))
    );
}

#[test]
fn string_lib_format_width_space_pad() {
    k9::assert_equal!(
        run_one("return string.format('%10d', 42)"),
        Value::String(Bytes::from("        42"))
    );
}

#[test]
fn string_lib_format_string_precision() {
    // %.3s truncates the string to 3 characters.
    k9::assert_equal!(
        run_one("return string.format('%.3s', 'hello')"),
        Value::String(Bytes::from("hel"))
    );
}

#[test]
fn string_lib_format_string_coercion_number() {
    // Formatting a number with %s should produce its string form.
    k9::assert_equal!(
        run_one("return string.format('%s', 42)"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_integer_from_string() {
    // %d with a numeric string coerces to integer.
    k9::assert_equal!(
        run_one("return string.format('%d', '42')"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_float_from_string() {
    // %f with a numeric string coerces to float.
    k9::assert_equal!(
        run_one("return string.format('%.1f', '3.14')"),
        Value::String(Bytes::from("3.1"))
    );
}

#[test]
fn string_lib_format_too_few_args() {
    let res = run_one(
        "\
        local ok, msg = pcall(string.format, '%d %d', 1)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_format_invalid_specifier() {
    let res = run_one(
        "\
        local ok, msg = pcall(string.format, '%z', 1)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_format_trailing_percent() {
    let res = run_one(
        "\
        local ok = pcall(string.format, 'oops%')
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_format_quoted_special_chars() {
    // %q should escape newlines, backslashes, null bytes, and \x1a.
    k9::assert_equal!(
        run_one("return string.format('%q', 'a\\nb')"),
        Value::String(Bytes::from("\"a\\nb\""))
    );
    k9::assert_equal!(
        run_one("return string.format('%q', 'a\"b')"),
        Value::String(Bytes::from("\"a\\\"b\""))
    );
}

#[test]
fn string_lib_format_coerce_to_string_nil() {
    k9::assert_equal!(
        run_one("return string.format('%s', nil)"),
        Value::String(Bytes::from("nil"))
    );
}

#[test]
fn string_lib_format_coerce_to_string_bool() {
    k9::assert_equal!(
        run_one("return string.format('%s', true)"),
        Value::String(Bytes::from("true"))
    );
}

// ===========================================================================
// table library
// ===========================================================================

// ---------------------------------------------------------------------------
