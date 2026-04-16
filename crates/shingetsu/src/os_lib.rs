//! Lua `os` standard library (LuaU subset).
//!
//! Time-related functions (`os.clock`, `os.time`, `os.date`,
//! `os.difftime`) are always registered via [`register`].
//!
//! Filesystem-related functions (`os.remove`, `os.rename`,
//! `os.tmpname`) are installed by [`register_fs`], invoked
//! automatically by [`crate::register_libs`] when [`crate::Libraries::IO`]
//! is enabled.
//!
//! Process execution (`os.execute`) is installed by [`register_exec`],
//! invoked automatically by [`crate::register_libs`] when
//! [`crate::Libraries::EXEC`] is enabled (alongside `io.popen`).
//!
//! Environment variable access (`os.getenv`) is installed by
//! [`register_env`], invoked automatically by [`crate::register_libs`]
//! when [`crate::Libraries::ENV`] is enabled.  Gated separately because
//! env vars commonly hold credentials.

use bytes::Bytes;

use crate::convert::{IntoLua, Variadic};
use crate::error::{PathIoError, VmError, VmResultExt};
use crate::file::close_status_to_lua;
use crate::io_lib::{bytes_to_os_str, bytes_to_path};
use crate::popen::exit_status_to_close_status;
use crate::value::Value;

/// Input table for `os.time({ year, month, day, hour?, min?, sec? })`.
#[derive(crate::FromLua)]
struct OsTimeInput {
    year: i64,
    month: i64,
    day: i64,
    #[lua(default = 12)]
    hour: i64,
    #[lua(default = 0)]
    min: i64,
    #[lua(default = 0)]
    sec: i64,
}

/// Output table returned by `os.date("*t")`.
#[derive(crate::IntoLua)]
struct DateTimeTable {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    min: i64,
    sec: i64,
    wday: i64,
    yday: i64,
    isdst: bool,
}

/// Baseline instant captured once at startup for `os.clock()`.
static CLOCK_EPOCH: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

/// Build the os library table and register it as the `os` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = os_mod::build_module_table(env)?;
    env.set_global("os", Value::Table(table));
    Ok(())
}

/// Install the filesystem-related `os` functions (`os.remove`,
/// `os.rename`, `os.tmpname`) into the `os` global table.
///
/// If the `os` table does not exist yet, a fresh one is created so
/// the functions are always reachable when [`crate::Libraries::IO`]
/// is enabled, regardless of whether [`crate::Libraries::OS`] is too.
pub fn register_fs(env: &crate::GlobalEnv) -> Result<(), VmError> {
    merge_into_os_table(env, os_fs_mod::build_module_table(env)?)
}

/// Install `os.execute` into the `os` global table.
///
/// Bundled with [`crate::io_lib::register_popen`] under
/// [`crate::Libraries::EXEC`] because both spawn an inherited
/// `/bin/sh -c` child.  Creates a fresh `os` table if none exists.
pub fn register_exec(env: &crate::GlobalEnv) -> Result<(), VmError> {
    merge_into_os_table(env, os_exec_mod::build_module_table(env)?)
}

/// Install `os.getenv` into the `os` global table.
///
/// Gated behind [`crate::Libraries::ENV`] rather than
/// [`crate::Libraries::OS`] because environment variables routinely
/// carry credentials (API tokens, passwords) and host fingerprinting
/// data; embedders should opt into that surface consciously and
/// independently of calendar/clock access.  Creates a fresh `os`
/// table if none exists.
pub fn register_env(env: &crate::GlobalEnv) -> Result<(), VmError> {
    merge_into_os_table(env, os_env_mod::build_module_table(env)?)
}

