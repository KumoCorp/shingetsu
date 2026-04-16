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
// string.pack / string.unpack / string.packsize
// ===========================================================================

#[test]
fn string_pack_unpack_integers() {
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('<i2i2', 1, 2)
               local a, b, pos = string.unpack('<i2i2', s)
               return a, b, pos"#
        ),
        vec![Value::Integer(1), Value::Integer(2), Value::Integer(5)]
    );
}

#[test]
fn string_pack_unpack_bytes() {
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('bBb', -1, 255, 42)
               local a, b, c = string.unpack('bBb', s)
               return a, b, c"#
        ),
        vec![Value::Integer(-1), Value::Integer(255), Value::Integer(42),]
    );
}

#[test]
fn string_pack_unpack_float_double() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('<d', 3.14)
               local v = string.unpack('<d', s)
               return v"#
        ),
        Value::Float(3.14)
    );
}

#[test]
fn string_pack_unpack_zstring() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('z', 'hello')
               return string.unpack('z', s)"#
        ),
        Value::String(Bytes::from("hello"))
    );
}

#[test]
fn string_pack_unpack_len_string() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('<s4', 'abc')
               return string.unpack('<s4', s)"#
        ),
        Value::String(Bytes::from("abc"))
    );
}

#[test]
fn string_packsize_basic() {
    k9::assert_equal!(run_one("return string.packsize('i4d')"), Value::Integer(12));
}

#[test]
fn string_pack_endianness() {
    // Big-endian 2-byte integer 0x0102 should be bytes 01 02.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('>i2', 0x0102)
               return string.byte(s, 1) * 256 + string.byte(s, 2)"#
        ),
        Value::Integer(0x0102)
    );
}

#[test]
fn string_pack_method_syntax() {
    // string.pack should also work via method syntax on format string.
    k9::assert_equal!(
        run_one(
            r#"local fmt = '<i4'
               local s = fmt:pack(42)
               return (fmt:unpack(s))"#
        ),
        Value::Integer(42)
    );
}

#[test]
fn string_unpack_with_position() {
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('<i2i2', 10, 20)
               local v, pos = string.unpack('<i2', s, 3)
               return v, pos"#
        ),
        vec![Value::Integer(20), Value::Integer(5)]
    );
}

#[test]
fn string_pack_fixed_string() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('c5', 'hi')
               return #s"#
        ),
        Value::Integer(5)
    );
}

#[test]
fn string_packsize_variable_length_errors() {
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(string.packsize, 'z')
               return ok"#
        ),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// Integration coverage for recent string.pack behavior fixes and Lua-compat
// coercion. These exercise the VM surface (not just the string_pack unit
// tests) to ensure errors propagate, method-call syntax works, and the
// coercion rules match reference Lua when reached via real scripts.
// ---------------------------------------------------------------------------

#[test]
fn string_unpack_negative_init_pos() {
    // Negative init_pos counts from end of string (Lua 5.4 semantics).
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('bbbb', 10, 20, 30, 40)
               return (string.unpack('b', s, -1))"#
        ),
        Value::Integer(40)
    );
}

#[test]
fn string_unpack_init_pos_zero_clamps_to_one() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', 99)
               return (string.unpack('b', s, 0))"#
        ),
        Value::Integer(99)
    );
}

#[test]
fn string_unpack_init_pos_past_end_errors() {
    // Errors surface to `pcall` as a string — user scripts can inspect it.
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.unpack, '', 'abc', 100)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #3 to 'unpack' (initial position out of string)"
            )),
        ]
    );
}

#[test]
fn string_pack_method_syntax_roundtrip() {
    // fmt:pack(...) and fmt:unpack(s) should work via the string metatable.
    k9::assert_equal!(
        run_all(
            r#"local fmt = '<i4i2'
               local s = fmt:pack(42, 7)
               local a, b = fmt:unpack(s)
               return a, b"#
        ),
        vec![Value::Integer(42), Value::Integer(7)]
    );
}

