//! Lua `os` standard library (LuaU subset).
//!
//! Only the time-related functions are in scope:
//! `os.clock`, `os.time`, `os.date`, `os.difftime`.

use crate::error::VmError;
use crate::table::Table;
use crate::value::Value;
use bytes::Bytes;

/// Baseline instant captured once at startup for `os.clock()`.
static CLOCK_EPOCH: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

/// Build the os library table and register it as the `os` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = os_mod::build_module_table(env)?;
    env.set_global("os", Value::Table(table));
    Ok(())
}

#[crate::module(name = "os")]
pub mod os_mod {
    use super::*;

    // -----------------------------------------------------------------
    // os.clock() -> number
    // Returns high-precision elapsed seconds since an arbitrary baseline.
    // -----------------------------------------------------------------
    #[function]
    fn clock() -> f64 {
        CLOCK_EPOCH.elapsed().as_secs_f64()
    }

    // -----------------------------------------------------------------
    // os.difftime(a, b) -> number
    // Returns the difference a - b in seconds.
    // -----------------------------------------------------------------
    #[function]
    fn difftime(a: f64, b: f64) -> f64 {
        a - b
    }

    // -----------------------------------------------------------------
    // os.time([table]) -> number
    // Without args: current Unix timestamp (integer seconds).
    // With table: interprets {year, month, day, hour, min, sec} as
    // UTC and returns the corresponding Unix timestamp.
    // -----------------------------------------------------------------
    #[function]
    fn time(t: Option<Table>) -> Result<Value, VmError> {
        match t {
            None => {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Ok(Value::Integer(secs as i64))
            }
            Some(tab) => {
                let year = table_int_field(&tab, "year", "os.time")?;
                let month = table_int_field(&tab, "month", "os.time")?;
                let day = table_int_field(&tab, "day", "os.time")?;
                let hour = table_opt_int_field(&tab, "hour")?.unwrap_or(12);
                let min = table_opt_int_field(&tab, "min")?.unwrap_or(0);
                let sec = table_opt_int_field(&tab, "sec")?.unwrap_or(0);

                let month_enum = match month {
                    1 => time::Month::January,
                    2 => time::Month::February,
                    3 => time::Month::March,
                    4 => time::Month::April,
                    5 => time::Month::May,
                    6 => time::Month::June,
                    7 => time::Month::July,
                    8 => time::Month::August,
                    9 => time::Month::September,
                    10 => time::Month::October,
                    11 => time::Month::November,
                    12 => time::Month::December,
                    _ => {
                        return Err(VmError::BadArgument {
                            position: 1,
                            function: "os.time".to_string(),
                            expected: "month in 1..12".to_string(),
                            got: format!("{}", month),
                        });
                    }
                };

                let date = time::Date::from_calendar_date(year as i32, month_enum, day as u8)
                    .map_err(|e| VmError::BadArgument {
                        position: 1,
                        function: "os.time".to_string(),
                        expected: "valid date".to_string(),
                        got: e.to_string(),
                    })?;

                let time_of_day =
                    time::Time::from_hms(hour as u8, min as u8, sec as u8).map_err(|e| {
                        VmError::BadArgument {
                            position: 1,
                            function: "os.time".to_string(),
                            expected: "valid time".to_string(),
                            got: e.to_string(),
                        }
                    })?;

                let dt = time::PrimitiveDateTime::new(date, time_of_day).assume_utc();
                Ok(Value::Integer(dt.unix_timestamp()))
            }
        }
    }

    // -----------------------------------------------------------------
    // os.date([format [, time]]) -> string | table
    // Formats a timestamp (defaults to now).
    // If format starts with '!', uses UTC; otherwise local time.
    // If format is "*t" (or "!*t"), returns a table.
    // Otherwise interprets format as strftime-like.
    // Default format is "%c".
    // -----------------------------------------------------------------
    #[function]
    fn date(fmt: Option<String>, timestamp: Option<f64>) -> Result<Value, VmError> {
        // Resolve the timestamp.
        let unix_secs: i64 = match timestamp {
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            Some(t) => t as i64,
        };

        let odt_utc = time::OffsetDateTime::from_unix_timestamp(unix_secs).map_err(|e| {
            VmError::BadArgument {
                position: 2,
                function: "os.date".to_string(),
                expected: "valid timestamp".to_string(),
                got: e.to_string(),
            }
        })?;

        // Parse format string.
        let fmt_str = fmt.unwrap_or_else(|| "%c".to_string());

        // Check for '!' prefix (UTC vs local).
        let (use_utc, fmt_body) = if let Some(rest) = fmt_str.strip_prefix('!') {
            (true, rest)
        } else {
            (false, fmt_str.as_str())
        };

        // Get the datetime in the appropriate timezone.
        let odt = if use_utc {
            odt_utc
        } else {
            // Try to get local offset; fall back to UTC if unavailable.
            match time::UtcOffset::current_local_offset() {
                Ok(offset) => odt_utc.to_offset(offset),
                Err(_) => odt_utc,
            }
        };

        // "*t" returns a table.
        if fmt_body == "*t" {
            return Ok(Value::Table(datetime_to_table(&odt)));
        }

        // Otherwise, format using strftime-like specifiers.
        Ok(Value::String(Bytes::from(strftime(&odt, fmt_body))))
    }
}

