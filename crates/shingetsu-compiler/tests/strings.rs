mod common;

use common::{run_all, run_err, run_one};
use shingetsu::valuevec;
use shingetsu_vm::{Bytes, Value};

// ---------------------------------------------------------------------------
// string library
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_len() {
    k9::assert_equal!(
        run_one("return string.len('hello')").await,
        Value::Integer(5)
    );
    k9::assert_equal!(run_one("return string.len('')").await, Value::Integer(0));
}

#[tokio::test]
async fn string_lib_len_method_syntax() {
    // Method-call syntax on string values via the string metatable.
    k9::assert_equal!(run_one("return ('hello'):len()").await, Value::Integer(5));
}

#[tokio::test]
async fn string_lib_upper_lower() {
    k9::assert_equal!(
        run_one("return string.upper('hello')").await,
        Value::string("HELLO")
    );
    k9::assert_equal!(
        run_one("return string.lower('HeLLo')").await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn string_lib_upper_method_syntax() {
    k9::assert_equal!(
        run_one("return ('hello'):upper()").await,
        Value::string("HELLO")
    );
}

#[tokio::test]
async fn string_lib_reverse() {
    k9::assert_equal!(
        run_one("return string.reverse('abcd')").await,
        Value::string("dcba")
    );
}

#[tokio::test]
async fn string_lib_byte() {
    // Single byte at default position (first).
    k9::assert_equal!(run_one("return string.byte('A')").await, Value::Integer(65));
    // Range: byte(s, 1, 3) returns three values.
    let res = run_all("return string.byte('ABC', 1, 3)").await;
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(65), Value::Integer(66), Value::Integer(67)]
    );
    // Out-of-range returns nothing.
    let res = run_all("return string.byte('A', 5, 6)").await;
    k9::assert_equal!(res.len(), 0);
}

#[tokio::test]
async fn string_lib_char() {
    k9::assert_equal!(
        run_one("return string.char(72, 101, 108, 108, 111)").await,
        Value::string("Hello")
    );
}

#[tokio::test]
async fn string_lib_sub() {
    k9::assert_equal!(
        run_one("return string.sub('Hello', 2, 4)").await,
        Value::string("ell")
    );
    // Negative index: -3 = third from end.
    k9::assert_equal!(
        run_one("return string.sub('Hello', -3)").await,
        Value::string("llo")
    );
}

#[tokio::test]
async fn string_lib_rep() {
    k9::assert_equal!(
        run_one("return string.rep('ab', 3)").await,
        Value::string("ababab")
    );
    // With separator.
    k9::assert_equal!(
        run_one("return string.rep('ab', 3, ',')").await,
        Value::string("ab,ab,ab")
    );
    // Zero repetitions.
    k9::assert_equal!(
        run_one("return string.rep('x', 0)").await,
        Value::string("")
    );
    // Float-typed integer count (2^3 == 8.0 coerced to 8).
    k9::assert_equal!(
        run_one("return string.rep('a', 2^3)").await,
        Value::string("aaaaaaaa")
    );
}

// ---------------------------------------------------------------------------
// string.find
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_find_plain() {
    let res = run_all("return string.find('hello world', 'world')").await;
    k9::assert_equal!(res, valuevec![Value::Integer(7), Value::Integer(11)]);
}

#[tokio::test]
async fn string_lib_find_plain_flag() {
    // With plain=true, pattern chars are literal.
    let res = run_all("return string.find('100%', '%', 1, true)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(4), Value::Integer(4)]);
}

#[tokio::test]
async fn string_lib_find_pattern() {
    let res = run_all("return string.find('hello 123 world', '(%d+)')").await;
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(7), Value::Integer(9), Value::string("123")]
    );
}

#[tokio::test]
async fn string_lib_find_no_match() {
    let res = run_all("return string.find('hello', 'xyz')").await;
    k9::assert_equal!(res, valuevec![Value::Nil]);
}

#[tokio::test]
async fn string_lib_find_with_init() {
    // Start search from position 6.
    let res = run_all("return string.find('abcabc', 'abc', 4)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(4), Value::Integer(6)]);
}

// ---------------------------------------------------------------------------
// string.match
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_match_captures() {
    let res = run_all("return string.match('2025-04-13', '(%d+)-(%d+)-(%d+)')").await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::string("2025"),
            Value::string("04"),
            Value::string("13"),
        ]
    );
}

#[tokio::test]
async fn string_lib_match_whole() {
    // No explicit captures — returns the whole match.
    let res = run_all("return string.match('hello world', '%a+')").await;
    k9::assert_equal!(res, valuevec![Value::string("hello")]);
}

