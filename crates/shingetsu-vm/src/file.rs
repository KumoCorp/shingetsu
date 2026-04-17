//! Lua file handle support.
//!
//! Defines the [`LuaFileOps`] trait and [`LuaFile`] userdata, providing the
//! bridge between Rust async I/O and Lua's file-object protocol (`f:read`,
//! `f:write`, `f:seek`, `f:close`, etc.).
//!
//! The concrete I/O backend (e.g. `tokio::fs::File`) lives in the `shingetsu`
//! crate; this module is runtime-agnostic.

use std::io::SeekFrom;
use std::sync::Arc;

use bytes::Bytes;
use futures::lock::Mutex;

use crate::call_context::CallContext;
use crate::convert::{IntoLuaMulti, Variadic};
use crate::error::{VmError, VmResultExt};
use crate::function::Function;
use crate::userdata::Userdata;
use crate::value::Value;

/// Output buffering mode for [`LuaFileOps::set_buffering`], corresponding
/// to the modes accepted by Lua's `file:setvbuf()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferMode {
    /// `"no"` — no buffering; every write is flushed immediately.
    No,
    /// `"full"` — buffer output until the buffer is full or
    /// [`flush`](LuaFileOps::flush) is called.  `size` is an optional
    /// hint for the buffer capacity in bytes.
    Full { size: Option<usize> },
    /// `"line"` — flush after each newline.  `size` is an optional
    /// hint for the buffer capacity in bytes.
    Line { size: Option<usize> },
}

/// Result of closing a file handle.
///
/// Regular files return [`CloseStatus::Ok`].  Process pipes return
/// [`CloseStatus::ProcessExit`] with the child's exit code, matching
/// Lua's `f:close()` contract for `io.popen` handles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseStatus {
    /// Normal file close succeeded.
    Ok,
    /// A child process exited normally.
    ProcessExit {
        /// `true` if the process exited with code 0.
        success: bool,
        /// The raw exit code.
        code: i32,
    },
    /// A child process was terminated by a signal.
    ProcessSignal {
        /// The signal number that terminated the process.
        signal: i32,
    },
}

/// Accumulator for parsing a Lua number from a byte stream.
///
/// Supports integers, floats, scientific notation (e.g. `3.14`, `-1e10`),
/// and hex literals (`0xff`, `0xDEAD`).  Feed bytes one at a time via
/// [`feed`](Self::feed); call [`finish`](Self::finish) to get the result.
///
/// Used by the default [`LuaFileOps::read_number`] implementation and by
/// buffered backends that feed bytes from their read buffer.
pub struct NumberAccumulator {
    buf: Vec<u8>,
    saw_hex_prefix: bool,
}

impl NumberAccumulator {
    /// Create a new empty accumulator.
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            saw_hex_prefix: false,
        }
    }

    /// Feed a single byte.  Returns `true` if the byte was accepted as
    /// part of the number, `false` if it is not a valid number character
    /// (the byte is *not* consumed in that case).
    pub fn feed(&mut self, b: u8) -> bool {
        // Leading sign.
        if self.buf.is_empty() && (b == b'-' || b == b'+') {
            self.buf.push(b);
            return true;
        }
        // Hex prefix: accept 'x'/'X' immediately after a leading '0'
        // (with optional sign).
        if !self.saw_hex_prefix
            && (b == b'x' || b == b'X')
            && self.buf.last() == Some(&b'0')
            && (self.buf.len() == 1
                || (self.buf.len() == 2 && (self.buf[0] == b'-' || self.buf[0] == b'+')))
        {
            self.saw_hex_prefix = true;
            self.buf.push(b);
            return true;
        }
        let is_number_char = b.is_ascii_digit()
            || b == b'.'
            || b == b'-'
            || b == b'+'
            || b == b'e'
            || b == b'E'
            || (self.saw_hex_prefix && (b.is_ascii_hexdigit() || b == b'p' || b == b'P'));
        if !is_number_char {
            return false;
        }
        self.buf.push(b);
        true
    }

    /// Feed as many bytes as possible from `data`.  Returns the number
    /// of bytes consumed (stops at the first byte that is not part of
    /// the number).
    pub fn feed_slice(&mut self, data: &[u8]) -> usize {
        let mut consumed = 0;
        for &b in data {
            if !self.feed(b) {
                break;
            }
            consumed += 1;
        }
        consumed
    }

    /// Parse the accumulated bytes and return the number, or `None` if
    /// no valid number was formed.
    pub fn finish(&self) -> Option<f64> {
        if self.buf.is_empty() {
            return None;
        }
        let s = std::str::from_utf8(&self.buf).unwrap_or("");
        // Try integer hex parse first (Lua treats 0xff as 255.0 for *n).
        if self.saw_hex_prefix && s.len() > 2 {
            // Strip the optional sign and "0x" prefix.
            let (sign, hex) = if s.starts_with('-') {
                (-1i64, &s[3..])
            } else if s.starts_with('+') {
                (1i64, &s[3..])
            } else {
                (1i64, &s[2..])
            };
            if let Ok(n) = i64::from_str_radix(hex, 16) {
                return Some((sign * n) as f64);
            }
        }
        s.parse::<f64>().ok()
    }
}

