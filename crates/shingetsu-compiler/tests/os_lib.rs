mod common;

use common::{run_all, run_one};
use shingetsu::valuevec;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{GlobalEnv, RuntimeError, Task, Value, ValueVec, VmError};

// ===========================================================================
// os library
// ===========================================================================

/// 2000-01-01 00:00:00 UTC.
const Y2K: i64 = 946684800;
/// 2000-03-05 08:07:09 UTC (a Sunday).
const MAR5: i64 = 952243629;

#[tokio::test]
async fn os_clock_returns_number() {
    // os.clock() returns a float >= 0.
    let v = run_one("return os.clock()").await;
    match v {
        Value::Float(f) => assert!(f >= 0.0, "os.clock() returned {}", f),
        other => panic!("expected float, got {:?}", other),
    }
}

#[tokio::test]
async fn os_clock_monotonic() {
    // Two successive calls should be non-decreasing.
    k9::assert_equal!(
        run_one("local a = os.clock(); local b = os.clock(); return b >= a").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn os_time_returns_integer() {
    // os.time() returns a positive integer (Unix timestamp).
    let v = run_one("return os.time()").await;
    match v {
        Value::Integer(n) => assert!(n > 1_000_000_000, "timestamp too small: {}", n),
        other => panic!("expected integer, got {:?}", other),
    }
}

#[tokio::test]
async fn os_time_with_table() {
    // Known epoch: 2000-01-01 00:00:00 UTC.
    k9::assert_equal!(
        run_one("return os.time({ year = 2000, month = 1, day = 1, hour = 0, min = 0, sec = 0 })")
            .await,
        Value::Integer(Y2K)
    );
}

#[tokio::test]
async fn os_time_table_defaults() {
    // hour/min/sec default to 12:00:00 when omitted.
    k9::assert_equal!(
        run_one("return os.time({ year = 2000, month = 1, day = 1 })").await,
        Value::Integer(Y2K + 12 * 3600)
    );
}

#[tokio::test]
async fn os_time_table_bad_month() {
    common::assert_runtime_error!(
        "os.time({ year = 2000, month = 13, day = 1 })",
        "\
error: bad argument #1 to 'time' (month in 1..12 expected, got 13)
 --> test.lua:1:9
  |
1 | os.time({ year = 2000, month = 13, day = 1 })
  |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (month in 1..12 expected, got 13)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_bad_arg() {
    common::assert_runtime_error!(
        "os.time(42)",
        "\
error: bad argument #1 to 'time' (table expected, got number)
 --> test.lua:1:9
  |
1 | os.time(42)
  |         ^^ bad argument #1 to 'time' (table expected, got number)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_difftime() {
    k9::assert_equal!(
        run_one("return os.difftime(100, 30)").await,
        Value::Float(70.0)
    );
}

#[tokio::test]
async fn os_difftime_negative() {
    k9::assert_equal!(
        run_one("return os.difftime(30, 100)").await,
        Value::Float(-70.0)
    );
}

#[tokio::test]
async fn os_date_star_t_utc() {
    // os.date("!*t", Y2K) should be 2000-01-01 00:00:00 UTC, Saturday.
    let results = run_all(&format!(
        "local t = os.date('!*t', {Y2K})\n\
         return t.year, t.month, t.day, t.hour, t.min, t.sec, t.wday, t.yday"
    ))
    .await;
    k9::assert_equal!(results[0], Value::Integer(2000)); // year
    k9::assert_equal!(results[1], Value::Integer(1)); // month
    k9::assert_equal!(results[2], Value::Integer(1)); // day
    k9::assert_equal!(results[3], Value::Integer(0)); // hour
    k9::assert_equal!(results[4], Value::Integer(0)); // min
    k9::assert_equal!(results[5], Value::Integer(0)); // sec
    k9::assert_equal!(results[6], Value::Integer(7)); // wday (Saturday)
    k9::assert_equal!(results[7], Value::Integer(1)); // yday
}

#[tokio::test]
async fn os_date_format_utc() {
    // Known timestamp: 2000-01-01 00:00:00 UTC.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y-%m-%d %H:%M:%S', {Y2K})")).await,
        Value::string("2000-01-01 00:00:00")
    );
}

#[tokio::test]
async fn os_date_weekday_names() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%A', {Y2K})")).await,
        Value::string("Saturday")
    );
    k9::assert_equal!(
        run_one(&format!("return os.date('!%a', {Y2K})")).await,
        Value::string("Sat")
    );
}

#[tokio::test]
async fn os_date_month_names() {
    // March 15, 2023 = 1678838400
    k9::assert_equal!(
        run_one("return os.date('!%B', 1678838400)").await,
        Value::string("March")
    );
    k9::assert_equal!(
        run_one("return os.date('!%b', 1678838400)").await,
        Value::string("Mar")
    );
}

#[tokio::test]
async fn os_date_twelve_hour() {
    // 2000-01-01 15:30:00 UTC = Y2K + 15*3600 + 30*60 = 946740600.
    k9::assert_equal!(
        run_one("return os.date('!%I:%M %p', 946740600)").await,
        Value::string("03:30 PM")
    );
}

#[tokio::test]
async fn os_date_day_of_year() {
    // Feb 1 2000 = day 32.
    // Y2K + 31*86400 = 949363200
    k9::assert_equal!(
        run_one("return os.date('!%j', 949363200)").await,
        Value::string("032")
    );
}

#[tokio::test]
async fn os_date_percent_escape() {
    k9::assert_equal!(
        run_one("return os.date('!100%%', 0)").await,
        Value::string("100%")
    );
}

#[tokio::test]
async fn os_date_default_format() {
    // os.date() with no args returns the %c format for the current time.
    // In C/POSIX locale this is "Sun Apr 20 14:30:00 2026" style.
    let v = run_one("return os.date()").await;
    match v {
        Value::String(s) => {
            let s = std::str::from_utf8(&s).expect("utf8");
            // Parse with the C locale %c format: "Www Mmm dd HH:MM:SS YYYY"
            let format = time::format_description::parse(
                "[weekday repr:short] [month repr:short] [day padding:space] \
                 [hour]:[minute]:[second] [year]",
            )
            .expect("format description");
            time::PrimitiveDateTime::parse(s, &format).unwrap_or_else(|e| {
                panic!("os.date() returned {s:?} which failed to parse as %%c: {e}")
            });
        }
        other => panic!("expected string, got {:?}", other),
    }
}

#[tokio::test]
async fn os_date_two_digit_year() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%y', {Y2K})")).await,
        Value::string("00")
    );
}

#[tokio::test]
async fn os_date_star_t_has_isdst() {
    // isdst field should be present (as boolean).
    k9::assert_equal!(
        run_one("local t = os.date('!*t', 0); return type(t.isdst)").await,
        Value::string("boolean")
    );
}

#[tokio::test]
async fn os_time_roundtrip() {
    // os.time(os.date("!*t", X)) should return X.
    k9::assert_equal!(
        run_one(&format!("return os.time(os.date('!*t', {Y2K}))")).await,
        Value::Integer(Y2K)
    );
}

// -- os.difftime edge cases --

#[tokio::test]
async fn os_difftime_float_args() {
    k9::assert_equal!(
        run_one("return os.difftime(100.5, 30.25)").await,
        Value::Float(70.25)
    );
}