// =====================================================================
// Helpers
// =====================================================================

fn table_int_field(tab: &Table, key: &str, func: &str) -> Result<i64, VmError> {
    let v = tab.raw_get(&Value::String(Bytes::copy_from_slice(key.as_bytes())))?;
    match v {
        Value::Integer(n) => Ok(n),
        Value::Float(f) => Ok(f as i64),
        _ => Err(VmError::BadArgument {
            position: 1,
            function: func.to_string(),
            expected: format!("number for field '{}'", key),
            got: if v == Value::Nil {
                format!("missing field '{}'", key)
            } else {
                v.type_name().to_string()
            },
        }),
    }
}

fn table_opt_int_field(tab: &Table, key: &str) -> Result<Option<i64>, VmError> {
    let v = tab.raw_get(&Value::String(Bytes::copy_from_slice(key.as_bytes())))?;
    match v {
        Value::Nil => Ok(None),
        Value::Integer(n) => Ok(Some(n)),
        Value::Float(f) => Ok(Some(f as i64)),
        _ => Ok(None),
    }
}

/// Build a Lua table from an `OffsetDateTime` with the standard fields.
fn datetime_to_table(odt: &time::OffsetDateTime) -> Table {
    let tab = Table::new();
    let set = |k: &str, v: i64| {
        tab.raw_set(
            Value::String(Bytes::copy_from_slice(k.as_bytes())),
            Value::Integer(v),
        )
        .expect("table set");
    };
    set("year", odt.year() as i64);
    set("month", odt.month() as i64);
    set("day", odt.day() as i64);
    set("hour", odt.hour() as i64);
    set("min", odt.minute() as i64);
    set("sec", odt.second() as i64);
    // wday: 1 = Sunday, 7 = Saturday (Lua convention).
    let wday = match odt.weekday() {
        time::Weekday::Sunday => 1,
        time::Weekday::Monday => 2,
        time::Weekday::Tuesday => 3,
        time::Weekday::Wednesday => 4,
        time::Weekday::Thursday => 5,
        time::Weekday::Friday => 6,
        time::Weekday::Saturday => 7,
    };
    set("wday", wday);
    set("yday", odt.to_ordinal_date().1 as i64);
    // isdst: we can't reliably determine DST from `time` crate alone;
    // report false (consistent with UTC and most embedded uses).
    tab.raw_set(
        Value::String(Bytes::from_static(b"isdst")),
        Value::Boolean(false),
    )
    .expect("table set");
    tab
}