/// Async operations that a Lua file handle can perform.
///
/// Implement this trait for your I/O backend (e.g. `tokio::fs::File`,
/// an in-memory buffer, a network stream, etc.) and wrap it in a
/// [`LuaFile`] to expose it to Lua scripts.
///
/// All methods are async.  Implementations that perform synchronous I/O
/// may do so directly within the async method body.  For operations that
/// may block for significant time (disk I/O without OS-level async,
/// serial ports, etc.), consider wrapping the blocking call in
/// `tokio::task::spawn_blocking` or your runtime's equivalent to avoid
/// stalling the executor.
#[async_trait::async_trait]
pub trait LuaFileOps: Send + Sync + 'static {
    /// Read exactly `n` bytes.  Returns fewer bytes only at EOF.
    async fn read_bytes(&mut self, n: usize) -> Result<Bytes, std::io::Error>;

    /// Read a single line.  Returns `None` at EOF.
    ///
    /// When `keep_newline` is `true`, the trailing `\n` (or `\r\n`) is
    /// included in the returned bytes (Lua `"*L"` / `"L"` format).
    /// When `false`, the newline is stripped (Lua `"*l"` / `"l"` format).
    ///
    /// The default implementation reads one byte at a time via
    /// [`read_bytes`](Self::read_bytes).  Backends with buffered I/O
    /// should override this for efficiency.
    async fn read_line(&mut self, keep_newline: bool) -> Result<Option<Bytes>, std::io::Error> {
        let mut buf = Vec::new();
        loop {
            let chunk = self.read_bytes(1).await?;
            if chunk.is_empty() {
                // EOF
                return if buf.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Bytes::from(buf)))
                };
            }
            let byte = chunk[0];
            buf.push(byte);
            if byte == b'\n' {
                if !keep_newline {
                    buf.pop(); // remove \n
                               // Also strip \r if CRLF.
                    if buf.last() == Some(&b'\r') {
                        buf.pop();
                    }
                }
                return Ok(Some(Bytes::from(buf)));
            }
        }
    }

    /// Read all remaining bytes until EOF.  Returns an empty `Bytes` if
    /// already at EOF (Lua `"*a"` format returns `""`, not `nil`).
    async fn read_all(&mut self) -> Result<Bytes, std::io::Error>;

    /// Skip whitespace and parse a number from the stream, consuming
    /// the whitespace and matched bytes.  Returns `None` if no number
    /// can be parsed (including at EOF).  Bytes consumed before a
    /// failed parse are not rewound — this matches Lua's behaviour on
    /// non-seekable streams (pipes).
    ///
    /// The default implementation reads one byte at a time via
    /// [`read_bytes`](Self::read_bytes).  Backends with buffered I/O
    /// should override this to feed [`NumberAccumulator`] from their
    /// buffer directly for efficiency.
    async fn read_number(&mut self) -> Result<Option<f64>, std::io::Error> {
        // Skip whitespace.
        loop {
            let chunk = self.read_bytes(1).await?;
            if chunk.is_empty() {
                return Ok(None); // EOF
            }
            if !chunk[0].is_ascii_whitespace() {
                let mut acc = NumberAccumulator::new();
                // Feed the first non-whitespace byte.
                if !acc.feed(chunk[0]) {
                    return Ok(acc.finish());
                }
                // Feed remaining bytes one at a time.
                loop {
                    let chunk = self.read_bytes(1).await?;
                    if chunk.is_empty() {
                        break; // EOF
                    }
                    if !acc.feed(chunk[0]) {
                        // Consumed one byte past the number.  On seekable
                        // streams we could rewind; on pipes we accept the
                        // loss.  This matches Lua's behaviour.
                        break;
                    }
                }
                return Ok(acc.finish());
            }
        }
    }

    /// Write all of `data` to the file.
    ///
    /// Implementations should ensure the entire buffer is written (i.e.
    /// use `write_all`, not `write`, on the underlying I/O object) so
    /// that short writes don't silently lose data.
    async fn write_bytes(&mut self, data: &[u8]) -> Result<(), std::io::Error>;

    /// Flush any buffered output.
    async fn flush(&mut self) -> Result<(), std::io::Error>;

    /// Seek to a position.  Returns the new absolute byte offset.
    async fn seek(&mut self, pos: SeekFrom) -> Result<u64, std::io::Error>;

    /// Close the file and release resources.
    async fn close(&mut self) -> Result<CloseStatus, std::io::Error>;

    /// Set the output buffering mode.
    ///
    /// The default implementation ignores the request.  Backends that
    /// manage their own buffer (e.g. wrapping a `BufWriter`) can
    /// override this to swap or resize the buffer layer.
    async fn set_buffering(&mut self, _mode: BufferMode) -> Result<(), std::io::Error> {
        Ok(())
    }

    /// Whether this handle supports reading.
    fn can_read(&self) -> bool {
        false
    }

    /// Whether this handle supports writing.
    fn can_write(&self) -> bool {
        false
    }

    /// Whether this handle supports seeking.
    fn can_seek(&self) -> bool {
        false
    }
}

/// A Lua file handle, exposed as userdata.
///
/// Wraps a [`LuaFileOps`] implementation behind a [`futures::lock::Mutex`]
/// (whose guard is `Send`, safe to hold across `.await` points).  The
/// `Option` layer represents the open/closed state: after `f:close()`,
/// the inner is `None` and all subsequent operations return
/// `nil, "attempt to use a closed file"`.
pub struct LuaFile {
    inner: Mutex<Option<Box<dyn LuaFileOps>>>,
    /// Display name for `tostring(f)` and error messages.
    name: String,
    /// Whether `f:close()` actually closes the handle.  Stdio handles
    /// set this to `false` — `close()` on them is a no-op (the handle
    /// stays open), matching Lua 5.4 behavior.
    closeable: bool,
}

impl LuaFile {
    /// Create a new file handle wrapping the given I/O backend.
    pub fn new(name: impl Into<String>, ops: Box<dyn LuaFileOps>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Some(ops)),
            name: name.into(),
            closeable: true,
        })
    }

    /// Create a non-closeable file handle (for stdio streams).
    /// `f:close()` on this handle is a no-op.
    pub fn new_uncloseable(name: impl Into<String>, ops: Box<dyn LuaFileOps>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Some(ops)),
            name: name.into(),
            closeable: false,
        })
    }

    /// Returns `true` if the file has been closed.
    pub async fn is_closed(&self) -> bool {
        self.inner.lock().await.is_none()
    }

    /// Returns `true` if this handle can be closed by Lua code.
    /// Stdio handles return `false`.
    pub fn is_closeable(&self) -> bool {
        self.closeable
    }

    /// Lock the inner I/O backend for direct access.
    ///
    /// Returns a guard holding `Option<Box<dyn LuaFileOps>>` — `None`
    /// if the file has been closed.  Useful for embedders that need
    /// to perform I/O operations outside the Lua dispatch path.
    pub async fn lock_inner(&self) -> futures::lock::MutexGuard<'_, Option<Box<dyn LuaFileOps>>> {
        self.inner.lock().await
    }
}

/// Helper: return the standard Lua error for operations on a closed file.
fn closed_file_error() -> Vec<Value> {
    vec![Value::Nil, Value::string("attempt to use a closed file")]
}

/// Read format specifier for `f:read()`.
#[derive(Clone)]
pub enum ReadFormat {
    /// `"*l"` or `"l"` — read a line, strip the newline.
    Line,
    /// `"*L"` or `"L"` — read a line, keep the newline.
    LineWithNewline,
    /// `"*a"` or `"a"` — read all remaining bytes.
    All,
    /// `"*n"` or `"n"` — read a number.
    Number,
    /// Read exactly `n` bytes.
    Bytes(usize),
}