#[tokio::test]
async fn os_difftime_bad_arg() {
    common::assert_runtime_error!(
        "os.difftime('hello', 1)",
        "\
error: bad argument #1 to 'difftime' (number expected, got string)
 --> test.lua:1:13
  |
1 | os.difftime('hello', 1)
  |             ^^^^^^^ bad argument #1 to 'difftime' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

// -- os.time error paths --

#[tokio::test]
async fn os_time_missing_year() {
    common::assert_runtime_error!(
        "os.time({ month = 1, day = 1 })",
        "\
error: bad argument #1 to 'time' (number for field 'year' expected, got field 'year' is missing)
 --> test.lua:1:9
  |
1 | os.time({ month = 1, day = 1 })
  |         ^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (number for field 'year' expected, got field 'year' is missing)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_missing_month() {
    common::assert_runtime_error!(
        "os.time({ year = 2000, day = 1 })",
        "\
error: bad argument #1 to 'time' (number for field 'month' expected, got field 'month' is missing)
 --> test.lua:1:9
  |
1 | os.time({ year = 2000, day = 1 })
  |         ^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (number for field 'month' expected, got field 'month' is missing)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_missing_day() {
    common::assert_runtime_error!(
        "os.time({ year = 2000, month = 1 })",
        "\
error: bad argument #1 to 'time' (number for field 'day' expected, got field 'day' is missing)
 --> test.lua:1:9
  |
1 | os.time({ year = 2000, month = 1 })
  |         ^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (number for field 'day' expected, got field 'day' is missing)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_invalid_day() {
    common::assert_runtime_error!(
        "os.time({ year = 2000, month = 1, day = 32 })",
        "\
error: bad argument #1 to 'time' (valid date expected, got day was not in range)
 --> test.lua:1:9
  |
1 | os.time({ year = 2000, month = 1, day = 32 })
  |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (valid date expected, got day was not in range)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_invalid_hour() {
    common::assert_runtime_error!(
        "os.time({ year = 2000, month = 1, day = 1, hour = 25 })",
        "\
error: bad argument #1 to 'time' (valid time expected, got hour was not in range)
 --> test.lua:1:9
  |
1 | os.time({ year = 2000, month = 1, day = 1, hour = 25 })
  |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (valid time expected, got hour was not in range)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_invalid_hour_multiline_table() {
    // When the table-constructor argument spans multiple lines, the
    // renderer caps the carets at three lines so that very large
    // constructors don't bury the diagnostic.  Three lines is
    // usually enough to keep the call site (`f({` ... a couple of
    // fields) visible without painting half the file.
    let src = "\
os.time({
    year = 2000,
    month = 1,
    day = 1,
    hour = 25,
    min = 0,
    sec = 0,
})";
    common::assert_runtime_error!(
        src,
        "\
error: bad argument #1 to 'time' (valid time expected, got hour was not in range)
 --> test.lua:1:9
  |
1 |   os.time({
  |  _________^
2 | |     year = 2000,
3 | |     month = 1,
  | |______________^ bad argument #1 to 'time' (valid time expected, got hour was not in range)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_month_zero() {
    common::assert_runtime_error!(
        "os.time({ year = 2000, month = 0, day = 1 })",
        "\
error: bad argument #1 to 'time' (month in 1..12 expected, got 0)
 --> test.lua:1:9
  |
1 | os.time({ year = 2000, month = 0, day = 1 })
  |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (month in 1..12 expected, got 0)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

// -- os.date strftime specifiers --

// Use a known timestamp: 2000-03-05 08:07:09 UTC (Sunday)
// Y2K + 63*86400 + 8*3600 + 7*60 + 9 = MAR5
// March 5 2000 is a Sunday.

#[tokio::test]
async fn os_date_zero_padded_day() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%d', {MAR5})")).await,
        Value::string("05")
    );
}

#[tokio::test]
async fn os_date_space_padded_day() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%e', {MAR5})")).await,
        Value::string(" 5")
    );
}

#[tokio::test]
async fn os_date_numeric_month() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%m', {MAR5})")).await,
        Value::string("03")
    );
}

#[tokio::test]
async fn os_date_minute() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%M', {MAR5})")).await,
        Value::string("07")
    );
}

#[tokio::test]
async fn os_date_second() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%S', {MAR5})")).await,
        Value::string("09")
    );
}

#[tokio::test]
async fn os_date_four_digit_year() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y', {MAR5})")).await,
        Value::string("2000")
    );
}

#[tokio::test]
async fn os_date_weekday_number() {
    // Sunday = 0
    k9::assert_equal!(
        run_one(&format!("return os.date('!%w', {MAR5})")).await,
        Value::string("0")
    );
}

#[tokio::test]
async fn os_date_abbreviated_month_h() {
    // %h is an alias for %b.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%h', {MAR5})")).await,
        Value::string("Mar")
    );
}

#[tokio::test]
async fn os_date_locale_date() {
    // %x expands to %m/%d/%y.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%x', {MAR5})")).await,
        Value::string("03/05/00")
    );
}

#[tokio::test]
async fn os_date_locale_time() {
    // %X expands to %H:%M:%S.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%X', {MAR5})")).await,
        Value::string("08:07:09")
    );
}

#[tokio::test]
async fn os_date_locale_datetime() {
    // %c expands to "%a %b %e %H:%M:%S %Y".
    k9::assert_equal!(
        run_one(&format!("return os.date('!%c', {MAR5})")).await,
        Value::string("Sun Mar  5 08:07:09 2000")
    );
}

#[tokio::test]
async fn os_date_week_number_sunday() {
    // 2000-03-05 is day 65, Sunday (wday=0).
    // %U = (65 - 0 + 7) / 7 = 72 / 7 = 10.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%U', {MAR5})")).await,
        Value::string("10")
    );
}

#[tokio::test]
async fn os_date_week_number_monday() {
    // 2000-03-05 is day 65, Sunday (Monday-based wday=6).
    // %W = (65 - 6 + 7) / 7 = 66 / 7 = 9.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%W', {MAR5})")).await,
        Value::string("09")
    );
}

#[tokio::test]
async fn os_date_utc_offset() {
    // With '!' prefix the offset is UTC → +0000.
    k9::assert_equal!(
        run_one("return os.date('!%z', 0)").await,
        Value::string("+0000")
    );
}

#[tokio::test]
async fn os_date_timezone_name_utc() {
    k9::assert_equal!(
        run_one("return os.date('!%Z', 0)").await,
        Value::string("UTC")
    );
}

#[tokio::test]
async fn os_date_twelve_hour_midnight() {
    // Midnight: hour=0, %I should show 12.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%I', {Y2K})")).await,
        Value::string("12")
    );
}

#[tokio::test]
async fn os_date_twelve_hour_noon() {
    // Noon: hour=12, %I should show 12.
    // Y2K + 12*3600 = 946728000
    k9::assert_equal!(
        run_one("return os.date('!%I', 946728000)").await,
        Value::string("12")
    );
}

#[tokio::test]
async fn os_date_am_indicator() {
    // Midnight is AM.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%p', {Y2K})")).await,
        Value::string("AM")
    );
}

#[tokio::test]
async fn os_date_trailing_percent() {
    // A lone '%' at end of format string.
    k9::assert_equal!(
        run_one("return os.date('!hello%', 0)").await,
        Value::string("hello%")
    );
}

#[tokio::test]
async fn os_date_unknown_specifier() {
    // Unknown specifier should be output literally.
    k9::assert_equal!(
        run_one("return os.date('!%q', 0)").await,
        Value::string("%q")
    );
}

#[tokio::test]
async fn os_date_bad_format_type() {
    common::assert_runtime_error!(
        "os.date(42)",
        "\
error: bad argument #1 to 'date' (string expected, got number)
 --> test.lua:1:9
  |
1 | os.date(42)
  |         ^^ bad argument #1 to 'date' (string expected, got number)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_date_epoch_star_t() {
    // Unix epoch: 1970-01-01 00:00:00 UTC, Thursday.
    let results = run_all(
        "local t = os.date('!*t', 0)\n\
         return t.year, t.month, t.day, t.wday, t.yday",
    )
    .await;
    k9::assert_equal!(results[0], Value::Integer(1970)); // year
    k9::assert_equal!(results[1], Value::Integer(1)); // month
    k9::assert_equal!(results[2], Value::Integer(1)); // day
    k9::assert_equal!(results[3], Value::Integer(5)); // wday (Thursday = 5)
    k9::assert_equal!(results[4], Value::Integer(1)); // yday
}

#[tokio::test]
async fn os_date_combined_specifiers() {
    // Multiple specifiers in one format string.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%d/%m/%Y', {MAR5})")).await,
        Value::string("05/03/2000")
    );
}