/// Merge all entries from `source` into the `os` global table,
/// creating that table if it does not exist yet.  Shared by
/// [`register_fs`] and [`register_exec`].
fn merge_into_os_table(env: &crate::GlobalEnv, source: crate::table::Table) -> Result<(), VmError> {
    let os_table = match env.get_global("os") {
        Some(Value::Table(t)) => t,
        _ => {
            let t = crate::table::Table::new();
            env.set_global("os", Value::Table(t.clone()));
            t
        }
    };
    let mut key = Value::Nil;
    loop {
        match source.next(&key)? {
            Some((k, v)) => {
                os_table.raw_set(k.clone(), v)?;
                key = k;
            }
            None => break,
        }
    }
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
    fn time(ctx: crate::CallContext, t: Option<OsTimeInput>) -> Result<Value, VmError> {
        match t {
            None => {
                let secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                Ok(Value::Integer(secs as i64))
            }
            Some(t) => {
                let month_enum = match t.month {
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
                            function: String::new(),
                            expected: "month in 1..12".to_string(),
                            got: format!("{}", t.month),
                        }
                        .with_arg_and_call_context(1, &ctx));
                    }
                };

                let date = time::Date::from_calendar_date(t.year as i32, month_enum, t.day as u8)
                    .map_err(|e| VmError::BadArgument {
                        position: 1,
                        function: String::new(),
                        expected: "valid date".to_string(),
                        got: e.to_string(),
                    })
                    .with_call_context(1, &ctx)?;

                let time_of_day = time::Time::from_hms(t.hour as u8, t.min as u8, t.sec as u8)
                    .map_err(|e| VmError::BadArgument {
                        position: 1,
                        function: String::new(),
                        expected: "valid time".to_string(),
                        got: e.to_string(),
                    })
                    .with_call_context(1, &ctx)?;

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
    fn date(
        ctx: crate::CallContext,
        fmt: Option<String>,
        timestamp: Option<f64>,
    ) -> Result<Value, VmError> {
        // Resolve the timestamp.
        let unix_secs: i64 = match timestamp {
            None => std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            Some(t) => t as i64,
        };

        let odt_utc = time::OffsetDateTime::from_unix_timestamp(unix_secs)
            .map_err(|e| VmError::BadArgument {
                position: 2,
                function: String::new(),
                expected: "valid timestamp".to_string(),
                got: e.to_string(),
            })
            .with_call_context(2, &ctx)?;

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
            return Ok(datetime_to_result(&odt).into_lua());
        }

        // Otherwise, format using strftime-like specifiers.
        Ok(Value::string(strftime(&odt, fmt_body)))
    }
}

// =====================================================================
// os filesystem functions (installed via `register_fs`)
// =====================================================================

#[crate::module(name = "os_fs")]
mod os_fs_mod {
    use super::*;

    // -----------------------------------------------------------------
    // os.rename(old, new) -> true | nil, errmsg
    //
    // On failure, the error message includes both paths
    // (`old -> new: <desc>`): `rename(2)` can fail because of either
    // side (missing source, missing destination parent, EXDEV, etc.)
    // and we cannot reliably attribute the error to one path without
    // re-racing the filesystem.  Path-conversion failures, which can
    // only be blamed on a specific argument, are reported separately.
    // -----------------------------------------------------------------
    #[function]
    async fn rename(old: Bytes, new: Bytes) -> Result<Variadic, VmError> {
        let old_path = match bytes_to_path(&old) {
            Ok(p) => p,
            Err(source) => {
                let msg = PathIoError {
                    path: old.clone(),
                    source,
                }
                .to_string();
                return Ok(Variadic(vec![Value::Nil, Value::string(msg)]));
            }
        };
        let new_path = match bytes_to_path(&new) {
            Ok(p) => p,
            Err(source) => {
                let msg = PathIoError {
                    path: new.clone(),
                    source,
                }
                .to_string();
                return Ok(Variadic(vec![Value::Nil, Value::string(msg)]));
            }
        };
        match tokio::fs::rename(&old_path, &new_path).await {
            Ok(()) => Ok(Variadic(vec![Value::Boolean(true)])),
            Err(source) => {
                let desc = crate::error::portable_io_error_description(&source);
                let msg = format!(
                    "{} -> {}: {}",
                    String::from_utf8_lossy(&old),
                    String::from_utf8_lossy(&new),
                    desc
                );
                Ok(Variadic(vec![Value::Nil, Value::string(msg)]))
            }
        }
    }

