//! Implementation of the `io` standard library module.

use crate::valuevec;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

use shingetsu::Bytes;
use shingetsu_vm::file::BufferMode;
use tokio::io::AsyncSeekExt;

use crate::call_context::CallContext;
use crate::convert::{StdlibResult, Variadic};
use crate::file::LuaFile;
use crate::tokio_file::TokioFileOps;
use crate::value::Value;
use crate::VmError;
use shingetsu_vm::error::PathIoError;

// =========================================================================
// Stdio singletons and default input/output
// =========================================================================

/// Process-wide stdin handle.
static STDIN: LazyLock<Arc<LuaFile>> = LazyLock::new(|| {
    let ops = stdio_file(
        std::io::stdin(),
        true,
        false,
        BufferMode::Full { size: Some(8192) },
    );
    LuaFile::new_uncloseable("stdin", Box::new(ops))
});

/// Process-wide stdout handle.
static STDOUT: LazyLock<Arc<LuaFile>> = LazyLock::new(|| {
    let buf_mode = if std::io::stdout().is_terminal() {
        BufferMode::Line { size: Some(8192) }
    } else {
        BufferMode::Full { size: Some(8192) }
    };
    let ops = stdio_file(std::io::stdout(), false, true, buf_mode);
    LuaFile::new_uncloseable("stdout", Box::new(ops))
});

/// Process-wide stderr handle.
static STDERR: LazyLock<Arc<LuaFile>> = LazyLock::new(|| {
    let ops = stdio_file(std::io::stderr(), false, true, BufferMode::No);
    LuaFile::new_uncloseable("stderr", Box::new(ops))
});

/// Create a [`TokioFileOps`] from a standard I/O handle by duping the
/// underlying file descriptor.
fn stdio_file(
    io: impl std::os::fd::AsFd,
    can_read: bool,
    can_write: bool,
    buf_mode: BufferMode,
) -> TokioFileOps {
    let owned = io
        .as_fd()
        .try_clone_to_owned()
        .expect("dup stdio file descriptor");
    let std_file = std::fs::File::from(owned);
    TokioFileOps::from_std(std_file, can_read, can_write).with_buf_mode(buf_mode)
}

/// Convert raw bytes from Lua into an OS string.
///
/// On Unix, this is a zero-copy conversion via `OsStrExt::from_bytes`
/// since OS strings are arbitrary byte sequences.  On other platforms,
/// the bytes must be valid UTF-8.
pub(crate) fn bytes_to_os_str(
    bytes: &[u8],
) -> Result<std::borrow::Cow<'_, std::ffi::OsStr>, std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(std::borrow::Cow::Borrowed(std::ffi::OsStr::from_bytes(
            bytes,
        )))
    }
    #[cfg(not(unix))]
    {
        let s = std::str::from_utf8(bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        Ok(std::borrow::Cow::Borrowed(std::ffi::OsStr::new(s)))
    }
}

/// Convert raw bytes from Lua into a filesystem path.
pub(crate) fn bytes_to_path(bytes: &[u8]) -> Result<std::path::PathBuf, std::io::Error> {
    bytes_to_os_str(bytes).map(|s| std::path::PathBuf::from(s.into_owned()))
}

/// Keys used to store the default input/output handles in the `io` table.
/// These are per-GlobalEnv since each `io` table is stored as a global.
const DEFAULT_INPUT_KEY: &str = "_default_input";
const DEFAULT_OUTPUT_KEY: &str = "_default_output";

/// Get the `io` table from the global environment.
fn get_io_table(ctx: &crate::call_context::CallContext) -> Result<crate::table::Table, VmError> {
    match ctx.global.get_global("io") {
        Some(Value::Table(t)) => Ok(t),
        _ => Err(lua_error("io table not found")),
    }
}

/// Get the default input file handle from the `io` table.
fn get_default_input(
    io_table: &crate::table::Table,
    env: &crate::GlobalEnv,
) -> Result<Arc<LuaFile>, VmError> {
    match io_table.raw_get(&Value::string(DEFAULT_INPUT_KEY))? {
        val @ Value::Userdata(_) => {
            let ud: crate::Ud<LuaFile> = crate::FromLua::from_lua(val, env)
                .map_err(|_| lua_error("default input is not a file"))?;
            Ok(Arc::clone(&ud))
        }
        _ => Err(lua_error("default input file is not set")),
    }
}

/// Get the default output file handle from the `io` table.
fn get_default_output(
    io_table: &crate::table::Table,
    env: &crate::GlobalEnv,
) -> Result<Arc<LuaFile>, VmError> {
    match io_table.raw_get(&Value::string(DEFAULT_OUTPUT_KEY))? {
        val @ Value::Userdata(_) => {
            let ud: crate::Ud<LuaFile> = crate::FromLua::from_lua(val, env)
                .map_err(|_| lua_error("default output is not a file"))?;
            Ok(Arc::clone(&ud))
        }
        _ => Err(lua_error("default output file is not set")),
    }
}

/// Set the default input/output in the `io` table.
fn set_default(
    io_table: &crate::table::Table,
    key: &str,
    file: &Arc<LuaFile>,
) -> Result<(), VmError> {
    io_table.raw_set(
        Value::string(key.to_owned()),
        crate::Ud(Arc::clone(file)).into(),
    )
}

/// Create a `VmError::LuaError` from a message string.
fn lua_error(msg: impl Into<String>) -> VmError {
    let s = msg.into();
    VmError::LuaError {
        display: s.clone(),
        value: Value::string(s),
    }
}