#[tokio::test]
async fn os_date_literal_text() {
    // Literal text passes through unchanged.
    k9::assert_equal!(
        run_one("return os.date('!hello world', 0)").await,
        Value::string("hello world")
    );
}

#[tokio::test]
async fn os_date_local_time_path() {
    // Without '!' prefix, exercises the local-time branch.
    // Result varies by environment, but should be a non-empty string.
    let v = run_one("return os.date('%Y', 0)").await;
    match v {
        Value::String(s) => {
            let year: i32 = std::str::from_utf8(&s)
                .expect("utf8")
                .parse()
                .expect("parse year");
            // Epoch year in any timezone is 1969 or 1970
            assert!(
                year == 1969 || year == 1970,
                "expected 1969 or 1970, got {year}"
            );
        }
        other => panic!("expected string, got {:?}", other),
    }
}

#[tokio::test]
async fn os_date_star_t_local() {
    // "*t" without '!' returns a table via the local-time path.
    let v = run_one("return type(os.date('*t', 0))").await;
    k9::assert_equal!(v, Value::string("table"));
}

#[tokio::test]
async fn os_date_float_timestamp() {
    // Float timestamp is accepted and truncated to integer.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y', {Y2K}.5)")).await,
        Value::string("2000")
    );
}

#[tokio::test]
async fn os_date_format_no_timestamp() {
    // Explicit format with no timestamp defaults to current time.
    let v = run_one("return os.date('!%Y')").await;
    match v {
        Value::String(s) => {
            let year: i32 = String::from_utf8_lossy(&s).parse().expect("parse year");
            assert!(year >= 2024, "year too small: {}", year);
        }
        other => panic!("expected string, got {:?}", other),
    }
}