#[tokio::test]
async fn string_lib_match_no_match() {
    let res = run_all("return string.match('hello', '%d+')").await;
    k9::assert_equal!(res, valuevec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// string.gmatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_gmatch_words() {
    let res = run_all(
        "\
        local t = {}
        for w in string.gmatch('one two three', '%a+') do
            t[#t+1] = w
        end
        return t[1], t[2], t[3]",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::string("one"),
            Value::string("two"),
            Value::string("three"),
        ]
    );
}

#[tokio::test]
async fn string_lib_gmatch_captures() {
    let res = run_all(
        "\
        local keys, vals = {}, {}
        for k, v in string.gmatch('a=1, b=2', '(%a+)=(%d+)') do
            keys[#keys+1] = k
            vals[#vals+1] = v
        end
        return keys[1], vals[1], keys[2], vals[2]",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::string("a"),
            Value::string("1"),
            Value::string("b"),
            Value::string("2"),
        ]
    );
}

// ---------------------------------------------------------------------------
// string.gsub
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_gsub_string() {
    let res = run_all("return string.gsub('hello world', 'world', 'lua')").await;
    k9::assert_equal!(
        res,
        valuevec![Value::string("hello lua"), Value::Integer(1)]
    );
}

#[tokio::test]
async fn string_lib_gsub_pattern() {
    let res = run_all("return string.gsub('abc 123 def 456', '%d+', 'NUM')").await;
    k9::assert_equal!(
        res,
        valuevec![Value::string("abc NUM def NUM"), Value::Integer(2)]
    );
}

#[tokio::test]
async fn string_lib_gsub_capture_ref() {
    // %1 references the first capture.
    let res = run_all("return string.gsub('hello', '(%w+)', '[%1]')").await;
    k9::assert_equal!(res, valuevec![Value::string("[hello]"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_max_n() {
    // Replace at most 1.
    let res = run_all("return string.gsub('aaa', 'a', 'b', 1)").await;
    k9::assert_equal!(res, valuevec![Value::string("baa"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_table() {
    let res = run_all(
        "\
        local t = { hello = 'HI', world = 'EARTH' }
        return string.gsub('hello world', '(%w+)', t)",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("HI EARTH"), Value::Integer(2)]);
}

// ---------------------------------------------------------------------------
// string.format
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_format_basic() {
    k9::assert_equal!(
        run_one("return string.format('%d + %d = %d', 1, 2, 3)").await,
        Value::string("1 + 2 = 3")
    );
}

#[tokio::test]
async fn string_lib_format_string() {
    k9::assert_equal!(
        run_one("return string.format('hello %s!', 'world')").await,
        Value::string("hello world!")
    );
}

#[tokio::test]
async fn string_lib_format_hex() {
    k9::assert_equal!(
        run_one("return string.format('%x', 255)").await,
        Value::string("ff")
    );
    k9::assert_equal!(
        run_one("return string.format('%X', 255)").await,
        Value::string("FF")
    );
}

#[tokio::test]
async fn string_lib_format_float() {
    k9::assert_equal!(
        run_one("return string.format('%.2f', 3.14159)").await,
        Value::string("3.14")
    );
}

#[tokio::test]
async fn string_lib_format_padded() {
    k9::assert_equal!(
        run_one("return string.format('%05d', 42)").await,
        Value::string("00042")
    );
}

#[tokio::test]
async fn string_lib_format_quoted() {
    k9::assert_equal!(
        run_one("return string.format('%q', 'hello')").await,
        Value::string(r#""hello""#)
    );
}

#[tokio::test]
async fn string_lib_format_percent() {
    k9::assert_equal!(
        run_one("return string.format('100%%')").await,
        Value::string("100%")
    );
}

// ---------------------------------------------------------------------------
// string.find — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_find_anchored_start() {
    // `^` anchored pattern should only match at the start.
    let res = run_all("return string.find('hello world', '^hello')").await;
    k9::assert_equal!(res, valuevec![Value::Integer(1), Value::Integer(5)]);
}

#[tokio::test]
async fn string_lib_find_anchored_start_no_match() {
    let res = run_all("return string.find('say hello', '^hello')").await;
    k9::assert_equal!(res, valuevec![Value::Nil]);
}

#[tokio::test]
async fn string_lib_find_anchored_end() {
    let res = run_all("return string.find('hello world', 'world$')").await;
    k9::assert_equal!(res, valuevec![Value::Integer(7), Value::Integer(11)]);
}

#[tokio::test]
async fn string_lib_find_negative_init() {
    // Negative init counts from the end.
    let res = run_all("return string.find('abcabc', 'abc', -3)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(4), Value::Integer(6)]);
}

#[tokio::test]
async fn string_lib_find_empty_pattern() {
    // Empty pattern matches at position 1.
    let res = run_all("return string.find('hello', '')").await;
    k9::assert_equal!(res, valuevec![Value::Integer(1), Value::Integer(0)]);
}

#[tokio::test]
async fn string_lib_find_empty_haystack() {
    let res = run_all("return string.find('', 'anything')").await;
    k9::assert_equal!(res, valuevec![Value::Nil]);
}

#[tokio::test]
async fn string_lib_find_plain_empty_pattern() {
    let res = run_all("return string.find('hello', '', 1, true)").await;
    k9::assert_equal!(res, valuevec![Value::Integer(1), Value::Integer(0)]);
}

// ---------------------------------------------------------------------------
// string.match — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_match_with_init() {
    // Start matching from position 5.
    let res = run_all("return string.match('abc 123 def 456', '%d+', 10)").await;
    k9::assert_equal!(res, valuevec![Value::string("456")]);
}

#[tokio::test]
async fn string_lib_match_anchored() {
    // `^%d+` only matches digits at the start.
    let res = run_all("return string.match('123abc', '^%d+')").await;
    k9::assert_equal!(res, valuevec![Value::string("123")]);
}

#[tokio::test]
async fn string_lib_match_anchored_no_match() {
    let res = run_all("return string.match('abc123', '^%d+')").await;
    k9::assert_equal!(res, valuevec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// string.gmatch — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_gmatch_no_matches() {
    let res = run_one(
        "\
        local count = 0
        for w in string.gmatch('hello', '%d+') do
            count = count + 1
        end
        return count",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(0));
}

#[tokio::test]
async fn string_lib_gmatch_empty_match() {
    // Empty pattern matches between every character; should not loop forever.
    let res = run_one(
        "\
        local t = {}
        for c in string.gmatch('ab', '.') do
            t[#t+1] = c
        end
        return #t",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(2));
}

#[tokio::test]
async fn string_lib_gmatch_init_positive() {
    let res = run_one(
        "\
        local t = {}
        for w in string.gmatch('one two three', '%a+', 5) do
            t[#t+1] = w
        end
        return table.concat(t, ',')",
    )
    .await;
    k9::assert_equal!(res, Value::string("two,three"));
}

#[tokio::test]
async fn string_lib_gmatch_init_negative() {
    let res = run_one(
        "\
        local t = {}
        for w in string.gmatch('one two three', '%a+', -5) do
            t[#t+1] = w
        end
        return table.concat(t, ',')",
    )
    .await;
    k9::assert_equal!(res, Value::string("three"));
}

#[tokio::test]
async fn string_lib_gmatch_init_zero() {
    // init=0 clamps to start of string (same as init=1)
    let res = run_one(
        "\
        local t = {}
        for w in string.gmatch('abc', '%a+', 0) do
            t[#t+1] = w
        end
        return table.concat(t, ',')",
    )
    .await;
    k9::assert_equal!(res, Value::string("abc"));
}

#[tokio::test]
async fn string_lib_gmatch_init_past_end_yields_nothing() {
    // Per Lua 5.4: init > #s + 1 must yield zero matches.  Without
    // the past-end check the empty pattern `%w*` would match the
    // empty suffix at end-of-string and produce a spurious match.
    let res = run_all(
        "\
        local one = 0
        for _ in string.gmatch('11 21 31', '%w*', 9) do one = one + 1 end
        local past = 0
        for _ in string.gmatch('11 21 31', '%w*', 10) do past = past + 1 end
        local far = 0
        for _ in string.gmatch('11 21 31', '%w*', 100) do far = far + 1 end
        return one, past, far",
    )
    .await;
    // init = #s + 1: matches the empty suffix exactly once.
    // init > #s + 1: zero matches.
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(1), Value::Integer(0), Value::Integer(0)]
    );
}

#[tokio::test]
async fn string_lib_gmatch_init_explicit_one() {
    // init=1 is the default — explicit should behave the same
    let res = run_one(
        "\
        local t = {}
        for w in string.gmatch('one two', '%a+', 1) do
            t[#t+1] = w
        end
        return table.concat(t, ',')",
    )
    .await;
    k9::assert_equal!(res, Value::string("one,two"));
}

#[tokio::test]
async fn string_lib_gmatch_init_method_syntax() {
    let res = run_one(
        "\
        local t = {}
        for w in ('hello world'):gmatch('%a+', 7) do
            t[#t+1] = w
        end
        return table.concat(t, ',')",
    )
    .await;
    k9::assert_equal!(res, Value::string("world"));
}

#[tokio::test]
async fn string_lib_gmatch_init_with_captures() {
    let res = run_all(
        "\
        local keys, vals = {}, {}
        for k, v in string.gmatch('a=1, b=2, c=3', '(%a+)=(%d+)', 6) do
            keys[#keys+1] = k
            vals[#vals+1] = v
        end
        return table.concat(keys, ','), table.concat(vals, ',')",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("b,c"), Value::string("2,3")]);
}

#[tokio::test]
async fn string_lib_gmatch_init_past_end() {
    let res = run_one(
        "\
        local t = {}
        for w in string.gmatch('hello', '%a+', 100) do
            t[#t+1] = w
        end
        return #t",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(0));
}

// ---------------------------------------------------------------------------
// string.gsub — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_gsub_capture_ref_zero() {
    // %0 references the whole match.
    let res = run_all("return string.gsub('hello', '%w+', '[%0]')").await;
    k9::assert_equal!(res, valuevec![Value::string("[hello]"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_percent_literal_in_replacement() {
    // %% in replacement string produces a literal %.
    let res = run_all("return string.gsub('abc', 'abc', '100%%')").await;
    k9::assert_equal!(res, valuevec![Value::string("100%"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_table_missing_key() {
    // When the table has no entry for a match, the original match is kept.
    let res = run_all(
        "\
        local t = { hello = 'HI' }
        return string.gsub('hello world', '(%w+)', t)",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("HI world"), Value::Integer(2)]);
}

#[tokio::test]
async fn string_lib_gsub_table_false_value() {
    // If the table value is false, the original match is preserved.
    let res = run_all(
        "\
        local t = { hello = false }
        return string.gsub('hello', '(%w+)', t)",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("hello"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_table_numeric_value() {
    // Numeric table values are coerced to string.
    let res = run_all(
        "\
        local t = { hello = 42 }
        return string.gsub('hello', '(%w+)', t)",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("42"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_function_replacement() {
    // Function replacement: function is called with each match,
    // return value becomes the replacement.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m) return m:upper() end)",
    )
    .await;
    k9::assert_equal!(res, Value::string("HELLO WORLD"));
}

#[tokio::test]
async fn string_lib_gsub_function_with_captures() {
    // Function receives each capture group as a separate argument.
    let res = run_one(
        "\
        return string.gsub('2025-04-13', '(%d+)-(%d+)-(%d+)', function(y, m, d)
            return d .. '/' .. m .. '/' .. y
        end)",
    )
    .await;
    k9::assert_equal!(res, Value::string("13/04/2025"));
}

#[tokio::test]
async fn string_lib_gsub_function_nil_keeps_original() {
    // If the function returns nil, the original match is kept.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m)
            if m == 'hello' then return nil end
            return m:upper()
        end)",
    )
    .await;
    k9::assert_equal!(res, Value::string("hello WORLD"));
}

#[tokio::test]
async fn string_lib_gsub_function_false_keeps_original() {
    // If the function returns false, the original match is kept.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m)
            if m == 'world' then return false end
            return m:upper()
        end)",
    )
    .await;
    k9::assert_equal!(res, Value::string("HELLO world"));
}

#[tokio::test]
async fn string_lib_gsub_function_returns_number() {
    // If the function returns a number, it is coerced to a string.
    let res = run_one("return string.gsub('a b c', '%w+', function(m) return 42 end)").await;
    k9::assert_equal!(res, Value::string("42 42 42"));
}

#[tokio::test]
async fn string_lib_gsub_function_with_max_n() {
    // max_n limits the number of replacements.
    let res = run_all(
        "\
        return string.gsub('aaa', 'a', function() return 'b' end, 2)",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("bba"), Value::Integer(2)]);
}

#[tokio::test]
async fn string_lib_gsub_function_invalid_return() {
    // If the function returns a table (not string/number/nil/false), error.
    let res = run_one(
        "\
        local ok = pcall(string.gsub, 'hello', '%w+', function() return {} end)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn string_lib_gsub_bad_replacement_type() {
    // Passing a boolean as replacement should error.
    let res = run_one(
        "\
        local ok, msg = pcall(string.gsub, 'hello', '%w+', true)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn string_lib_gsub_anchored_pattern() {
    // `^%w+` should only replace the first word (anchored at start).
    let res = run_all("return string.gsub('hello world', '^%w+', 'BYE')").await;
    k9::assert_equal!(
        res,
        valuevec![Value::string("BYE world"), Value::Integer(1)]
    );
}

#[tokio::test]
async fn string_lib_gsub_empty_pattern_on_empty_string() {
    // Matches once at position 0 (end of zero-length subject).
    let res = run_all("return string.gsub('', '', '-')").await;
    k9::assert_equal!(res, valuevec![Value::string("-"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_empty_pattern_inserts_between_chars() {
    // Empty pattern matches between every character and at both ends
    // — four empty matches in "abc".  Subject characters must be
    // preserved between the inserted replacements.
    let res = run_all("return string.gsub('abc', '', '-')").await;
    k9::assert_equal!(res, valuevec![Value::string("-a-b-c-"), Value::Integer(4)]);
}

#[tokio::test]
async fn string_lib_gsub_empty_matching_pattern_no_duplicates() {
    // `a*` can match empty at every position after consuming an `a`
    // run; the duplicate-empty-match suppression should produce five
    // replacements on "aabbcc" (one non-empty "aa", then four empty).
    let res = run_all("return string.gsub('aabbcc', 'a*', '[%0]')").await;
    k9::assert_equal!(
        res,
        valuevec![Value::string("[aa]b[]b[]c[]c[]"), Value::Integer(5),]
    );
}

#[tokio::test]
async fn string_lib_gsub_table_with_position_capture_key() {
    // Position captures become 1-based integer keys when used as the
    // lookup key in a table replacement.
    let res = run_all(
        "\
        local t = { [2] = 'X', [3] = 'Y' }
        return string.gsub('abc', '()(%a)', t)",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("aXY"), Value::Integer(3)]);
}

#[tokio::test]
async fn string_lib_gmatch_always_empty_pattern() {
    // `a*` with `gmatch` on "aabbcc" should yield 5 matches: the
    // non-empty "aa" and four empty strings at each subsequent
    // position.  Duplicate empty matches at the same position must
    // be suppressed.
    let res = run_one(
        "\
        local parts = {}
        for m in string.gmatch('aabbcc', 'a*') do
            parts[#parts+1] = '['..m..']'
        end
        return table.concat(parts)",
    )
    .await;
    k9::assert_equal!(res, Value::string("[aa][][][][]"));
}

#[tokio::test]
async fn string_lib_match_balanced_brackets() {
    // `%b[]` matches a balanced `[...]` pair (new feature enabled by
    // the byte-level pattern matcher).
    let res = run_one("return string.match('x[[a]b]y', '%b[]')").await;
    k9::assert_equal!(res, Value::string("[[a]b]"));
}

#[tokio::test]
async fn string_lib_match_frontier_pattern() {
    // `%f[%u]` marks every boundary where a non-uppercase byte is
    // followed by an uppercase one.  Captured through gmatch to show
    // it fires at each frontier independently.
    let res = run_one(
        "\
        local parts = {}
        for w in string.gmatch('foo BAR baz QUX', '%f[%u]%u+') do
            parts[#parts+1] = w
        end
        return table.concat(parts, ',')",
    )
    .await;
    k9::assert_equal!(res, Value::string("BAR,QUX"));
}

#[tokio::test]
async fn string_lib_match_position_capture() {
    // `()` captures a 1-based byte position as an integer.
    let res = run_all("return string.match('abc XYZ', '()(%u+)')").await;
    k9::assert_equal!(res, valuevec![Value::Integer(5), Value::string("XYZ")]);
}

#[tokio::test]
async fn string_lib_match_backref_in_pattern() {
    // `(%a)%1` matches a repeated letter.
    let res = run_all("return string.match('aabbcc', '(%a)%1')").await;
    k9::assert_equal!(res, valuevec![Value::string("a")]);
}

#[tokio::test]
async fn string_lib_gsub_non_ascii_dot_matches_bytes() {
    // The motivating regression: `.` must match individual bytes of a
    // UTF-8 sequence, not whole codepoints.
    let res = run_one(
        "\
        local n = 0
        string.gsub('\\xc3\\xa9', '.', function() n = n + 1 end)
        return n",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(2));
}

// ---------------------------------------------------------------------------
// string.format — additional coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_format_integer_i() {
    // %i is an alias for %d.
    k9::assert_equal!(
        run_one("return string.format('%i', 42)").await,
        Value::string("42")
    );
}

#[tokio::test]
async fn string_lib_format_unsigned() {
    k9::assert_equal!(
        run_one("return string.format('%u', 42)").await,
        Value::string("42")
    );
}

#[tokio::test]
async fn string_lib_format_octal() {
    k9::assert_equal!(
        run_one("return string.format('%o', 255)").await,
        Value::string("377")
    );
}

#[tokio::test]
async fn string_lib_format_octal_alt() {
    // `#` flag prepends a `0` for octal.
    k9::assert_equal!(
        run_one("return string.format('%#o', 255)").await,
        Value::string("0377")
    );
}

#[tokio::test]
async fn string_lib_format_scientific() {
    let res = run_one("return string.format('%.2e', 314.159)").await;
    k9::assert_equal!(res, Value::string("3.14e2"));
}

#[tokio::test]
async fn string_lib_format_scientific_upper() {
    let res = run_one("return string.format('%.2E', 314.159)").await;
    k9::assert_equal!(res, Value::string("3.14E2"));
}

#[tokio::test]
async fn string_lib_format_general_float() {
    // %g uses shorter of %e and %f.
    k9::assert_equal!(
        run_one("return string.format('%g', 100000.0)").await,
        Value::string("100000")
    );
    k9::assert_equal!(
        run_one("return string.format('%g', 0.00123)").await,
        Value::string("0.00123")
    );
}

#[tokio::test]
async fn string_lib_format_char() {
    k9::assert_equal!(
        run_one("return string.format('%c', 65)").await,
        Value::string("A")
    );
}

#[tokio::test]
async fn string_lib_format_hex_alt() {
    // `#` flag prepends `0x` / `0X`.
    k9::assert_equal!(
        run_one("return string.format('%#x', 255)").await,
        Value::string("0xff")
    );
    k9::assert_equal!(
        run_one("return string.format('%#X', 255)").await,
        Value::string("0XFF")
    );
}

#[tokio::test]
async fn string_lib_format_plus_flag() {
    k9::assert_equal!(
        run_one("return string.format('%+d', 42)").await,
        Value::string("+42")
    );
    k9::assert_equal!(
        run_one("return string.format('%+d', -42)").await,
        Value::string("-42")
    );
}

#[tokio::test]
async fn string_lib_format_space_flag() {
    k9::assert_equal!(
        run_one("return string.format('% d', 42)").await,
        Value::string(" 42")
    );
    k9::assert_equal!(
        run_one("return string.format('% d', -42)").await,
        Value::string("-42")
    );
}

#[tokio::test]
async fn string_lib_format_left_align() {
    k9::assert_equal!(
        run_one("return string.format('%-10d|', 42)").await,
        Value::string("42        |")
    );
}

#[tokio::test]
async fn string_lib_format_width_space_pad() {
    k9::assert_equal!(
        run_one("return string.format('%10d', 42)").await,
        Value::string("        42")
    );
}

#[tokio::test]
async fn string_lib_format_string_precision() {
    // %.3s truncates the string to 3 characters.
    k9::assert_equal!(
        run_one("return string.format('%.3s', 'hello')").await,
        Value::string("hel")
    );
}

#[tokio::test]
async fn string_lib_format_string_coercion_number() {
    // Formatting a number with %s should produce its string form.
    k9::assert_equal!(
        run_one("return string.format('%s', 42)").await,
        Value::string("42")
    );
}

#[tokio::test]
async fn string_lib_format_integer_from_string() {
    // %d with a numeric string coerces to integer.
    k9::assert_equal!(
        run_one("return string.format('%d', '42')").await,
        Value::string("42")
    );
}

#[tokio::test]
async fn string_lib_format_float_from_string() {
    // %f with a numeric string coerces to float.
    k9::assert_equal!(
        run_one("return string.format('%.1f', '3.14')").await,
        Value::string("3.1")
    );
}

#[tokio::test]
async fn string_lib_format_too_few_args() {
    let res = run_one(
        "\
        local ok, msg = pcall(string.format, '%d %d', 1)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn string_lib_format_invalid_specifier() {
    let res = run_one(
        "\
        local ok, msg = pcall(string.format, '%z', 1)
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn string_lib_format_trailing_percent() {
    let res = run_one(
        "\
        local ok = pcall(string.format, 'oops%')
        return ok",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn string_lib_format_quoted_special_chars() {
    // %q should escape newlines, backslashes, null bytes, and \x1a.
    k9::assert_equal!(
        run_one("return string.format('%q', 'a\\nb')").await,
        Value::string("\"a\\nb\"")
    );
    k9::assert_equal!(
        run_one("return string.format('%q', 'a\"b')").await,
        Value::string("\"a\\\"b\"")
    );
}

#[tokio::test]
async fn string_lib_format_coerce_to_string_nil() {
    k9::assert_equal!(
        run_one("return string.format('%s', nil)").await,
        Value::string("nil")
    );
}

#[tokio::test]
async fn string_lib_format_coerce_to_string_bool() {
    k9::assert_equal!(
        run_one("return string.format('%s', true)").await,
        Value::string("true")
    );
}

#[tokio::test]
async fn string_lib_format_pointer_table() {
    // %p on a table should produce the same address as tostring gives
    let res = run_one(
        "\
        local t = {}
        local p = string.format('%p', t)
        local s = tostring(t)
        -- tostring gives 'table: 0x...', %p gives just '0x...'
        return s == 'table: ' .. p",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn string_lib_format_pointer_same_table() {
    // Same table should produce the same %p result
    let res = run_one(
        "\
        local t = {}
        return string.format('%p', t) == string.format('%p', t)",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn string_lib_format_pointer_different_tables() {
    // Different tables should produce different %p results
    let res = run_one(
        "\
        local a = {}
        local b = {}
        return string.format('%p', a) ~= string.format('%p', b)",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn string_lib_format_pointer_nil() {
    // %p on nil should produce a null pointer
    k9::assert_equal!(
        run_one("return string.format('%p', nil)").await,
        Value::string("0x0")
    );
}

#[tokio::test]
async fn string_lib_format_pointer_function() {
    // %p on a function should produce a non-null hex address
    let res = run_one(
        "\
        local f = function() end
        local p = string.format('%p', f)
        return p:sub(1, 2) == '0x' and #p > 3",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn string_lib_format_pointer_string() {
    // %p on a string should produce a non-null hex address
    let res = run_one(
        "\
        local p = string.format('%p', 'hello')
        return p:sub(1, 2) == '0x' and #p > 3",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn string_lib_format_pointer_integer() {
    // %p on a value type produces 0x0
    k9::assert_equal!(
        run_one("return string.format('%p', 42)").await,
        Value::string("0x0")
    );
}

#[tokio::test]
async fn string_lib_format_pointer_float() {
    k9::assert_equal!(
        run_one("return string.format('%p', 3.14)").await,
        Value::string("0x0")
    );
}

#[tokio::test]
async fn string_lib_format_pointer_boolean() {
    k9::assert_equal!(
        run_one("return string.format('%p', true)").await,
        Value::string("0x0")
    );
}

#[tokio::test]
async fn string_lib_format_pointer_in_larger_format() {
    // %p embedded in a format string with other specifiers
    let res = run_one(
        "\
        local t = {}
        local s = string.format('id=%p ok', t)
        -- should start with 'id=0x' and end with ' ok'
        return s:sub(1, 5) == 'id=0x' and s:sub(-3) == ' ok'",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

// ===========================================================================
// string.pack / string.unpack / string.packsize
// ===========================================================================

#[tokio::test]
async fn string_pack_unpack_integers() {
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('<i2i2', 1, 2)
               local a, b, pos = string.unpack('<i2i2', s)
               return a, b, pos"#
        )
        .await,
        valuevec![Value::Integer(1), Value::Integer(2), Value::Integer(5)]
    );
}

#[tokio::test]
async fn string_pack_unpack_bytes() {
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('bBb', -1, 255, 42)
               local a, b, c = string.unpack('bBb', s)
               return a, b, c"#
        )
        .await,
        valuevec![Value::Integer(-1), Value::Integer(255), Value::Integer(42),]
    );
}

#[tokio::test]
async fn string_pack_unpack_float_double() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('<d', 3.14)
               local v = string.unpack('<d', s)
               return v"#
        )
        .await,
        Value::Float(3.14)
    );
}

#[tokio::test]
async fn string_pack_unpack_zstring() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('z', 'hello')
               return string.unpack('z', s)"#
        )
        .await,
        Value::string("hello")
    );
}

#[tokio::test]
async fn string_pack_unpack_len_string() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('<s4', 'abc')
               return string.unpack('<s4', s)"#
        )
        .await,
        Value::string("abc")
    );
}

#[tokio::test]
async fn string_packsize_basic() {
    k9::assert_equal!(
        run_one("return string.packsize('i4d')").await,
        Value::Integer(12)
    );
}

#[tokio::test]
async fn string_pack_endianness() {
    // Big-endian 2-byte integer 0x0102 should be bytes 01 02.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('>i2', 0x0102)
               return string.byte(s, 1) * 256 + string.byte(s, 2)"#
        )
        .await,
        Value::Integer(0x0102)
    );
}

#[tokio::test]
async fn string_pack_method_syntax() {
    // string.pack should also work via method syntax on format string.
    k9::assert_equal!(
        run_one(
            r#"local fmt = '<i4'
               local s = fmt:pack(42)
               return (fmt:unpack(s))"#
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn string_unpack_with_position() {
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('<i2i2', 10, 20)
               local v, pos = string.unpack('<i2', s, 3)
               return v, pos"#
        )
        .await,
        valuevec![Value::Integer(20), Value::Integer(5)]
    );
}

#[tokio::test]
async fn string_pack_fixed_string() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('c5', 'hi')
               return #s"#
        )
        .await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn string_packsize_variable_length_errors() {
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(string.packsize, 'z')
               return ok"#
        )
        .await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// Integration coverage for recent string.pack behavior fixes and Lua-compat
// coercion. These exercise the VM surface (not just the string_pack unit
// tests) to ensure errors propagate, method-call syntax works, and the
// coercion rules match reference Lua when reached via real scripts.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_unpack_negative_init_pos() {
    // Negative init_pos counts from end of string (Lua 5.4 semantics).
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('bbbb', 10, 20, 30, 40)
               return (string.unpack('b', s, -1))"#
        )
        .await,
        Value::Integer(40)
    );
}

#[tokio::test]
async fn string_unpack_init_pos_zero_clamps_to_one() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', 99)
               return (string.unpack('b', s, 0))"#
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn string_unpack_init_pos_past_end_errors() {
    // Errors surface to `pcall` as a string — user scripts can inspect it.
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.unpack, '', 'abc', 100)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #3 to 'unpack' (initial position out of string)"),
        ]
    );
}

#[tokio::test]
async fn string_pack_method_syntax_roundtrip() {
    // fmt:pack(...) and fmt:unpack(s) should work via the string metatable.
    k9::assert_equal!(
        run_all(
            r#"local fmt = '<i4i2'
               local s = fmt:pack(42, 7)
               local a, b = fmt:unpack(s)
               return a, b"#
        )
        .await,
        valuevec![Value::Integer(42), Value::Integer(7)]
    );
}

#[tokio::test]
async fn string_pack_coerces_numeric_string_to_integer() {
    // Lua auto-coerces numeric strings for number slots in pack.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', '42')
               return string.byte(s, 1)"#
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn string_pack_coerces_hex_string_to_integer() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', '0x2a')
               return string.byte(s, 1)"#
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn string_pack_coerces_integer_to_string_slot() {
    // 42 stringifies to "42" for the c3 fixed-width slot (padded with NUL).
    k9::assert_equal!(
        run_all(
            r#"local s = string.pack('c3', 42)
               return string.byte(s, 1), string.byte(s, 2), string.byte(s, 3)"#
        )
        .await,
        valuevec![
            Value::Integer(b'4' as i64),
            Value::Integer(b'2' as i64),
            Value::Integer(0),
        ]
    );
}

#[tokio::test]
async fn string_pack_coerces_float_to_zstring() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('z', 3.14)
               return (string.unpack('z', s))"#
        )
        .await,
        Value::string("3.14")
    );
}

#[tokio::test]
async fn string_pack_rejects_boolean_for_number_slot() {
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, 'b', true)
               return ok"#
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn string_pack_rejects_nil_for_string_slot() {
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, 'c3', nil)
               return ok"#
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn string_pack_rejects_table_for_string_slot() {
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, 'z', {})
               return ok"#
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn string_pack_s1_length_overflow_error() {
    k9::assert_equal!(
        run_all(
            r#"local big = string.rep('x', 256)
               local ok, err = pcall(string.pack, 's1', big)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (string length does not fit in given size)"),
        ]
    );
}

#[tokio::test]
async fn string_pack_error_is_readable_string() {
    // Pack errors surface to `pcall` as strings (not nil), matching Lua's
    // `bad argument #N to 'funcname' (msg)` format from `luaL_argerror`.
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.unpack, 'z', 'abc')
               return type(err), err"#
        )
        .await,
        valuevec![
            Value::string("string"),
            Value::string("bad argument #2 to 'unpack' (unfinished string for format 'z')"),
        ]
    );
}

#[tokio::test]
async fn string_pack_extra_args_silently_ignored() {
    // Pack consumes only as many args as the format requires.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', 1, 2, 3, 4)
               return #s"#
        )
        .await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn string_pack_binary_roundtrip_preserves_bytes() {
    // Non-ASCII / NUL-containing payloads round-trip through s<n>.
    k9::assert_equal!(
        run_all(
            r#"local data = '\0\255\127\128'
               local s = string.pack('<s1', data)
               local out, pos = string.unpack('<s1', s)
               return out, pos"#
        )
        .await,
        valuevec![
            Value::string(&[0x00u8, 0xFF, 0x7F, 0x80][..]),
            Value::Integer(6),
        ]
    );
}

#[tokio::test]
async fn string_pack_empty_format_noop() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('')
               return #s"#
        )
        .await,
        Value::Integer(0)
    );
    k9::assert_equal!(
        run_one("return string.packsize('')").await,
        Value::Integer(0)
    );
}

#[tokio::test]
async fn string_pack_alignment_mid_format() {
    // `<b !4 i4` pads the i4 to a 4-byte boundary after the leading byte.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('<b !4 i4', 1, 0x12345678)
               return #s"#
        )
        .await,
        Value::Integer(8)
    );
}

#[tokio::test]
async fn string_pack_non_power_of_2_alignment_only_when_applied() {
    // `!3 b` is accepted (b has align 1); `!3 b i4` is rejected at the i4.
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('!3 b', 1)
               return #s"#
        )
        .await,
        Value::Integer(1)
    );
    k9::assert_equal!(
        run_one(
            r#"local ok = pcall(string.pack, '!3 b i4', 1, 2)
               return ok"#
        )
        .await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// X-op follower validation — Lua rejects bytes its `getoption` classifies
// as `Knop` (space, `<`, `>`, `=`, `!`) and another `X`, even though the
// outer parser loop would otherwise skip those silently.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_pack_x_followed_by_space_errors() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'X i4', 1)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'pack' (invalid next option for option 'X')"),
        ]
    );
}

#[tokio::test]
async fn string_pack_x_followed_by_endian_errors() {
    for byte in ["<", ">", "="] {
        let script = format!(
            "local ok, err = pcall(string.pack, 'X{}i4', 1)\n return ok, err",
            byte
        );
        k9::assert_equal!(
            run_all(&script).await,
            valuevec![
                Value::Boolean(false),
                Value::string("bad argument #1 to 'pack' (invalid next option for option 'X')"),
            ],
            "fmt: X{}i4",
            byte
        );
    }
}

#[tokio::test]
async fn string_pack_x_followed_by_bang_errors() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'X!4i4', 1)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'pack' (invalid next option for option 'X')"),
        ]
    );
}

// ---------------------------------------------------------------------------
// Float-to-integer coercion — Lua requires an exact integer representation.
// These error paths are shared with `string.format("%d", ...)`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_pack_rejects_fractional_float() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i4', 3.5)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (number has no integer representation)"),
        ]
    );
}

#[tokio::test]
async fn string_pack_accepts_whole_valued_float() {
    k9::assert_equal!(
        run_one(
            r#"local s = string.pack('b', 42.0)
               return string.byte(s, 1)"#
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn string_pack_rejects_infinity_and_nan() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i4', 1/0)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (number has no integer representation)"),
        ]
    );
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i4', 0/0)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (number has no integer representation)"),
        ]
    );
}

// When `string.pack` is given fewer arguments than the format consumes,
// reference Lua reads nil from past the stack top and reports
// `got nil` — not `got no value`.  Earlier our helper distinguished the
// two; now a missing arg is treated as nil and flows through the
// standard coerce path.
#[tokio::test]
async fn string_pack_missing_int_arg_reports_nil() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'bb', 1)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #3 to 'pack' (number expected, got nil)"),
        ]
    );
}

