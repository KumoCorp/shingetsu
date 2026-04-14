mod common;

use bytes::Bytes;
use common::{run_all, run_err, run_one};
use shingetsu_vm::Value;

// ===========================================================================
// os library
// ===========================================================================

/// 2000-01-01 00:00:00 UTC.
const Y2K: i64 = 946684800;
/// 2000-03-05 08:07:09 UTC (a Sunday).
const MAR5: i64 = 952243629;

#[test]
fn os_clock_returns_number() {
    // os.clock() returns a float >= 0.
    let v = run_one("return os.clock()");
    match v {
        Value::Float(f) => assert!(f >= 0.0, "os.clock() returned {}", f),
        other => panic!("expected float, got {:?}", other),
    }
}

#[test]
fn os_clock_monotonic() {
    // Two successive calls should be non-decreasing.
    k9::assert_equal!(
        run_one("local a = os.clock(); local b = os.clock(); return b >= a"),
        Value::Boolean(true)
    );
}

#[test]
fn os_time_returns_integer() {
    // os.time() returns a positive integer (Unix timestamp).
    let v = run_one("return os.time()");
    match v {
        Value::Integer(n) => assert!(n > 1_000_000_000, "timestamp too small: {}", n),
        other => panic!("expected integer, got {:?}", other),
    }
}

#[test]
fn os_time_with_table() {
    // Known epoch: 2000-01-01 00:00:00 UTC.
    k9::assert_equal!(
        run_one("return os.time({ year = 2000, month = 1, day = 1, hour = 0, min = 0, sec = 0 })"),
        Value::Integer(Y2K)
    );
}

#[test]
fn os_time_table_defaults() {
    // hour/min/sec default to 12:00:00 when omitted.
    k9::assert_equal!(
        run_one("return os.time({ year = 2000, month = 1, day = 1 })"),
        Value::Integer(Y2K + 12 * 3600)
    );
}

#[test]
fn os_time_table_bad_month() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 13, day = 1 })"),
        "bad argument #1 to 'time' (month in 1..12 expected, got 13)"
    );
}

#[test]
fn os_time_bad_arg() {
    k9::assert_equal!(
        run_err("os.time(42)"),
        "bad argument #1 to 'time' (table expected, got number)"
    );
}

#[test]
fn os_difftime() {
    k9::assert_equal!(run_one("return os.difftime(100, 30)"), Value::Float(70.0));
}

#[test]
fn os_difftime_negative() {
    k9::assert_equal!(run_one("return os.difftime(30, 100)"), Value::Float(-70.0));
}

#[test]
fn os_date_star_t_utc() {
    // os.date("!*t", Y2K) should be 2000-01-01 00:00:00 UTC, Saturday.
    let results = run_all(&format!(
        "local t = os.date('!*t', {Y2K})\n\
         return t.year, t.month, t.day, t.hour, t.min, t.sec, t.wday, t.yday"
    ));
    k9::assert_equal!(results[0], Value::Integer(2000)); // year
    k9::assert_equal!(results[1], Value::Integer(1)); // month
    k9::assert_equal!(results[2], Value::Integer(1)); // day
    k9::assert_equal!(results[3], Value::Integer(0)); // hour
    k9::assert_equal!(results[4], Value::Integer(0)); // min
    k9::assert_equal!(results[5], Value::Integer(0)); // sec
    k9::assert_equal!(results[6], Value::Integer(7)); // wday (Saturday)
    k9::assert_equal!(results[7], Value::Integer(1)); // yday
}

#[test]
fn os_date_format_utc() {
    // Known timestamp: 2000-01-01 00:00:00 UTC.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y-%m-%d %H:%M:%S', {Y2K})")),
        Value::String(Bytes::from("2000-01-01 00:00:00"))
    );
}

#[test]
fn os_date_weekday_names() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%A', {Y2K})")),
        Value::String(Bytes::from("Saturday"))
    );
    k9::assert_equal!(
        run_one(&format!("return os.date('!%a', {Y2K})")),
        Value::String(Bytes::from("Sat"))
    );
}