#[tokio::test]
async fn os_time_bad_field_type() {
    common::assert_runtime_error!(
        "os.time({ year = 'hello', month = 1, day = 1 })",
        "\
error: bad argument #1 to 'time' (number for field 'year' expected, got string)
 --> test.lua:1:9
  |
1 | os.time({ year = 'hello', month = 1, day = 1 })
  |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'time' (number for field 'year' expected, got string)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_difftime_bad_second_arg() {
    common::assert_runtime_error!(
        "os.difftime(1, 'hello')",
        "\
error: bad argument #2 to 'difftime' (number expected, got string)
 --> test.lua:1:16
  |
1 | os.difftime(1, 'hello')
  |                ^^^^^^^ bad argument #2 to 'difftime' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_difftime_nil_arg() {
    common::assert_runtime_error!(
        "os.difftime(nil, 1)",
        "\
error: bad argument #1 to 'difftime' (number expected, got nil)
 --> test.lua:1:13
  |
1 | os.difftime(nil, 1)
  |             ^^^ bad argument #1 to 'difftime' (number expected, got nil)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_difftime_bool_arg() {
    common::assert_runtime_error!(
        "os.difftime(true, 1)",
        "\
error: bad argument #1 to 'difftime' (number expected, got boolean)
 --> test.lua:1:13
  |
1 | os.difftime(true, 1)
  |             ^^^^ bad argument #1 to 'difftime' (number expected, got boolean)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_bool_arg() {
    common::assert_runtime_error!(
        "os.time(true)",
        "\
error: bad argument #1 to 'time' (table expected, got boolean)
 --> test.lua:1:9
  |
1 | os.time(true)
  |         ^^^^ bad argument #1 to 'time' (table expected, got boolean)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_date_bad_timestamp_type() {
    common::assert_runtime_error!(
        "os.date('!%Y', 'hello')",
        "\
error: bad argument #2 to 'date' (number expected, got string)
 --> test.lua:1:16
  |
1 | os.date('!%Y', 'hello')
  |                ^^^^^^^ bad argument #2 to 'date' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_date_bool_format() {
    common::assert_runtime_error!(
        "os.date(true)",
        "\
error: bad argument #1 to 'date' (string expected, got boolean)
 --> test.lua:1:9
  |
1 | os.date(true)
  |         ^^^^ bad argument #1 to 'date' (string expected, got boolean)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn os_time_extra_field_ignored() {
    // Extra fields in the table are silently ignored. This is correct Lua
    // behavior — os.date("*t") returns wday/yday/isdst which os.time ignores.
    k9::assert_equal!(
        run_one(&format!("return os.time({{ year = 2000, month = 1, day = 1, hour = 0, min = 0, sec = 0, bogus = 42 }})")).await,
        Value::Integer(Y2K)
    );
}

// ===========================================================================
// os filesystem functions: os.remove, os.rename, os.tmpname
// ===========================================================================

/// Create an environment with builtins + os time functions + os fs functions.
fn fs_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::os::register_fs(&env).expect("register os fs");
    env
}

/// Run with os fs functions available, return all values.
async fn run_fs(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = fs_env();
    let func = bc.into_function();
    Task::new(env, func, valuevec![]).await.expect("run")
}

/// Run with os fs functions available, return the first value.
async fn run_fs_one(src: &str) -> Value {
    run_fs(src).await.into_iter().next().unwrap_or(Value::Nil)
}

// ---------------------------------------------------------------------------
// os.tmpname
// ---------------------------------------------------------------------------

#[tokio::test]
async fn os_tmpname_returns_string() {
    let v = run_fs_one("return os.tmpname()").await;
    match v {
        Value::String(s) => {
            let path = std::path::Path::new(std::str::from_utf8(&s).expect("utf8"));
            k9::assert_equal!(path.parent(), Some(std::env::temp_dir().as_path()));
        }
        other => panic!("expected string, got {:?}", other),
    }
}

#[tokio::test]
async fn os_tmpname_does_not_create_file() {
    // Per Lua docs, os.tmpname does not create the file.
    let v = run_fs_one("return os.tmpname()").await;
    let s = match v {
        Value::String(s) => String::from_utf8(s.to_vec()).expect("utf-8"),
        other => panic!("expected string, got {:?}", other),
    };
    assert!(
        !std::path::Path::new(&s).exists(),
        "os.tmpname should not create {:?}",
        s
    );
}

#[tokio::test]
async fn os_tmpname_unique() {
    // Two calls should yield different names.
    let vs = run_fs("return os.tmpname(), os.tmpname()").await;
    k9::assert_equal!(vs.len(), 2);
    let a = match &vs[0] {
        Value::String(s) => s.clone(),
        _ => panic!("expected string"),
    };
    let b = match &vs[1] {
        Value::String(s) => s.clone(),
        _ => panic!("expected string"),
    };
    assert_ne!(a, b, "two os.tmpname calls produced the same name");
}

// ---------------------------------------------------------------------------
// os.remove
// ---------------------------------------------------------------------------

#[tokio::test]
async fn os_remove_file_ok() {
    use std::io::Write;
    let mut tmp = tempfile::NamedTempFile::new().expect("create");
    tmp.write_all(b"contents").expect("write");
    let path = tmp.path().to_path_buf();
    // Detach so the guard does not try to delete on drop.
    let (_file, path_owned) = tmp.keep().expect("keep");
    assert!(path_owned.exists());

    let src = format!("return os.remove({:?})", path.to_str().expect("path"));
    k9::assert_equal!(run_fs_one(&src).await, Value::Boolean(true));
    assert!(!path_owned.exists(), "file was not removed");
}

#[tokio::test]
async fn os_remove_empty_dir_ok() {
    let dir = tempfile::TempDir::new().expect("create dir");
    let path = dir.keep();
    assert!(path.exists());

    let src = format!("return os.remove({:?})", path.to_str().expect("path"));
    k9::assert_equal!(run_fs_one(&src).await, Value::Boolean(true));
    assert!(!path.exists(), "directory was not removed");
}

#[tokio::test]
async fn os_remove_nonempty_dir_fails() {
    use std::io::Write;
    let dir = tempfile::TempDir::new().expect("create dir");
    let inner = dir.path().join("child.txt");
    let mut f = std::fs::File::create(&inner).expect("create child");
    f.write_all(b"data").expect("write");

    let src = format!("return os.remove({:?})", dir.path().to_str().expect("path"));
    let vs = run_fs(&src).await;
    k9::assert_equal!(vs.len(), 2);
    k9::assert_equal!(vs[0], Value::Nil);
    match &vs[1] {
        Value::String(s) => {
            let msg = String::from_utf8_lossy(s).into_owned();
            assert!(
                msg.starts_with(dir.path().to_str().expect("path")),
                "expected path prefix in {:?}",
                msg
            );
        }
        other => panic!("expected error string, got {:?}", other),
    }
    // Directory must still exist.
    assert!(dir.path().exists());
}

#[tokio::test]
async fn os_remove_nonexistent_returns_err() {
    let dir = tempfile::TempDir::new().expect("create dir");
    let missing = dir.path().join("nope.txt");
    let src = format!("return os.remove({:?})", missing.to_str().expect("path"));
    let vs = run_fs(&src).await;
    k9::assert_equal!(vs.len(), 2);
    k9::assert_equal!(vs[0], Value::Nil);
    match &vs[1] {
        Value::String(s) => {
            let msg = String::from_utf8_lossy(s).into_owned();
            k9::assert_equal!(
                msg,
                format!(
                    "{}: No such file or directory",
                    missing.to_str().expect("path")
                )
            );
        }
        other => panic!("expected error string, got {:?}", other),
    }
}

#[tokio::test]
async fn os_remove_symlink_removes_link_not_target() {
    // Create a real file and a symlink to it.  os.remove on the symlink
    // should unlink the symlink itself, not its target.
    #[cfg(unix)]
    {
        use std::io::Write;
        let dir = tempfile::TempDir::new().expect("create dir");
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link");
        let mut f = std::fs::File::create(&target).expect("create target");
        f.write_all(b"hi").expect("write");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(link.symlink_metadata().is_ok());

        let src = format!("return os.remove({:?})", link.to_str().expect("path"));
        k9::assert_equal!(run_fs_one(&src).await, Value::Boolean(true));
        assert!(link.symlink_metadata().is_err(), "link should be gone");
        assert!(target.exists(), "target should still exist");
    }
}

#[tokio::test]
async fn os_remove_missing_arg() {
    // Arity / type error from the macro layer; emits a bad-argument error.
    k9::assert_equal!(
        fs_err("os.remove()").await,
        "bad argument #1 to 'remove' (value expected, got no value)"
    );
}

// ---------------------------------------------------------------------------
// os.rename
// ---------------------------------------------------------------------------

#[tokio::test]
async fn os_rename_ok() {
    use std::io::Write;
    let dir = tempfile::TempDir::new().expect("create dir");
    let src_path = dir.path().join("a.txt");
    let dst_path = dir.path().join("b.txt");
    let mut f = std::fs::File::create(&src_path).expect("create");
    f.write_all(b"move me").expect("write");

    let code = format!(
        "return os.rename({:?}, {:?})",
        src_path.to_str().expect("src"),
        dst_path.to_str().expect("dst")
    );
    k9::assert_equal!(run_fs_one(&code).await, Value::Boolean(true));
    assert!(!src_path.exists());
    assert!(dst_path.exists());
    k9::assert_equal!(
        std::fs::read(&dst_path).expect("read dst"),
        b"move me".to_vec()
    );
}

#[tokio::test]
async fn os_rename_source_missing() {
    let dir = tempfile::TempDir::new().expect("create dir");
    let src_path = dir.path().join("nope.txt");
    let dst_path = dir.path().join("whatever.txt");
    let code = format!(
        "return os.rename({:?}, {:?})",
        src_path.to_str().expect("src"),
        dst_path.to_str().expect("dst")
    );
    let vs = run_fs(&code).await;
    k9::assert_equal!(vs.len(), 2);
    k9::assert_equal!(vs[0], Value::Nil);
    match &vs[1] {
        Value::String(s) => {
            let msg = String::from_utf8_lossy(s).into_owned();
            k9::assert_equal!(
                msg,
                format!(
                    "{} -> {}: No such file or directory",
                    src_path.to_str().expect("src"),
                    dst_path.to_str().expect("dst")
                )
            );
        }
        other => panic!("expected error string, got {:?}", other),
    }
}

#[tokio::test]
async fn os_rename_overwrite_existing() {
    // POSIX rename atomically replaces an existing destination.
    use std::io::Write;
    let dir = tempfile::TempDir::new().expect("create dir");
    let src_path = dir.path().join("a.txt");
    let dst_path = dir.path().join("b.txt");
    let mut a = std::fs::File::create(&src_path).expect("create a");
    a.write_all(b"new").expect("write a");
    let mut b = std::fs::File::create(&dst_path).expect("create b");
    b.write_all(b"old").expect("write b");

    let code = format!(
        "return os.rename({:?}, {:?})",
        src_path.to_str().expect("src"),
        dst_path.to_str().expect("dst")
    );
    k9::assert_equal!(run_fs_one(&code).await, Value::Boolean(true));
    k9::assert_equal!(std::fs::read(&dst_path).expect("read dst"), b"new".to_vec());
}

#[tokio::test]
async fn os_rename_missing_args() {
    k9::assert_equal!(
        fs_err("os.rename('/tmp/a')").await,
        "bad argument #2 to 'rename' (value expected, got no value)"
    );
}

// ---------------------------------------------------------------------------
// Registration-model sanity checks
// ---------------------------------------------------------------------------

#[test]
fn register_libs_io_provides_os_fs() {
    // Libraries::IO (no OS flag) should still expose os.remove/rename/tmpname.
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::IO,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    for name in ["remove", "rename", "tmpname"] {
        let v = os
            .raw_get(&Value::string(name.to_owned()))
            .expect("raw_get");
        assert!(
            matches!(v, Value::Function(_)),
            "os.{} missing or not a function: {:?}",
            name,
            v
        );
    }
}

#[test]
fn register_libs_os_without_io_has_no_fs() {
    // Libraries::OS alone (no IO) should keep the sandbox-safe promise
    // — os.remove / os.rename / os.tmpname must be absent.
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::OS,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    for name in ["remove", "rename", "tmpname"] {
        let v = os
            .raw_get(&Value::string(name.to_owned()))
            .expect("raw_get");
        k9::assert_equal!(v, Value::Nil);
    }
}

// ---------------------------------------------------------------------------
// Additional coverage: os.remove, os.rename, os.tmpname edge cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn os_remove_broken_symlink() {
    // A symlink whose target no longer exists.  The kernel unlinks
    // the link itself without needing to resolve the target —
    // verifies that we reach `remove_file` / `unlink(2)` directly
    // rather than inspecting metadata first.
    #[cfg(unix)]
    {
        let dir = tempfile::TempDir::new().expect("create dir");
        let missing_target = dir.path().join("target.txt"); // never created
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&missing_target, &link).expect("symlink");
        assert!(link.symlink_metadata().is_ok(), "link should exist");
        assert!(!missing_target.exists(), "target should not exist");

        let code = format!("return os.remove({:?})", link.to_str().expect("path"));
        k9::assert_equal!(run_fs_one(&code).await, Value::Boolean(true));
        assert!(link.symlink_metadata().is_err(), "link should be gone");
    }
}

#[tokio::test]
async fn os_rename_directory() {
    // Rename works for directories as well as files.
    let parent = tempfile::TempDir::new().expect("create parent");
    let src_dir = parent.path().join("a");
    let dst_dir = parent.path().join("b");
    std::fs::create_dir(&src_dir).expect("mkdir");
    // Drop a marker inside so we can confirm the directory contents
    // moved with it.
    std::fs::write(src_dir.join("marker"), b"x").expect("write marker");

    let code = format!(
        "return os.rename({:?}, {:?})",
        src_dir.to_str().expect("src"),
        dst_dir.to_str().expect("dst")
    );
    k9::assert_equal!(run_fs_one(&code).await, Value::Boolean(true));
    assert!(!src_dir.exists(), "src should be gone");
    assert!(dst_dir.is_dir(), "dst should be a directory");
    k9::assert_equal!(
        std::fs::read(dst_dir.join("marker")).expect("read marker"),
        b"x".to_vec()
    );
}

#[tokio::test]
async fn os_rename_symlink_moves_link() {
    // Renaming a symlink moves the link entry itself; it must not
    // resolve to the target and copy/move that instead.
    #[cfg(unix)]
    {
        let dir = tempfile::TempDir::new().expect("create dir");
        let target = dir.path().join("target.txt");
        let link_a = dir.path().join("link_a");
        let link_b = dir.path().join("link_b");
        std::fs::write(&target, b"hi").expect("write target");
        std::os::unix::fs::symlink(&target, &link_a).expect("symlink");

        let code = format!(
            "return os.rename({:?}, {:?})",
            link_a.to_str().expect("src"),
            link_b.to_str().expect("dst")
        );
        k9::assert_equal!(run_fs_one(&code).await, Value::Boolean(true));
        assert!(link_a.symlink_metadata().is_err(), "link_a gone");

        let md = link_b
            .symlink_metadata()
            .expect("link_b should still be a symlink");
        assert!(
            md.file_type().is_symlink(),
            "link_b should be a symlink, not a copy of target"
        );
        // Target is untouched.
        k9::assert_equal!(std::fs::read(&target).expect("read target"), b"hi".to_vec());
    }
}

#[tokio::test]
async fn os_rename_source_equals_dest() {
    // POSIX: if old and new point to the same existing file, rename
    // is a successful no-op.
    let dir = tempfile::TempDir::new().expect("create dir");
    let path = dir.path().join("a.txt");
    std::fs::write(&path, b"keep me").expect("write");

    let s = path.to_str().expect("path");
    let code = format!("return os.rename({:?}, {:?})", s, s);
    k9::assert_equal!(run_fs_one(&code).await, Value::Boolean(true));
    assert!(path.exists(), "file should still exist");
    k9::assert_equal!(std::fs::read(&path).expect("read"), b"keep me".to_vec());
}

#[tokio::test]
async fn os_rename_dest_parent_missing() {
    // When the failure is about the destination's parent directory,
    // the error should still surface both paths so the caller can
    // see which argument was problematic.
    use std::io::Write;
    let dir = tempfile::TempDir::new().expect("create dir");
    let src_path = dir.path().join("a.txt");
    let dst_path = dir.path().join("missing_subdir").join("b.txt");
    let mut f = std::fs::File::create(&src_path).expect("create");
    f.write_all(b"data").expect("write");

    let code = format!(
        "return os.rename({:?}, {:?})",
        src_path.to_str().expect("src"),
        dst_path.to_str().expect("dst")
    );
    let vs = run_fs(&code).await;
    k9::assert_equal!(vs.len(), 2);
    k9::assert_equal!(vs[0], Value::Nil);
    match &vs[1] {
        Value::String(s) => {
            let msg = String::from_utf8_lossy(s).into_owned();
            k9::assert_equal!(
                msg,
                format!(
                    "{} -> {}: No such file or directory",
                    src_path.to_str().expect("src"),
                    dst_path.to_str().expect("dst")
                )
            );
        }
        other => panic!("expected error string, got {:?}", other),
    }
    // Source must remain in place after a failed rename.
    assert!(src_path.exists());
}

#[test]
fn register_fs_creates_os_table_when_absent() {
    // Fresh env with no os table pre-registered — register_fs must
    // create the table itself.  We use `GlobalEnv::new` directly
    // rather than going through builtins::register (which would
    // install an os table).
    let env = GlobalEnv::new();
    assert!(env.get_global("os").is_none());

    shingetsu::os::register_fs(&env).expect("register_fs");

    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    for name in ["remove", "rename", "tmpname"] {
        let v = os
            .raw_get(&Value::string(name.to_owned()))
            .expect("raw_get");
        assert!(
            matches!(v, Value::Function(_)),
            "os.{} missing or not a function: {:?}",
            name,
            v
        );
    }
}

#[test]
fn register_fs_preserves_existing_os_entries() {
    // When os is already registered, register_fs merges into it
    // without clobbering os.time, os.date, etc.
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    shingetsu::os::register_fs(&env).expect("register_fs");

    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    // Pre-existing time functions must still be present.
    for name in ["clock", "time", "date", "difftime"] {
        let v = os
            .raw_get(&Value::string(name.to_owned()))
            .expect("raw_get");
        assert!(
            matches!(v, Value::Function(_)),
            "os.{} was clobbered: {:?}",
            name,
            v
        );
    }
    // And the fs functions are there too.
    for name in ["remove", "rename", "tmpname"] {
        let v = os
            .raw_get(&Value::string(name.to_owned()))
            .expect("raw_get");
        assert!(
            matches!(v, Value::Function(_)),
            "os.{} missing: {:?}",
            name,
            v
        );
    }
}

#[tokio::test]
async fn os_tmpname_format() {
    // Filename portion matches `lua_<16 hex>` exactly, and the
    // parent directory is the current process temp dir.
    let v = run_fs_one("return os.tmpname()").await;
    let s = match v {
        Value::String(s) => String::from_utf8(s.to_vec()).expect("utf-8"),
        other => panic!("expected string, got {:?}", other),
    };
    let path = std::path::PathBuf::from(&s);
    k9::assert_equal!(
        path.parent().expect("has parent"),
        std::env::temp_dir().as_path()
    );
    let fname = path
        .file_name()
        .and_then(|n| n.to_str())
        .expect("utf-8 filename");
    let hex = match fname.strip_prefix("lua_") {
        Some(h) => h,
        None => panic!("expected 'lua_' prefix, got {:?}", fname),
    };
    k9::assert_equal!(hex.len(), 16);
    assert!(
        hex.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "expected lowercase hex suffix, got {:?}",
        hex
    );
}

// ---------------------------------------------------------------------------
// os.execute
// ---------------------------------------------------------------------------

/// Create an environment with builtins + `os.execute` registered.
fn exec_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::os::register_exec(&env).expect("register os exec");
    env
}

/// Run with `os.execute` available, return all values.
async fn run_exec(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = exec_env();
    let func = bc.into_function();
    Task::new(env, func, valuevec![]).await.expect("run")
}

#[tokio::test]
async fn os_execute_no_args_returns_true() {
    // With no command, os.execute returns a boolean indicating that a
    // command processor is available.  We always have /bin/sh.
    let vs = run_exec("return os.execute()").await;
    k9::assert_equal!(vs, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn os_execute_exit_0() {
    // A successful command returns (true, "exit", 0).
    let vs = run_exec("return os.execute('exit 0')").await;
    k9::assert_equal!(
        vs,
        valuevec![
            Value::Boolean(true),
            Value::string("exit"),
            Value::Integer(0)
        ]
    );
}

#[tokio::test]
async fn os_execute_exit_42() {
    // A non-zero exit returns (nil, "exit", 42).
    let vs = run_exec("return os.execute('exit 42')").await;
    k9::assert_equal!(
        vs,
        valuevec![Value::Nil, Value::string("exit"), Value::Integer(42)]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn os_execute_terminated_by_signal() {
    // Having the shell kill itself with SIGTERM produces a
    // signal-terminated exit status that we surface as
    // (nil, "signal", SIGTERM).  The numeric value of SIGTERM is
    // pulled from `libc` rather than hard-coded.
    let vs = run_exec("return os.execute('kill -TERM $$')").await;
    k9::assert_equal!(
        vs,
        valuevec![
            Value::Nil,
            Value::string("signal"),
            Value::Integer(libc::SIGTERM as i64)
        ]
    );
}

#[tokio::test]
async fn os_execute_shell_not_found_maps_to_127() {
    // `command_not_found_for_sure` is an arbitrary invalid name; the
    // shell itself runs fine, reports "not found", and exits 127.
    // This exercises the common-case sh-level failure path without
    // depending on /bin/sh being absent.
    let vs = run_exec(
        "return os.execute('exec >/dev/null 2>&1; \
         /this/definitely/does/not/exist --never')",
    )
    .await;
    k9::assert_equal!(
        vs,
        valuevec![Value::Nil, Value::string("exit"), Value::Integer(127)]
    );
}

#[tokio::test]
async fn os_execute_returns_exactly_three_values_on_command() {
    // `local a, b, c, d = os.execute('true')` must bind exactly
    // three values — `d` should be nil because os.execute only
    // produces the (ok, how, code) tuple.
    let vs = run_exec("local a, b, c, d = os.execute('true') return a, b, c, d").await;
    k9::assert_equal!(
        vs,
        valuevec![
            Value::Boolean(true),
            Value::string("exit"),
            Value::Integer(0),
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn os_execute_returns_exactly_one_value_without_args() {
    // With no command, only one value (the boolean) is returned —
    // `b` must be nil rather than padded with `"exit"`/0.
    let vs = run_exec("local a, b = os.execute() return a, b").await;
    k9::assert_equal!(vs, valuevec![Value::Boolean(true), Value::Nil]);
}

#[tokio::test]
async fn os_execute_nil_arg_same_as_no_args() {
    // Explicit nil should take the no-args code path via Option<Bytes>::None.
    let vs = run_exec("return os.execute(nil)").await;
    k9::assert_equal!(vs, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn os_execute_number_arg_rejected() {
    // Strict typing: a number does not coerce to a string for the
    // command argument, even though Lua semantically allows it in
    // many contexts.  The macro-generated FromLua for Bytes rejects
    // non-string values.
    k9::assert_equal!(
        exec_err("os.execute(42)").await,
        "bad argument #1 to 'execute' (string expected, got number)"
    );
}

#[tokio::test]
async fn os_execute_boolean_arg_rejected() {
    k9::assert_equal!(
        exec_err("os.execute(true)").await,
        "bad argument #1 to 'execute' (string expected, got boolean)"
    );
}

#[tokio::test]
async fn os_execute_table_arg_rejected() {
    k9::assert_equal!(
        exec_err("os.execute({})").await,
        "bad argument #1 to 'execute' (string expected, got table)"
    );
}

#[tokio::test]
async fn os_execute_empty_command() {
    // An empty command string is a no-op success under /bin/sh.
    let vs = run_exec("return os.execute('')").await;
    k9::assert_equal!(
        vs,
        valuevec![
            Value::Boolean(true),
            Value::string("exit"),
            Value::Integer(0),
        ]
    );
}

#[tokio::test]
async fn os_execute_shell_metacharacters_evaluated() {
    // `true && false` only works if we really route the command
    // through `/bin/sh -c`; an `execvp` of the first word would try
    // to find a program literally named `true` and pass the rest as
    // argv, where `&&` would be a bare argument.  Confirms the shell
    // evaluation path.
    let vs = run_exec("return os.execute('true && false')").await;
    k9::assert_equal!(
        vs,
        valuevec![Value::Nil, Value::string("exit"), Value::Integer(1)]
    );
}

#[tokio::test]
async fn os_execute_redirection_works() {
    // `> /dev/null` at the shell level should silently discard the
    // output without preventing the command from succeeding.  Also
    // keeps test runner output clean.
    let vs = run_exec("return os.execute('echo ignored > /dev/null')").await;
    k9::assert_equal!(
        vs,
        valuevec![
            Value::Boolean(true),
            Value::string("exit"),
            Value::Integer(0),
        ]
    );
}

#[test]
fn register_exec_creates_os_table_when_absent() {
    // Fresh env with no os table — register_exec must create it.
    let env = GlobalEnv::new();
    assert!(env.get_global("os").is_none());

    shingetsu::os::register_exec(&env).expect("register_exec");

    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    let v = os.raw_get(&Value::string("execute")).expect("raw_get");
    assert!(
        matches!(v, Value::Function(_)),
        "os.execute missing or not a function: {:?}",
        v
    );
}

#[test]
fn register_exec_and_fs_compose() {
    // Calling register_exec and register_fs against the same env
    // must leave both sets of functions present.  Guards against the
    // merge loop accidentally overwriting the table.
    let env = GlobalEnv::new();
    shingetsu::os::register_fs(&env).expect("register_fs");
    shingetsu::os::register_exec(&env).expect("register_exec");

    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    for name in ["remove", "rename", "tmpname", "execute"] {
        let v = os
            .raw_get(&Value::string(name.to_owned()))
            .expect("raw_get");
        assert!(
            matches!(v, Value::Function(_)),
            "os.{} missing or not a function: {:?}",
            name,
            v
        );
    }
}

#[test]
fn register_libs_exec_provides_os_execute() {
    // Libraries::EXEC must install os.execute on the os table.
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::EXEC,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    let v = os.raw_get(&Value::string("execute")).expect("raw_get");
    assert!(
        matches!(v, Value::Function(_)),
        "os.execute missing or not a function: {:?}",
        v
    );
}

#[test]
fn register_libs_io_without_exec_has_no_execute() {
    // Libraries::IO alone must not expose os.execute — execution is
    // gated separately under Libraries::EXEC.
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::IO,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    let v = os.raw_get(&Value::string("execute")).expect("raw_get");
    k9::assert_equal!(v, Value::Nil);
}

// ---------------------------------------------------------------------------
// os.getenv
// ---------------------------------------------------------------------------

/// Create an environment with builtins + `os.getenv` registered.
fn env_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::os::register_env(&env).expect("register os env");
    env
}

/// Run with `os.getenv` available, return all values.
async fn run_env(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = env_env();
    let func = bc.into_function();
    Task::new(env, func, valuevec![]).await.expect("run")
}

#[tokio::test]
async fn os_getenv_returns_string_for_set_var() {
    // PATH is universally set on POSIX and Windows; we only assert
    // that the result is a non-empty String, not any specific value.
    let vs = run_env("return os.getenv('PATH')").await;
    match vs.as_slice() {
        [Value::String(b)] => {
            // PATH is always set and non-empty; verify it contains
            // at least one path separator as a structural check
            let s = std::str::from_utf8(b).expect("utf8");
            assert!(
                s.contains(':') || s.contains(';'),
                "PATH should contain path separators, got: {s:?}"
            );
        }
        other => panic!("expected single string, got {:?}", other),
    }
}

#[tokio::test]
async fn os_getenv_returns_nil_for_unset_var() {
    // Use a unique name that no real process environment should have.
    let v = run_env("return os.getenv('__SHINGETSU_TEST_DEFINITELY_UNSET_VAR_9f2a7c3b1e4d')").await;
    k9::assert_equal!(v, valuevec![Value::Nil]);
}

#[tokio::test]
async fn os_getenv_empty_name_returns_nil() {
    // An empty name cannot match any real env var; the impl returns
    // nil up-front without touching the stdlib (which would panic).
    let v = run_env("return os.getenv('')").await;
    k9::assert_equal!(v, valuevec![Value::Nil]);
}

#[tokio::test]
async fn os_getenv_name_with_embedded_nul_returns_nil() {
    // Embedded NUL: likewise cannot match.
    let v = run_env("return os.getenv('PA\\0TH')").await;
    k9::assert_equal!(v, valuevec![Value::Nil]);
}

#[tokio::test]
async fn os_getenv_name_with_equals_returns_nil() {
    // `=` is the env-entry delimiter and cannot appear in a name.
    let v = run_env("return os.getenv('FOO=BAR')").await;
    k9::assert_equal!(v, valuevec![Value::Nil]);
}

#[tokio::test]
async fn os_getenv_number_arg_rejected() {
    // Strict typing (same as os.execute, io.open, etc.): no numeric
    // coercion to string for the name argument.
    k9::assert_equal!(
        env_err("os.getenv(42)").await,
        "bad argument #1 to 'getenv' (string expected, got number)"
    );
}

#[tokio::test]
async fn os_getenv_boolean_arg_rejected() {
    k9::assert_equal!(
        env_err("os.getenv(true)").await,
        "bad argument #1 to 'getenv' (string expected, got boolean)"
    );
}

#[tokio::test]
async fn os_getenv_table_arg_rejected() {
    k9::assert_equal!(
        env_err("os.getenv({})").await,
        "bad argument #1 to 'getenv' (string expected, got table)"
    );
}

#[tokio::test]
async fn os_getenv_missing_arg_rejected() {
    k9::assert_equal!(
        env_err("os.getenv()").await,
        "bad argument #1 to 'getenv' (value expected, got no value)"
    );
}

#[test]
fn register_env_creates_os_table_when_absent() {
    // register_env into an env with no prior `os` table: a fresh table
    // is created and getenv lives on it.  Uses register_sandboxed to
    // skip the os-lib registration that the non-sandboxed `register`
    // would pull in.
    let env = GlobalEnv::new();
    shingetsu::builtins::register_sandboxed(&env).expect("builtins");
    shingetsu::os::register_env(&env).expect("register env");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    // getenv present; clock absent (wasn't registered).
    assert!(
        !os.raw_get(&Value::string("getenv"))
            .expect("getenv")
            .is_nil(),
        "getenv should be present"
    );
    k9::assert_equal!(
        os.raw_get(&Value::string("clock")).expect("clock"),
        Value::Nil
    );
}

#[test]
fn register_env_and_os_compose() {
    // register_env merges into an existing os table from register().
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    shingetsu::os::register(&env).expect("register os");
    shingetsu::os::register_env(&env).expect("register env");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    // Both getenv and clock are reachable.
    assert!(!os
        .raw_get(&Value::string("getenv"))
        .expect("getenv")
        .is_nil());
    assert!(!os.raw_get(&Value::string("clock")).expect("clock").is_nil());
}

#[test]
fn register_libs_env_provides_os_getenv() {
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::ENV,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    assert!(!os
        .raw_get(&Value::string("getenv"))
        .expect("getenv")
        .is_nil());
}

#[test]
fn register_libs_os_without_env_has_no_getenv() {
    // Libraries::OS alone must not expose os.getenv — environment
    // access is gated separately under Libraries::ENV because env
    // vars routinely carry credentials.
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::OS,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    k9::assert_equal!(
        os.raw_get(&Value::string("getenv")).expect("raw_get"),
        Value::Nil
    );
}

// ---------------------------------------------------------------------------
// Error-path helper
// ---------------------------------------------------------------------------

/// Run with os fs registered, expect an error, return its message.
async fn fs_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = fs_env();
    let func = bc.into_function();
    Task::new(env, func, valuevec![])
        .await
        .unwrap_err()
        .to_string()
}

/// Run with os exec registered, expect an error, return its message.
async fn exec_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = exec_env();
    let func = bc.into_function();
    Task::new(env, func, valuevec![])
        .await
        .unwrap_err()
        .to_string()
}

/// Run with os env registered, expect an error, return its message.
async fn env_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = env_env();
    let func = bc.into_function();
    Task::new(env, func, valuevec![])
        .await
        .unwrap_err()
        .to_string()
}

// ---------------------------------------------------------------------------
// os.exit
// ---------------------------------------------------------------------------

/// Create an environment with builtins + `os.exit` registered.
fn exit_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::os::register_exit(&env).expect("register os exit");
    env
}

/// Run with `os.exit` available, return the raw VM result so tests can
/// match on `VmError::ExitRequested`.
async fn run_exit(src: &str) -> Result<ValueVec, RuntimeError> {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = exit_env();
    let func = bc.into_function();
    Task::new(env, func, valuevec![]).await
}

/// Run a snippet expecting `ExitRequested`, return `(code, close)`.
/// `VmError` doesn't implement `PartialEq`, so we destructure instead
/// of comparing directly.
async fn exit_result(src: &str) -> (i32, bool) {
    match run_exit(src).await {
        Err(re) => match re.error {
            VmError::ExitRequested { code, close } => (code, close),
            e => panic!("expected ExitRequested, got {e:?}"),
        },
        Ok(v) => panic!("expected ExitRequested, got Ok({v:?})"),
    }
}

#[tokio::test]
async fn os_exit_no_args_defaults_to_success() {
    k9::assert_equal!(exit_result("os.exit()").await, (0, false));
}

#[tokio::test]
async fn os_exit_true_is_success() {
    k9::assert_equal!(exit_result("os.exit(true)").await, (0, false));
}

#[tokio::test]
async fn os_exit_false_is_failure() {
    k9::assert_equal!(exit_result("os.exit(false)").await, (1, false));
}

#[tokio::test]
async fn os_exit_integer_code() {
    k9::assert_equal!(exit_result("os.exit(42)").await, (42, false));
}

#[tokio::test]
async fn os_exit_negative_integer_code() {
    k9::assert_equal!(exit_result("os.exit(-1)").await, (-1, false));
}

#[tokio::test]
async fn os_exit_integer_valued_float() {
    // 2.0 is representable as an integer, so it's accepted.
    k9::assert_equal!(exit_result("os.exit(2.0)").await, (2, false));
}

#[tokio::test]
async fn os_exit_non_integer_float_rejected() {
    // Matches `luaL_optinteger`: non-integer floats produce the stdlib
    // "number has no integer representation" error.
    k9::assert_equal!(
        run_exit("os.exit(1.5)").await.unwrap_err().to_string(),
        "bad argument #1 to 'exit' (number has no integer representation)"
    );
}

#[tokio::test]
async fn os_exit_numeric_string_accepted() {
    // Lua's integer coercion accepts numeric strings.
    k9::assert_equal!(exit_result("os.exit('7')").await, (7, false));
}

#[tokio::test]
async fn os_exit_non_numeric_string_rejected() {
    // A string that doesn't parse as a number raises the standard
    // `bad argument` error.  Complements the numeric-string-accepted
    // test above: coerce_to_integer is shared machinery, but this
    // locks down the os.exit-specific error message shape.
    k9::assert_equal!(
        run_exit("os.exit('abc')").await.unwrap_err().to_string(),
        "bad argument #1 to 'exit' (number expected, got string)"
    );
}

#[tokio::test]
async fn os_exit_explicit_nil_first_arg() {
    // `os.exit(nil)` behaves identically to `os.exit()` — both go
    // through the `None | Some(Value::Nil)` arm and default to 0.
    k9::assert_equal!(exit_result("os.exit(nil)").await, (0, false));
}

#[tokio::test]
async fn os_exit_out_of_i32_range_truncates() {
    // Reference Lua casts `luaL_optinteger` (long long) to (int),
    // silently truncating.  We mirror that behavior.  0x1_0000_0000
    // is 2^32 — truncated to i32 this is 0.
    k9::assert_equal!(exit_result("os.exit(0x100000000)").await, (0, false));
    // 0x1_0000_0001 truncates to 1.
    k9::assert_equal!(exit_result("os.exit(0x100000001)").await, (1, false));
}

#[tokio::test]
async fn os_exit_i32_max_value() {
    // i32::MAX round-trips unchanged.  Boundary on the positive side
    // of the `as i32` truncation.
    k9::assert_equal!(exit_result("os.exit(2147483647)").await, (i32::MAX, false));
}

#[tokio::test]
async fn os_exit_i32_min_value() {
    // i32::MIN round-trips unchanged.  Boundary on the negative side.
    k9::assert_equal!(exit_result("os.exit(-2147483648)").await, (i32::MIN, false));
}

#[tokio::test]
async fn os_exit_2_31_truncates_to_i32_min() {
    // 2^31 = 2147483648 is one past i32::MAX; `as i32` wraps to
    // i32::MIN.  Complements the 2^32 case above with an overflow at
    // the signed/unsigned boundary.
    k9::assert_equal!(exit_result("os.exit(0x80000000)").await, (i32::MIN, false));
}

#[tokio::test]
async fn os_exit_table_arg_rejected() {
    k9::assert_equal!(
        run_exit("os.exit({})").await.unwrap_err().to_string(),
        "bad argument #1 to 'exit' (number expected, got table)"
    );
}

#[tokio::test]
async fn os_exit_close_true() {
    k9::assert_equal!(exit_result("os.exit(3, true)").await, (3, true));
}

#[tokio::test]
async fn os_exit_close_false_explicit() {
    k9::assert_equal!(exit_result("os.exit(3, false)").await, (3, false));
}

#[tokio::test]
async fn os_exit_close_truthy_non_bool() {
    // Lua truthiness: any value other than false/nil is truthy.  A
    // table, a number, a string all enable close=true.
    k9::assert_equal!(exit_result("os.exit(0, {})").await, (0, true));
    k9::assert_equal!(exit_result("os.exit(0, 0)").await, (0, true));
    k9::assert_equal!(exit_result("os.exit(0, '')").await, (0, true));
}

#[tokio::test]
async fn os_exit_close_nil_is_false() {
    k9::assert_equal!(exit_result("os.exit(0, nil)").await, (0, false));
}

#[tokio::test]
async fn os_exit_not_caught_by_pcall() {
    // pcall must re-propagate ExitRequested so the exit signal
    // reaches the task boundary.  Matches reference Lua where
    // os.exit is a non-returning C function that pcall cannot catch.
    k9::assert_equal!(
        exit_result(
            r#"
local ok, err = pcall(os.exit, 5)
-- These lines should be unreachable:
print("unreachable")
return ok, err
"#,
        )
        .await,
        (5, false)
    );
}

#[tokio::test]
async fn os_exit_not_caught_by_xpcall() {
    k9::assert_equal!(
        exit_result(
            r#"
local handler = function(e) return "handled" end
xpcall(os.exit, handler, 9)
print("unreachable")
"#,
        )
        .await,
        (9, false)
    );
}

#[tokio::test]
async fn os_exit_xpcall_msgh_not_invoked() {
    // Tighter than `os_exit_not_caught_by_xpcall`: verify the
    // message handler is bypassed entirely, not merely that its
    // return value is ignored.  Reference Lua's `os.exit` is a
    // non-returning C call, so `xpcall` never reaches the
    // msgh dispatch path.
    let env = exit_env();
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(
            r#"
msgh_called = false
local handler = function(e) msgh_called = true; return "handled" end
xpcall(os.exit, handler, 9)
print("unreachable")
"#,
        )
        .await
        .expect("compile");
    let func = bc.into_function();
    let err = Task::new(env.clone(), func, valuevec![]).await.unwrap_err();
    match err.error {
        VmError::ExitRequested { code, close } => {
            k9::assert_equal!(code, 9);
            k9::assert_equal!(close, false);
        }
        other => panic!("expected ExitRequested, got {:?}", other),
    }
    k9::assert_equal!(
        env.get_global("msgh_called").expect("msgh_called"),
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn os_exit_from_deep_call_chain() {
    // Exit raised several Lua frames deep must unwind cleanly
    // through every intermediate frame.  Exercises the frame-clearing
    // path in `begin_unwind` for multi-frame stacks.
    k9::assert_equal!(
        exit_result(
            r#"
local function level3() os.exit(11) end
local function level2() level3() end
local function level1() level2() end
level1()
print("unreachable")
"#,
        )
        .await,
        (11, false)
    );
}

#[tokio::test]
async fn os_exit_multiple_close_vars_reverse_order() {
    // Multiple `<close>` locals in a single scope must be closed in
    // reverse declaration order (innermost-first), per Lua 5.4.  We
    // observe the order by having each __close record its tag into a
    // shared counter: c (tag 3) closes first, then b (tag 2), then a
    // (tag 1) — producing decimal 321.
    let env = exit_env();
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(
            r#"
order = 0
local function make(n)
    return setmetatable({}, {
        __close = function() order = order * 10 + n end
    })
end
local a <close> = make(1)
local b <close> = make(2)
local c <close> = make(3)
os.exit(0)
"#,
        )
        .await
        .expect("compile");
    let func = bc.into_function();
    let err = Task::new(env.clone(), func, valuevec![]).await.unwrap_err();
    match err.error {
        VmError::ExitRequested { code, close } => {
            k9::assert_equal!(code, 0);
            k9::assert_equal!(close, false);
        }
        other => panic!("expected ExitRequested, got {:?}", other),
    }
    k9::assert_equal!(env.get_global("order").expect("order"), Value::Integer(321));
}

#[tokio::test]
async fn os_exit_runs_close_metamethod() {
    // `<close>` locals in frames between the os.exit call and the
    // task boundary must have their `__close` dispatched during the
    // error unwind.  This is more cleanup than reference Lua does
    // for os.exit, but it falls out naturally from modeling exit as
    // a propagating error.  We observe it by having __close set a
    // global flag before the task returns.
    let env = exit_env();
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(
            r#"
close_called = false
local mt = { __close = function() close_called = true end }
local guard <close> = setmetatable({}, mt)
os.exit(7)
"#,
        )
        .await
        .expect("compile");
    let func = bc.into_function();
    let err = Task::new(env.clone(), func, valuevec![]).await.unwrap_err();
    match err.error {
        VmError::ExitRequested { code, close } => {
            k9::assert_equal!(code, 7);
            k9::assert_equal!(close, false);
        }
        other => panic!("expected ExitRequested, got {:?}", other),
    }
    k9::assert_equal!(
        env.get_global("close_called").expect("close_called"),
        Value::Boolean(true)
    );
}

#[test]
fn register_libs_exit_provides_os_exit() {
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::EXIT,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    assert!(!os.raw_get(&Value::string("exit")).expect("exit").is_nil());
}

#[test]
fn register_libs_os_without_exit_has_no_exit() {
    // Libraries::OS alone must not expose os.exit — process
    // termination is gated separately under Libraries::EXIT.
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::OS,
    )
    .expect("register");
    let os = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        other => panic!("expected os table, got {:?}", other),
    };
    k9::assert_equal!(
        os.raw_get(&Value::string("exit")).expect("raw_get"),
        Value::Nil
    );
}
