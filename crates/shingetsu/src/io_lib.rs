//! Lua `io` standard library (opt-in).
//!
//! Provides file I/O backed by [`TokioFileOps`].  The host decides
//! whether to enable it:
//!
//! ```ignore
//! shingetsu::io_lib::register(&env)?;
//! ```
//!
//! Functions that require stdio (`io.stdin`, `io.read`, etc.) are
//! registered separately via [`register_stdio`].

use std::sync::Arc;

use bytes::Bytes;

use crate::convert::Variadic;
use crate::error::VmError;
use crate::file::LuaFile;
use crate::tokio_file::TokioFileOps;
use crate::value::Value;

/// Check if a userdata `Arc` holds a [`LuaFile`].
fn is_lua_file(ud: &dyn crate::userdata::Userdata) -> bool {
    ud.is::<LuaFile>()
}

/// Downcast a userdata reference to [`LuaFile`].
fn as_lua_file(ud: &dyn crate::userdata::Userdata) -> Option<&LuaFile> {
    ud.downcast_ref::<LuaFile>()
}

/// Register the `io` global table with filesystem functions.
///
/// This does **not** install stdio handles (`io.stdin`, `io.stdout`,
/// `io.stderr`) — call [`register_stdio`] for those.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = io_mod::build_module_table(env)?;
    env.set_global("io", Value::Table(table));
    Ok(())
}

// =========================================================================
// Mode string parsing
// =========================================================================

/// Parsed Lua file mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileMode {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
}

/// Parse a Lua mode string (`"r"`, `"w"`, `"a"`, `"r+"`, `"w+"`, `"a+"`)
/// into a [`FileMode`].  A trailing `"b"` (binary) is accepted and ignored
/// (Lua convention, no effect on Unix).
fn parse_mode(mode: &[u8]) -> Result<FileMode, String> {
    // Strip trailing 'b' if present.
    let mode = if mode.last() == Some(&b'b') {
        &mode[..mode.len() - 1]
    } else {
        mode
    };
    match mode {
        b"r" => Ok(FileMode {
            read: true,
            write: false,
            append: false,
            truncate: false,
        }),
        b"w" => Ok(FileMode {
            read: false,
            write: true,
            append: false,
            truncate: true,
        }),
        b"a" => Ok(FileMode {
            read: false,
            write: true,
            append: true,
            truncate: false,
        }),
        b"r+" => Ok(FileMode {
            read: true,
            write: true,
            append: false,
            truncate: false,
        }),
        b"w+" => Ok(FileMode {
            read: true,
            write: true,
            append: false,
            truncate: true,
        }),
        b"a+" => Ok(FileMode {
            read: true,
            write: true,
            append: true,
            truncate: false,
        }),
        _ => {
            let s = String::from_utf8_lossy(mode);
            Err(format!("invalid mode '{s}'"))
        }
    }
}

/// Open a file with the given [`FileMode`], returning a [`LuaFile`].
async fn open_file(filename: &str, mode: FileMode) -> Result<Arc<LuaFile>, std::io::Error> {
    let file = tokio::fs::OpenOptions::new()
        .read(mode.read)
        .write(mode.write)
        .append(mode.append)
        .truncate(mode.truncate)
        .create(mode.write || mode.append)
        .open(filename)
        .await?;
    let ops = TokioFileOps::new(file, mode.read, mode.write);
    Ok(LuaFile::new(filename, Box::new(ops)))
}

// =========================================================================
// Module functions
// =========================================================================

#[crate::module(name = "io")]
pub mod io_mod {
    use super::*;

    // -----------------------------------------------------------------
    // io.open(filename [, mode]) -> file | nil, errmsg
    // -----------------------------------------------------------------
    #[function]
    async fn open(filename: Bytes, mode: Option<Bytes>) -> Result<Variadic, VmError> {
        let mode_bytes = mode.as_deref().unwrap_or(b"r");
        let parsed = parse_mode(mode_bytes).map_err(|msg| VmError::BadArgument {
            position: 2,
            function: "open".to_owned(),
            expected: msg.clone(),
            got: msg,
        })?;
        let name = String::from_utf8_lossy(&filename);
        match open_file(&name, parsed).await {
            Ok(file) => Ok(Variadic(vec![Value::Userdata(file)])),
            Err(e) => {
                // Lua convention: nil, error message, error code
                let msg = format!("{}: {}", name, e);
                Ok(Variadic(vec![Value::Nil, Value::String(Bytes::from(msg))]))
            }
        }
    }