/// Whether a [`std::io::ErrorKind`] is plausibly attributable to
/// the path argument of an I/O call (versus a system-level
/// condition like running out of file descriptors).  Used by the
/// `io.*` functions to decide whether to tag the error with the
/// filename argument's position so the diagnostic carets land on
/// the offending argument.
fn io_kind_implicates_path(kind: std::io::ErrorKind) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        kind,
        NotFound | PermissionDenied | AlreadyExists | InvalidInput | InvalidData
    )
}

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
    env.register_module_type("io", io_mod::module_type());
    env.register_userdata_type(LuaFile::userdata_type());
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

impl FileMode {
    /// Read-only mode (`"r"`).
    const READ: Self = Self {
        read: true,
        write: false,
        append: false,
        truncate: false,
    };

    /// Write-only mode (`"w"`), truncating.
    const WRITE: Self = Self {
        read: false,
        write: true,
        append: false,
        truncate: true,
    };
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
///
/// `filename` is raw bytes from Lua; on Unix these are passed through
/// as-is via `OsStr::from_encoded_bytes_unchecked` (filenames are
/// arbitrary byte sequences, not necessarily UTF-8).
async fn open_file(filename: &[u8], mode: FileMode) -> Result<Arc<LuaFile>, PathIoError> {
    let raw_path = Bytes::from(filename);
    let path = bytes_to_path(filename).map_err(|source| PathIoError {
        path: raw_path.clone(),
        source,
    })?;

    let mut file = tokio::fs::OpenOptions::new()
        .read(mode.read)
        .write(mode.write)
        .append(mode.append)
        .truncate(mode.truncate)
        .create(mode.write || mode.append)
        .open(&path)
        .await
        .map_err(|source| PathIoError {
            path: raw_path.clone(),
            source,
        })?;
    let display_name = String::from_utf8_lossy(filename);
    let can_seek = file.seek(tokio::io::SeekFrom::Current(0)).await.is_ok();
    let ops = TokioFileOps::new(file, mode.read, mode.write, can_seek);
    Ok(LuaFile::new(display_name.as_ref(), Box::new(ops)))
}

// =========================================================================
// Module functions
// =========================================================================

/// Parameter for `io.input` / `io.output`: either an existing file
/// handle or a filename to open.
#[derive(crate::FromLua, crate::LuaTyped)]
enum FileOrName {
    File(crate::Ud<LuaFile>),
    Name(Bytes),
}

/// Return type for `io.close`: close status or `(nil, errmsg)` for
/// already-closed files.
enum IoCloseResult {
    Status(crate::file::CloseStatus),
    Error(String),
}

impl crate::convert::IntoLuaMulti for IoCloseResult {
    fn into_lua_multi(self) -> crate::ValueVec {
        match self {
            IoCloseResult::Status(s) => s.into_lua_multi(),
            IoCloseResult::Error(msg) => valuevec![Value::Nil, Value::string(msg)],
        }
    }
}

impl crate::convert::LuaTypedMulti for IoCloseResult {
    fn lua_types() -> Vec<crate::types::LuaType> {
        use crate::types::LuaType;
        // boolean | (boolean?, string, integer) | (nil, string)
        vec![LuaType::Union(vec![
            LuaType::Boolean,
            LuaType::Tuple(vec![
                LuaType::Optional(Box::new(LuaType::Boolean)),
                LuaType::String,
                LuaType::Integer,
            ]),
            LuaType::Tuple(vec![LuaType::Nil, LuaType::String]),
        ])]
    }
}

/// File and stream input/output.
///
/// The `io` module exposes Lua's file-handling API: open files,
/// read from and write to them, run subprocesses through a pipe,
/// and access the program's standard streams.  Files are returned
/// as `file` userdata; see the `file` reference for the methods
/// you can call on them.
///
/// Most functions follow the convention that recoverable errors
/// produce `nil` plus an error message, while a successful call
/// returns the meaningful value.  Programmatic mistakes (passing
/// the wrong type, an invalid mode string, etc.) raise an error
/// instead.
///
/// `io.read` / `io.write` / `io.input` / `io.output` operate on
/// the program's default input and output streams (initially the
/// process's stdin and stdout).  Switching the default with
/// `io.input(file)` or `io.output(file)` lets you redirect output
/// from `io.write` without rewriting every call.
#[crate::module(name = "io")]
pub mod io_mod {
    use super::*;
    use shingetsu_vm::VmResultExt;