#[test]
fn os_date_month_names() {
    // March 15, 2023 = 1678838400
    k9::assert_equal!(
        run_one("return os.date('!%B', 1678838400)"),
        Value::String(Bytes::from("March"))
    );
    k9::assert_equal!(
        run_one("return os.date('!%b', 1678838400)"),
        Value::String(Bytes::from("Mar"))
    );
}

#[test]
fn os_date_twelve_hour() {
    // 2000-01-01 15:30:00 UTC = Y2K + 15*3600 + 30*60 = 946740600.
    k9::assert_equal!(
        run_one("return os.date('!%I:%M %p', 946740600)"),
        Value::String(Bytes::from("03:30 PM"))
    );
}

#[test]
fn os_date_day_of_year() {
    // Feb 1 2000 = day 32.
    // Y2K + 31*86400 = 949363200
    k9::assert_equal!(
        run_one("return os.date('!%j', 949363200)"),
        Value::String(Bytes::from("032"))
    );
}

#[test]
fn os_date_percent_escape() {
    k9::assert_equal!(
        run_one("return os.date('!100%%', 0)"),
        Value::String(Bytes::from("100%"))
    );
}

#[test]
fn os_date_default_format() {
    // os.date() with no args should return a non-empty string.
    let v = run_one("return os.date()");
    match v {
        Value::String(s) => assert!(!s.is_empty(), "os.date() returned empty string"),
        other => panic!("expected string, got {:?}", other),
    }
}

#[test]
fn os_date_two_digit_year() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%y', {Y2K})")),
        Value::String(Bytes::from("00"))
    );
}

#[test]
fn os_date_star_t_has_isdst() {
    // isdst field should be present (as boolean).
    k9::assert_equal!(
        run_one("local t = os.date('!*t', 0); return type(t.isdst)"),
        Value::String(Bytes::from("boolean"))
    );
}

#[test]
fn os_time_roundtrip() {
    // os.time(os.date("!*t", X)) should return X.
    k9::assert_equal!(
        run_one(&format!("return os.time(os.date('!*t', {Y2K}))")),
        Value::Integer(Y2K)
    );
}

// -- os.difftime edge cases --

#[test]
fn os_difftime_float_args() {
    k9::assert_equal!(
        run_one("return os.difftime(100.5, 30.25)"),
        Value::Float(70.25)
    );
}

#[test]
fn os_difftime_bad_arg() {
    k9::assert_equal!(
        run_err("os.difftime('hello', 1)"),
        "bad argument #1 to 'difftime' (number expected, got string)"
    );
}

// -- os.time error paths --

#[test]
fn os_time_missing_year() {
    k9::assert_equal!(
        run_err("os.time({ month = 1, day = 1 })"),
        "bad argument #1 to 'time' (integer for field 'year' expected, got field 'year' is missing)"
    );
}

#[test]
fn os_time_missing_month() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, day = 1 })"),
        "bad argument #1 to 'time' (integer for field 'month' expected, got field 'month' is missing)"
    );
}

#[test]
fn os_time_missing_day() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 1 })"),
        "bad argument #1 to 'time' (integer for field 'day' expected, got field 'day' is missing)"
    );
}

#[test]
fn os_time_invalid_day() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 1, day = 32 })"),
        "bad argument #1 to 'time' (valid date expected, got day was not in range)"
    );
}

#[test]
fn os_time_invalid_hour() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 1, day = 1, hour = 25 })"),
        "bad argument #1 to 'time' (valid time expected, got hour was not in range)"
    );
}

#[test]
fn os_time_month_zero() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 0, day = 1 })"),
        "bad argument #1 to 'time' (month in 1..12 expected, got 0)"
    );
}

// -- os.date strftime specifiers --

// Use a known timestamp: 2000-03-05 08:07:09 UTC (Sunday)
// Y2K + 63*86400 + 8*3600 + 7*60 + 9 = MAR5
// March 5 2000 is a Sunday.

#[test]
fn os_date_zero_padded_day() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%d', {MAR5})")),
        Value::String(Bytes::from("05"))
    );
}

#[test]
fn os_date_space_padded_day() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%e', {MAR5})")),
        Value::String(Bytes::from(" 5"))
    );
}