impl ReadFormat {
    pub fn from_value(v: &Value, function: &str) -> Result<Self, VmError> {
        match v {
            Value::String(s) => {
                let s = s.as_ref();
                match s {
                    b"*l" | b"l" => Ok(ReadFormat::Line),
                    b"*L" | b"L" => Ok(ReadFormat::LineWithNewline),
                    b"*a" | b"a" => Ok(ReadFormat::All),
                    b"*n" | b"n" => Ok(ReadFormat::Number),
                    _ => Err(VmError::BadArgument {
                        position: 0,
                        function: function.to_owned(),
                        expected: "invalid format".to_owned(),
                        got: format!("{:?}", bstr::BStr::new(s)),
                    }),
                }
            }
            Value::Integer(n) => {
                if *n < 0 {
                    return Err(VmError::BadArgument {
                        position: 0,
                        function: function.to_owned(),
                        expected: "non-negative integer".to_owned(),
                        got: format!("{}", n),
                    });
                }
                Ok(ReadFormat::Bytes(*n as usize))
            }
            Value::Float(f) => {
                let n = *f as i64;
                if n < 0 {
                    return Err(VmError::BadArgument {
                        position: 0,
                        function: function.to_owned(),
                        expected: "non-negative integer".to_owned(),
                        got: format!("{}", f),
                    });
                }
                Ok(ReadFormat::Bytes(n as usize))
            }
            other => Err(VmError::BadArgument {
                position: 0,
                function: function.to_owned(),
                expected: "string or number".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

/// Execute a single read format against the file ops.
pub async fn read_one(ops: &mut dyn LuaFileOps, fmt: &ReadFormat) -> Result<Value, std::io::Error> {
    match fmt {
        ReadFormat::Line => match ops.read_line(false).await? {
            Some(b) => Ok(Value::String(b)),
            None => Ok(Value::Nil),
        },
        ReadFormat::LineWithNewline => match ops.read_line(true).await? {
            Some(b) => Ok(Value::String(b)),
            None => Ok(Value::Nil),
        },
        ReadFormat::All => {
            // Lua's *a returns "" at EOF, not nil.
            Ok(Value::String(ops.read_all().await?))
        }
        ReadFormat::Number => match ops.read_number().await? {
            Some(n) => Ok(Value::Float(n)),
            None => Ok(Value::Nil),
        },
        ReadFormat::Bytes(n) => {
            let data = ops.read_bytes(*n).await?;
            if data.is_empty() {
                Ok(Value::Nil)
            } else {
                Ok(Value::String(data))
            }
        }
    }
}

// =========================================================================
// Public helpers for io.read / io.write (called from io_lib)
// =========================================================================

/// Execute `f:read(...)` logic on a raw `LuaFileOps`.  The `args` slice
/// contains the format arguments (no self).  Arg positions in errors are
/// 1-based.
pub async fn lua_file_read(ops: &mut dyn LuaFileOps, args: &[Value]) -> Result<Variadic, VmError> {
    if args.is_empty() {
        let val = read_one(ops, &ReadFormat::Line)
            .await
            .map_err(|e| io_err_to_vm("read", e))?;
        return Ok(Variadic(vec![val]));
    }
    let mut results = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        let fmt = ReadFormat::from_value(arg, "read").with_arg_position(i + 1)?;
        let val = read_one(ops, &fmt)
            .await
            .map_err(|e| io_err_to_vm("read", e))?;
        results.push(val);
    }
    Ok(Variadic(results))
}

/// Execute `f:write(...)` logic on a raw `LuaFileOps`.  The `args` slice
/// contains the values to write (no self).  On success returns the
/// `handle` wrapped in a `Variadic` for chaining.
pub async fn lua_file_write(
    ops: &mut dyn LuaFileOps,
    args: &[Value],
    handle: &Arc<LuaFile>,
) -> Result<Variadic, VmError> {
    for (i, arg) in args.iter().enumerate() {
        let data = match arg {
            Value::String(s) => s.clone(),
            Value::Integer(n) => Bytes::from(n.to_string()),
            Value::Float(f) => Bytes::from(f.to_string()),
            other => {
                return Err(VmError::BadArgument {
                    position: i + 1,
                    function: "write".to_owned(),
                    expected: "string or number".to_owned(),
                    got: other.type_name().to_owned(),
                });
            }
        };
        ops.write_bytes(&data)
            .await
            .map_err(|e| io_err_to_vm("write", e))?;
    }
    Ok(Variadic(vec![Value::Userdata(
        Arc::clone(handle) as Arc<dyn Userdata>
    )]))
}

#[shingetsu_derive::userdata(crate = "crate", index_fallback = "nil")]
impl LuaFile {
    fn type_name(&self) -> &'static str {
        "file"
    }

    /// Best-effort close for `__gc` and `__close` metamethods.
    async fn gc_close(&self) -> Result<Variadic, VmError> {
        let mut guard = self.inner.lock().await;
        if let Some(ops) = guard.as_mut() {
            // Best-effort close; ignore errors during GC.
            let _ = ops.close().await;
            *guard = None;
        }
        Ok(Variadic(vec![]))
    }

    #[lua_method(rename = "read")]
    async fn lua_read(
        self: Arc<Self>,
        ctx: CallContext,
        args: Variadic,
    ) -> Result<Variadic, VmError> {
        let mut guard = self.inner.lock().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(Variadic(closed_file_error()));
        };
        if !ops.can_read() {
            return Err(io_err_to_vm(
                "read",
                std::io::Error::new(std::io::ErrorKind::Unsupported, "not open for reading"),
            ));
        }
        // Default format is "*l" when called with no arguments.
        if args.0.is_empty() {
            let val = read_one(ops.as_mut(), &ReadFormat::Line)
                .await
                .map_err(|e| io_err_to_vm("read", e))?;
            return Ok(Variadic(vec![val]));
        }
        let mut results = Vec::with_capacity(args.0.len());
        for (i, arg) in args.0.iter().enumerate() {
            let fmt = ReadFormat::from_value(arg, "read").with_call_context(i + 2, &ctx)?; // +2: arg 1 is self
            let val = read_one(ops.as_mut(), &fmt)
                .await
                .map_err(|e| io_err_to_vm("read", e))?;
            results.push(val);
        }
        Ok(Variadic(results))
    }

    #[lua_method(rename = "write")]
    async fn lua_write(self: Arc<Self>, args: Variadic) -> Result<Variadic, VmError> {
        let mut guard = self.inner.lock().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(Variadic(closed_file_error()));
        };
        if !ops.can_write() {
            return Err(io_err_to_vm(
                "write",
                std::io::Error::new(std::io::ErrorKind::Unsupported, "not open for writing"),
            ));
        }
        for (i, arg) in args.0.iter().enumerate() {
            let data = match arg {
                Value::String(s) => s.clone(),
                Value::Integer(n) => Bytes::from(n.to_string()),
                Value::Float(f) => Bytes::from(f.to_string()),
                other => {
                    return Err(VmError::BadArgument {
                        position: i + 2, // +2: arg 1 is self
                        function: "write".to_owned(),
                        expected: "string or number".to_owned(),
                        got: other.type_name().to_owned(),
                    });
                }
            };
            ops.write_bytes(&data)
                .await
                .map_err(|e| io_err_to_vm("write", e))?;
        }
        // Return the file handle for chaining: f:write("a"):write("b")
        drop(guard);
        Ok(Variadic(vec![Value::Userdata(self)]))
    }

    #[lua_method(rename = "close")]
    async fn lua_close(self: Arc<Self>) -> Result<Variadic, VmError> {
        if !self.closeable {
            // Stdio handles: close is a no-op, handle stays open.
            return Ok(Variadic(CloseStatus::Ok.into_lua_multi()));
        }
        let mut guard = self.inner.lock().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(Variadic(closed_file_error()));
        };
        let status = ops.close().await.map_err(|e| io_err_to_vm("close", e))?;
        *guard = None;
        Ok(Variadic(status.into_lua_multi()))
    }