    // -----------------------------------------------------------------
    // io.close([file])
    //
    // Without arguments, closes the default output file (not yet
    // implemented — requires stdio state).  With a file argument,
    // equivalent to file:close().
    // -----------------------------------------------------------------
    #[function]
    async fn close(file: Value) -> Result<Variadic, VmError> {
        let lua_file = match &file {
            Value::Userdata(ud) => match as_lua_file(ud.as_ref()) {
                Some(f) => f,
                None => {
                    return Err(VmError::BadArgument {
                        position: 1,
                        function: "close".to_owned(),
                        expected: "file".to_owned(),
                        got: "userdata".to_owned(),
                    });
                }
            },
            _ => {
                return Err(VmError::BadArgument {
                    position: 1,
                    function: "close".to_owned(),
                    expected: "file".to_owned(),
                    got: file.type_name().to_owned(),
                });
            }
        };
        let mut guard = lua_file.lock_inner().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(Variadic(vec![
                Value::Nil,
                Value::String(Bytes::from_static(b"attempt to use a closed file")),
            ]));
        };
        let status = ops.close().await.map_err(|e| VmError::HostError {
            name: "close".to_owned(),
            source: e.to_string().into(),
        })?;
        *guard = None;
        Ok(Variadic(crate::file::close_status_to_lua(status)))
    }

    // -----------------------------------------------------------------
    // io.type(obj) -> "file" | "closed file" | nil
    // -----------------------------------------------------------------
    #[function(rename = "type")]
    async fn r#type(obj: Value) -> Result<Value, VmError> {
        match &obj {
            Value::Userdata(ud) if is_lua_file(ud.as_ref()) => {
                let lua_file = as_lua_file(ud.as_ref()).expect("checked by guard");
                if lua_file.is_closed().await {
                    Ok(Value::String(Bytes::from_static(b"closed file")))
                } else {
                    Ok(Value::String(Bytes::from_static(b"file")))
                }
            }
            _ => Ok(Value::Nil),
        }
    }

    // -----------------------------------------------------------------
    // io.tmpfile() -> file | nil, errmsg
    // -----------------------------------------------------------------
    #[function]
    async fn tmpfile() -> Result<Variadic, VmError> {
        // `tempfile::tempfile()` returns an anonymous `std::fs::File`
        // that is already unlinked from the filesystem.  The OS reclaims
        // the storage when the file descriptor is closed — no leak.
        let std_file = match tempfile::tempfile() {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("tmpfile: {}", e);
                return Ok(Variadic(vec![Value::Nil, Value::String(Bytes::from(msg))]));
            }
        };
        // Convert std::fs::File → tokio::fs::File for async I/O.
        let file = tokio::fs::File::from_std(std_file);
        let ops = TokioFileOps::new(file, true, true);
        Ok(Variadic(vec![Value::Userdata(LuaFile::new(
            "(tmpfile)",
            Box::new(ops),
        ))]))
    }

    // io.lines(filename, ...) is deferred — see TODO.md for discussion
    // about file handle lifetime / leak concerns.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::LuaFileOps;
    use std::io::Write;

    // =====================================================================
    // parse_mode
    // =====================================================================

    #[test]
    fn parse_mode_r() {
        let m = parse_mode(b"r").expect("valid");
        k9::assert_equal!(m.read, true);
        k9::assert_equal!(m.write, false);
        k9::assert_equal!(m.append, false);
        k9::assert_equal!(m.truncate, false);
    }

    #[test]
    fn parse_mode_w() {
        let m = parse_mode(b"w").expect("valid");
        k9::assert_equal!(m.read, false);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.append, false);
        k9::assert_equal!(m.truncate, true);
    }

    #[test]
    fn parse_mode_a() {
        let m = parse_mode(b"a").expect("valid");
        k9::assert_equal!(m.read, false);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.append, true);
        k9::assert_equal!(m.truncate, false);
    }

    #[test]
    fn parse_mode_r_plus() {
        let m = parse_mode(b"r+").expect("valid");
        k9::assert_equal!(m.read, true);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.append, false);
        k9::assert_equal!(m.truncate, false);
    }

    #[test]
    fn parse_mode_w_plus() {
        let m = parse_mode(b"w+").expect("valid");
        k9::assert_equal!(m.read, true);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.append, false);
        k9::assert_equal!(m.truncate, true);
    }

    #[test]
    fn parse_mode_a_plus() {
        let m = parse_mode(b"a+").expect("valid");
        k9::assert_equal!(m.read, true);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.append, true);
        k9::assert_equal!(m.truncate, false);
    }

    #[test]
    fn parse_mode_binary_suffix() {
        let m = parse_mode(b"rb").expect("valid");
        k9::assert_equal!(m.read, true);
        k9::assert_equal!(m.write, false);

        let m = parse_mode(b"w+b").expect("valid");
        k9::assert_equal!(m.read, true);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.truncate, true);
    }

    #[test]
    fn parse_mode_invalid() {
        let err = parse_mode(b"x").unwrap_err();
        k9::assert_equal!(err, "invalid mode 'x'");
    }

    #[test]
    fn parse_mode_empty() {
        let err = parse_mode(b"").unwrap_err();
        k9::assert_equal!(err, "invalid mode ''");
    }

    #[test]
    fn parse_mode_just_binary() {
        // "b" alone is not a valid mode — there's no base mode.
        let err = parse_mode(b"b").unwrap_err();
        k9::assert_equal!(err, "invalid mode ''");
    }

    #[test]
    fn parse_mode_plus_only() {
        let err = parse_mode(b"+").unwrap_err();
        k9::assert_equal!(err, "invalid mode '+'");
    }

    #[test]
    fn parse_mode_doubled() {
        let err = parse_mode(b"rr").unwrap_err();
        k9::assert_equal!(err, "invalid mode 'rr'");
    }

    #[test]
    fn parse_mode_ab() {
        let m = parse_mode(b"ab").expect("valid");
        k9::assert_equal!(m.append, true);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.read, false);
    }

    #[test]
    fn parse_mode_a_plus_b() {
        let m = parse_mode(b"a+b").expect("valid");
        k9::assert_equal!(m.read, true);
        k9::assert_equal!(m.write, true);
        k9::assert_equal!(m.append, true);
    }

    // =====================================================================
    // open_file
    // =====================================================================

    #[tokio::test]
    async fn open_read_existing() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
        tmp.write_all(b"hello").expect("write");
        let path = tmp.path().to_str().expect("path");
        let mode = parse_mode(b"r").expect("mode");
        let file = open_file(path, mode).await.expect("open");
        k9::assert_equal!(file.is_closed().await, false);
    }

    #[tokio::test]
    async fn open_read_nonexistent() {
        let mode = parse_mode(b"r").expect("mode");
        let err = open_file("/tmp/nonexistent_shingetsu_test_file_xyz", mode).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn open_write_creates_file() {
        let dir = tempfile::TempDir::new().expect("create dir");
        let path = dir.path().join("new_file.txt");
        let mode = parse_mode(b"w").expect("mode");
        let _file = open_file(path.to_str().expect("path"), mode)
            .await
            .expect("open");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn open_write_truncates() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
        tmp.write_all(b"existing content").expect("write");
        let path = tmp.path().to_str().expect("path");
        let mode = parse_mode(b"w").expect("mode");
        let _file = open_file(path, mode).await.expect("open");
        // File should be truncated — reading it should give empty.
        let contents = std::fs::read(tmp.path()).expect("read");
        k9::assert_equal!(contents.as_slice(), b"");
    }

    #[tokio::test]
    async fn open_append_preserves() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
        tmp.write_all(b"existing").expect("write");
        let path = tmp.path().to_str().expect("path");
        let mode = parse_mode(b"a").expect("mode");
        let file = open_file(path, mode).await.expect("open");
        // Write via the LuaFile's inner ops.
        {
            let mut guard = file.lock_inner().await;
            let ops = guard.as_mut().expect("not closed");
            ops.write_bytes(b" appended").await.expect("write");
            ops.flush().await.expect("flush");
        }
        let contents = std::fs::read(tmp.path()).expect("read");
        k9::assert_equal!(contents.as_slice(), b"existing appended");
    }

    #[tokio::test]
    async fn open_read_write() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
        tmp.write_all(b"hello world").expect("write");
        let path = tmp.path().to_str().expect("path");
        let mode = parse_mode(b"r+").expect("mode");
        let file = open_file(path, mode).await.expect("open");
        {
            let mut guard = file.lock_inner().await;
            let ops = guard.as_mut().expect("not closed");
            let data = ops.read_bytes(5).await.expect("read");
            k9::assert_equal!(data.as_ref(), b"hello");
            // Seek to position 5 before writing (mixed read/write
            // requires an intervening seek).
            ops.seek(std::io::SeekFrom::Start(5)).await.expect("seek");
            ops.write_bytes(b" rust").await.expect("write");
            ops.flush().await.expect("flush");
        }
        let contents = std::fs::read(tmp.path()).expect("read");
        k9::assert_equal!(contents.as_slice(), b"hello rustd");
    }

    // =====================================================================
    // open: w+ mode (truncate + read+write)
    // =====================================================================

    #[tokio::test]
    async fn open_write_plus_truncates_and_allows_read() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
        tmp.write_all(b"old data").expect("write");
        let path = tmp.path().to_str().expect("path");
        let mode = parse_mode(b"w+").expect("mode");
        let file = open_file(path, mode).await.expect("open");
        {
            let mut guard = file.lock_inner().await;
            let ops = guard.as_mut().expect("not closed");
            // File should be truncated.
            let all = ops.read_all().await.expect("read");
            k9::assert_equal!(all.as_ref(), b"");
            // Write new data and read it back.
            ops.write_bytes(b"new").await.expect("write");
            ops.seek(std::io::SeekFrom::Start(0)).await.expect("seek");
            let all = ops.read_all().await.expect("read");
            k9::assert_equal!(all.as_ref(), b"new");
        }
    }

    // =====================================================================
    // open: a+ mode (append + read)
    // =====================================================================

    #[tokio::test]
    async fn open_append_plus_preserves_and_allows_read() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
        tmp.write_all(b"existing").expect("write");
        let path = tmp.path().to_str().expect("path");
        let mode = parse_mode(b"a+").expect("mode");
        let file = open_file(path, mode).await.expect("open");
        {
            let mut guard = file.lock_inner().await;
            let ops = guard.as_mut().expect("not closed");
            // Read from the start.
            let all = ops.read_all().await.expect("read");
            k9::assert_equal!(all.as_ref(), b"existing");
            // Append writes go to the end regardless of position.
            ops.write_bytes(b" more").await.expect("write");
            ops.flush().await.expect("flush");
        }
        let contents = std::fs::read(tmp.path()).expect("read");
        k9::assert_equal!(contents.as_slice(), b"existing more");
    }

    // =====================================================================
    // open: write mode does not allow reading
    // =====================================================================

    #[tokio::test]
    async fn open_write_only_cannot_read() {
        let dir = tempfile::TempDir::new().expect("create dir");
        let path = dir.path().join("write_only.txt");
        let mode = parse_mode(b"w").expect("mode");
        let file = open_file(path.to_str().expect("path"), mode)
            .await
            .expect("open");
        let guard = file.lock_inner().await;
        let ops = guard.as_ref().expect("not closed");
        k9::assert_equal!(ops.can_read(), false);
        k9::assert_equal!(ops.can_write(), true);
    }

    // =====================================================================
    // tmpfile: round-trip write and read
    // =====================================================================

    #[tokio::test]
    async fn tmpfile_round_trip() {
        let std_file = tempfile::tempfile().expect("create tmp");
        let file = tokio::fs::File::from_std(std_file);
        let mut ops = TokioFileOps::new(file, true, true);
        ops.write_bytes(b"tmp data").await.expect("write");
        ops.seek(std::io::SeekFrom::Start(0)).await.expect("seek");
        let data = ops.read_all().await.expect("read");
        k9::assert_equal!(data.as_ref(), b"tmp data");
    }

    #[tokio::test]
    async fn tmpfile_is_seekable() {
        let std_file = tempfile::tempfile().expect("create tmp");
        let file = tokio::fs::File::from_std(std_file);
        let mut ops = TokioFileOps::new(file, true, true);
        ops.write_bytes(b"abcdef").await.expect("write");
        let pos = ops.seek(std::io::SeekFrom::Start(3)).await.expect("seek");
        k9::assert_equal!(pos, 3);
        let data = ops.read_bytes(3).await.expect("read");
        k9::assert_equal!(data.as_ref(), b"def");
    }

    // =====================================================================
    // io.type logic (tested via helper functions directly)
    // =====================================================================

    #[tokio::test]
    async fn io_type_open_file() {
        let file = LuaFile::new(
            "test",
            Box::new(crate::tokio_file::TokioFileOps::new(
                tokio::fs::File::from_std(tempfile::tempfile().expect("tmp")),
                true,
                true,
            )),
        );
        k9::assert_equal!(is_lua_file(file.as_ref()), true);
        k9::assert_equal!(file.is_closed().await, false);
    }

    #[tokio::test]
    async fn io_type_closed_file() {
        let file = LuaFile::new(
            "test",
            Box::new(crate::tokio_file::TokioFileOps::new(
                tokio::fs::File::from_std(tempfile::tempfile().expect("tmp")),
                true,
                true,
            )),
        );
        {
            let mut guard = file.lock_inner().await;
            let ops = guard.as_mut().expect("not closed");
            ops.close().await.expect("close");
            *guard = None;
        }
        k9::assert_equal!(is_lua_file(file.as_ref()), true);
        k9::assert_equal!(file.is_closed().await, true);
    }

    #[tokio::test]
    async fn io_type_non_file_value() {
        // Non-userdata values should not be identified as files.
        let check = |v: &Value| -> bool {
            match v {
                Value::Userdata(ud) => is_lua_file(ud.as_ref()),
                _ => false,
            }
        };
        k9::assert_equal!(check(&Value::Nil), false);
        k9::assert_equal!(check(&Value::Integer(42)), false);
        k9::assert_equal!(check(&Value::String(Bytes::from_static(b"hello"))), false);
    }
}