#[tokio::test]
async fn string_pack_missing_string_arg_reports_nil() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'c3')
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (string expected, got nil)"),
        ]
    );
}

// Lua's string→number parser (`l_str2d`) rejects any input containing
// `n` or `N`, so strings like `"nan"`/`"inf"`/`"Inf"` are NOT numbers.
// In pack's integer slot this surfaces as a type error (the value is
// still a string), distinct from `"3.5"` which parses as a valid float
// that merely lacks an integer representation.
#[tokio::test]
async fn string_pack_int_rejects_nan_string_as_type_error() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i8', 'nan')
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (number expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn string_pack_int_rejects_inf_string_as_type_error() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'i8', 'inf')
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (number expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn string_pack_float_rejects_nan_string() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.pack, 'f', 'nan')
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'pack' (number expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn string_format_f_rejects_nan_string() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.format, '%f', 'nan')
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'format' (number expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn string_format_rejects_fractional_float_for_d() {
    // Same underlying coercion as pack — verifies the fix propagates
    // to `string.format("%d", ...)`.
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.format, '%d', 3.5)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'format' (number has no integer representation)"),
        ]
    );
}

// ---------------------------------------------------------------------------
// `string.unpack` init_pos coercion — accepts numeric strings, rejects
// fractional floats.  Mirrors Lua's behavior for integer-typed arguments.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_unpack_init_pos_accepts_numeric_string() {
    k9::assert_equal!(
        run_one(r#"return (string.unpack('b', 'abc', '2'))"#).await,
        Value::Integer(b'b' as i64)
    );
}