    #[lua_method(rename = "flush")]
    async fn lua_flush(self: Arc<Self>) -> Result<Variadic, VmError> {
        let mut guard = self.inner.lock().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(Variadic(closed_file_error()));
        };
        ops.flush().await.map_err(|e| io_err_to_vm("flush", e))?;
        // Return the file handle for chaining.
        drop(guard);
        Ok(Variadic(vec![Value::Userdata(self)]))
    }

    #[lua_method(rename = "seek")]
    async fn lua_seek(self: Arc<Self>, args: Variadic) -> Result<Variadic, VmError> {
        let mut guard = self.inner.lock().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(Variadic(closed_file_error()));
        };
        // Defaults: whence = "cur", offset = 0
        let whence_str = match args.0.first() {
            Some(Value::String(s)) => s.as_ref(),
            Some(other) => {
                return Err(VmError::BadArgument {
                    position: 2,
                    function: "seek".to_owned(),
                    expected: "string".to_owned(),
                    got: other.type_name().to_owned(),
                });
            }
            None => b"cur" as &[u8],
        };
        let offset = match args.0.get(1) {
            Some(Value::Integer(n)) => *n,
            Some(Value::Float(f)) => *f as i64,
            Some(other) => {
                return Err(VmError::BadArgument {
                    position: 3,
                    function: "seek".to_owned(),
                    expected: "number".to_owned(),
                    got: other.type_name().to_owned(),
                });
            }
            None => 0,
        };
        let pos = match whence_str {
            b"set" => SeekFrom::Start(offset as u64),
            b"cur" => SeekFrom::Current(offset),
            b"end" => SeekFrom::End(offset),
            _ => {
                return Err(VmError::BadArgument {
                    position: 2,
                    function: "seek".to_owned(),
                    expected: "'set', 'cur', or 'end'".to_owned(),
                    got: format!("{:?}", bstr::BStr::new(whence_str)),
                });
            }
        };
        let new_pos = ops.seek(pos).await.map_err(|e| io_err_to_vm("seek", e))?;
        Ok(Variadic(vec![Value::Integer(new_pos as i64)]))
    }

    #[lua_method(rename = "lines")]
    async fn lua_lines(
        self: Arc<Self>,
        ctx: CallContext,
        args: Variadic,
    ) -> Result<Variadic, VmError> {
        // Check file is open.
        {
            let guard = self.inner.lock().await;
            if guard.is_none() {
                return Ok(Variadic(closed_file_error()));
            }
        }
        // Parse format args now; default is "*l".
        let formats: Vec<ReadFormat> = if args.0.is_empty() {
            vec![ReadFormat::Line]
        } else {
            args.0
                .iter()
                .enumerate()
                .map(|(i, v)| ReadFormat::from_value(v, "lines").with_call_context(i + 2, &ctx))
                .collect::<Result<_, _>>()?
        };
        let file = self;
        let iter_fn = Function::wrap("file:lines iterator", move || {
            let file = Arc::clone(&file);
            let formats = formats.clone();
            async move {
                let mut guard = file.inner.lock().await;
                let Some(ops) = guard.as_mut() else {
                    return Ok(Variadic(vec![Value::Nil]));
                };
                let mut results = Vec::with_capacity(formats.len());
                for fmt in &formats {
                    let val = read_one(ops.as_mut(), fmt)
                        .await
                        .map_err(|e| io_err_to_vm("lines iterator", e))?;
                    results.push(val);
                }
                // generic-for terminates when the first
                // value is nil.
                Ok(Variadic(results))
            }
        });
        Ok(Variadic(vec![Value::Function(iter_fn)]))
    }

    #[lua_method(rename = "setvbuf")]
    async fn lua_setvbuf(
        self: Arc<Self>,
        ctx: CallContext,
        args: Variadic,
    ) -> Result<Variadic, VmError> {
        let mut guard = self.inner.lock().await;
        let Some(ops) = guard.as_mut() else {
            return Ok(Variadic(closed_file_error()));
        };
        let mode_str = match args.0.first() {
            Some(Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(VmError::BadArgument {
                    position: 2,
                    function: ctx
                        .native_name
                        .as_ref()
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                        .unwrap_or_default(),
                    expected: "string".to_owned(),
                    got: other.type_name().to_owned(),
                });
            }
            None => {
                return Err(VmError::BadArgument {
                    position: 2,
                    function: ctx
                        .native_name
                        .as_ref()
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                        .unwrap_or_default(),
                    expected: "string".to_owned(),
                    got: "no value".to_owned(),
                });
            }
        };
        let size = match args.0.get(1) {
            Some(Value::Integer(n)) => Some(*n as usize),
            Some(Value::Float(f)) => Some(*f as usize),
            Some(_) | None => None,
        };
        let mode = match mode_str.as_ref() {
            b"no" => BufferMode::No,
            b"full" => BufferMode::Full { size },
            b"line" => BufferMode::Line { size },
            _ => {
                return Err(VmError::BadArgument {
                    position: 2,
                    function: ctx
                        .native_name
                        .as_ref()
                        .map(|n| String::from_utf8_lossy(n).into_owned())
                        .unwrap_or_default(),
                    expected: "'no', 'full', or 'line'".to_owned(),
                    got: format!("{:?}", bstr::BStr::new(&mode_str)),
                });
            }
        };
        ops.set_buffering(mode)
            .await
            .map_err(|e| io_err_to_vm("setvbuf", e))?;
        drop(guard);
        Ok(Variadic(vec![Value::Userdata(self)]))
    }

    #[lua_metamethod(ToString)]
    async fn lua_tostring(self: Arc<Self>) -> Result<Variadic, VmError> {
        let guard = self.inner.lock().await;
        if guard.is_some() {
            Ok(Variadic(vec![Value::string(format!(
                "file ({})",
                self.name
            ))]))
        } else {
            Ok(Variadic(vec![Value::string("file (closed)")]))
        }
    }

    #[lua_metamethod(Gc)]
    async fn lua_gc(self: Arc<Self>) -> Result<Variadic, VmError> {
        self.gc_close().await
    }

    #[lua_metamethod(Close)]
    async fn lua_close_meta(self: Arc<Self>) -> Result<Variadic, VmError> {
        self.gc_close().await
    }
}

impl crate::convert::IntoLuaMulti for CloseStatus {
    fn into_lua_multi(self) -> Vec<Value> {
        match self {
            CloseStatus::Ok => vec![Value::Boolean(true)],
            CloseStatus::ProcessExit { success, code } => {
                vec![
                    if success {
                        Value::Boolean(true)
                    } else {
                        Value::Nil
                    },
                    Value::string("exit"),
                    Value::Integer(code as i64),
                ]
            }
            CloseStatus::ProcessSignal { signal } => {
                vec![
                    Value::Nil,
                    Value::string("signal"),
                    Value::Integer(signal as i64),
                ]
            }
        }
    }
}

impl crate::convert::LuaTypedMulti for CloseStatus {
    fn lua_types() -> Vec<crate::types::LuaType> {
        use crate::types::LuaType;
        // true | (boolean?, string, integer)
        vec![LuaType::Union(vec![
            LuaType::Boolean,
            LuaType::Tuple(vec![
                LuaType::Optional(Box::new(LuaType::Boolean)),
                LuaType::String,
                LuaType::Integer,
            ]),
        ])]
    }
}

