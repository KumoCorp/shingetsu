//! [`LuaFileOps`] implementation for process pipes created by `io.popen`.
//!
//! The pipe I/O is handled by [`TokioFileOps`] (via fd conversion),
//! while the child process lifetime is managed separately for async
//! wait on close.

use std::io::SeekFrom;

use bytes::Bytes;
use shingetsu_vm::file::{BufferMode, CloseStatus, LuaFileOps};
use tokio::process::Child;

use crate::tokio_file::TokioFileOps;

/// A [`LuaFileOps`] implementation for a pipe to/from a child process.
///
/// Delegates all I/O to an inner [`TokioFileOps`] (the pipe fd converted
/// to a `File`).  On [`close`](LuaFileOps::close), drops the pipe and
/// waits for the child, returning the exit status.
pub struct PopenOps {
    /// The pipe I/O handle (child's stdout for read mode, stdin for write).
    io: Option<TokioFileOps>,
    /// The child process, kept alive for async wait on close.
    child: Child,
    can_read: bool,
    can_write: bool,
}

impl PopenOps {
    /// Create a new `PopenOps` wrapping a pipe and child process.
    pub fn new(io: TokioFileOps, child: Child, can_read: bool, can_write: bool) -> Self {
        Self {
            io: Some(io),
            child,
            can_read,
            can_write,
        }
    }
}

#[async_trait::async_trait]
impl LuaFileOps for PopenOps {
    async fn read_bytes(&mut self, n: usize) -> Result<Bytes, std::io::Error> {
        match self.io.as_mut() {
            Some(io) => io.read_bytes(n).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "pipe is closed",
            )),
        }
    }

    async fn read_line(&mut self, keep_newline: bool) -> Result<Option<Bytes>, std::io::Error> {
        match self.io.as_mut() {
            Some(io) => io.read_line(keep_newline).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "pipe is closed",
            )),
        }
    }

    async fn read_all(&mut self) -> Result<Bytes, std::io::Error> {
        match self.io.as_mut() {
            Some(io) => io.read_all().await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "pipe is closed",
            )),
        }
    }

    async fn read_number(&mut self) -> Result<Option<f64>, std::io::Error> {
        match self.io.as_mut() {
            Some(io) => io.read_number().await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "pipe is closed",
            )),
        }
    }

    async fn write_bytes(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        match self.io.as_mut() {
            Some(io) => io.write_bytes(data).await,
            None => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "pipe is closed",
            )),
        }
    }

    async fn flush(&mut self) -> Result<(), std::io::Error> {
        match self.io.as_mut() {
            Some(io) => io.flush().await,
            None => Ok(()),
        }
    }

    async fn seek(&mut self, _pos: SeekFrom) -> Result<u64, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "seek not supported on pipes",
        ))
    }

    async fn close(&mut self) -> Result<CloseStatus, std::io::Error> {
        // Flush and drop the pipe to signal EOF to the child.
        if let Some(mut io) = self.io.take() {
            let _ = io.flush().await;
        }

        // Wait for the child to exit.
        let status = self.child.wait().await?;

        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return Ok(CloseStatus::ProcessSignal { signal });
            }
        }

        let code = status.code().unwrap_or(-1);
        Ok(CloseStatus::ProcessExit {
            success: status.success(),
            code,
        })
    }

    async fn set_buffering(&mut self, mode: BufferMode) -> Result<(), std::io::Error> {
        match self.io.as_mut() {
            Some(io) => io.set_buffering(mode).await,
            None => Ok(()),
        }
    }

    fn can_read(&self) -> bool {
        self.can_read
    }

    fn can_write(&self) -> bool {
        self.can_write
    }

    fn can_seek(&self) -> bool {
        false
    }
}
