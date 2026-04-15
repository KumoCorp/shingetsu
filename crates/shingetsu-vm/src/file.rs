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
use crate::error::VmError;
use crate::userdata::Userdata;
use crate::value::Value;

/// Result of closing a file handle.
///
/// Regular files return [`CloseStatus::Ok`].  Process pipes return
/// [`CloseStatus::ProcessExit`] with the child's exit code, matching
/// Lua's `f:close()` contract for `io.popen` handles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseStatus {
    /// Normal file close succeeded.
    Ok,
    /// A child process exited.
    ProcessExit {
        /// `true` if the process exited with code 0.
        success: bool,
        /// The raw exit code.
        code: i32,
    },
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
    /// [`read_bytes`](Self::read_bytes).  Backends may override if
    /// they can peek without consuming.
    async fn read_number(&mut self) -> Result<Option<f64>, std::io::Error> {
        // Skip whitespace.
        let mut first_non_ws = None;
        loop {
            let chunk = self.read_bytes(1).await?;
            if chunk.is_empty() {
                return Ok(None); // EOF
            }
            if !chunk[0].is_ascii_whitespace() {
                first_non_ws = Some(chunk[0]);
                break;
            }
        }
        let Some(first) = first_non_ws else {
            return Ok(None);
        };

        // Accumulate bytes that could form a number.
        // Supports integers, floats, and scientific notation
        // (e.g. "3.14", "-1e10", "0xff").
        let mut buf = vec![first];
        let mut saw_hex_prefix = first == b'0';
        loop {
            let chunk = self.read_bytes(1).await?;
            if chunk.is_empty() {
                break; // EOF
            }
            let b = chunk[0];
            let is_number_char = b.is_ascii_digit()
                || b == b'.'
                || b == b'-'
                || b == b'+'
                || b == b'e'
                || b == b'E'
                || (saw_hex_prefix
                    && (b == b'x' || b == b'X' || b.is_ascii_hexdigit() || b == b'p' || b == b'P'));
            if !is_number_char {
                // We consumed one byte past the number.  On seekable
                // streams we could rewind; on pipes we accept the loss.
                // This matches Lua's behaviour.
                break;
            }
            if buf.len() == 1 && buf[0] == b'0' && (b == b'x' || b == b'X') {
                saw_hex_prefix = true;
            }
            buf.push(b);
        }

        let s = std::str::from_utf8(&buf).unwrap_or("");
        // Try integer hex parse first (Lua treats 0xff as 255.0 for *n).
        if saw_hex_prefix && s.len() > 2 {
            if let Ok(n) = i64::from_str_radix(&s[2..], 16) {
                return Ok(Some(n as f64));
            }
        }
        match s.parse::<f64>() {
            Ok(n) => Ok(Some(n)),
            Err(_) => Ok(None),
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
}

impl LuaFile {
    /// Create a new file handle wrapping the given I/O backend.
    pub fn new(name: impl Into<String>, ops: Box<dyn LuaFileOps>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Some(ops)),
            name: name.into(),
        })
    }

    /// Returns `true` if the file has been closed.
    pub async fn is_closed(&self) -> bool {
        self.inner.lock().await.is_none()
    }
}

#[async_trait::async_trait]
impl Userdata for LuaFile {
    fn type_name(&self) -> &'static str {
        "file"
    }

    async fn dispatch(
        self: Arc<Self>,
        context: CallContext,
        metamethod: &str,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, VmError> {
        let _ = (context, metamethod, args);
        // Method dispatch will be implemented in F2.
        Err(VmError::HostError {
            name: format!("file:{}", metamethod),
            source: format!(
                "metamethod '{}' not yet implemented for file handles",
                metamethod,
            )
            .into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