    // -----------------------------------------------------------------
    // os.remove(filename) -> true | nil, errmsg
    //
    // Per Lua 5.4: deletes the file (or empty directory, on POSIX
    // systems) with the given name.  We issue `remove_file` first
    // (which calls `unlink(2)` and never follows symlinks — matching
    // POSIX `remove(3)`); if that reports the target is a directory,
    // retry with `remove_dir`.  Going through the kernel avoids the
    // TOCTOU window of a separate metadata probe.
    // -----------------------------------------------------------------
    #[function]
    async fn remove(filename: Bytes) -> Result<Variadic, VmError> {
        let result: Result<(), PathIoError> = async {
            let path = bytes_to_path(&filename).map_err(|source| PathIoError {
                path: filename.clone(),
                source,
            })?;
            match tokio::fs::remove_file(&path).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => {
                    tokio::fs::remove_dir(&path)
                        .await
                        .map_err(|source| PathIoError {
                            path: filename.clone(),
                            source,
                        })
                }
                Err(source) => Err(PathIoError {
                    path: filename.clone(),
                    source,
                }),
            }
        }
        .await;
        match result {
            Ok(()) => Ok(Variadic(vec![Value::Boolean(true)])),
            Err(e) => Ok(Variadic(vec![Value::Nil, Value::string(e.to_string())])),
        }
    }

    // -----------------------------------------------------------------
    // os.tmpname() -> string
    //
    // Generate `<temp_dir>/lua_<rand>` without creating anything on
    // disk.  Lua's documented contract is explicitly TOCTOU-prone —
    // callers sometimes use the name for a directory, and on Windows
    // a pre-created file would linger as a visible entry after
    // deletion.  The Lua manual recommends `io.tmpfile()` for secure
    // use; that lives in `io_lib`.
    //
    // On Unix the returned `Bytes` is the raw `OsStr` content (paths
    // are arbitrary byte sequences).  On other platforms the path
    // must round-trip through UTF-8; a non-UTF-8 temp directory
    // raises a Lua error rather than being silently lossy-encoded
    // — the resulting name would not actually name the same file.
    // -----------------------------------------------------------------
    #[function]
    fn tmpname() -> Result<Bytes, VmError> {
        let dir = std::env::temp_dir();
        let rand_val: u64 = rand::random();
        let path = dir.join(format!("lua_{:016x}", rand_val));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Ok(Bytes::copy_from_slice(path.as_os_str().as_bytes()))
        }
        #[cfg(not(unix))]
        {
            match path.to_str() {
                Some(s) => Ok(Bytes::copy_from_slice(s.as_bytes())),
                None => {
                    let msg = format!(
                        "os.tmpname: temp directory path is not valid UTF-8: {}",
                        path.display()
                    );
                    Err(VmError::LuaError {
                        display: msg.clone(),
                        value: Value::string(msg),
                    })
                }
            }
        }
    }
}

// =====================================================================
// os process-execution function (installed via `register_exec`)
// =====================================================================

#[crate::module(name = "os_exec")]
mod os_exec_mod {
    use super::*;