    /// Open a file and return its handle.
    ///
    /// `mode` is a string controlling read/write access:
    ///
    /// - `"r"` (default) — read-only
    /// - `"w"` — write-only, truncating any existing content
    /// - `"a"` — write-only, appending to existing content
    /// - `"r+"` — read and write, file must exist
    /// - `"w+"` — read and write, truncates
    /// - `"a+"` — read and write, appends
    ///
    /// A trailing `"b"` (e.g. `"rb"`, `"w+b"`) is accepted but has
    /// no effect: shingetsu treats all files as binary.
    ///
    /// Returns the file handle on success, or `nil` plus an error
    /// message on failure (file not found, permission denied, etc.).
    /// Invalid mode strings raise an error.
    ///
    /// # Parameters
    ///
    /// - `filename` — path to the file
    /// - `mode` — access mode string; defaults to `"r"`
    ///
    /// # Returns
    ///
    /// - the file handle on success
    /// - `nil` plus an error message on failure
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Write to a temp file, then read it back.
    /// local path = os.tmpname()
    /// local f = io.open(path, "w")
    /// f:write("hello\n")
    /// f:close()
    ///
    /// local r = io.open(path, "r")
    /// assert(r:read("l") == "hello")
    /// r:close()
    /// os.remove(path)
    /// ```
    ///
    /// ```lua
    /// -- Failure path: opening a non-existent file for reading.
    /// local f, err = io.open(os.tmpname() .. ".missing", "r")
    /// assert(f == nil)
    /// assert(type(err) == "string")
    /// ```
    #[function]
    async fn open(
        filename: Bytes,
        mode: Option<Bytes>,
    ) -> Result<StdlibResult<crate::Ud<LuaFile>>, VmError> {
        let mode_bytes = mode.as_deref().unwrap_or(b"r");
        let parsed = parse_mode(mode_bytes).map_err(|msg| {
            VmError::ArgError {
                position: 2,
                function: "open".to_owned(),
                msg,
            }
            .or_suggest(
                mode_bytes,
                "mode",
                &[b"r", b"w", b"a", b"r+", b"w+", b"a+"],
                "valid modes are `r`, `w`, `a`, `r+`, `w+`, `a+`; a \
                 trailing `b` is accepted for compatibility but has \
                 no effect",
            )
        })?;
        match open_file(&filename, parsed).await {
            Ok(file) => Ok(StdlibResult::Ok(file.into())),
            Err(e) => Ok(StdlibResult::Err(e.to_string())),
        }
    }

    /// Close a file.
    ///
    /// With a file argument, closes that file (equivalent to
    /// calling `file:close()`).  With no argument, closes the
    /// default output file.
    ///
    /// Closing the standard streams (`io.stdin`, `io.stdout`,
    /// `io.stderr`) is a no-op that returns `nil` plus an error
    /// message: those streams stay open for the lifetime of the
    /// process.
    ///
    /// Returns the close status from the underlying handle:
    /// usually `true`, but for files that wrap subprocess pipes
    /// (`io.popen`) returns the same `(success, kind, code)` tuple
    /// as `os.execute`.
    ///
    /// # Parameters
    ///
    /// - `file` — file handle to close; defaults to the current
    ///   default output file
    ///
    /// # Returns
    ///
    /// - `true` for normal files
    /// - `(boolean?, string, integer)` for `io.popen` files
    /// - `nil` plus an error message when the file is already
    ///   closed or is a non-closeable standard stream
    ///
    /// # Examples
    ///
    /// ```lua
    /// local path = os.tmpname()
    /// local f = io.open(path, "w")
    /// io.close(f)               -- equivalent to f:close()
    /// os.remove(path)
    /// ```
    ///
    /// ```lua
    /// -- Closing an already-closed file returns nil plus a message.
    /// local path = os.tmpname()
    /// local f = io.open(path, "w")
    /// f:close()
    /// local ok, err = io.close(f)
    /// assert(ok == nil)
    /// assert(type(err) == "string")
    /// os.remove(path)
    /// ```
    #[function]
    async fn close(
        ctx: CallContext,
        file: Option<crate::Ud<LuaFile>>,
    ) -> Result<super::IoCloseResult, VmError> {
        let lua_file: Arc<LuaFile> = match file {
            Some(f) => f.into(),
            None => {
                // No argument — close the default output file.
                let io_table = get_io_table(&ctx)?;
                get_default_output(&io_table, &ctx.global)?
            }
        };
        if !lua_file.is_closeable() {
            return Ok(super::IoCloseResult::Error(
                "cannot close standard file".to_owned(),
            ));
        }
        let mut guard = lua_file.lock_inner().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(super::IoCloseResult::Error(
                "attempt to use a closed file".to_owned(),
            ));
        };
        let status = ops.close().await.map_err(|e| VmError::HostError {
            name: "close".to_owned(),
            source: e.to_string().into(),
        })?;
        *guard = None;
        Ok(super::IoCloseResult::Status(status))
    }