#[test]
fn os_date_numeric_month() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%m', {MAR5})")),
        Value::String(Bytes::from("03"))
    );
}

#[test]
fn os_date_minute() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%M', {MAR5})")),
        Value::String(Bytes::from("07"))
    );
}

#[test]
fn os_date_second() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%S', {MAR5})")),
        Value::String(Bytes::from("09"))
    );
}

#[test]
fn os_date_four_digit_year() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y', {MAR5})")),
        Value::String(Bytes::from("2000"))
    );
}

#[test]
fn os_date_weekday_number() {
    // Sunday = 0
    k9::assert_equal!(
        run_one(&format!("return os.date('!%w', {MAR5})")),
        Value::String(Bytes::from("0"))
    );
}

#[test]
fn os_date_abbreviated_month_h() {
    // %h is an alias for %b.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%h', {MAR5})")),
        Value::String(Bytes::from("Mar"))
    );
}

#[test]
fn os_date_locale_date() {
    // %x expands to %m/%d/%y.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%x', {MAR5})")),
        Value::String(Bytes::from("03/05/00"))
    );
}

#[test]
fn os_date_locale_time() {
    // %X expands to %H:%M:%S.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%X', {MAR5})")),
        Value::String(Bytes::from("08:07:09"))
    );
}

#[test]
fn os_date_locale_datetime() {
    // %c expands to "%a %b %e %H:%M:%S %Y".
    k9::assert_equal!(
        run_one(&format!("return os.date('!%c', {MAR5})")),
        Value::String(Bytes::from("Sun Mar  5 08:07:09 2000"))
    );
}

#[test]
fn os_date_week_number_sunday() {
    // 2000-03-05 is day 65, Sunday (wday=0).
    // %U = (65 - 0 + 7) / 7 = 72 / 7 = 10.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%U', {MAR5})")),
        Value::String(Bytes::from("10"))
    );
}

#[test]
fn os_date_week_number_monday() {
    // 2000-03-05 is day 65, Sunday (Monday-based wday=6).
    // %W = (65 - 6 + 7) / 7 = 66 / 7 = 9.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%W', {MAR5})")),
        Value::String(Bytes::from("09"))
    );
}

#[test]
fn os_date_utc_offset() {
    // With '!' prefix the offset is UTC → +0000.
    k9::assert_equal!(
        run_one("return os.date('!%z', 0)"),
        Value::String(Bytes::from("+0000"))
    );
}

#[test]
fn os_date_timezone_name_utc() {
    k9::assert_equal!(
        run_one("return os.date('!%Z', 0)"),
        Value::String(Bytes::from("UTC"))
    );
}

#[test]
fn os_date_twelve_hour_midnight() {
    // Midnight: hour=0, %I should show 12.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%I', {Y2K})")),
        Value::String(Bytes::from("12"))
    );
}

#[test]
fn os_date_twelve_hour_noon() {
    // Noon: hour=12, %I should show 12.
    // Y2K + 12*3600 = 946728000
    k9::assert_equal!(
        run_one("return os.date('!%I', 946728000)"),
        Value::String(Bytes::from("12"))
    );
}

#[test]
fn os_date_am_indicator() {
    // Midnight is AM.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%p', {Y2K})")),
        Value::String(Bytes::from("AM"))
    );
}

#[test]
fn os_date_trailing_percent() {
    // A lone '%' at end of format string.
    k9::assert_equal!(
        run_one("return os.date('!hello%', 0)"),
        Value::String(Bytes::from("hello%"))
    );
}

#[test]
fn os_date_unknown_specifier() {
    // Unknown specifier should be output literally.
    k9::assert_equal!(
        run_one("return os.date('!%q', 0)"),
        Value::String(Bytes::from("%q"))
    );
}

#[test]
fn os_date_bad_format_type() {
    k9::assert_equal!(
        run_err("os.date(42)"),
        "bad argument #1 to 'date' (string expected, got number)"
    );
}