#[test]
fn string_pack_coerces_numeric_string_to_integer() {
    // Lua auto-coerces numeric strings for number slots in pack.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', '42')
               return string.byte(s, 1)"#
        ),
        Value::Integer(42)
    );
}

#[test]
fn string_pack_coerces_hex_string_to_integer() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', '0x2a')
               return string.byte(s, 1)"#
        ),
        Value::Integer(42)
    );
}

#[test]
fn string_pack_coerces_integer_to_string_slot() {
    // 42 stringifies to "42" for the c3 fixed-width slot (padded with NUL).
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('c3', 42)
               return string.byte(s, 1), string.byte(s, 2), string.byte(s, 3)"#
        ),
        vec![
            Value::Integer(b'4' as i64),
            Value::Integer(b'2' as i64),
            Value::Integer(0),
        ]
    );
}

#[test]
fn string_pack_coerces_float_to_zstring() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('z', 3.14)
               return (string.unpack('z', s))"#
        ),
        Value::String(Bytes::from("3.14"))
    );
}

#[test]
fn string_pack_rejects_boolean_for_number_slot() {
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, 'b', true)
               return ok"#
        ),
        Value::Boolean(false)
    );
}

#[test]
fn string_pack_rejects_nil_for_string_slot() {
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, 'c3', nil)
               return ok"#
        ),
        Value::Boolean(false)
    );
}

#[test]
fn string_pack_rejects_table_for_string_slot() {
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, 'z', {})
               return ok"#
        ),
        Value::Boolean(false)
    );
}

#[test]
fn string_pack_s1_length_overflow_error() {
    k9::assert_equal!(
        run_all(
            r#"local big = string.rep('x', 256)
               local ok, err = pcall(string.pack, 's1', big)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'pack' (string length does not fit in given size)"
            )),
        ]
    );
}

#[test]
fn string_pack_error_is_readable_string() {
    // Pack errors surface to `pcall` as strings (not nil), matching Lua's
    // `bad argument #N to 'funcname' (msg)` format from `luaL_argerror`.
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.unpack, 'z', 'abc')
               return type(err), err"#
        ),
        vec![
            Value::String(Bytes::from("string")),
            Value::String(Bytes::from(
                "bad argument #2 to 'unpack' (unfinished string for format 'z')"
            )),
        ]
    );
}

#[test]
fn string_pack_extra_args_silently_ignored() {
    // Pack consumes only as many args as the format requires.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', 1, 2, 3, 4)
               return #s"#
        ),
        Value::Integer(1)
    );
}

#[test]
fn string_pack_binary_roundtrip_preserves_bytes() {
    // Non-ASCII / NUL-containing payloads round-trip through s<n>.
    k9::assert_equal!(
        run_all(
            r#"local data = '\0\255\127\128'
               local s = string.pack('<s1', data)
               local out, pos = string.unpack('<s1', s)
               return out, pos"#
        ),
        vec![
            Value::String(Bytes::from_static(&[0x00, 0xFF, 0x7F, 0x80])),
            Value::Integer(6),
        ]
    );
}

#[test]
fn string_pack_empty_format_noop() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('')
               return #s"#
        ),
        Value::Integer(0)
    );
    k9::assert_equal!(run_one("return string.packsize('')"), Value::Integer(0));
}

#[test]
fn string_pack_alignment_mid_format() {
    // `<b !4 i4` pads the i4 to a 4-byte boundary after the leading byte.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('<b !4 i4', 1, 0x12345678)
               return #s"#
        ),
        Value::Integer(8)
    );
}

#[test]
fn string_pack_non_power_of_2_alignment_only_when_applied() {
    // `!3 b` is accepted (b has align 1); `!3 b i4` is rejected at the i4.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('!3 b', 1)
               return #s"#
        ),
        Value::Integer(1)
    );
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, '!3 b i4', 1, 2)
               return ok"#
        ),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// X-op follower validation — Lua rejects bytes its `getoption` classifies