    /// Test whether a value is a file handle.
    ///
    /// Returns:
    ///
    /// - `"file"` when `obj` is an open file handle
    /// - `"closed file"` when `obj` is a file handle that has
    ///   been closed
    /// - `nil` when `obj` is anything else
    ///
    /// # Parameters
    ///
    /// - `obj` — any value
    ///
    /// # Returns
    ///
    /// - `"file"`, `"closed file"`, or `nil`
    ///
    /// # Examples
    ///
    /// ```lua
    /// local path = os.tmpname()
    /// local f = io.open(path, "w")
    /// assert(io.type(f) == "file")
    /// f:close()
    /// assert(io.type(f) == "closed file")
    /// assert(io.type("hello") == nil)
    /// assert(io.type(42) == nil)
    /// os.remove(path)
    /// ```
    #[function(rename = "type")]
    async fn r#type(obj: Value) -> Option<&'static str> {
        match &obj {
            Value::Userdata(ud) if is_lua_file(ud.as_ref()) => {
                let lua_file = as_lua_file(ud.as_ref()).expect("checked by guard");
                if lua_file.is_closed().await {
                    Some("closed file")
                } else {
                    Some("file")
                }
            }
            _ => None,
        }
    }

    /// Open an anonymous temporary file.
    ///
    /// Returns a fresh file handle backed by an anonymous file in
    /// the operating system's temp directory.  The file is opened
    /// for reading and writing, has no visible directory entry,
    /// and is reclaimed when the handle is closed (or when the
    /// process exits).
    ///
    /// `io.tmpfile` is the safe choice for short-lived scratch
    /// storage — unlike `os.tmpname`, no other process can race
    /// to create or alter the file because there's no name to
    /// race on.
    ///
    /// On failure (e.g. the temp directory is full or unwritable)
    /// returns `nil` plus an error message.
    ///
    /// # Parameters
    ///
    /// (none)
    ///
    /// # Returns
    ///
    /// - the file handle on success
    /// - `nil` plus an error message on failure
    ///
    /// # Examples
    ///
    /// ```lua
    /// local f = io.tmpfile()
    /// f:write("scratch")
    /// f:seek("set", 0)
    /// assert(f:read("a") == "scratch")
    /// f:close()
    /// ```
    #[function]
    async fn tmpfile() -> Result<StdlibResult<crate::Ud<LuaFile>>, VmError> {
        // `tempfile::tempfile()` returns an anonymous `std::fs::File`
        // that is already unlinked from the filesystem.  The OS reclaims
        // the storage when the file descriptor is closed — no leak.
        let std_file = match tempfile::tempfile() {
            Ok(f) => f,
            Err(e) => {
                return Ok(StdlibResult::Err(format!("tmpfile: {e}")));
            }
        };
        // Convert std::fs::File → TokioFileOps, probing seekability.
        let ops = TokioFileOps::from_std(std_file, true, true);
        Ok(StdlibResult::Ok(
            LuaFile::new("(tmpfile)", Box::new(ops)).into(),
        ))
    }

    /// Open a file and return an iterator over its contents.
    ///
    /// Designed for use with the generic `for` loop:
    ///
    /// ```lua
    /// for line in io.lines("file.txt") do
    ///     -- ...
    /// end
    /// ```
    ///
    /// Each iteration reads one line by default; pass additional
    /// format arguments (the same set accepted by `file:read`) to
    /// read multiple values per iteration.  Iteration stops when
    /// the first format value is `nil` (typically end-of-file).
    ///
    /// The file is closed automatically when the iterator reaches
    /// end-of-file, and also when the loop exits early via
    /// `break`, an error, or the enclosing scope's `<close>`
    /// handling — you do not need to close it manually.
    ///
    /// Errors opening the file or reading from it surface as Lua
    /// errors (not as `nil`-plus-message returns).
    ///
    /// # Parameters
    ///
    /// - `filename` — path to read
    ///
    /// # Returns
    ///
    /// - the four values used by the generic `for` loop:
    ///   iterator, state, control, and a `<close>` handle that
    ///   guarantees the file is closed on early exit
    ///
    /// # Examples
    ///
    /// ```lua
    /// local path = os.tmpname()
    /// local f = io.open(path, "w")
    /// f:write("alpha\nbeta\ngamma\n")
    /// f:close()
    ///
    /// local lines = {}
    /// for line in io.lines(path) do
    ///     table.insert(lines, line)
    /// end
    /// assert(table.concat(lines, ",") == "alpha,beta,gamma")
    /// os.remove(path)
    /// ```
    #[function]
    async fn lines(ctx: CallContext, filename: Bytes, args: Variadic) -> Result<Variadic, VmError> {
        // Tag a failure to open the file with arg position 1 so the
        // diagnostic carets land on the filename argument when the
        // failure is plausibly attributable to it (NotFound,
        // PermissionDenied, etc.).  System-level errors
        // (TooManyOpenFiles, OutOfMemory, Interrupted) are not
        // attributable to the argument, so we tag only when the
        // kind points at the path.
        let file = open_file(&filename, FileMode::READ).await.map_err(|e| {
            let attributable = io_kind_implicates_path(e.source.kind());
            let err = lua_error(e.to_string());
            if attributable {
                err.with_arg_position(1)
            } else {
                err
            }
        })?;

        // Parse format args now; default is "*l".
        let formats: Vec<crate::file::ReadFormat> = if args.0.is_empty() {
            vec![crate::file::ReadFormat::Line]
        } else {
            args.0
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    crate::file::ReadFormat::from_value(v, "lines").with_call_context(i + 2, &ctx)
                })
                .collect::<Result<_, _>>()?
        };

        let iter_file = Arc::clone(&file);
        let iter_fn = crate::Function::wrap("io.lines iterator", move || {
            let file = Arc::clone(&iter_file);
            let formats = formats.clone();
            async move {
                let mut guard = file.lock_inner().await;
                let Some(ops) = guard.as_mut() else {
                    // File already closed — return nil to stop iteration.
                    return Ok(Variadic(valuevec![Value::Nil]));
                };
                let mut results = Vec::with_capacity(formats.len());
                for fmt in &formats {
                    let val = crate::file::read_one(ops.as_mut(), fmt)
                        .await
                        .map_err(|e| crate::file::io_err_to_vm("lines iterator", e))?;
                    results.push(val);
                }
                // At EOF the first value is nil, terminating the for loop.
                // Auto-close the file when we hit EOF.
                if results.first().map_or(true, |v| v.is_nil()) {
                    if let Some(ops) = guard.as_mut() {
                        let _ = ops.close().await;
                    }
                    *guard = None;
                }
                Ok(Variadic(results.into()))
            }
        });

        // Return (iter_fn, nil, nil, file_handle).
        // The 4th value is the generic-for closing variable with <close>.
        Ok(Variadic(valuevec![
            Value::Function(iter_fn),
            Value::Nil,
            Value::Nil,
            crate::Ud(file).into(),
        ]))
    }
}