    // -----------------------------------------------------------------
    // os.execute([command]) -> true | (true | nil, "exit" | "signal", code)
    //
    // Without arguments: returns `true` to indicate a command
    // processor is available.  The underlying shell is `/bin/sh`, as
    // with `io.popen` — see `register_exec`.
    //
    // With a command: spawns `/bin/sh -c command` inheriting the
    // parent's stdio, waits for the child, and returns the Lua
    // tuple `(true|nil, "exit"|"signal", code)`.  `true` means the
    // process exited with status 0; otherwise the first result is
    // `nil`.  On Unix, a process terminated by a signal produces
    // `(nil, "signal", signum)`.
    //
    // If the shell itself fails to spawn, we follow the POSIX
    // convention and report it as `(nil, "exit", 127)` (the
    // `command not found` exit code).  We cannot raise a Lua error
    // here because the Lua 5.4 contract pins the shape of the
    // return tuple.
    // -----------------------------------------------------------------
    #[function]
    async fn execute(command: Option<Bytes>) -> Result<Variadic, VmError> {
        let Some(command) = command else {
            return Ok(Variadic(vec![Value::Boolean(true)]));
        };

        let prog_os = match bytes_to_os_str(&command) {
            Ok(s) => s.into_owned(),
            Err(_) => {
                // Non-UTF-8 command on non-Unix: cannot form an OsStr.
                return Ok(Variadic(vec![
                    Value::Nil,
                    Value::string("exit"),
                    Value::Integer(127),
                ]));
            }
        };

        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(&prog_os);
        match cmd.status().await {
            Ok(status) => Ok(Variadic(close_status_to_lua(exit_status_to_close_status(
                status,
            )))),
            Err(_) => Ok(Variadic(vec![
                Value::Nil,
                Value::string("exit"),
                Value::Integer(127),
            ])),
        }
    }
}

// =====================================================================
// os environment-access function (installed via `register_env`)
// =====================================================================

#[crate::module(name = "os_env")]
mod os_env_mod {
    use super::*;

    // -----------------------------------------------------------------
    // os.getenv(name) -> string | nil
    //
    // Returns the value of the environment variable `name`, or nil if
    // it is not set.  A set-but-empty variable returns the empty
    // string, matching Lua semantics.
    //
    // Names containing characters that cannot appear in a real
    // environment variable (NUL, `=`, or empty) always return nil
    // rather than surfacing a stdlib error — these simply cannot
    // match any actual environment entry.
    //
    // On Unix the returned `Bytes` is the raw value from the process
    // environment (which is an arbitrary byte sequence).  On other
    // platforms the value must round-trip through UTF-8; a value
    // containing invalid UTF-16 raises a Lua error rather than being
    // silently lossy-encoded.
    // -----------------------------------------------------------------
    #[function]
    fn getenv(name: Bytes) -> Result<Option<Bytes>, VmError> {
        // Names that can't name a real env var: skip the syscall and
        // avoid any stdlib validation panics.
        if name.is_empty() || name.contains(&0u8) || name.contains(&b'=') {
            return Ok(None);
        }
        let name_os = match bytes_to_os_str(&name) {
            Ok(s) => s,
            Err(_) => {
                // Non-OsStr name (non-UTF-8 on Windows) cannot match
                // any real env var either.
                return Ok(None);
            }
        };
        let Some(val) = std::env::var_os(&name_os) else {
            return Ok(None);
        };
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(Some(Bytes::from(val.into_vec())))
        }
        #[cfg(not(unix))]
        {
            match val.into_string() {
                Ok(s) => Ok(Some(Bytes::from(s.into_bytes()))),
                Err(_) => {
                    let msg = format!(
                        "os.getenv: value of environment variable {} is not valid UTF-8",
                        String::from_utf8_lossy(&name)
                    );
                    Err(VmError::LuaError {
                        display: msg.clone(),
                        value: Value::string(msg),
                    })
                }
            }
        }
    }
}

// =====================================================================
// Helpers
// =====================================================================

/// Build a `DateTimeTable` from an `OffsetDateTime`.
fn datetime_to_result(odt: &time::OffsetDateTime) -> DateTimeTable {
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
    DateTimeTable {
        year: odt.year() as i64,
        month: odt.month() as i64,
        day: odt.day() as i64,
        hour: odt.hour() as i64,
        min: odt.minute() as i64,
        sec: odt.second() as i64,
        wday,
        yday: odt.to_ordinal_date().1 as i64,
        // isdst: we can't reliably determine DST from `time` crate alone;
        // report false (consistent with UTC and most embedded uses).
        isdst: false,
    }
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