/// Convert an `io::Error` into a `VmError::HostError`.
///
/// For errors originating from the OS (with a raw OS error code), uses
/// [`portable_io_error_description`](crate::error::portable_io_error_description)
/// to produce a platform-stable message.  Errors constructed via
/// `io::Error::new()` (no raw OS error) keep their original message.
pub fn io_err_to_vm(method: &str, e: std::io::Error) -> VmError {
    let msg = if e.raw_os_error().is_some() {
        crate::error::portable_io_error_description(&e)
    } else {
        e.to_string()
    };
    VmError::HostError {
        name: format!("file:{}", method),
        source: msg.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_context::CallContext;
    use crate::userdata::Userdata;

    // =================================================================
    // NumberAccumulator
    // =================================================================

    #[test]
    fn number_acc_integer() {
        let mut acc = NumberAccumulator::new();
        assert!(acc.feed(b'4'));
        assert!(acc.feed(b'2'));
        assert!(!acc.feed(b' '));
        k9::assert_equal!(acc.finish(), Some(42.0));
    }

    #[test]
    fn number_acc_float() {
        let mut acc = NumberAccumulator::new();
        k9::assert_equal!(acc.feed_slice(b"3.14"), 4);
        k9::assert_equal!(acc.finish(), Some(3.14));
    }

    #[test]
    fn number_acc_scientific() {
        let mut acc = NumberAccumulator::new();
        k9::assert_equal!(acc.feed_slice(b"1.5e2xyz"), 5);
        k9::assert_equal!(acc.finish(), Some(150.0));
    }

    #[test]
    fn number_acc_negative() {
        let mut acc = NumberAccumulator::new();
        k9::assert_equal!(acc.feed_slice(b"-30 "), 3);
        k9::assert_equal!(acc.finish(), Some(-30.0));
    }

    #[test]
    fn number_acc_hex() {
        let mut acc = NumberAccumulator::new();
        k9::assert_equal!(acc.feed_slice(b"0xff!"), 4);
        k9::assert_equal!(acc.finish(), Some(255.0));
    }

    #[test]
    fn number_acc_hex_with_sign() {
        let mut acc = NumberAccumulator::new();
        k9::assert_equal!(acc.feed_slice(b"-0xA "), 4);
        k9::assert_equal!(acc.finish(), Some(-10.0));
    }

    #[test]
    fn number_acc_empty() {
        let acc = NumberAccumulator::new();
        k9::assert_equal!(acc.finish(), None);
    }

    #[test]
    fn number_acc_non_number() {
        let mut acc = NumberAccumulator::new();
        assert!(!acc.feed(b'x'));
        k9::assert_equal!(acc.finish(), None);
    }

    #[test]
    fn number_acc_feed_slice_partial() {
        let mut acc = NumberAccumulator::new();
        // First chunk ends mid-number.
        k9::assert_equal!(acc.feed_slice(b"12"), 2);
        // Second chunk has the rest plus a terminator.
        k9::assert_equal!(acc.feed_slice(b"34 rest"), 2);
        k9::assert_equal!(acc.finish(), Some(1234.0));
    }

    /// Minimal in-memory file backend for testing.
    struct MemFile {
        data: Vec<u8>,
        pos: usize,
        closed: bool,
    }

    impl MemFile {
        fn new(data: impl Into<Vec<u8>>) -> Self {
            Self {
                data: data.into(),
                pos: 0,
                closed: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl LuaFileOps for MemFile {
        async fn read_bytes(&mut self, n: usize) -> Result<Bytes, std::io::Error> {
            let end = (self.pos + n).min(self.data.len());
            let chunk = Bytes::copy_from_slice(&self.data[self.pos..end]);
            self.pos = end;
            Ok(chunk)
        }

        async fn read_all(&mut self) -> Result<Bytes, std::io::Error> {
            let rest = Bytes::copy_from_slice(&self.data[self.pos..]);
            self.pos = self.data.len();
            Ok(rest)
        }

        async fn write_bytes(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
            // Extend or overwrite at current position.
            let end = self.pos + data.len();
            if end > self.data.len() {
                self.data.resize(end, 0);
            }
            self.data[self.pos..end].copy_from_slice(data);
            self.pos = end;
            Ok(())
        }

        async fn flush(&mut self) -> Result<(), std::io::Error> {
            Ok(())
        }

        async fn seek(&mut self, pos: SeekFrom) -> Result<u64, std::io::Error> {
            let new_pos = match pos {
                SeekFrom::Start(n) => n as i64,
                SeekFrom::Current(n) => self.pos as i64 + n,
                SeekFrom::End(n) => self.data.len() as i64 + n,
            };
            if new_pos < 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "seek before start of file",
                ));
            }
            self.pos = new_pos as usize;
            Ok(self.pos as u64)
        }

        async fn close(&mut self) -> Result<CloseStatus, std::io::Error> {
            self.closed = true;
            Ok(CloseStatus::Ok)
        }

        fn can_read(&self) -> bool {
            true
        }

        fn can_write(&self) -> bool {
            true
        }

        fn can_seek(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn read_bytes_basic() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"hello world")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");
        let chunk = ops.read_bytes(5).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn read_bytes_at_eof() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"hi")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");
        let chunk = ops.read_bytes(100).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hi");
    }

    #[tokio::test]
    async fn read_line_strips_newline() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"line1\nline2\n")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"line1");

        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"line2");

        let eof = ops.read_line(false).await.expect("read");
        k9::assert_equal!(eof, None);
    }

    #[tokio::test]
    async fn read_line_keeps_newline() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abc\ndef\n")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let line = ops.read_line(true).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"abc\n");

        let line = ops.read_line(true).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"def\n");
    }

    #[tokio::test]
    async fn read_line_strips_crlf() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"dos\r\nline\r\n")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"dos");
    }

    #[tokio::test]
    async fn read_line_no_trailing_newline() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"last")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"last");

        let eof = ops.read_line(false).await.expect("read");
        k9::assert_equal!(eof, None);
    }

    #[tokio::test]
    async fn read_all_basic() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"everything")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");
        let all = ops.read_all().await.expect("read");
        k9::assert_equal!(all.as_ref(), b"everything");
    }

    #[tokio::test]
    async fn read_all_at_eof_returns_empty() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");
        let all = ops.read_all().await.expect("read");
        k9::assert_equal!(all.as_ref(), b"");
    }

    #[tokio::test]
    async fn read_number_basic() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"  42.5  99")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 42.5);

        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 99.0);
    }

    #[tokio::test]
    async fn read_number_at_eof() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"   ")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");
        let n = ops.read_number().await.expect("read");
        k9::assert_equal!(n, None);
    }

    #[tokio::test]
    async fn read_number_hex() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"0xff 0xDEAD")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 255.0);

        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 57005.0); // 0xDEAD
    }

    #[tokio::test]
    async fn read_number_scientific() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"1.5e2 -3E1")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 150.0);

        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, -30.0);
    }

    #[tokio::test]
    async fn write_and_seek() {
        let file = LuaFile::new("test", Box::new(MemFile::new(Vec::new())));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        ops.write_bytes(b"hello").await.expect("write");
        let pos = ops.seek(SeekFrom::Start(0)).await.expect("seek");
        k9::assert_equal!(pos, 0);

        let all = ops.read_all().await.expect("read");
        k9::assert_equal!(all.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn seek_from_end() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abcdef")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let pos = ops.seek(SeekFrom::End(-2)).await.expect("seek");
        k9::assert_equal!(pos, 4);

        let rest = ops.read_all().await.expect("read");
        k9::assert_equal!(rest.as_ref(), b"ef");
    }

    #[tokio::test]
    async fn seek_before_start_errors() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abc")));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");

        let err = ops.seek(SeekFrom::Start(0)).await.expect("seek to start");
        k9::assert_equal!(err, 0);

        let err = ops.seek(SeekFrom::Current(-1)).await.unwrap_err();
        k9::assert_equal!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn close_sets_closed_state() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        {
            let mut inner = file.inner.lock().await;
            let ops = inner.as_mut().expect("not closed");
            let status = ops.close().await.expect("close");
            k9::assert_equal!(status, CloseStatus::Ok);
            *inner = None;
        }
        k9::assert_equal!(file.is_closed().await, true);
    }

    #[tokio::test]
    async fn capability_queries() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let inner = file.inner.lock().await;
        let ops = inner.as_ref().expect("not closed");
        k9::assert_equal!(ops.can_read(), true);
        k9::assert_equal!(ops.can_write(), true);
        k9::assert_equal!(ops.can_seek(), true);
    }

    /// Verify that a type with default capability methods reports all false.
    #[tokio::test]
    async fn default_capabilities_are_false() {
        struct Minimal;

        #[async_trait::async_trait]
        impl LuaFileOps for Minimal {
            async fn read_bytes(&mut self, _n: usize) -> Result<Bytes, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not supported",
                ))
            }
            async fn read_line(
                &mut self,
                _keep_newline: bool,
            ) -> Result<Option<Bytes>, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not supported",
                ))
            }
            async fn read_all(&mut self) -> Result<Bytes, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not supported",
                ))
            }
            async fn read_number(&mut self) -> Result<Option<f64>, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not supported",
                ))
            }
            async fn write_bytes(&mut self, _data: &[u8]) -> Result<(), std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not supported",
                ))
            }
            async fn flush(&mut self) -> Result<(), std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not supported",
                ))
            }
            async fn seek(&mut self, _pos: SeekFrom) -> Result<u64, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not supported",
                ))
            }
            async fn close(&mut self) -> Result<CloseStatus, std::io::Error> {
                Ok(CloseStatus::Ok)
            }
        }

        let file = LuaFile::new("minimal", Box::new(Minimal));
        let inner = file.inner.lock().await;
        let ops = inner.as_ref().expect("not closed");
        k9::assert_equal!(ops.can_read(), false);
        k9::assert_equal!(ops.can_write(), false);
        k9::assert_equal!(ops.can_seek(), false);
    }

    #[tokio::test]
    async fn lua_file_type_name() {
        let file = LuaFile::new("test.txt", Box::new(MemFile::new(b"")));
        k9::assert_equal!(file.type_name(), "file");
    }

    #[tokio::test]
    async fn close_status_process_exit() {
        struct FakeProcess;

        #[async_trait::async_trait]
        impl LuaFileOps for FakeProcess {
            async fn read_bytes(&mut self, _n: usize) -> Result<Bytes, std::io::Error> {
                Ok(Bytes::new())
            }
            async fn read_line(
                &mut self,
                _keep_newline: bool,
            ) -> Result<Option<Bytes>, std::io::Error> {
                Ok(None)
            }
            async fn read_all(&mut self) -> Result<Bytes, std::io::Error> {
                Ok(Bytes::new())
            }
            async fn read_number(&mut self) -> Result<Option<f64>, std::io::Error> {
                Ok(None)
            }
            async fn write_bytes(&mut self, _data: &[u8]) -> Result<(), std::io::Error> {
                Ok(())
            }
            async fn flush(&mut self) -> Result<(), std::io::Error> {
                Ok(())
            }
            async fn seek(&mut self, _pos: SeekFrom) -> Result<u64, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "pipes are not seekable",
                ))
            }
            async fn close(&mut self) -> Result<CloseStatus, std::io::Error> {
                Ok(CloseStatus::ProcessExit {
                    success: false,
                    code: 1,
                })
            }
            fn can_read(&self) -> bool {
                true
            }
        }

        let file = LuaFile::new("proc", Box::new(FakeProcess));
        let mut inner = file.inner.lock().await;
        let ops = inner.as_mut().expect("not closed");
        let status = ops.close().await.expect("close");
        k9::assert_equal!(
            status,
            CloseStatus::ProcessExit {
                success: false,
                code: 1
            }
        );
    }

    // =====================================================================
    // Dispatch / method tests
    // =====================================================================

    /// Helper: get a file method by name via __index dispatch.
    fn get_method(file: &Arc<LuaFile>, name: &str) -> Function {
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: None,
        };
        let result = futures::executor::block_on(Arc::clone(file).dispatch(
            ctx,
            "__index",
            vec![file_as_value(file), Value::string(name.to_owned())],
        ))
        .expect("dispatch __index");
        match &result[0] {
            Value::Function(f) => f.clone(),
            other => panic!("expected Function for {:?}, got {:?}", name, other),
        }
    }

    /// Helper: call a file method function with the given args.
    fn call_method(method: &Function, args: Vec<Value>) -> Result<Vec<Value>, VmError> {
        let n = match &*method.0 {
            crate::function::FunctionState::Native(n) => n,
            _ => panic!("expected native function"),
        };
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: Some(n.signature.name.clone()),
        };
        futures::executor::block_on((n.call)(ctx, args))
    }

    fn file_as_value(file: &Arc<LuaFile>) -> Value {
        Value::Userdata(Arc::clone(file) as Arc<dyn Userdata + Send + Sync>)
    }

    #[test]
    fn method_read_default_line() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"hello\nworld\n")));
        let read = get_method(&file, "read");

        let result = call_method(&read, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(result, vec![Value::string("hello")]);

        let result = call_method(&read, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(result, vec![Value::string("world")]);

        let result = call_method(&read, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(result, vec![Value::Nil]);
    }

    #[test]
    fn method_read_all() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"all data")));
        let read = get_method(&file, "read");
        let result = call_method(&read, vec![file_as_value(&file), Value::string("*a")]).unwrap();
        k9::assert_equal!(result, vec![Value::string("all data")]);
    }

    #[test]
    fn method_read_bytes() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abcdef")));
        let read = get_method(&file, "read");
        let result = call_method(&read, vec![file_as_value(&file), Value::Integer(3)]).unwrap();
        k9::assert_equal!(result, vec![Value::string("abc")]);
    }

    #[test]
    fn method_read_number_format() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"  42.5")));
        let read = get_method(&file, "read");
        let result = call_method(&read, vec![file_as_value(&file), Value::string("*n")]).unwrap();
        k9::assert_equal!(result, vec![Value::Float(42.5)]);
    }

    #[test]
    fn method_read_multiple_formats() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"line\nrest")));
        let read = get_method(&file, "read");
        let result = call_method(
            &read,
            vec![
                file_as_value(&file),
                Value::string("*l"),
                Value::string("*a"),
            ],
        )
        .unwrap();
        k9::assert_equal!(result, vec![Value::string("line"), Value::string("rest"),]);
    }

    #[test]
    fn method_write_and_read_back() {
        let file = LuaFile::new("test", Box::new(MemFile::new(Vec::new())));
        let write = get_method(&file, "write");
        let seek = get_method(&file, "seek");
        let read = get_method(&file, "read");

        // Write returns the file handle for chaining.
        let result =
            call_method(&write, vec![file_as_value(&file), Value::string("hello")]).unwrap();
        k9::assert_equal!(result.len(), 1);
        assert!(matches!(result[0], Value::Userdata(_)));

        // Seek back to start.
        let result = call_method(
            &seek,
            vec![
                file_as_value(&file),
                Value::string("set"),
                Value::Integer(0),
            ],
        )
        .unwrap();
        k9::assert_equal!(result, vec![Value::Integer(0)]);

        // Read it back.
        let result = call_method(&read, vec![file_as_value(&file), Value::string("*a")]).unwrap();
        k9::assert_equal!(result, vec![Value::string("hello")]);
    }

    #[test]
    fn method_write_number() {
        let file = LuaFile::new("test", Box::new(MemFile::new(Vec::new())));
        let write = get_method(&file, "write");
        let seek = get_method(&file, "seek");
        let read = get_method(&file, "read");

        call_method(&write, vec![file_as_value(&file), Value::Integer(42)]).unwrap();

        call_method(
            &seek,
            vec![
                file_as_value(&file),
                Value::string("set"),
                Value::Integer(0),
            ],
        )
        .unwrap();

        let result = call_method(&read, vec![file_as_value(&file), Value::string("*a")]).unwrap();
        k9::assert_equal!(result, vec![Value::string("42")]);
    }

    #[test]
    fn method_close() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let close = get_method(&file, "close");

        let result = call_method(&close, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(result, vec![Value::Boolean(true)]);

        // Second close returns closed-file error.
        let result = call_method(&close, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("attempt to use a closed file"),]
        );
    }

    #[test]
    fn method_flush() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let flush = get_method(&file, "flush");

        let result = call_method(&flush, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(result.len(), 1);
        assert!(matches!(result[0], Value::Userdata(_)));
    }

    #[test]
    fn method_seek_defaults() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abcdef")));
        let seek = get_method(&file, "seek");
        let read = get_method(&file, "read");

        // Read 3 bytes to advance position.
        call_method(&read, vec![file_as_value(&file), Value::Integer(3)]).unwrap();

        // seek() with no args defaults to ("cur", 0) — returns current pos.
        let result = call_method(&seek, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(3)]);
    }

    #[test]
    fn method_lines_default() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"a\nb\nc\n")));
        let lines = get_method(&file, "lines");

        // lines() returns an iterator function.
        let result = call_method(&lines, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(result.len(), 1);
        let iter_fn = match &result[0] {
            Value::Function(f) => f.clone(),
            other => panic!("expected function, got {:?}", other),
        };

        // Call the iterator repeatedly.
        let r = call_method(&iter_fn, vec![]).unwrap();
        k9::assert_equal!(r, vec![Value::string("a")]);

        let r = call_method(&iter_fn, vec![]).unwrap();
        k9::assert_equal!(r, vec![Value::string("b")]);

        let r = call_method(&iter_fn, vec![]).unwrap();
        k9::assert_equal!(r, vec![Value::string("c")]);

        // EOF — nil terminates the for loop.
        let r = call_method(&iter_fn, vec![]).unwrap();
        k9::assert_equal!(r, vec![Value::Nil]);
    }

    #[test]
    fn method_read_on_closed_file() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"data")));
        let close = get_method(&file, "close");
        let read = get_method(&file, "read");

        call_method(&close, vec![file_as_value(&file)]).unwrap();

        let result = call_method(&read, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("attempt to use a closed file"),]
        );
    }

    #[tokio::test]
    async fn dispatch_tostring() {
        let file = LuaFile::new("test.txt", Box::new(MemFile::new(b"")));
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: None,
        };
        let result = Arc::clone(&file)
            .dispatch(ctx.clone(), "__tostring", vec![])
            .await
            .unwrap();
        k9::assert_equal!(result, vec![Value::string("file (test.txt)")]);

        // Close and check again.
        {
            let mut guard = file.inner.lock().await;
            if let Some(ops) = guard.as_mut() {
                ops.close().await.unwrap();
            }
            *guard = None;
        }
        let result = Arc::clone(&file)
            .dispatch(ctx, "__tostring", vec![])
            .await
            .unwrap();
        k9::assert_equal!(result, vec![Value::string("file (closed)")]);
    }

    #[tokio::test]
    async fn dispatch_gc_closes_file() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: None,
        };
        k9::assert_equal!(file.is_closed().await, false);
        Arc::clone(&file)
            .dispatch(ctx, "__gc", vec![])
            .await
            .unwrap();
        k9::assert_equal!(file.is_closed().await, true);
    }

    #[tokio::test]
    async fn dispatch_index_returns_method() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: None,
        };
        let result = Arc::clone(&file)
            .dispatch(
                ctx.clone(),
                "__index",
                vec![file_as_value(&file), Value::string("read")],
            )
            .await
            .unwrap();
        k9::assert_equal!(result.len(), 1);
        assert!(matches!(result[0], Value::Function(_)));

        // Unknown key returns nil.
        let result = Arc::clone(&file)
            .dispatch(
                ctx,
                "__index",
                vec![file_as_value(&file), Value::string("nonexistent")],
            )
            .await
            .unwrap();
        k9::assert_equal!(result, vec![Value::Nil]);
    }

    #[test]
    fn method_setvbuf_full() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let setvbuf = get_method(&file, "setvbuf");
        let result = call_method(
            &setvbuf,
            vec![
                file_as_value(&file),
                Value::string("full"),
                Value::Integer(4096),
            ],
        )
        .unwrap();
        k9::assert_equal!(result.len(), 1);
        assert!(matches!(result[0], Value::Userdata(_)));
    }

    #[test]
    fn method_setvbuf_no() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let setvbuf = get_method(&file, "setvbuf");
        let result =
            call_method(&setvbuf, vec![file_as_value(&file), Value::string("no")]).unwrap();
        k9::assert_equal!(result.len(), 1);
        assert!(matches!(result[0], Value::Userdata(_)));
    }

    #[test]
    fn method_setvbuf_line() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let setvbuf = get_method(&file, "setvbuf");
        let result =
            call_method(&setvbuf, vec![file_as_value(&file), Value::string("line")]).unwrap();
        k9::assert_equal!(result.len(), 1);
        assert!(matches!(result[0], Value::Userdata(_)));
    }

    #[test]
    fn method_setvbuf_invalid_mode() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let setvbuf = get_method(&file, "setvbuf");
        let err =
            call_method(&setvbuf, vec![file_as_value(&file), Value::string("bogus")]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'setvbuf' ('no', 'full', or 'line' expected, got \"bogus\")"
        );
    }

    #[test]
    fn method_setvbuf_missing_mode() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let setvbuf = get_method(&file, "setvbuf");
        let err = call_method(&setvbuf, vec![file_as_value(&file)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'setvbuf' (string expected, got no value)"
        );
    }

    // =================================================================
    // Closed-file error paths for every method
    // =================================================================

    #[test]
    fn method_write_on_closed_file() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let close = get_method(&file, "close");
        let write = get_method(&file, "write");
        call_method(&close, vec![file_as_value(&file)]).unwrap();

        let result =
            call_method(&write, vec![file_as_value(&file), Value::string("hello")]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("attempt to use a closed file"),]
        );
    }

    #[test]
    fn method_flush_on_closed_file() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let close = get_method(&file, "close");
        let flush = get_method(&file, "flush");
        call_method(&close, vec![file_as_value(&file)]).unwrap();

        let result = call_method(&flush, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("attempt to use a closed file"),]
        );
    }

    #[test]
    fn method_seek_on_closed_file() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let close = get_method(&file, "close");
        let seek = get_method(&file, "seek");
        call_method(&close, vec![file_as_value(&file)]).unwrap();

        let result = call_method(&seek, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("attempt to use a closed file"),]
        );
    }

    #[test]
    fn method_lines_on_closed_file() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let close = get_method(&file, "close");
        let lines = get_method(&file, "lines");
        call_method(&close, vec![file_as_value(&file)]).unwrap();

        let result = call_method(&lines, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("attempt to use a closed file"),]
        );
    }

    #[test]
    fn method_setvbuf_on_closed_file() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let close = get_method(&file, "close");
        let setvbuf = get_method(&file, "setvbuf");
        call_method(&close, vec![file_as_value(&file)]).unwrap();

        let result =
            call_method(&setvbuf, vec![file_as_value(&file), Value::string("full")]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("attempt to use a closed file"),]
        );
    }

    // =================================================================
    // __close metamethod
    // =================================================================

    #[tokio::test]
    async fn dispatch_close_metamethod() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: None,
        };
        k9::assert_equal!(file.is_closed().await, false);
        Arc::clone(&file)
            .dispatch(ctx, "__close", vec![])
            .await
            .unwrap();
        k9::assert_equal!(file.is_closed().await, true);
    }

    // =================================================================
    // Unknown metamethod
    // =================================================================

    #[tokio::test]
    async fn dispatch_unknown_metamethod() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"")));
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: None,
        };
        let err = Arc::clone(&file)
            .dispatch(ctx, "__add", vec![file_as_value(&file)])
            .await
            .unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "error in 'file:__add': metamethod '__add' not implemented for 'file'"
        );
    }

    // =================================================================
    // write argument errors
    // =================================================================

    #[test]
    fn method_write_bad_arg_type() {
        let file = LuaFile::new("test", Box::new(MemFile::new(Vec::new())));
        let write = get_method(&file, "write");
        let err =
            call_method(&write, vec![file_as_value(&file), Value::Boolean(true)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'write' (string or number expected, got boolean)"
        );
    }

    #[test]
    fn method_write_float() {
        let file = LuaFile::new("test", Box::new(MemFile::new(Vec::new())));
        let write = get_method(&file, "write");
        let seek = get_method(&file, "seek");
        let read = get_method(&file, "read");

        call_method(&write, vec![file_as_value(&file), Value::Float(3.14)]).unwrap();
        call_method(
            &seek,
            vec![
                file_as_value(&file),
                Value::string("set"),
                Value::Integer(0),
            ],
        )
        .unwrap();
        let result = call_method(&read, vec![file_as_value(&file), Value::string("*a")]).unwrap();
        k9::assert_equal!(result, vec![Value::string("3.14")]);
    }

    // =================================================================
    // seek argument errors
    // =================================================================

    #[test]
    fn method_seek_bad_whence_type() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abc")));
        let seek = get_method(&file, "seek");
        let err = call_method(&seek, vec![file_as_value(&file), Value::Integer(0)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'seek' (string expected, got number)"
        );
    }

    #[test]
    fn method_seek_bad_whence_value() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abc")));
        let seek = get_method(&file, "seek");
        let err =
            call_method(&seek, vec![file_as_value(&file), Value::string("bogus")]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'seek' ('set', 'cur', or 'end' expected, got \"bogus\")"
        );
    }

    #[test]
    fn method_seek_bad_offset_type() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abc")));
        let seek = get_method(&file, "seek");
        let err = call_method(
            &seek,
            vec![
                file_as_value(&file),
                Value::string("set"),
                Value::Boolean(true),
            ],
        )
        .unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #3 to 'seek' (number expected, got boolean)"
        );
    }

    // =================================================================
    // read format errors
    // =================================================================

    #[test]
    fn method_read_invalid_format_string() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"data")));
        let read = get_method(&file, "read");
        let err = call_method(&read, vec![file_as_value(&file), Value::string("*z")]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'read' (invalid format expected, got \"*z\")"
        );
    }

    #[test]
    fn method_read_negative_byte_count() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"data")));
        let read = get_method(&file, "read");
        let err = call_method(&read, vec![file_as_value(&file), Value::Integer(-1)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'read' (non-negative integer expected, got -1)"
        );
    }

    #[test]
    fn method_read_bad_arg_type() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"data")));
        let read = get_method(&file, "read");
        let err = call_method(&read, vec![file_as_value(&file), Value::Boolean(true)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'read' (string or number expected, got boolean)"
        );
    }

    // =================================================================
    // read *L format through method dispatch
    // =================================================================

    #[test]
    fn method_read_line_with_newline() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abc\ndef\n")));
        let read = get_method(&file, "read");
        let result = call_method(&read, vec![file_as_value(&file), Value::string("*L")]).unwrap();
        k9::assert_equal!(result, vec![Value::string("abc\n")]);
    }

    // =================================================================
    // lines with custom format
    // =================================================================

    #[test]
    fn method_lines_custom_format() {
        let file = LuaFile::new("test", Box::new(MemFile::new(b"abcdef")));
        let lines = get_method(&file, "lines");

        // lines(3) reads 3 bytes at a time.
        let result = call_method(&lines, vec![file_as_value(&file), Value::Integer(3)]).unwrap();
        k9::assert_equal!(result.len(), 1);
        let iter_fn = match &result[0] {
            Value::Function(f) => f.clone(),
            other => panic!("expected function, got {:?}", other),
        };

        let r = call_method(&iter_fn, vec![]).unwrap();
        k9::assert_equal!(r, vec![Value::string("abc")]);

        let r = call_method(&iter_fn, vec![]).unwrap();
        k9::assert_equal!(r, vec![Value::string("def")]);

        // EOF
        let r = call_method(&iter_fn, vec![]).unwrap();
        k9::assert_equal!(r, vec![Value::Nil]);
    }

    // =================================================================
    // close returns ProcessExit through method dispatch
    // =================================================================

    #[test]
    fn method_close_process_exit() {
        struct FakeProc;

        #[async_trait::async_trait]
        impl LuaFileOps for FakeProc {
            async fn read_bytes(&mut self, _n: usize) -> Result<Bytes, std::io::Error> {
                Ok(Bytes::new())
            }
            async fn read_all(&mut self) -> Result<Bytes, std::io::Error> {
                Ok(Bytes::new())
            }
            async fn write_bytes(&mut self, _data: &[u8]) -> Result<(), std::io::Error> {
                Ok(())
            }
            async fn flush(&mut self) -> Result<(), std::io::Error> {
                Ok(())
            }
            async fn seek(&mut self, _pos: SeekFrom) -> Result<u64, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not seekable",
                ))
            }
            async fn close(&mut self) -> Result<CloseStatus, std::io::Error> {
                Ok(CloseStatus::ProcessExit {
                    success: false,
                    code: 42,
                })
            }
        }

        let file = LuaFile::new("proc", Box::new(FakeProc));
        let close = get_method(&file, "close");
        let result = call_method(&close, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("exit"), Value::Integer(42),]
        );
    }

    // =================================================================
    // CloseStatus::into_lua_multi unit tests
    // =================================================================

    #[test]
    fn close_status_into_lua_multi_ok() {
        let result = CloseStatus::Ok.into_lua_multi();
        k9::assert_equal!(result, vec![Value::Boolean(true)]);
    }

    #[test]
    fn close_status_into_lua_multi_process_exit_success() {
        let result = CloseStatus::ProcessExit {
            success: true,
            code: 0,
        }
        .into_lua_multi();
        k9::assert_equal!(
            result,
            vec![
                Value::Boolean(true),
                Value::string("exit"),
                Value::Integer(0),
            ]
        );
    }

    #[test]
    fn close_status_into_lua_multi_process_exit_failure() {
        let result = CloseStatus::ProcessExit {
            success: false,
            code: 1,
        }
        .into_lua_multi();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("exit"), Value::Integer(1),]
        );
    }

    #[test]
    fn close_status_into_lua_multi_process_signal() {
        let result = CloseStatus::ProcessSignal { signal: 9 }.into_lua_multi();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("signal"), Value::Integer(9),]
        );
    }

    // =================================================================
    // method_close returning ProcessSignal through dispatch
    // =================================================================

    #[test]
    fn method_close_process_signal() {
        struct FakeSignalProc;

        #[async_trait::async_trait]
        impl LuaFileOps for FakeSignalProc {
            async fn read_bytes(&mut self, _n: usize) -> Result<Bytes, std::io::Error> {
                Ok(Bytes::new())
            }
            async fn read_all(&mut self) -> Result<Bytes, std::io::Error> {
                Ok(Bytes::new())
            }
            async fn write_bytes(&mut self, _data: &[u8]) -> Result<(), std::io::Error> {
                Ok(())
            }
            async fn flush(&mut self) -> Result<(), std::io::Error> {
                Ok(())
            }
            async fn seek(&mut self, _pos: SeekFrom) -> Result<u64, std::io::Error> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "not seekable",
                ))
            }
            async fn close(&mut self) -> Result<CloseStatus, std::io::Error> {
                Ok(CloseStatus::ProcessSignal { signal: 11 })
            }
        }

        let file = LuaFile::new("proc", Box::new(FakeSignalProc));
        let close = get_method(&file, "close");
        let result = call_method(&close, vec![file_as_value(&file)]).unwrap();
        k9::assert_equal!(
            result,
            vec![Value::Nil, Value::string("signal"), Value::Integer(11),]
        );
    }
}