// =========================================================================
// Stdio module — registered separately via `register_stdio`
// =========================================================================

// Stdio additions: stdin/stdout/stderr fields plus the
// `io.read`/`io.write`/`io.flush`/`io.input`/`io.output` functions
// that operate on the program's default streams.  Registered as a
// merge into the existing `io` table.
#[crate::module(name = "io_stdio")]
pub mod io_stdio_mod {
    use super::*;

    /// The standard input stream, as a non-closeable file handle.
    ///
    /// Reading from `io.stdin` consumes data from the program's
    /// standard input.  Calling `io.stdin:close()` is a no-op that
    /// returns `nil` plus an error message; the stream lives for
    /// the entire process.
    ///
    /// # Examples
    ///
    /// ```lua,no_run
    /// -- Read one line from standard input.
    /// local line = io.stdin:read("l")
    /// print("got: " .. line)
    /// ```
    #[field]
    fn stdin() -> crate::Ud<LuaFile> {
        Arc::clone(&STDIN).into()
    }

    /// The standard output stream, as a non-closeable file handle.
    ///
    /// `io.stdout` is the initial default output for `io.write`.
    /// Writes are buffered; call `io.flush()` (or rely on process
    /// exit) to push them through.
    ///
    /// # Examples
    ///
    /// ```lua,no_run
    /// io.stdout:write("hello\n")
    /// ```
    #[field]
    fn stdout() -> crate::Ud<LuaFile> {
        Arc::clone(&STDOUT).into()
    }

    /// The standard error stream, as a non-closeable file handle.
    ///
    /// `io.stderr` writes to the program's diagnostic stream
    /// (separate from `io.stdout`).  By convention, programs use
    /// it for error messages and progress output.
    ///
    /// # Examples
    ///
    /// ```lua,no_run
    /// io.stderr:write("warning: foo\n")
    /// ```
    #[field]
    fn stderr() -> crate::Ud<LuaFile> {
        Arc::clone(&STDERR).into()
    }

    /// Read from the default input stream.
    ///
    /// Equivalent to `io.input():read(...)`: reads from whichever
    /// file is currently the default input (initially
    /// `io.stdin`).  The format arguments behave exactly as in
    /// `file:read`.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more read formats; defaults to a single
    ///   line read when no formats are given
    ///
    /// # Returns
    ///
    /// - one value per format, in source order
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Redirect input to a temp file, read from it, then restore.
    /// local path = os.tmpname()
    /// local w = io.open(path, "w"); w:write("hello\nworld\n"); w:close()
    /// io.input(path)
    /// assert(io.read("l") == "hello")
    /// assert(io.read("l") == "world")
    /// io.input(io.stdin) -- restore the default
    /// os.remove(path)
    /// ```
    #[function]
    async fn read(ctx: CallContext, args: Variadic) -> Result<Variadic, VmError> {
        let io_table = get_io_table(&ctx)?;
        let input = get_default_input(&io_table, &ctx.global)?;
        let mut guard = input.lock_inner().await;
        let Some(ops) = guard.as_mut() else {
            return Err(lua_error("default input file is closed")
                .with_hint("open a new file with `io.input(filename)` first"));
        };
        crate::file::lua_file_read(ops.as_mut(), &args.0).await
    }

    /// Write to the default output stream.
    ///
    /// Equivalent to `io.output():write(...)`: writes to whichever
    /// file is currently the default output (initially
    /// `io.stdout`).  Each argument must be a string or number;
    /// other types raise an error.  Returns the default output
    /// file for chaining.
    ///
    /// # Parameters
    ///
    /// - `...` — zero or more strings or numbers to write
    ///
    /// # Returns
    ///
    /// - the default output file (so calls can be chained)
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Redirect output to a temp file, write to it, then restore.
    /// local path = os.tmpname()
    /// io.output(path)
    /// io.write("line1\n", "line2\n")
    /// io.output():close()
    /// io.output(io.stdout) -- restore the default
    ///
    /// local f = io.open(path, "r")
    /// assert(f:read("a") == "line1\nline2\n")
    /// f:close()
    /// os.remove(path)
    /// ```
    #[function]
    async fn write(ctx: CallContext, args: Variadic) -> Result<crate::Ud<LuaFile>, VmError> {
        let io_table = get_io_table(&ctx)?;
        let output = get_default_output(&io_table, &ctx.global)?;
        let mut guard = output.lock_inner().await;
        let Some(ops) = guard.as_mut() else {
            return Err(lua_error("default output file is closed")
                .with_hint("open a new file with `io.output(filename)` first"));
        };
        crate::file::lua_file_write(ops.as_mut(), &args.0, &output).await
    }