#[test]
fn os_date_epoch_star_t() {
    // Unix epoch: 1970-01-01 00:00:00 UTC, Thursday.
    let results = run_all(
        "local t = os.date('!*t', 0)\n\
         return t.year, t.month, t.day, t.wday, t.yday",
    );
    k9::assert_equal!(results[0], Value::Integer(1970)); // year
    k9::assert_equal!(results[1], Value::Integer(1)); // month
    k9::assert_equal!(results[2], Value::Integer(1)); // day
    k9::assert_equal!(results[3], Value::Integer(5)); // wday (Thursday = 5)
    k9::assert_equal!(results[4], Value::Integer(1)); // yday
}

#[test]
fn os_date_combined_specifiers() {
    // Multiple specifiers in one format string.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%d/%m/%Y', {MAR5})")),
        Value::String(Bytes::from("05/03/2000"))
    );
}

#[test]
fn os_date_literal_text() {
    // Literal text passes through unchanged.
    k9::assert_equal!(
        run_one("return os.date('!hello world', 0)"),
        Value::String(Bytes::from("hello world"))
    );
}

#[test]
fn os_date_local_time_path() {
    // Without '!' prefix, exercises the local-time branch.
    // Result varies by environment, but should be a non-empty string.
    let v = run_one("return os.date('%Y', 0)");
    match v {
        Value::String(s) => assert!(!s.is_empty(), "os.date local returned empty"),
        other => panic!("expected string, got {:?}", other),
    }
}

#[test]
fn os_date_star_t_local() {
    // "*t" without '!' returns a table via the local-time path.
    let v = run_one("return type(os.date('*t', 0))");
    k9::assert_equal!(v, Value::String(Bytes::from("table")));
}

#[test]
fn os_date_float_timestamp() {
    // Float timestamp is accepted and truncated to integer.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y', {Y2K}.5)")),
        Value::String(Bytes::from("2000"))
    );
}

#[test]
fn os_date_format_no_timestamp() {
    // Explicit format with no timestamp defaults to current time.
    let v = run_one("return os.date('!%Y')");
    match v {
        Value::String(s) => {
            let year: i32 = String::from_utf8_lossy(&s).parse().expect("parse year");
            assert!(year >= 2024, "year too small: {}", year);
        }
        other => panic!("expected string, got {:?}", other),
    }
}

#[test]
fn os_time_bad_field_type() {
    k9::assert_equal!(
        run_err("os.time({ year = 'hello', month = 1, day = 1 })"),
        "bad argument #1 to 'time' (integer for field 'year' expected, got string)"
    );
}

#[test]
fn os_difftime_bad_second_arg() {
    k9::assert_equal!(
        run_err("os.difftime(1, 'hello')"),
        "bad argument #2 to 'difftime' (number expected, got string)"
    );
}

#[test]
fn os_difftime_nil_arg() {
    k9::assert_equal!(
        run_err("os.difftime(nil, 1)"),
        "bad argument #1 to 'difftime' (number expected, got nil)"
    );
}

#[test]
fn os_difftime_bool_arg() {
    k9::assert_equal!(
        run_err("os.difftime(true, 1)"),
        "bad argument #1 to 'difftime' (number expected, got boolean)"
    );
}

#[test]
fn os_time_bool_arg() {
    k9::assert_equal!(
        run_err("os.time(true)"),
        "bad argument #1 to 'time' (table expected, got boolean)"
    );
}

#[test]
fn os_date_bad_timestamp_type() {
    k9::assert_equal!(
        run_err("os.date('!%Y', 'hello')"),
        "bad argument #2 to 'date' (number expected, got string)"
    );
}

#[test]
fn os_date_bool_format() {
    k9::assert_equal!(
        run_err("os.date(true)"),
        "bad argument #1 to 'date' (string expected, got boolean)"
    );
}

#[test]
fn os_time_extra_field_ignored() {
    // Extra fields in the table are silently ignored. This is correct Lua
    // behavior — os.date("*t") returns wday/yday/isdst which os.time ignores.
    k9::assert_equal!(
        run_one(&format!("return os.time({{ year = 2000, month = 1, day = 1, hour = 0, min = 0, sec = 0, bogus = 42 }})")),
        Value::Integer(Y2K)
    );
}

// ===========================================================================
