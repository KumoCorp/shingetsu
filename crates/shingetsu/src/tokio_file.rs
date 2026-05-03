//! [`LuaFileOps`] implementation backed by [`tokio::fs::File`] with manual
//! read/write buffering.
//!
//! Buffering is managed directly rather than via `BufReader`/`BufWriter`
//! so that seek can invalidate the read buffer and flush the write buffer
//! in a single operation, and `set_buffering` can change the write mode
//! at runtime.

use std::io::{Seek, SeekFrom};

use shingetsu::Bytes;
use shingetsu_vm::file::{BufferMode, CloseStatus, LuaFileOps, NumberAccumulator};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Default buffer capacity (8 KiB, matching `BufReader`/`BufWriter` defaults).
const DEFAULT_BUF_SIZE: usize = 8192;

/// A [`LuaFileOps`] implementation backed by a Tokio filesystem file with
/// manual read and write buffering.
pub(crate) struct TokioFileOps {
    file: File,
    /// Read buffer and the valid data window within it.
    read_buf: Vec<u8>,
    /// Start of unconsumed data in `read_buf`.
    read_pos: usize,
    /// One past the last valid byte in `read_buf`.
    read_len: usize,
    /// Write buffer.
    write_buf: Vec<u8>,
    /// Current output buffering mode.
    buf_mode: BufferMode,
    can_read: bool,
    can_write: bool,
    can_seek: bool,
}

impl TokioFileOps {
    /// Wrap an already-opened [`tokio::fs::File`].
    ///
    /// `can_read` / `can_write` should reflect the mode the file was opened
    /// with (e.g. `"r"` → read-only, `"w"` → write-only, `"r+"` → both).
    pub fn new(file: File, can_read: bool, can_write: bool, can_seek: bool) -> Self {
        Self {
            file,
            read_buf: vec![0u8; DEFAULT_BUF_SIZE],
            read_pos: 0,
            read_len: 0,
            write_buf: Vec::new(),
            buf_mode: BufferMode::Full {
                size: Some(DEFAULT_BUF_SIZE),
            },
            can_read,
            can_write,
            can_seek,
        }
    }

    /// Create from a [`std::fs::File`], probing seekability synchronously
    /// via a no-op `lseek(SEEK_CUR, 0)` call.
    pub fn from_std(mut file: std::fs::File, can_read: bool, can_write: bool) -> Self {
        let can_seek = file.seek(SeekFrom::Current(0)).is_ok();
        let tokio_file = File::from_std(file);
        Self::new(tokio_file, can_read, can_write, can_seek)
    }

    /// Set the initial output buffering mode.  Must be called before any
    /// writes (the write buffer is assumed to be empty).
    pub fn with_buf_mode(mut self, mode: BufferMode) -> Self {
        debug_assert!(
            self.write_buf.is_empty(),
            "with_buf_mode called after writes"
        );
        self.buf_mode = mode;
        self
    }

    /// Number of buffered bytes available for reading without a syscall.
    fn buffered_read(&self) -> usize {
        self.read_len - self.read_pos
    }

    /// Return a slice of the buffered read data.
    fn read_buf_slice(&self) -> &[u8] {
        &self.read_buf[self.read_pos..self.read_len]
    }

    /// Consume `n` bytes from the read buffer.
    fn consume(&mut self, n: usize) {
        self.read_pos += n;
    }

    /// Refill the read buffer from the underlying file.  Returns the number
    /// of bytes now available (0 at EOF).
    async fn fill_buf(&mut self) -> Result<usize, std::io::Error> {
        // Compact: move unconsumed data to the front.
        if self.read_pos > 0 {
            let remaining = self.buffered_read();
            self.read_buf.copy_within(self.read_pos..self.read_len, 0);
            self.read_pos = 0;
            self.read_len = remaining;
        }
        let n = self.file.read(&mut self.read_buf[self.read_len..]).await?;
        self.read_len += n;
        Ok(self.buffered_read())
    }

    /// Flush the write buffer to the underlying file.
    async fn flush_write_buf(&mut self) -> Result<(), std::io::Error> {
        if !self.write_buf.is_empty() {
            self.file.write_all(&self.write_buf).await?;
            self.write_buf.clear();
            self.file.flush().await?;
        }
        Ok(())
    }

    /// Discard any buffered read data (e.g. after a seek).
    fn invalidate_read_buf(&mut self) {
        self.read_pos = 0;
        self.read_len = 0;
    }

    /// The effective write buffer capacity for the current mode.
    fn write_buf_capacity(&self) -> usize {
        match self.buf_mode {
            BufferMode::No => 0,
            BufferMode::Full { size } | BufferMode::Line { size } => {
                size.unwrap_or(DEFAULT_BUF_SIZE)
            }
        }
    }
}

#[async_trait::async_trait]
impl LuaFileOps for TokioFileOps {
    async fn read_bytes(&mut self, n: usize) -> Result<Bytes, std::io::Error> {
        // Cap the initial allocation to avoid OOM when `n` is
        // enormous (e.g. Lua `file:read(math.maxinteger)`), while
        // still allocating generously for normal reads to avoid
        // heap fragmentation from repeated growth.
        const MAX_INITIAL_CAP: usize = 1024 * 1024; // 1 MiB
        let initial_cap = n.min(MAX_INITIAL_CAP);
        let mut result = Vec::with_capacity(initial_cap);
        while result.len() < n {
            if self.buffered_read() == 0 {
                if self.fill_buf().await? == 0 {
                    break; // EOF
                }
            }
            let want = n - result.len();
            let avail = self.buffered_read().min(want);
            result.extend_from_slice(&self.read_buf_slice()[..avail]);
            self.consume(avail);
        }
        Ok(Bytes::from(result))
    }