    // -----------------------------------------------------------------
    // io.flush() — equivalent to io.output():flush()
    /// Flush buffered writes on the default output stream.
    ///
    /// Equivalent to `io.output():flush()`.  Buffered output
    /// becomes visible to readers after this call returns.  On
    /// failure, returns `nil` plus an error message.
    ///
    /// # Parameters
    ///
    /// (none)
    ///
    /// # Returns
    ///
    /// - `true` on success
    /// - `nil` plus an error message on failure
    ///
    /// # Examples
    ///
    /// ```lua,no_run
    /// io.write("progress\n")
    /// io.flush()  -- ensure the line reaches stdout immediately
    /// ```
    #[function]
    async fn flush(ctx: CallContext) -> Result<StdlibResult, VmError> {
        let io_table = get_io_table(&ctx)?;
        let output = get_default_output(&io_table, &ctx.global)?;
        let mut guard = output.lock_inner().await;
        let Some(ops) = guard.as_mut() else {
            return Err(lua_error("default output file is closed")
                .with_hint("open a new file with `io.output(filename)` first"));
        };
        ops.flush()
            .await
            .map_err(|e| lua_error(shingetsu_vm::error::portable_io_error_description(&e)))?;
        Ok(StdlibResult::Ok(true))
    }

    /// Get or set the default input file.
    ///
    /// With no argument, returns the current default input file.
    /// With a file handle, sets it as the new default and returns
    /// it.  With a string, opens the named file in read mode,
    /// makes it the default, and returns it.  Subsequent
    /// `io.read` calls operate on whatever the default is.
    ///
    /// Setting the default does not close the previous default;
    /// callers that need to clean up should keep a reference
    /// before switching.
    ///
    /// # Parameters
    ///
    /// - `file` — optional file handle or path string
    ///
    /// # Returns
    ///
    /// - the default input file (the new one when set, or the
    ///   current one when called with no argument)
    ///
    /// # Examples
    ///
    /// ```lua
    /// local path = os.tmpname()
    /// local w = io.open(path, "w"); w:write("42\n"); w:close()
    /// io.input(path)                  -- string: opens path
    /// assert(io.read("l") == "42")
    /// io.input(io.stdin)              -- file: restore stdin
    /// os.remove(path)
    /// ```
    #[function]
    async fn input(
        ctx: CallContext,
        file: Option<super::FileOrName>,
    ) -> Result<crate::Ud<LuaFile>, VmError> {
        let io_table = get_io_table(&ctx)?;
        match file {
            None => {
                let input = get_default_input(&io_table, &ctx.global)?;
                Ok(input.into())
            }
            Some(super::FileOrName::File(f)) => {
                set_default(&io_table, DEFAULT_INPUT_KEY, &f.0)?;
                Ok(f)
            }
            Some(super::FileOrName::Name(name)) => {
                let new_input = open_file(&name, FileMode::READ).await.map_err(|e| {
                    let attributable = io_kind_implicates_path(e.source.kind());
                    let err = VmError::IoError { source: e };
                    if attributable {
                        err.with_arg_position(1)
                    } else {
                        err
                    }
                })?;
                set_default(&io_table, DEFAULT_INPUT_KEY, &new_input)?;
                Ok(new_input.into())
            }
        }
    }

    /// Get or set the default output file.
    ///
    /// With no argument, returns the current default output file.
    /// With a file handle, sets it as the new default and returns
    /// it.  With a string, opens the named file in write mode,
    /// makes it the default, and returns it.  Subsequent
    /// `io.write` calls operate on whatever the default is.
    ///
    /// # Parameters
    ///
    /// - `file` — optional file handle or path string
    ///
    /// # Returns
    ///
    /// - the default output file
    ///
    /// # Examples
    ///
    /// ```lua
    /// local path = os.tmpname()
    /// io.output(path)                 -- string: opens path for writing
    /// io.write("redirected\n")
    /// io.output():close()
    /// io.output(io.stdout)            -- file: restore stdout
    ///
    /// local f = io.open(path, "r")
    /// assert(f:read("l") == "redirected")
    /// f:close()
    /// os.remove(path)
    /// ```
    #[function]
    async fn output(
        ctx: CallContext,
        file: Option<super::FileOrName>,
    ) -> Result<crate::Ud<LuaFile>, VmError> {
        let io_table = get_io_table(&ctx)?;
        match file {
            None => {
                let output = get_default_output(&io_table, &ctx.global)?;
                Ok(output.into())
            }
            Some(super::FileOrName::File(f)) => {
                set_default(&io_table, DEFAULT_OUTPUT_KEY, &f.0)?;
                Ok(f)
            }
            Some(super::FileOrName::Name(name)) => {
                let new_output = open_file(&name, FileMode::WRITE).await.map_err(|e| {
                    let attributable = io_kind_implicates_path(e.source.kind());
                    let err = VmError::IoError { source: e };
                    if attributable {
                        err.with_arg_position(1)
                    } else {
                        err
                    }
                })?;
                set_default(&io_table, DEFAULT_OUTPUT_KEY, &new_output)?;
                Ok(new_output.into())
            }
        }
    }
}

/// Register stdio handles and related functions into the existing `io`
/// global table.  Requires [`crate::io::register`] to have been called first.
///
/// Call [`flush_stdio`] before process exit to ensure buffered output
/// is flushed (safe to call unconditionally — it is a no-op if stdio
/// was not registered).
pub fn register_stdio(env: &crate::GlobalEnv) -> Result<(), VmError> {
    // Build the stdio module table (contains stdin/stdout/stderr fields
    // and read/write/flush/input/output functions).
    let stdio_table = io_stdio_mod::build_module_table(env)?;

    // Merge entries into the existing `io` global table.
    let io_table = match env.get_global("io") {
        Some(Value::Table(t)) => t,
        _ => {
            return Err(lua_error("io table not found; call io::register() first"));
        }
    };
    let mut key = Value::Nil;
    loop {
        match stdio_table.next(&key)? {
            Some((k, v)) => {
                io_table.raw_set(k.clone(), v)?;
                key = k;
            }
            None => break,
        }
    }

    // Set the default input/output to stdin/stdout.
    set_default(&io_table, DEFAULT_INPUT_KEY, &STDIN)?;
    set_default(&io_table, DEFAULT_OUTPUT_KEY, &STDOUT)?;

    env.register_module_type("io", io_stdio_mod::module_type());

    STDIO_REGISTERED.store(true, Ordering::Release);

    Ok(())
}