/// Minimal strftime implementation covering the Lua `os.date` specifiers.
fn strftime(odt: &time::OffsetDateTime, fmt: &str) -> String {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            None => out.push('%'),
            Some('%') => out.push('%'),
            // Abbreviated weekday name.
            Some('a') => out.push_str(short_weekday(odt.weekday())),
            // Full weekday name.
            Some('A') => out.push_str(full_weekday(odt.weekday())),
            // Abbreviated month name.
            Some('b') | Some('h') => out.push_str(short_month(odt.month())),
            // Full month name.
            Some('B') => out.push_str(full_month(odt.month())),
            // Locale date and time (equivalent to "%a %b %e %H:%M:%S %Y").
            Some('c') => {
                out.push_str(&strftime(odt, "%a %b %e %H:%M:%S %Y"));
            }
            // Day of month, zero-padded [01..31].
            Some('d') => out.push_str(&format!("{:02}", odt.day())),
            // Day of month, space-padded [ 1..31].
            Some('e') => out.push_str(&format!("{:2}", odt.day())),
            // Hour (24-hour) [00..23].
            Some('H') => out.push_str(&format!("{:02}", odt.hour())),
            // Hour (12-hour) [01..12].
            Some('I') => {
                let h = odt.hour() % 12;
                out.push_str(&format!("{:02}", if h == 0 { 12 } else { h }));
            }
            // Day of year [001..366].
            Some('j') => out.push_str(&format!("{:03}", odt.to_ordinal_date().1)),
            // Month [01..12].
            Some('m') => out.push_str(&format!("{:02}", odt.month() as u8)),
            // Minute [00..59].
            Some('M') => out.push_str(&format!("{:02}", odt.minute())),
            // AM/PM.
            Some('p') => out.push_str(if odt.hour() < 12 { "AM" } else { "PM" }),
            // Second [00..60].
            Some('S') => out.push_str(&format!("{:02}", odt.second())),
            // Week number (Sunday as first day) [00..53].
            Some('U') => {
                let yday = odt.to_ordinal_date().1 as i32;
                let wday = odt.weekday().number_days_from_sunday() as i32;
                out.push_str(&format!("{:02}", (yday - wday + 7) / 7));
            }
            // Weekday number (Sunday = 0) [0..6].
            Some('w') => out.push_str(&format!("{}", odt.weekday().number_days_from_sunday())),
            // Week number (Monday as first day) [00..53].
            Some('W') => {
                let yday = odt.to_ordinal_date().1 as i32;
                let wday = odt.weekday().number_days_from_monday() as i32;
                out.push_str(&format!("{:02}", (yday - wday + 7) / 7));
            }
            // Locale date (%m/%d/%y).
            Some('x') => out.push_str(&strftime(odt, "%m/%d/%y")),
            // Locale time (%H:%M:%S).
            Some('X') => out.push_str(&strftime(odt, "%H:%M:%S")),
            // Two-digit year [00..99].
            Some('y') => out.push_str(&format!("{:02}", odt.year() % 100)),
            // Four-digit year.
            Some('Y') => out.push_str(&format!("{:04}", odt.year())),
            // UTC offset (+hhmm or -hhmm).
            Some('z') => {
                let off = odt.offset();
                let total_secs = off.whole_seconds();
                let sign = if total_secs < 0 { '-' } else { '+' };
                let h = (total_secs.abs() / 3600) as u32;
                let m = ((total_secs.abs() % 3600) / 60) as u32;
                out.push_str(&format!("{}{:02}{:02}", sign, h, m));
            }
            // Timezone abbreviation — not reliably available, output offset.
            Some('Z') => {
                let off = odt.offset();
                if off.is_utc() {
                    out.push_str("UTC");
                } else {
                    let total_secs = off.whole_seconds();
                    let sign = if total_secs < 0 { '-' } else { '+' };
                    let h = (total_secs.abs() / 3600) as u32;
                    let m = ((total_secs.abs() % 3600) / 60) as u32;
                    out.push_str(&format!("UTC{}{:02}:{:02}", sign, h, m));
                }
            }
            // Unknown specifier — output literally.
            Some(other) => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

fn short_weekday(w: time::Weekday) -> &'static str {
    match w {
        time::Weekday::Monday => "Mon",
        time::Weekday::Tuesday => "Tue",
        time::Weekday::Wednesday => "Wed",
        time::Weekday::Thursday => "Thu",
        time::Weekday::Friday => "Fri",
        time::Weekday::Saturday => "Sat",
        time::Weekday::Sunday => "Sun",
    }
}

fn full_weekday(w: time::Weekday) -> &'static str {
    match w {
        time::Weekday::Monday => "Monday",
        time::Weekday::Tuesday => "Tuesday",
        time::Weekday::Wednesday => "Wednesday",
        time::Weekday::Thursday => "Thursday",
        time::Weekday::Friday => "Friday",
        time::Weekday::Saturday => "Saturday",
        time::Weekday::Sunday => "Sunday",
    }
}

fn short_month(m: time::Month) -> &'static str {
    match m {
        time::Month::January => "Jan",
        time::Month::February => "Feb",
        time::Month::March => "Mar",
        time::Month::April => "Apr",
        time::Month::May => "May",
        time::Month::June => "Jun",
        time::Month::July => "Jul",
        time::Month::August => "Aug",
        time::Month::September => "Sep",
        time::Month::October => "Oct",
        time::Month::November => "Nov",
        time::Month::December => "Dec",
    }
}

fn full_month(m: time::Month) -> &'static str {
    match m {
        time::Month::January => "January",
        time::Month::February => "February",
        time::Month::March => "March",
        time::Month::April => "April",
        time::Month::May => "May",
        time::Month::June => "June",
        time::Month::July => "July",
        time::Month::August => "August",
        time::Month::September => "September",
        time::Month::October => "October",
        time::Month::November => "November",
        time::Month::December => "December",
    }
}
