// Allow proc-macro generated code (which references `::shingetsu::*`) to
// resolve within this crate.
extern crate self as shingetsu;

// Re-export the VM public API so embedders only need to depend on `shingetsu`.
pub use shingetsu_vm::*;

pub mod builtins;
pub mod io_lib;
pub mod lua_pattern;
pub mod math_lib;
pub mod os_lib;
pub mod popen;
pub mod string_lib;
pub mod table_lib;
pub mod tokio_file;
pub mod utf8_lib;

// Re-export the compiler under a sub-module for advanced users.
pub use shingetsu_compiler as compiler;

bitflags::bitflags! {
    /// Controls which standard libraries are registered in a [`GlobalEnv`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Libraries: u32 {
        /// Core globals (print, type, pcall, …) plus math, string, table, utf8.
        const BUILTINS = 1 << 0;
        /// `os` library (os.clock, os.time, …).
        const OS       = 1 << 1;
        /// File I/O (`io.open`, `io.tmpfile`, …).
        const IO       = 1 << 2;
        /// Stdio handles (`io.stdin`, `io.stdout`, `io.stderr`,
        /// `io.read`, `io.write`, `io.flush`).
        ///
        /// Call [`io_lib::flush_stdio`] before process exit to ensure
        /// buffered output is flushed (safe to call unconditionally).
        const STDIO    = 1 << 3;
        /// Process execution (`io.popen`).
        const EXEC     = 1 << 4;

        /// Everything enabled.
        const ALL = Self::BUILTINS.bits() | Self::OS.bits()
                  | Self::IO.bits() | Self::STDIO.bits()
                  | Self::EXEC.bits();
        /// Sandbox-safe subset (no OS, I/O, or exec).
        const SANDBOXED = Self::BUILTINS.bits();
    }
}

/// Register the requested set of standard libraries into `env`.
///
/// Implicit dependencies are handled automatically: [`Libraries::STDIO`]
/// pulls in [`Libraries::IO`] since the stdio functions live in the `io`
/// table.
///
/// Call [`io_lib::flush_stdio`] before process exit to ensure
/// buffered stdio output is flushed.  It is safe to call
/// unconditionally — it is a no-op if stdio was not registered.
pub fn register_libs(env: &GlobalEnv, mut libs: Libraries) -> Result<(), VmError> {
    // Resolve implicit dependencies.
    if libs.contains(Libraries::STDIO) || libs.contains(Libraries::EXEC) {
        libs |= Libraries::IO;
    }

    if libs.contains(Libraries::BUILTINS) {
        builtins::register_sandboxed(env)?;
    }
    if libs.contains(Libraries::OS) {
        os_lib::register(env)?;
    }
    if libs.contains(Libraries::IO) {
        io_lib::register(env)?;
    }
    if libs.contains(Libraries::STDIO) {
        io_lib::register_stdio(env)?;
    }
    if libs.contains(Libraries::EXEC) {
        io_lib::register_popen(env)?;
    }
    Ok(())
}

// Re-export downcast_rs so proc-macro generated code can reference
// `::shingetsu::downcast_rs::impl_downcast!` without the embedder having
// to add a direct dependency on downcast-rs.
pub use downcast_rs;

// Re-export proc macros so users only need `shingetsu` as a dependency.
pub use shingetsu_derive::{module, userdata, FromLua, IntoLua, UserData};

// Re-export external crates referenced in proc-macro generated code so that
// generated code can use `::shingetsu::bytes` / `::shingetsu::async_trait`
// without the embedder needing direct dependencies on those crates.
pub use async_trait;
pub use bytes;