/// Tracks whether [`register_stdio`] has been called.  Used by
/// [`flush_stdio`] to avoid forcing `LazyLock` initialization.
static STDIO_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Flush the process-wide stdout and stderr handles.
///
/// Call this before process exit to ensure buffered `io.write` output
/// is actually written.  The stdio `LazyLock` statics live for the
/// process lifetime and have no `Drop`-based flush.
///
/// Safe to call even if [`register_stdio`] was never called — it is
/// a no-op in that case.
pub async fn flush_stdio() {
    if !STDIO_REGISTERED.load(Ordering::Acquire) {
        return;
    }
    for handle in [&*STDOUT, &*STDERR] {
        let mut guard = handle.lock_inner().await;
        if let Some(ops) = guard.as_mut() {
            let _ = ops.flush().await;
        }
    }
}

// =========================================================================
// io.popen — registered separately via `register_popen`
// =========================================================================

/// Spawn a child process connected via a pipe.
///
/// Uses `/bin/sh -c prog` (POSIX `popen` semantics).  Mode `"r"` (default)
/// pipes the child's stdout for reading; mode `"w"` pipes the child's
/// stdin for writing.  Unpiped stdio streams are inherited from the
/// parent process.
async fn popen_impl(
    prog: Bytes,
    mode: Option<Bytes>,
) -> Result<StdlibResult<crate::Ud<LuaFile>>, VmError> {
    let mode_bytes = mode.as_deref().unwrap_or(b"r");
    let (pipe_read, pipe_write) = match mode_bytes {
        b"r" => (true, false),
        b"w" => (false, true),
        _ => {
            return Err(VmError::BadArgument {
                position: 2,
                function: "popen".to_owned(),
                expected: "'r' or 'w'".to_owned(),
                got: format!("{:?}", bstr::BStr::new(mode_bytes)),
            }
            .or_suggest(
                mode_bytes,
                "mode",
                &[b"r", b"w"],
                "`io.popen` only supports unidirectional pipes: `r` \
                 to read the child's stdout, `w` to write to the \
                 child's stdin",
            ));
        }
    };

    let prog_os = bytes_to_os_str(&prog).map_err(|e| lua_error(format!("popen: {}", e)))?;

    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg("-c").arg(&*prog_os);

    if pipe_read {
        cmd.stdout(std::process::Stdio::piped());
    } else {
        cmd.stdin(std::process::Stdio::piped());
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Ok(StdlibResult::Err(format!("popen: {e}")));
        }
    };

    // Convert the piped fd to a TokioFileOps via OwnedFd -> std::fs::File.
    let std_file: std::fs::File = if pipe_read {
        let stdout = child.stdout.take().expect("stdout was piped");
        stdout
            .into_owned_fd()
            .map_err(|e| lua_error(format!("popen: {}", e)))?
            .into()
    } else {
        let stdin = child.stdin.take().expect("stdin was piped");
        stdin
            .into_owned_fd()
            .map_err(|e| lua_error(format!("popen: {}", e)))?
            .into()
    };
    let io_ops = TokioFileOps::from_std(std_file, pipe_read, pipe_write);

    let popen_ops = crate::popen::PopenOps::new(io_ops, child, pipe_read, pipe_write);
    let display = format!("(popen: /bin/sh -c {})", String::from_utf8_lossy(&prog));
    let file = LuaFile::new(&display, Box::new(popen_ops));
    Ok(StdlibResult::Ok(file.into()))
}

/// Register `io.popen` into the existing `io` global table.
///
/// Requires [`crate::io::register`] to have been called first.
pub fn register_popen(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let io_table = match env.get_global("io") {
        Some(Value::Table(t)) => t,
        _ => {
            return Err(lua_error("io table not found; call io::register() first"));
        }
    };

    // Build a tiny module table containing just `popen`, then merge.
    let popen_table = io_popen_mod::build_module_table(env)?;
    let mut key = Value::Nil;
    loop {
        match popen_table.next(&key)? {
            Some((k, v)) => {
                io_table.raw_set(k.clone(), v)?;
                key = k;
            }
            None => break,
        }
    }

    env.register_module_type("io", io_popen_mod::module_type());

    Ok(())
}

// `io.popen`: file-based view of a subprocess pipe.  Registered
// alongside `os.execute` under the `Libraries::EXEC` option since
// both spawn `/bin/sh -c`.
#[crate::module(name = "io_popen")]
mod io_popen_mod {
    use super::*;