#[tokio::test]
async fn string_unpack_init_pos_rejects_fractional_float() {
    k9::assert_equal!(
        run_all(
            r#"local ok, err = pcall(string.unpack, 'b', 'abc', 2.5)
               return ok, err"#
        )
        .await,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #3 to 'unpack' (number has no integer representation)"),
        ]
    );
}

#[tokio::test]
async fn string_unpack_init_pos_accepts_whole_float() {
    k9::assert_equal!(
        run_one(r#"return (string.unpack('b', 'abc', 2.0))"#).await,
        Value::Integer(b'b' as i64)
    );
}

// ---------------------------------------------------------------------------
// Pattern edge cases: position-capture backrefs, anchored gsub, and
// metatable-aware table replacement (tracked against Lua 5.4.4 reference).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_match_position_capture_backref_fails() {
    // `()%1` can never match: the backref's "length" derived from a
    // position capture is a sentinel value that always fails the
    // length check in the reference.  The overall match returns nil
    // without raising a pattern error.
    k9::assert_equal!(
        run_one("return string.match('abc', '()%1')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn string_lib_find_position_capture_backref_fails() {
    k9::assert_equal!(
        run_one("return string.find('abc', '()%1')").await,
        Value::Nil
    );
}

#[tokio::test]
async fn string_lib_gsub_anchored_replaces_once() {
    // `string.gsub("aaa", "^a", "X", 5)` only replaces the first `a`
    // even though `max_n = 5` would permit more: the `^` anchor binds
    // to the start of the subject and cannot match a second time.
    let res = run_all(r#"return string.gsub('aaa', '^a', 'X', 5)"#).await;
    k9::assert_equal!(res, valuevec![Value::string("Xaa"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_anchored_without_max_replaces_once() {
    // Same behaviour with no explicit `max_n`.
    let res = run_all(r#"return string.gsub('aaa', '^a', 'X')"#).await;
    k9::assert_equal!(res, valuevec![Value::string("Xaa"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_table_repl_uses_index_table_metamethod() {
    // The replacement table has no `a` key raw, but its `__index`
    // metatable does.  Reference Lua looks up through `__index`.
    let src = r#"
        local fallback = { a = 'A', b = 'B', c = 'C' }
        local t = setmetatable({}, { __index = fallback })
        return string.gsub('abc', '%a', t)
    "#;
    let res = run_all(src).await;
    k9::assert_equal!(res, valuevec![Value::string("ABC"), Value::Integer(3)]);
}

#[tokio::test]
async fn string_lib_gsub_table_repl_uses_index_function_metamethod() {
    // Function `__index` is dispatched by the VM so that it can run
    // Lua code (e.g. upper-casing the key).
    let src = r#"
        local t = setmetatable({}, {
            __index = function(_, k) return string.upper(k) end,
        })
        return string.gsub('abc', '%a', t)
    "#;
    let res = run_all(src).await;
    k9::assert_equal!(res, valuevec![Value::string("ABC"), Value::Integer(3)]);
}

#[tokio::test]
async fn string_lib_gsub_table_repl_raw_hit_bypasses_metamethod() {
    // Raw keys take precedence over `__index`, matching `lua_gettable`.
    let src = r#"
        local fallback = { a = 'FALLBACK' }
        local t = setmetatable({ a = 'RAW' }, { __index = fallback })
        return (string.gsub('a', '%a', t))
    "#;
    k9::assert_equal!(run_one(src).await, Value::string("RAW"));
}

// ---------------------------------------------------------------------------
// Anchor-only patterns, multi-position captures, and replacement-string
// escape edge cases cross-checked against Lua 5.4.4.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_match_bare_caret_matches_empty_at_start() {
    k9::assert_equal!(
        run_one("return string.match('abc', '^')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn string_lib_match_bare_dollar_matches_empty_at_end() {
    k9::assert_equal!(
        run_one("return string.match('abc', '$')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn string_lib_match_caret_dollar_on_nonempty_fails() {
    // `^$` only matches the empty string.
    k9::assert_equal!(
        run_one("return string.match('abc', '^$')").await,
        Value::Nil
    );
    // But does match an empty subject.
    k9::assert_equal!(
        run_one("return string.match('', '^$')").await,
        Value::string("")
    );
}

#[tokio::test]
async fn string_lib_match_multiple_position_captures() {
    let res = run_all("return string.match('hello', '()l()l()')").await;
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(3), Value::Integer(4), Value::Integer(5),]
    );
}

#[tokio::test]
async fn string_lib_match_palindromic_backref() {
    let res = run_all("return string.match('abba', '(.)(.)%2%1')").await;
    k9::assert_equal!(res, valuevec![Value::string("a"), Value::string("b")]);
}

#[tokio::test]
async fn string_lib_gsub_replacement_percent_zero_is_whole_match() {
    let res = run_all(r#"return string.gsub('abc', '%a', '[%0]')"#).await;
    k9::assert_equal!(
        res,
        valuevec![Value::string("[a][b][c]"), Value::Integer(3),]
    );
}

#[tokio::test]
async fn string_lib_gsub_replacement_percent_one_zero_is_cap_plus_literal() {
    // Reference Lua treats `%10` as `%1` followed by literal `0`,
    // not as a 10th capture reference.
    let res = run_all(r#"return string.gsub('abc', '(%a)', '%10')"#).await;
    k9::assert_equal!(res, valuevec![Value::string("a0b0c0"), Value::Integer(3),]);
}

#[tokio::test]
async fn string_lib_gsub_max_n_zero_returns_input_unchanged() {
    let res = run_all(r#"return string.gsub('abc', 'a', 'X', 0)"#).await;
    k9::assert_equal!(res, valuevec![Value::string("abc"), Value::Integer(0)]);
}

#[tokio::test]
async fn string_lib_gsub_max_n_negative_treated_as_zero() {
    let res = run_all(r#"return string.gsub('abc', 'a', 'X', -1)"#).await;
    k9::assert_equal!(res, valuevec![Value::string("abc"), Value::Integer(0)]);
}

#[tokio::test]
async fn string_lib_gsub_replacement_trailing_percent_errors() {
    let err = common::run_err(r#"return string.gsub('abc', 'a', '%')"#).await;
    k9::assert_equal!(
        err,
        "\
error: invalid use of '%' in replacement string
 --> test.lua:1:8
  |
1 | return string.gsub('abc', 'a', '%')
  |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ invalid use of '%' in replacement string
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn string_lib_gsub_function_receives_position_as_integer() {
    // A position capture `()` passed to the replacement function
    // arrives as a Lua integer, not a string.
    let src = r#"
        local types = {}
        string.gsub('abc', '()', function(p)
            table.insert(types, type(p))
        end)
        return table.concat(types, ',')
    "#;
    k9::assert_equal!(
        run_one(src).await,
        Value::string("number,number,number,number")
    );
}

#[tokio::test]
async fn string_lib_match_balanced_square_brackets() {
    k9::assert_equal!(
        run_one(r#"return string.match('foo[bar[baz]qux]end', '%b[]')"#).await,
        Value::string("[bar[baz]qux]")
    );
}

#[tokio::test]
async fn string_lib_match_balanced_parens_with_nested_braces() {
    // `%b()` only tracks `(` / `)`, so an unbalanced-looking brace
    // inside is still captured transparently.
    k9::assert_equal!(
        run_one(r#"return string.match('foo(bar{baz}qux)', '%b()')"#).await,
        Value::string("(bar{baz}qux)")
    );
}

// ---------------------------------------------------------------------------
// string.split (LuaU extension)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_split_default_separator() {
    // Default separator is ",".
    k9::assert_equal!(
        run_all(
            "local t = string.split('a,b,c')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_custom_separator() {
    k9::assert_equal!(
        run_all(
            "local t = string.split('a-b-c', '-')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_multi_byte_separator() {
    k9::assert_equal!(
        run_all(
            "local t = string.split('aXXbXXc', 'XX')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_empty_separator_yields_bytes() {
    // Empty separator splits into individual bytes.
    k9::assert_equal!(
        run_all(
            "local t = string.split('abc', '')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_separator_not_found() {
    k9::assert_equal!(
        run_all(
            "local t = string.split('abc', 'x')
return #t, t[1]"
        )
        .await,
        valuevec![Value::Integer(1), Value::string("abc")]
    );
}

#[tokio::test]
async fn string_lib_split_empty_input() {
    // An empty input with a non-empty separator yields one empty piece.
    k9::assert_equal!(
        run_all(
            "local t = string.split('', ',')
return #t, t[1]"
        )
        .await,
        valuevec![Value::Integer(1), Value::string("")]
    );
}

#[tokio::test]
async fn string_lib_split_empty_input_empty_sep() {
    // LuaU returns an empty table when both input and separator are
    // empty — no bytes to emit.
    k9::assert_equal!(
        run_one(
            "local t = string.split('', '')
return #t"
        )
        .await,
        Value::Integer(0)
    );
}

#[tokio::test]
async fn string_lib_split_preserves_empty_segments() {
    // Leading and trailing separators surface as empty pieces.
    k9::assert_equal!(
        run_all(
            "local t = string.split(',a,', ',')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string(""),
            Value::string("a"),
            Value::string(""),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_consecutive_separators() {
    k9::assert_equal!(
        run_all(
            "local t = string.split('a,,b', ',')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string("a"),
            Value::string(""),
            Value::string("b"),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_method_syntax() {
    k9::assert_equal!(
        run_all(
            "local t = ('one;two;three'):split(';')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string("one"),
            Value::string("two"),
            Value::string("three"),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_separator_is_literal_not_a_pattern() {
    // `.` is a pattern metacharacter in `string.match` / `gmatch`, but
    // `split` treats its separator as a plain byte sequence.  If this
    // ever regresses to pattern semantics, the result would be 3 empty
    // strings ("." matches every byte) instead of 3 pieces.
    k9::assert_equal!(
        run_all(
            "local t = string.split('a.b.c', '.')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
        ]
    );
}

#[tokio::test]
async fn string_lib_split_matches_are_non_overlapping() {
    // After a match, the separator is consumed; we do not re-examine
    // its interior.  `"aaa"` split by `"aa"` therefore yields two
    // pieces, not three.
    k9::assert_equal!(
        run_all(
            "local t = string.split('aaa', 'aa')
return #t, t[1], t[2]"
        )
        .await,
        valuevec![Value::Integer(2), Value::string(""), Value::string("a"),]
    );
}

#[tokio::test]
async fn string_lib_split_separator_longer_than_input() {
    k9::assert_equal!(
        run_all(
            "local t = string.split('ab', 'abcd')
return #t, t[1]"
        )
        .await,
        valuevec![Value::Integer(1), Value::string("ab")]
    );
}

#[tokio::test]
async fn string_lib_split_separator_equals_whole_input() {
    // A single whole-string match splits into two empty pieces.
    k9::assert_equal!(
        run_all(
            "local t = string.split('abc', 'abc')
return #t, t[1], t[2]"
        )
        .await,
        valuevec![Value::Integer(2), Value::string(""), Value::string(""),]
    );
}

#[tokio::test]
async fn string_lib_split_is_byte_based() {
    // Embedded NULs and non-UTF-8 bytes go through unchanged; offsets
    // are measured in bytes, not codepoints.
    k9::assert_equal!(
        run_all(
            "local t = string.split('\\0a\\0b', '\\0')
return #t, t[1], t[2], t[3]"
        )
        .await,
        valuevec![
            Value::Integer(3),
            Value::string(""),
            Value::string("a"),
            Value::string("b"),
        ]
    );
}

// ---------------------------------------------------------------------------
// string.gsub — implicit capture (%1 with no explicit captures)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_lib_gsub_no_captures_pct1_is_whole_match() {
    // When the pattern has no explicit captures, %1 in the replacement
    // refers to the whole match (same as %0). This matches Lua 5.4.
    let res = run_all("return string.gsub('abc', '%w', '%1%0')").await;
    k9::assert_equal!(res, valuevec![Value::string("aabbcc"), Value::Integer(3)]);
}

#[tokio::test]
async fn string_lib_gsub_no_captures_pct1_whole_word() {
    // Same behavior with a multi-char match.
    let res = run_all("return string.gsub('abc', '%w+', '%0%1')").await;
    k9::assert_equal!(res, valuevec![Value::string("abcabc"), Value::Integer(1)]);
}

#[tokio::test]
async fn string_lib_gsub_no_captures_pct2_is_error() {
    // %2 with no explicit captures is an error, even though %1 is valid.
    let res = run_err("return string.gsub('abc', '%w', '%2')").await;
    k9::assert_equal!(
        res,
        "\
error: invalid capture index %2
 --> test.lua:1:8
  |
1 | return string.gsub('abc', '%w', '%2')
  |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ invalid capture index %2
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn string_lib_gsub_invalid_replacement_table_value() {
    k9::assert_equal!(
        run_err("return string.gsub('alo', '.', {a = {}})").await,
        "\
error: invalid replacement value (a table)
 --> test.lua:1:8
  |
1 | return string.gsub('alo', '.', {a = {}})
  |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ invalid replacement value (a table)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn string_lib_gsub_invalid_replacement_boolean() {
    k9::assert_equal!(
        run_err("return string.gsub('alo', '.', function() return true end)").await,
        "\
error: invalid replacement value (a boolean)
 --> test.lua:1:8
  |
1 | return string.gsub('alo', '.', function() return true end)
  |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ invalid replacement value (a boolean)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn string_packsize_overflow_in_size() {
    k9::assert_equal!(
        run_err(&format!(
            "return string.packsize('c1{}')" ,
            "0".repeat(40)
        )).await,
        "\
error: invalid format (size '10000000000000000000000000000000000000000' too large)
 --> test.lua:1:8
  |
1 | return string.packsize('c10000000000000000000000000000000000000000')
  |        ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ invalid format (size '10000000000000000000000000000000000000000' too large)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}