    async fn read_line(&mut self, keep_newline: bool) -> Result<Option<Bytes>, std::io::Error> {
        let mut line = Vec::new();
        loop {
            // Scan the buffer for a newline.
            if self.buffered_read() > 0 {
                let buf = self.read_buf_slice();
                if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                    // Found newline at `pos` within the buffered slice.
                    line.extend_from_slice(&buf[..pos]);
                    self.consume(pos + 1); // consume through the \n
                    if keep_newline {
                        // Re-add \r\n or \n as appropriate.
                        if line.last() == Some(&b'\r') {
                            // The \r is already in `line`; add back the \n.
                            line.push(b'\n');
                        } else {
                            line.push(b'\n');
                        }
                    } else {
                        // Strip trailing \r if CRLF.
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                    }
                    return Ok(Some(Bytes::from(line)));
                }
                // No newline in buffer — take everything and refill.
                line.extend_from_slice(buf);
                let consumed = self.buffered_read();
                self.consume(consumed);
            }
            if self.fill_buf().await? == 0 {
                // EOF — return what we have, or None if nothing.
                return if line.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Bytes::from(line)))
                };
            }
        }
    }

    async fn read_all(&mut self) -> Result<Bytes, std::io::Error> {
        let mut result = Vec::new();
        // Drain anything already buffered.
        if self.buffered_read() > 0 {
            result.extend_from_slice(self.read_buf_slice());
            let consumed = self.buffered_read();
            self.consume(consumed);
        }
        // Read the rest directly from the file (no need to go through
        // our buffer for a bulk read-to-end).
        self.file.read_to_end(&mut result).await?;
        Ok(Bytes::from(result))
    }

    async fn read_number(&mut self) -> Result<Option<f64>, std::io::Error> {
        // Skip whitespace.
        loop {
            if self.buffered_read() > 0 {
                let buf = self.read_buf_slice();
                if let Some(pos) = buf.iter().position(|b| !b.is_ascii_whitespace()) {
                    self.consume(pos);
                    break;
                }
                // Entire buffer is whitespace — discard and refill.
                let consumed = self.buffered_read();
                self.consume(consumed);
            }
            if self.fill_buf().await? == 0 {
                return Ok(None); // EOF while skipping whitespace
            }
        }

        // Accumulate number bytes from the buffer.
        let mut acc = NumberAccumulator::new();
        loop {
            if self.buffered_read() == 0 {
                if self.fill_buf().await? == 0 {
                    break; // EOF
                }
            }
            let buf_len = self.buffered_read();
            let consumed = acc.feed_slice(self.read_buf_slice());
            self.consume(consumed);
            if consumed < buf_len {
                // Stopped early — hit a non-number byte.
                break;
            }
        }

        Ok(acc.finish())
    }

    async fn write_bytes(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        if data.is_empty() {
            return Ok(());
        }
        match self.buf_mode {
            BufferMode::No => {
                // Unbuffered: write directly to the file.
                debug_assert!(
                    self.write_buf.is_empty(),
                    "write buffer should be empty in No mode"
                );
                self.file.write_all(data).await?;
                self.file.flush().await?;
            }
            BufferMode::Full { .. } => {
                let cap = self.write_buf_capacity();
                if self.write_buf.len() + data.len() > cap {
                    // Would exceed buffer — flush first.
                    self.flush_write_buf().await?;
                    if data.len() >= cap {
                        // Data alone exceeds the buffer; bypass buffering
                        // entirely (same strategy as std BufWriter).
                        self.file.write_all(data).await?;
                        self.file.flush().await?;
                    } else {
                        self.write_buf.extend_from_slice(data);
                    }
                } else {
                    self.write_buf.extend_from_slice(data);
                }
            }
            BufferMode::Line { .. } => {
                let cap = self.write_buf_capacity();
                // Find the last newline — everything up to and including
                // it must be flushed; the tail after it stays buffered.
                if let Some(nl_pos) = data.iter().rposition(|&b| b == b'\n') {
                    let (through_nl, tail) = data.split_at(nl_pos + 1);
                    // Flush pending buffer + data through the last newline.
                    self.flush_write_buf().await?;
                    self.file.write_all(through_nl).await?;
                    self.file.flush().await?;
                    // Buffer the remainder after the last newline.
                    if !tail.is_empty() {
                        self.write_buf.extend_from_slice(tail);
                    }
                } else if self.write_buf.len() + data.len() > cap {
                    // No newline, but would exceed buffer — spill.
                    self.flush_write_buf().await?;
                    if data.len() >= cap {
                        self.file.write_all(data).await?;
                        self.file.flush().await?;
                    } else {
                        self.write_buf.extend_from_slice(data);
                    }
                } else {
                    self.write_buf.extend_from_slice(data);
                }
            }
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), std::io::Error> {
        self.flush_write_buf().await?;
        self.file.flush().await
    }

    async fn seek(&mut self, pos: SeekFrom) -> Result<u64, std::io::Error> {
        // Flush writes so the file position is consistent.
        self.flush_write_buf().await?;
        // If seeking relative to current, adjust for buffered read data
        // that the file cursor has already advanced past.
        let adjusted = match pos {
            SeekFrom::Current(offset) => {
                let buffered = self.buffered_read() as i64;
                SeekFrom::Current(offset - buffered)
            }
            other => other,
        };
        self.invalidate_read_buf();
        self.file.seek(adjusted).await
    }

    async fn close(&mut self) -> Result<CloseStatus, std::io::Error> {
        // Best-effort flush before closing.
        let _ = self.flush_write_buf().await;
        // Tokio's File is closed on drop; we just need to flush.
        self.file.flush().await?;
        Ok(CloseStatus::Ok)
    }

    async fn set_buffering(&mut self, mode: BufferMode) -> Result<(), std::io::Error> {
        // Flush any pending writes under the old mode before switching.
        self.flush_write_buf().await?;
        self.buf_mode = mode;
        Ok(())
    }

    fn can_read(&self) -> bool {
        self.can_read
    }

    fn can_write(&self) -> bool {
        self.can_write
    }

    fn can_seek(&self) -> bool {
        self.can_seek
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::SeekFrom;
    use tempfile::NamedTempFile;
    use tokio::fs::File;

    /// Helper: create a temp file with the given contents, return an
    /// opened `TokioFileOps` in read mode.
    async fn read_file(contents: &[u8]) -> (TokioFileOps, NamedTempFile) {
        let tmp = NamedTempFile::new().expect("create temp file");
        std::fs::write(tmp.path(), contents).expect("write temp file");
        let file = File::open(tmp.path()).await.expect("open");
        (TokioFileOps::new(file, true, false, true), tmp)
    }

    /// Helper: create a temp file and return a `TokioFileOps` in write mode.
    async fn write_file() -> (TokioFileOps, NamedTempFile) {
        let tmp = NamedTempFile::new().expect("create temp file");
        let file = File::create(tmp.path()).await.expect("create");
        (TokioFileOps::new(file, false, true, true), tmp)
    }

    /// Helper: create a temp file with contents and return a `TokioFileOps`
    /// in read+write mode.
    async fn rw_file(contents: &[u8]) -> (TokioFileOps, NamedTempFile) {
        let tmp = NamedTempFile::new().expect("create temp file");
        std::fs::write(tmp.path(), contents).expect("write temp file");
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(tmp.path())
            .await
            .expect("open r+");
        (TokioFileOps::new(file, true, true, true), tmp)
    }

    // =====================================================================
    // read_bytes
    // =====================================================================

    #[tokio::test]
    async fn read_bytes_basic() {
        let (mut ops, _tmp) = read_file(b"hello world").await;
        let chunk = ops.read_bytes(5).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hello");
    }

    #[tokio::test]
    async fn read_bytes_at_eof() {
        let (mut ops, _tmp) = read_file(b"hi").await;
        let chunk = ops.read_bytes(100).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hi");
    }

    #[tokio::test]
    async fn read_bytes_empty_file() {
        let (mut ops, _tmp) = read_file(b"").await;
        let chunk = ops.read_bytes(10).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"");
    }

    #[tokio::test]
    async fn read_bytes_larger_than_buffer() {
        // Create data larger than DEFAULT_BUF_SIZE to exercise multi-fill.
        let data: Vec<u8> = (0..DEFAULT_BUF_SIZE * 2 + 100)
            .map(|i| (i % 256) as u8)
            .collect();
        let (mut ops, _tmp) = read_file(&data).await;
        let chunk = ops.read_bytes(data.len()).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), data.as_slice());
    }

    // =====================================================================
    // read_line
    // =====================================================================

    #[tokio::test]
    async fn read_line_strips_newline() {
        let (mut ops, _tmp) = read_file(b"line1\nline2\n").await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"line1");
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"line2");
        let eof = ops.read_line(false).await.expect("read");
        k9::assert_equal!(eof, None);
    }

    #[tokio::test]
    async fn read_line_keeps_newline() {
        let (mut ops, _tmp) = read_file(b"abc\ndef\n").await;
        let line = ops.read_line(true).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"abc\n");
        let line = ops.read_line(true).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"def\n");
    }

    #[tokio::test]
    async fn read_line_strips_crlf() {
        let (mut ops, _tmp) = read_file(b"dos\r\nline\r\n").await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"dos");
    }

    #[tokio::test]
    async fn read_line_no_trailing_newline() {
        let (mut ops, _tmp) = read_file(b"last").await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"last");
        let eof = ops.read_line(false).await.expect("read");
        k9::assert_equal!(eof, None);
    }

    #[tokio::test]
    async fn read_line_spanning_buffer_boundary() {
        // A line longer than the buffer to exercise multi-fill in read_line.
        let long_line: Vec<u8> = std::iter::repeat(b'x')
            .take(DEFAULT_BUF_SIZE * 2 + 50)
            .chain(std::iter::once(b'\n'))
            .collect();
        let (mut ops, _tmp) = read_file(&long_line).await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.len(), DEFAULT_BUF_SIZE * 2 + 50);
    }

    // =====================================================================
    // read_all
    // =====================================================================

    #[tokio::test]
    async fn read_all_basic() {
        let (mut ops, _tmp) = read_file(b"everything").await;
        let all = ops.read_all().await.expect("read");
        k9::assert_equal!(all.as_ref(), b"everything");
    }

    #[tokio::test]
    async fn read_all_after_partial_read() {
        let (mut ops, _tmp) = read_file(b"hello world").await;
        ops.read_bytes(6).await.expect("read");
        let rest = ops.read_all().await.expect("read");
        k9::assert_equal!(rest.as_ref(), b"world");
    }

    #[tokio::test]
    async fn read_all_empty_file() {
        let (mut ops, _tmp) = read_file(b"").await;
        let all = ops.read_all().await.expect("read");
        k9::assert_equal!(all.as_ref(), b"");
    }

    // =====================================================================
    // read_number
    // =====================================================================

    #[tokio::test]
    async fn read_number_basic() {
        let (mut ops, _tmp) = read_file(b"  42.5  99").await;
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 42.5);
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 99.0);
    }

    #[tokio::test]
    async fn read_number_hex() {
        let (mut ops, _tmp) = read_file(b"0xff 0xDEAD").await;
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 255.0);
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 57005.0);
    }

    #[tokio::test]
    async fn read_number_scientific() {
        let (mut ops, _tmp) = read_file(b"1.5e2 -3E1").await;
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 150.0);
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, -30.0);
    }

    #[tokio::test]
    async fn read_number_at_eof() {
        let (mut ops, _tmp) = read_file(b"   ").await;
        let n = ops.read_number().await.expect("read");
        k9::assert_equal!(n, None);
    }

    // =====================================================================
    // write_bytes and buffering modes
    // =====================================================================

    #[tokio::test]
    async fn write_and_read_back() {
        let (mut ops, tmp) = write_file().await;
        ops.write_bytes(b"hello").await.expect("write");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"hello");
    }

    #[tokio::test]
    async fn write_buffered_needs_flush() {
        let (mut ops, tmp) = write_file().await;
        ops.write_bytes(b"buffered").await.expect("write");
        // Without flush, data may still be in the write buffer.
        // After close, it should be flushed.
        ops.close().await.expect("close");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"buffered");
    }

    #[tokio::test]
    async fn write_unbuffered_mode() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::No).await.expect("set mode");
        ops.write_bytes(b"immediate").await.expect("write");
        // In No mode, data is written directly without needing flush.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"immediate");
    }

    #[tokio::test]
    async fn write_line_mode_flushes_on_newline() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"no newline yet").await.expect("write");
        // May or may not be flushed yet (no newline).
        ops.write_bytes(b" done\n").await.expect("write");
        // After a newline, the buffer should be flushed.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"no newline yet done\n");
    }

    #[tokio::test]
    async fn write_line_mode_buffers_after_last_newline() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(8192) })
            .await
            .expect("set mode");
        // Write data with a newline in the middle — the tail after
        // the last newline should stay buffered, not be flushed.
        ops.write_bytes(b"hello\nworld").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"hello\n");
        // Now flush to get the buffered tail.
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"hello\nworld");
    }

    #[tokio::test]
    async fn write_line_mode_long_line_exceeds_buffer() {
        let (mut ops, tmp) = write_file().await;
        // Use a tiny buffer so we can test overflow easily.
        ops.set_buffering(BufferMode::Line { size: Some(16) })
            .await
            .expect("set mode");
        // Write a line longer than the 16-byte buffer with no newline.
        let long_line = b"this line is definitely longer than sixteen bytes";
        ops.write_bytes(long_line).await.expect("write");
        // Data should be spilled to the file despite no newline.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), long_line.as_slice());
    }

    #[tokio::test]
    async fn write_line_mode_newline_at_end_flushes_all() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"complete line\n").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"complete line\n");
    }

    #[tokio::test]
    async fn write_line_mode_multiple_newlines() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"aaa\nbbb\nccc").await.expect("write");
        // Everything through the last newline is flushed; "ccc" stays buffered.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"aaa\nbbb\n");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"aaa\nbbb\nccc");
    }

    #[tokio::test]
    async fn write_line_mode_long_line_with_newline_at_end() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(16) })
            .await
            .expect("set mode");
        // A line longer than the buffer that ends with a newline —
        // everything should be flushed, nothing buffered.
        let long_line = b"a very long complete line ending here\n";
        ops.write_bytes(long_line).await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), long_line.as_slice());
    }

    // =====================================================================
    // seek
    // =====================================================================

    #[tokio::test]
    async fn seek_set() {
        let (mut ops, _tmp) = read_file(b"abcdef").await;
        ops.read_bytes(3).await.expect("read");
        let pos = ops.seek(SeekFrom::Start(1)).await.expect("seek");
        k9::assert_equal!(pos, 1);
        let chunk = ops.read_bytes(3).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"bcd");
    }

    #[tokio::test]
    async fn seek_current() {
        let (mut ops, _tmp) = read_file(b"abcdef").await;
        ops.read_bytes(2).await.expect("read");
        // Seek forward 1 from logical current position (2).
        let pos = ops.seek(SeekFrom::Current(1)).await.expect("seek");
        k9::assert_equal!(pos, 3);
        let chunk = ops.read_bytes(3).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"def");
    }

    #[tokio::test]
    async fn seek_end() {
        let (mut ops, _tmp) = read_file(b"abcdef").await;
        let pos = ops.seek(SeekFrom::End(-2)).await.expect("seek");
        k9::assert_equal!(pos, 4);
        let chunk = ops.read_all().await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"ef");
    }

    #[tokio::test]
    async fn seek_flushes_writes() {
        let (mut ops, tmp) = rw_file(b"abcdef").await;
        ops.write_bytes(b"XY").await.expect("write");
        // Seek should flush the write buffer.
        ops.seek(SeekFrom::Start(0)).await.expect("seek");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"XYcdef");
    }

    // =====================================================================
    // read+write mode
    // =====================================================================

    #[tokio::test]
    async fn read_then_write_then_read() {
        let (mut ops, tmp) = rw_file(b"hello world").await;
        // Read first 5 bytes.
        let chunk = ops.read_bytes(5).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hello");
        // Seek to position 5, write " rust".
        ops.seek(SeekFrom::Start(5)).await.expect("seek");
        ops.write_bytes(b" rust!").await.expect("write");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"hello rust!");
    }

    // =====================================================================
    // close
    // =====================================================================

    #[tokio::test]
    async fn close_returns_ok() {
        let (mut ops, _tmp) = read_file(b"").await;
        let status = ops.close().await.expect("close");
        k9::assert_equal!(status, CloseStatus::Ok);
    }

    // =====================================================================
    // set_buffering
    // =====================================================================

    #[tokio::test]
    async fn set_buffering_flushes_pending() {
        let (mut ops, tmp) = write_file().await;
        ops.write_bytes(b"pending").await.expect("write");
        // Switching mode should flush.
        ops.set_buffering(BufferMode::No).await.expect("set mode");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"pending");
    }

    // =====================================================================
    // capability queries
    // =====================================================================

    #[tokio::test]
    async fn capabilities_read_only() {
        let (ops, _tmp) = read_file(b"").await;
        k9::assert_equal!(ops.can_read(), true);
        k9::assert_equal!(ops.can_write(), false);
        k9::assert_equal!(ops.can_seek(), true);
    }

    #[tokio::test]
    async fn capabilities_write_only() {
        let (ops, _tmp) = write_file().await;
        k9::assert_equal!(ops.can_read(), false);
        k9::assert_equal!(ops.can_write(), true);
        k9::assert_equal!(ops.can_seek(), true);
    }

    #[tokio::test]
    async fn capabilities_read_write() {
        let (ops, _tmp) = rw_file(b"").await;
        k9::assert_equal!(ops.can_read(), true);
        k9::assert_equal!(ops.can_write(), true);
        k9::assert_equal!(ops.can_seek(), true);
    }

    // =====================================================================
    // read_line: bare CR is not a line ending
    // =====================================================================

    #[tokio::test]
    async fn read_line_bare_cr_not_line_ending() {
        // Bare \r is not a line terminator in Lua — only \n is.
        let (mut ops, _tmp) = read_file(b"abc\rdef\n").await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"abc\rdef");
    }

    #[tokio::test]
    async fn read_line_crlf_keep_newline() {
        let (mut ops, _tmp) = read_file(b"dos\r\n").await;
        let line = ops.read_line(true).await.expect("read").expect("not eof");
        // CRLF kept: the \r is part of the line data, \n is the newline.
        k9::assert_equal!(line.as_ref(), b"dos\r\n");
    }

    // =====================================================================
    // read_line: newline at exact buffer boundary
    // =====================================================================

    #[tokio::test]
    async fn read_line_newline_at_buffer_boundary() {
        // Place the \n exactly at position DEFAULT_BUF_SIZE so it's the
        // first byte of the second buffer fill.
        let mut data = vec![b'a'; DEFAULT_BUF_SIZE];
        data.push(b'\n');
        data.extend_from_slice(b"next\n");
        let (mut ops, _tmp) = read_file(&data).await;

        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.len(), DEFAULT_BUF_SIZE);
        assert!(line.iter().all(|&b| b == b'a'));

        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"next");
    }

    // =====================================================================
    // write_bytes: large data bypasses buffer
    // =====================================================================

    // =====================================================================
    // read_number: spanning buffer boundary
    // =====================================================================

    #[tokio::test]
    async fn read_number_spanning_buffer_boundary() {
        // Place a number so it starts near the end of the first buffer
        // fill and finishes in the second.  The buffer is DEFAULT_BUF_SIZE
        // bytes; fill with whitespace up to 4 bytes before the boundary,
        // then write "12345678" which straddles the edge.
        let pad = DEFAULT_BUF_SIZE - 4;
        let mut data = vec![b' '; pad];
        data.extend_from_slice(b"12345678 next");
        let (mut ops, _tmp) = read_file(&data).await;
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 12345678.0);
    }

    #[tokio::test]
    async fn read_number_hex_spanning_buffer_boundary() {
        // "0x" at the end of one fill, hex digits in the next.
        let pad = DEFAULT_BUF_SIZE - 2;
        let mut data = vec![b' '; pad];
        data.extend_from_slice(b"0xff end");
        let (mut ops, _tmp) = read_file(&data).await;
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 255.0);
    }

    // =====================================================================
    // write_bytes: full-mode auto-flush at capacity
    // =====================================================================

    #[tokio::test]
    async fn write_full_mode_auto_flush_on_overflow() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Full { size: Some(16) })
            .await
            .expect("set mode");
        // First write fits in the 16-byte buffer.
        ops.write_bytes(b"aaaaaaaaaa").await.expect("write"); // 10 bytes
                                                              // Not flushed yet.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"");
        // Second write exceeds capacity (10 + 10 > 16), triggering auto-flush
        // of the first chunk, then the second chunk fits the now-empty buffer.
        ops.write_bytes(b"bbbbbbbbbb").await.expect("write"); // 10 bytes
                                                              // The first 10 bytes should have been flushed to disk.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"aaaaaaaaaa");
        // Final flush to get everything.
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"aaaaaaaaaabbbbbbbbbb");
    }

    // =====================================================================
    // seek: negative Current after reads
    // =====================================================================

    #[tokio::test]
    async fn seek_current_negative() {
        let (mut ops, _tmp) = read_file(b"abcdefghij").await;
        ops.read_bytes(6).await.expect("read"); // logical pos = 6
                                                // Seek back 3 from current → logical pos 3.
        let pos = ops.seek(SeekFrom::Current(-3)).await.expect("seek");
        k9::assert_equal!(pos, 3);
        let chunk = ops.read_bytes(4).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"defg");
    }

    #[tokio::test]
    async fn seek_current_negative_with_buffered_data() {
        // Read a small amount so the buffer has been filled with more
        // data than we consumed.  Seek backwards should account for
        // the buffered-but-unconsumed bytes.
        let data: Vec<u8> = (0..200).collect();
        let (mut ops, _tmp) = read_file(&data).await;
        ops.read_bytes(10).await.expect("read"); // logical pos = 10
                                                 // Seek back to position 5.
        let pos = ops.seek(SeekFrom::Current(-5)).await.expect("seek");
        k9::assert_equal!(pos, 5);
        let chunk = ops.read_bytes(5).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), &data[5..10]);
    }

    // =====================================================================
    // close: flushes buffered writes
    // =====================================================================

    #[tokio::test]
    async fn close_flushes_buffered_writes() {
        let (mut ops, tmp) = write_file().await;
        ops.write_bytes(b"before close").await.expect("write");
        // Don't call flush — close should handle it.
        ops.close().await.expect("close");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"before close");
    }

    // =====================================================================
    // write_bytes: empty data in each mode
    // =====================================================================

    #[tokio::test]
    async fn write_empty_data_no_mode() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::No).await.expect("set mode");
        ops.write_bytes(b"").await.expect("write empty");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"");
    }

    #[tokio::test]
    async fn write_empty_data_full_mode() {
        let (mut ops, tmp) = write_file().await;
        ops.write_bytes(b"existing").await.expect("write");
        ops.write_bytes(b"").await.expect("write empty");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"existing");
    }

    #[tokio::test]
    async fn write_empty_data_line_mode() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"existing").await.expect("write");
        ops.write_bytes(b"").await.expect("write empty");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"existing");
    }

    // =====================================================================
    // set_buffering: mode transitions
    // =====================================================================

    #[tokio::test]
    async fn set_buffering_full_to_line() {
        let (mut ops, tmp) = write_file().await;
        // Start in Full mode (default), write data.
        ops.write_bytes(b"full ").await.expect("write");
        // Switch to Line mode — should flush "full ".
        ops.set_buffering(BufferMode::Line { size: Some(8192) })
            .await
            .expect("set mode");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"full ");
        // Now line-mode behavior: newline triggers flush.
        ops.write_bytes(b"line\n").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"full line\n");
    }

    #[tokio::test]
    async fn set_buffering_line_to_no() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"buffered").await.expect("write");
        // Switch to No mode — should flush "buffered".
        ops.set_buffering(BufferMode::No).await.expect("set mode");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"buffered");
        // Now unbuffered: writes appear immediately.
        ops.write_bytes(b" immediate").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"buffered immediate");
    }

    #[tokio::test]
    async fn set_buffering_no_to_full() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::No).await.expect("set mode");
        ops.write_bytes(b"unbuf ").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"unbuf ");
        // Switch to Full mode — subsequent writes should be buffered.
        ops.set_buffering(BufferMode::Full { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"now buffered").await.expect("write");
        // Not yet flushed.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"unbuf ");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"unbuf now buffered");
    }

    // =====================================================================
    // set_buffering: resize buffer without losing data
    // =====================================================================

    #[tokio::test]
    async fn set_buffering_resize_smaller() {
        let (mut ops, tmp) = write_file().await;
        // Start with a large buffer.
        ops.set_buffering(BufferMode::Full { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"hello ").await.expect("write");
        // Shrink the buffer — pending data must be flushed first.
        ops.set_buffering(BufferMode::Full { size: Some(16) })
            .await
            .expect("set mode");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"hello ");
        // Continue writing under the smaller buffer.
        ops.write_bytes(b"world").await.expect("write");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"hello world");
    }

    #[tokio::test]
    async fn set_buffering_resize_larger() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Full { size: Some(16) })
            .await
            .expect("set mode");
        ops.write_bytes(b"small ").await.expect("write");
        // Grow the buffer — pending data flushed, then new buffer is bigger.
        ops.set_buffering(BufferMode::Full { size: Some(65536) })
            .await
            .expect("set mode");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"small ");
        // A large write now fits in the bigger buffer.
        let big = vec![b'X'; 1000];
        ops.write_bytes(&big).await.expect("write");
        // Still buffered — not flushed yet.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"small ");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.len(), 6 + 1000);
        k9::assert_equal!(&contents[..6], b"small ");
    }

    #[tokio::test]
    async fn set_buffering_resize_full_to_line_preserves_data() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Full { size: Some(8192) })
            .await
            .expect("set mode");
        ops.write_bytes(b"aaa ").await.expect("write");
        ops.write_bytes(b"bbb ").await.expect("write");
        // Switch to Line with a different size — all pending data flushed.
        ops.set_buffering(BufferMode::Line { size: Some(32) })
            .await
            .expect("set mode");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"aaa bbb ");
        // Line mode now active with size=32.
        ops.write_bytes(b"ccc\n").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"aaa bbb ccc\n");
    }

    // =====================================================================
    // write_bytes: large data bypasses buffer
    // =====================================================================

    #[tokio::test]
    async fn write_large_data_bypasses_buffer() {
        let (mut ops, tmp) = write_file().await;
        // Write something small to populate the buffer.
        ops.write_bytes(b"small").await.expect("write");
        // Write a chunk larger than the buffer capacity.
        let big = vec![b'X'; DEFAULT_BUF_SIZE * 2];
        ops.write_bytes(&big).await.expect("write");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.len(), 5 + DEFAULT_BUF_SIZE * 2);
        k9::assert_equal!(&contents[..5], b"small");
    }

    // =====================================================================
    // full mode: write exactly fills buffer capacity
    // =====================================================================

    #[tokio::test]
    async fn write_full_mode_exact_capacity() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Full { size: Some(8) })
            .await
            .expect("set mode");
        // Write exactly 8 bytes into an 8-byte buffer — fits, no flush.
        ops.write_bytes(b"12345678").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"");
        // One more byte overflows, triggering a flush of the first 8.
        ops.write_bytes(b"9").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"12345678");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"123456789");
    }

    // =====================================================================
    // full mode: data exactly equals buffer capacity (bypass boundary)
    // =====================================================================

    #[tokio::test]
    async fn write_full_mode_data_equals_capacity() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Full { size: Some(8) })
            .await
            .expect("set mode");
        // Buffer has some data.
        ops.write_bytes(b"ab").await.expect("write");
        // Write exactly `cap` bytes — triggers flush of "ab", then the
        // new data (len == cap) bypasses the buffer entirely.
        ops.write_bytes(b"12345678").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"ab12345678");
    }

    // =====================================================================
    // full mode: sequential small writes accumulate then overflow
    // =====================================================================

    #[tokio::test]
    async fn write_full_mode_many_small_writes() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Full { size: Some(8) })
            .await
            .expect("set mode");
        for i in 0u8..8 {
            ops.write_bytes(&[b'a' + i]).await.expect("write");
        }
        // Buffer is exactly full but hasn't been flushed yet.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"");
        // One more byte overflows.
        ops.write_bytes(b"i").await.expect("write");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"abcdefgh");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"abcdefghi");
    }

    // =====================================================================
    // line mode: newline with tail longer than buffer (matches std behavior)
    // =====================================================================

    #[tokio::test]
    async fn write_line_mode_newline_with_long_tail() {
        let (mut ops, tmp) = write_file().await;
        ops.set_buffering(BufferMode::Line { size: Some(8) })
            .await
            .expect("set mode");
        // "Line1\n" + a tail longer than the 8-byte buffer.
        // std's LineWriter buffers the tail regardless of size.
        ops.write_bytes(b"Line1\n0123456789abcdef")
            .await
            .expect("write");
        // "Line1\n" flushed, long tail buffered.
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"Line1\n");
        ops.flush().await.expect("flush");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"Line1\n0123456789abcdef");
    }

    // =====================================================================
    // read_bytes(0) returns empty without touching buffer
    // =====================================================================

    #[tokio::test]
    async fn read_bytes_zero() {
        let (mut ops, _tmp) = read_file(b"hello").await;
        let chunk = ops.read_bytes(0).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"");
        // Subsequent read should return all data, proving the buffer
        // wasn't disturbed.
        let chunk = ops.read_bytes(5).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hello");
    }

    // =====================================================================
    // seek invalidates read buffer, subsequent reads refill correctly
    // =====================================================================

    #[tokio::test]
    async fn seek_invalidates_read_buffer() {
        let (mut ops, _tmp) = read_file(b"abcdefghij").await;
        // Read to fill the buffer.
        let chunk = ops.read_bytes(3).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"abc");
        // Seek to a different position.
        ops.seek(SeekFrom::Start(7)).await.expect("seek");
        // Read — should get fresh data from position 7, not stale buffer.
        let chunk = ops.read_bytes(3).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hij");
    }

    #[tokio::test]
    async fn seek_start_0_after_partial_read_rereads() {
        let (mut ops, _tmp) = read_file(b"abcdef").await;
        ops.read_bytes(4).await.expect("read");
        ops.seek(SeekFrom::Start(0)).await.expect("seek");
        let chunk = ops.read_bytes(6).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"abcdef");
    }

    // =====================================================================
    // read after write on r/w file without intervening seek
    // =====================================================================

    #[tokio::test]
    async fn write_then_read_without_seek() {
        let (mut ops, _tmp) = rw_file(b"hello world").await;
        // Write at position 0.
        ops.write_bytes(b"HELLO").await.expect("write");
        ops.flush().await.expect("flush");
        // Read without seeking — should continue from position 5.
        let chunk = ops.read_bytes(6).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b" world");
    }

    // =====================================================================
    // read_line: CRLF straddling buffer boundary
    // =====================================================================

    #[tokio::test]
    async fn read_line_crlf_straddles_buffer_boundary() {
        // Place \r at the last byte of the buffer and \n as the first
        // byte of the next fill.
        let mut data = vec![b'x'; DEFAULT_BUF_SIZE - 1];
        data.push(b'\r');
        data.push(b'\n');
        data.extend_from_slice(b"next\n");
        let (mut ops, _tmp) = read_file(&data).await;

        // Strip mode: \r\n should be stripped.
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.len(), DEFAULT_BUF_SIZE - 1);
        assert!(line.iter().all(|&b| b == b'x'));

        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"next");
    }

    #[tokio::test]
    async fn read_line_crlf_straddles_buffer_boundary_keep() {
        let mut data = vec![b'x'; DEFAULT_BUF_SIZE - 1];
        data.push(b'\r');
        data.push(b'\n');
        let (mut ops, _tmp) = read_file(&data).await;

        // Keep mode: \r\n should be preserved.
        let line = ops.read_line(true).await.expect("read").expect("not eof");
        k9::assert_equal!(line.len(), DEFAULT_BUF_SIZE + 1);
        k9::assert_equal!(line[line.len() - 2], b'\r');
        k9::assert_equal!(line[line.len() - 1], b'\n');
    }

    // =====================================================================
    // read_line: empty lines
    // =====================================================================

    #[tokio::test]
    async fn read_line_empty_lines() {
        let (mut ops, _tmp) = read_file(b"a\n\n\nb").await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"a");
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"");
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"");
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"b");
        let eof = ops.read_line(false).await.expect("read");
        k9::assert_equal!(eof, None);
    }

    #[tokio::test]
    async fn read_line_just_newline() {
        let (mut ops, _tmp) = read_file(b"\n").await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"");
        let eof = ops.read_line(false).await.expect("read");
        k9::assert_equal!(eof, None);
    }

    // =====================================================================
    // read_line: bare \r at EOF is data, not stripped
    // =====================================================================

    #[tokio::test]
    async fn read_line_bare_cr_at_eof() {
        let (mut ops, _tmp) = read_file(b"abc\r").await;
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        // No \n found, so \r is data, not a line ending.
        k9::assert_equal!(line.as_ref(), b"abc\r");
    }

    // =====================================================================
    // read_number: edge cases
    // =====================================================================

    #[tokio::test]
    async fn read_number_no_valid_number() {
        let (mut ops, _tmp) = read_file(b"abc").await;
        let n = ops.read_number().await.expect("read");
        k9::assert_equal!(n, None);
    }

    #[tokio::test]
    async fn read_number_at_eof_no_delimiter() {
        let (mut ops, _tmp) = read_file(b"42").await;
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 42.0);
    }

    #[tokio::test]
    async fn read_number_negative() {
        let (mut ops, _tmp) = read_file(b"  -42  7").await;
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, -42.0);
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 7.0);
    }

    // =====================================================================
    // interleaved read types share buffer position correctly
    // =====================================================================

    #[tokio::test]
    async fn interleaved_read_types() {
        let (mut ops, _tmp) = read_file(b"hello\n42 rest").await;
        // read_bytes: consume "hello"
        let chunk = ops.read_bytes(5).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"hello");
        // read_line: consume "\n" (the newline after "hello").
        // Buffer position is at the \n.
        let line = ops.read_line(false).await.expect("read").expect("not eof");
        k9::assert_equal!(line.as_ref(), b"");
        // read_number: consume "42".
        let n = ops.read_number().await.expect("read").expect("parsed");
        k9::assert_equal!(n, 42.0);
        // read_all: consume " rest".
        let rest = ops.read_all().await.expect("read");
        k9::assert_equal!(rest.as_ref(), b" rest");
    }

    // =====================================================================
    // flush when nothing pending (idempotent)
    // =====================================================================

    #[tokio::test]
    async fn flush_idempotent() {
        let (mut ops, tmp) = write_file().await;
        ops.write_bytes(b"data").await.expect("write");
        ops.flush().await.expect("flush");
        // Second flush with nothing pending.
        ops.flush().await.expect("flush again");
        let contents = std::fs::read(tmp.path()).expect("read back");
        k9::assert_equal!(contents.as_slice(), b"data");
    }

    // =====================================================================
    // seek beyond EOF then read
    // =====================================================================

    #[tokio::test]
    async fn read_line_keep_newline_at_eof_no_newline() {
        let (mut ops, _tmp) = read_file(b"abc").await;
        let line = ops.read_line(true).await.expect("read").expect("not eof");
        // No newline in the file — returned as-is, no synthetic \n.
        k9::assert_equal!(line.as_ref(), b"abc");
    }

    #[tokio::test]
    async fn seek_beyond_eof_then_read() {
        let (mut ops, _tmp) = read_file(b"short").await;
        ops.seek(SeekFrom::Start(100)).await.expect("seek");
        let chunk = ops.read_bytes(10).await.expect("read");
        k9::assert_equal!(chunk.as_ref(), b"");
    }
}