// as `Knop` (space, `<`, `>`, `=`, `!`) and another `X`, even though the
// outer parser loop would otherwise skip those silently.
// ---------------------------------------------------------------------------

#[test]
fn string_pack_x_followed_by_space_errors() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'X i4', 1)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'pack' (invalid next option for option 'X')"
            )),
        ]
    );
}

#[test]
fn string_pack_x_followed_by_endian_errors() {
    for byte in ["<", ">", "="] {
        let script = format!(
            "local ok, err = pcall(string.pack, 'X{}i4', 1)\n return ok, err",
            byte
        );
        k9::assert_equal!(
            run_all(&script),
            vec![
                Value::Boolean(false),
                Value::String(Bytes::from(
                    "bad argument #1 to 'pack' (invalid next option for option 'X')"
                )),
            ],
            "fmt: X{}i4",
            byte
        );
    }
}

#[test]
fn string_pack_x_followed_by_bang_errors() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'X!4i4', 1)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'pack' (invalid next option for option 'X')"
            )),
        ]
    );
}

// ---------------------------------------------------------------------------
// Float-to-integer coercion — Lua requires an exact integer representation.
// These error paths are shared with `string.format("%d", ...)`.
// ---------------------------------------------------------------------------

#[test]
fn string_pack_rejects_fractional_float() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i4', 3.5)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'pack' (number has no integer representation)"
            )),
        ]
    );
}

#[test]
fn string_pack_accepts_whole_valued_float() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', 42.0)
               return string.byte(s, 1)"#
        ),
        Value::Integer(42)
    );
}

#[test]
fn string_pack_rejects_infinity_and_nan() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i4', 1/0)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'pack' (number has no integer representation)"
            )),
        ]
    );
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i4', 0/0)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'pack' (number has no integer representation)"
            )),
        ]
    );
}

// Lua's string→number parser (`l_str2d`) rejects any input containing
// `n` or `N`, so strings like `"nan"`/`"inf"`/`"Inf"` are NOT numbers.
// In pack's integer slot this surfaces as a type error (the value is
// still a string), distinct from `"3.5"` which parses as a valid float
// that merely lacks an integer representation.
#[test]
fn string_pack_int_rejects_nan_string_as_type_error() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i8', 'nan')
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'pack' (number expected, got string)"
            )),
        ]
    );
}

#[test]
fn string_pack_int_rejects_inf_string_as_type_error() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i8', 'inf')
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'pack' (number expected, got string)"
            )),
        ]
    );
}

#[test]
fn string_pack_float_rejects_nan_string() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'f', 'nan')
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'pack' (number expected, got string)"
            )),
        ]
    );
}

#[test]
fn string_format_f_rejects_nan_string() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.format, '%f', 'nan')
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'format' (number expected, got string)"
            )),
        ]
    );
}

#[test]
fn string_format_rejects_fractional_float_for_d() {
    // Same underlying coercion as pack — verifies the fix propagates
    // to `string.format("%d", ...)`.
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.format, '%d', 3.5)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'format' (number has no integer representation)"
            )),
        ]
    );
}

// ---------------------------------------------------------------------------
// `string.unpack` init_pos coercion — accepts numeric strings, rejects
// fractional floats.  Mirrors Lua's behavior for integer-typed arguments.
// ---------------------------------------------------------------------------

#[test]
fn string_unpack_init_pos_accepts_numeric_string() {
    k9::assert_equal!(
        run_one(r#"return (string.unpack('b', 'abc', '2'))"#),
        Value::Integer(b'b' as i64)
    );
}

#[test]
fn string_unpack_init_pos_rejects_fractional_float() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.unpack, 'b', 'abc', 2.5)
               return ok, err"#
        ),
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #3 to 'unpack' (number has no integer representation)"
            )),
        ]
    );
}

#[test]
fn string_unpack_init_pos_accepts_whole_float() {
    k9::assert_equal!(
        run_one(r#"return (string.unpack('b', 'abc', 2.0))"#),
        Value::Integer(b'b' as i64)
    );
}