    /// Spawn a subprocess and connect to its standard input or
    /// output as a file handle.
    ///
    /// Runs `prog` via `/bin/sh -c` (so shell features like
    /// pipelines and redirection inside `prog` work) and returns a
    /// file handle to one end of the pipe:
    ///
    /// - `mode = "r"` (default): read mode; the handle reads from
    ///   the child's standard output.  The child's standard
    ///   input is inherited from the parent.
    /// - `mode = "w"`: write mode; the handle writes to the
    ///   child's standard input.  The child's standard output is
    ///   inherited.
    ///
    /// Closing the file handle waits for the child to exit and
    /// returns the same `(success, kind, code)` tuple as
    /// `os.execute`, so you can recover the exit status.
    ///
    /// On failure to spawn (typically when the shell can't be
    /// found) returns `nil` plus an error message.
    ///
    /// # Parameters
    ///
    /// - `prog` — shell command to run
    /// - `mode` — `"r"` or `"w"`; defaults to `"r"`
    ///
    /// # Returns
    ///
    /// - the file handle on success
    /// - `nil` plus an error message on failure
    ///
    /// # Examples
    ///
    /// ```lua,no_run
    /// -- Run a command and read its output line by line.
    /// local p = io.popen("echo hello")
    /// for line in p:lines() do print(line) end
    /// p:close()
    /// ```
    ///
    /// ```lua,no_run
    /// -- Pipe input into a command.
    /// local p = io.popen("cat > /tmp/out.txt", "w")
    /// p:write("hi\n")
    /// p:close()
    /// ```
    #[function]
    async fn popen(
        prog: Bytes,
        mode: Option<Bytes>,
    ) -> Result<StdlibResult<crate::Ud<LuaFile>>, VmError> {
        popen_impl(prog, mode).await
    }
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
        let file = open_file(path.as_bytes(), mode).await.expect("open");
        k9::assert_equal!(file.is_closed().await, false);
    }

    #[tokio::test]
    async fn open_read_nonexistent() {
        let mode = parse_mode(b"r").expect("mode");
        let result = open_file(b"/tmp/nonexistent_shingetsu_test_file_xyz", mode).await;
        let err = match result {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => e,
        };
        k9::assert_equal!(
            err.to_string(),
            "/tmp/nonexistent_shingetsu_test_file_xyz: No such file or directory"
        );
    }

    #[tokio::test]
    async fn open_write_creates_file() {
        let dir = tempfile::TempDir::new().expect("create dir");
        let path = dir.path().join("new_file.txt");
        let mode = parse_mode(b"w").expect("mode");
        let path_str = path.to_str().expect("path");
        let _file = open_file(path_str.as_bytes(), mode).await.expect("open");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn open_write_truncates() {
        let mut tmp = tempfile::NamedTempFile::new().expect("create temp");
        tmp.write_all(b"existing content").expect("write");
        let path = tmp.path().to_str().expect("path");
        let mode = parse_mode(b"w").expect("mode");
        let _file = open_file(path.as_bytes(), mode).await.expect("open");
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
        let file = open_file(path.as_bytes(), mode).await.expect("open");
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
        let file = open_file(path.as_bytes(), mode).await.expect("open");
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
        let file = open_file(path.as_bytes(), mode).await.expect("open");
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
        let file = open_file(path.as_bytes(), mode).await.expect("open");
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
        let path_str = path.to_str().expect("path");
        let file = open_file(path_str.as_bytes(), mode).await.expect("open");
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
        let mut ops = TokioFileOps::from_std(std_file, true, true);
        ops.write_bytes(b"tmp data").await.expect("write");
        ops.seek(std::io::SeekFrom::Start(0)).await.expect("seek");
        let data = ops.read_all().await.expect("read");
        k9::assert_equal!(data.as_ref(), b"tmp data");
    }

    #[tokio::test]
    async fn tmpfile_is_seekable() {
        let std_file = tempfile::tempfile().expect("create tmp");
        let mut ops = TokioFileOps::from_std(std_file, true, true);
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
            Box::new(crate::tokio_file::TokioFileOps::from_std(
                tempfile::tempfile().expect("tmp"),
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
            Box::new(crate::tokio_file::TokioFileOps::from_std(
                tempfile::tempfile().expect("tmp"),
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
        k9::assert_equal!(check(&Value::string("hello")), false);
    }

    // =====================================================================
    // FileMode const constructors
    // =====================================================================

    #[test]
    fn file_mode_read_const() {
        let parsed = parse_mode(b"r").expect("valid");
        k9::assert_equal!(FileMode::READ, parsed);
    }

    #[test]
    fn file_mode_write_const() {
        let parsed = parse_mode(b"w").expect("valid");
        k9::assert_equal!(FileMode::WRITE, parsed);
    }

    // =====================================================================
    // bytes_to_os_str
    // =====================================================================

    #[test]
    fn bytes_to_os_str_ascii() {
        let result = bytes_to_os_str(b"hello").expect("valid");
        k9::assert_equal!(result.into_owned(), std::ffi::OsString::from("hello"));
    }

    #[test]
    fn bytes_to_os_str_empty() {
        let result = bytes_to_os_str(b"").expect("valid");
        k9::assert_equal!(result.into_owned(), std::ffi::OsString::from(""));
    }

    #[cfg(unix)]
    #[test]
    fn bytes_to_os_str_non_utf8_unix() {
        // On Unix, arbitrary bytes are valid OsStr.
        let result = bytes_to_os_str(b"\xff\xfe").expect("valid on unix");
        k9::assert_equal!(result.len(), 2);
    }

    // =====================================================================
    // bytes_to_path
    // =====================================================================

    #[test]
    fn bytes_to_path_basic() {
        let path = bytes_to_path(b"/tmp/test").expect("valid");
        k9::assert_equal!(path, std::path::PathBuf::from("/tmp/test"));
    }
}
