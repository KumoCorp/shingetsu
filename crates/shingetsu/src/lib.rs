// Allow proc-macro generated code (which references `::shingetsu::*`) to
// resolve within this crate.
extern crate self as shingetsu;

// ---------------------------------------------------------------------------
// Sub-modules: standard library implementations
// ---------------------------------------------------------------------------

/// `bit32` table implementation — bitwise operations on unsigned
/// 32-bit integers.
pub mod bit32;
/// Core Lua globals: `print`, `type`, `pcall`, `tostring`, `math`, `string`,
/// `table`, `utf8`, and similar.  See [`register_libs`] for the recommended
/// way to install them.
pub mod builtins;
/// `debug` table implementation.
pub mod debug;
/// Diagnostic rendering for compile-time and runtime errors.
pub mod diagnostic;
/// `io` table implementation.
pub mod io;
pub mod lint_plugin;
/// File-based `require` loader.
pub mod module_loader;
/// `os` table implementation.
pub mod os;
/// Pretty-printer for [`Value`] with cycle detection and depth/entry limits.
pub mod pretty_print;
/// Project configuration (`shingetsu.toml`).
pub mod project_config;
/// Concurrent task library: spawning Lua functions onto tokio,
/// awaiting/cancelling them, and observing task lifecycles.
pub mod task;

/// Per-environment hook for capturing `print`'s output.
///
/// When a `PrintCapture` is registered as a `GlobalEnv` extension,
/// the standard `print` function writes lines to it instead of
/// process stdout.  Used by the docgen example validator to surface
/// printed output in the rendered docs.
pub struct PrintCapture {
    sink: crate::sync::Mutex<String>,
    notify: std::sync::Arc<tokio::sync::Notify>,
}

impl PrintCapture {
    pub fn new() -> Self {
        Self {
            sink: crate::sync::Mutex::new(String::new()),
            notify: std::sync::Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Append the contents of one `print` call to the buffer,
    /// followed by a `\n` line separator.  Callers should pass the
    /// joined argument string without a trailing newline; the
    /// newline is added here, matching the line-oriented behaviour
    /// of `print`.
    ///
    /// After appending, wakes a consumer waiting on the [`notifier`]
    /// so that streaming/live capture can react to new output.
    ///
    /// [`notifier`]: PrintCapture::notifier
    pub fn write_line(&self, line: &str) {
        {
            let mut buf = self.sink.lock();
            buf.push_str(line);
            buf.push('\n');
        }
        // Release the buffer lock before waking the consumer so it
        // isn't immediately contended when it re-reads the buffer.
        self.notify.notify_one();
    }

    /// A clonable handle to the internal [`tokio::sync::Notify`].
    ///
    /// A consumer can `await` on `notifier().notified()` to be woken
    /// when new output is written.  Because a single-permit `Notify`
    /// is used, a `write_line` that occurs between a consumer's
    /// [`take`] and its next `notified().await` is not missed; the
    /// permit is stored and the subsequent wait returns immediately.
    /// Drain the buffer with [`take`] after each wake and re-await
    /// in a loop.
    ///
    /// [`take`]: PrintCapture::take
    pub fn notifier(&self) -> std::sync::Arc<tokio::sync::Notify> {
        self.notify.clone()
    }

    /// Take the captured output, leaving the buffer empty.
    pub fn take(&self) -> String {
        std::mem::take(&mut *self.sink.lock())
    }
}

impl Default for PrintCapture {
    fn default() -> Self {
        Self::new()
    }
}

// Implementation modules — not part of the public API.
pub(crate) mod lua_pattern;
pub(crate) mod math_lib;
pub(crate) mod popen;
pub(crate) mod regex_lib;
pub(crate) mod string_lib;
pub(crate) mod string_pack;
pub(crate) mod table_lib;
pub(crate) mod tokio_file;
pub(crate) mod utf8_lib;

// ---------------------------------------------------------------------------
// Re-export shingetsu_vm sub-modules for use within this crate.
// These are pub(crate) so internal stdlib modules can use short crate-relative
// paths (e.g. `crate::table::Table`) without changes.  Not public API.
// ---------------------------------------------------------------------------
pub(crate) use shingetsu_vm::{
    call_context, call_stack, convert, file, global_env, proto, table, traceback, userdata, value,
};

// ---------------------------------------------------------------------------
// Public embedder API re-exported from shingetsu-vm
// ---------------------------------------------------------------------------

// The `types` module is re-exported as a whole because proc-macro generated
// code references paths like `::shingetsu::types::ModuleTypeInfo`.
#[doc(inline)]
pub use shingetsu_vm::types;

// `diagnostics` is re-exported as a module so derive-generated code can
// reference `::shingetsu::diagnostics::render_field_suggestion(...)`.
#[doc(inline)]
pub use shingetsu_vm::diagnostics;

#[doc(inline)]
pub use shingetsu_vm::callback;
#[doc(inline)]
pub use shingetsu_vm::declare_event;

/// Lock primitives whose guards are deliberately `!Send`.
///
/// Use these in preference to `crate::sync::Mutex` / `RwLock` for
/// any state that might be accessed from an `async fn` exposed to
/// Lua.  See [`shingetsu_vm::sync`] for the rationale.
#[doc(inline)]
pub use shingetsu_vm::sync;

#[doc(inline)]
pub use shingetsu_vm::serde_bridge;
pub use shingetsu_vm::serde_ser;

#[doc(inline)]
pub use shingetsu_vm::serde_lua::SerdeLua;

#[doc(inline)]
pub use shingetsu_vm::{
    // Macro
    valuevec,
    // Userdata extension points
    BinOpSide,
    // Byte strings
    Bytes,
    // Core runtime types
    CallContext,
    // Call stack (needed for debug introspection and traceback rendering)
    CallStack,
    // Captured Lua function with its environment
    Callable,
    // Alias kept so migration code that names `LuaCallback`
    // resolves to the canonical `Callable` after the
    // `s/shingetsu_migrate::/shingetsu::` end-state rewrite.
    Callable as LuaCallback,
    FrameLocals,
    // Conversion traits
    FromLua,
    FromLuaBorrow,
    FromLuaMulti,
    // Functions
    Function,
    // Type system — most-used items; full set in `shingetsu::types`
    FunctionLuaType,
    GlobalEnv,
    GlobalTypeMap,
    IntoLua,
    IntoLuaMulti,
    // Module system
    LoadedModule,
    LuaTableShape,
    LuaType,
    LuaTyped,
    LuaTypedMulti,
    MetaMethod,
    ModuleLoader,
    // Conversion helpers
    Never,
    Number,
    // Bytecode / source info (used by diagnostic rendering)
    Proto,
    SharedRegistry,
    Snapshot,
    SnapshotValue,
    SourceLocation,
    StackFrame,
    // Data structures
    Table,
    TableField,
    TableLuaType,
    Task,
    TypedParam,
    TypedVariadic,
    Ud,
    Userdata,
    Value,
    ValueVec,
    Variadic,
    // Errors (primary surface)
    VmError,
    VmResultExt,
};

/// Error detail types for inspecting and constructing runtime errors.
///
/// [`VmError`] and [`VmResultExt`] cover the common cases; the items here
/// are for code that needs to inspect or build errors in detail.
pub mod error {
    #[doc(inline)]
    pub use shingetsu_vm::error::{Hint, RuntimeError, VarContext};
}

/// Low-level types for constructing native [`Function`] values
/// without the closure-based [`Function::wrap`] API.
pub mod function {
    #[doc(inline)]
    pub use shingetsu_vm::function::{NativeCall, NativeFunction};
}

// Items below are required by proc-macro generated code (via `#k::X`) but
// are not part of the primary embedding API.  Hidden from top-level docs;
// find the documented versions in `shingetsu::function` and `shingetsu::types`.
#[doc(hidden)]
pub use shingetsu_vm::{
    function::{NativeCall, NativeFunction},
    types::{FunctionSignature, ParamSpec, ValueType},
    value_matches_type,
};

#[doc(inline)]
pub use pretty_print::{pretty_print, PrettyPrintConfig};

/// Flush all buffered stdio output.
///
/// Call this before process exit when [`Libraries::STDIO`] has been
/// registered.  It is safe to call unconditionally — it is a no-op if
/// stdio was not registered.
#[doc(inline)]
pub use io::flush_stdio;

// Re-export the compiler under a sub-module for advanced users.
#[doc(inline)]
pub use shingetsu_compiler as compiler;

// ---------------------------------------------------------------------------
// Proc-macro support re-exports
//
// These are needed so that code generated by `shingetsu-derive` can reference
// `::shingetsu::async_trait`, `::shingetsu::bytes`, etc. without the embedder
// having to add direct dependencies on those crates.  They are hidden from
// documentation because they are not part of the embedding API.
// ---------------------------------------------------------------------------

pub use shingetsu_derive::{
    module, userdata, FromLua, FromLuaMulti, IntoLua, IntoLuaMulti, LuaRepr, LuaTyped, UserData,
};

#[doc(hidden)]
pub use async_trait;
#[doc(hidden)]
pub use bytes;
#[doc(hidden)]
pub use downcast_rs;
#[doc(hidden)]
pub use futures;
#[doc(hidden)]
pub use smallvec;

// ---------------------------------------------------------------------------
// Libraries bitflag + register_libs
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Controls which standard libraries are registered in a [`GlobalEnv`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Libraries: u32 {
        /// Core globals (print, type, pcall, …) plus math, string, table, utf8.
        const BUILTINS = 1 << 0;
        /// `os` library (os.clock, os.time, …).
        const OS       = 1 << 1;
        /// File I/O (`io.open`, `io.tmpfile`, …) plus the filesystem
        /// subset of the `os` table (`os.remove`, `os.rename`,
        /// `os.tmpname`).
        const IO       = 1 << 2;
        /// Stdio handles (`io.stdin`, `io.stdout`, `io.stderr`,
        /// `io.read`, `io.write`, `io.flush`).
        ///
        /// Call [`flush_stdio`] before process exit to ensure buffered
        /// output is flushed (safe to call unconditionally).
        const STDIO    = 1 << 3;
        /// Process execution: `io.popen` plus `os.execute`.
        const EXEC     = 1 << 4;
        /// Environment variable access: `os.getenv`.  Gated separately
        /// from `OS` because env vars routinely carry credentials and
        /// host fingerprinting data, which should require a conscious
        /// embedder opt-in independent of calendar/clock access.
        const ENV      = 1 << 5;
        /// Process termination: `os.exit`.  Gated separately from `OS`
        /// because it surrenders control of the host process; embedders
        /// who merely want the clock/calendar surface should not also
        /// be handing scripts the power to kill the process.  The
        /// function raises [`VmError::ExitRequested`] which the
        /// embedder must pattern-match and act on (the shingetsu CLI
        /// calls [`std::process::exit`]; other embedders may log,
        /// capture, or ignore it).
        const EXIT     = 1 << 6;
        /// Frame and upvalue introspection: `debug.getlocal`,
        /// `debug.getupvalue`, `debug.setupvalue`, `debug.upvalueid`.
        /// The sandbox-safe debug functions (`traceback`, `info`,
        /// `getinfo`) are always registered.
        const DEBUG    = 1 << 7;
        /// File-based `require`: enables searching `package.path`
        /// for `.lua`/`.luau` modules on the filesystem.
        const PACKAGE  = 1 << 8;
        /// `load()` function: compile and execute a string or function
        /// as a Lua chunk at runtime.  Excluded from sandboxed mode
        /// (following Luau convention) because it can execute arbitrary
        /// code from untrusted strings.
        const LOAD     = 1 << 9;
        /// Concurrent task library: `task.spawn`, `task.taskset`,
        /// `task.join`, `task.sleep`, etc., plus the `Task` and
        /// `TaskSet` userdata types and the `RuntimeError`
        /// userdata returned by `Task:pawait()`.  Spawned tasks
        /// run on the surrounding tokio runtime; embedders that
        /// don't have a tokio runtime should not enable this.
        const TASK     = 1 << 10;
        /// `regex` library: `regex.compile`, `regex.compile_bytes`,
        /// `regex.escape`, plus the `Regex`, `BytesRegex`,
        /// `Captures`, and `BytesCaptures` userdata types.
        /// Sandbox-safe: no I/O, bounded backtracking via the
        /// engine's own limits.
        const REGEX    = 1 << 11;

        /// Everything enabled except debug introspection (which
        /// requires an explicit `Libraries::DEBUG` opt-in because it
        /// exposes frame locals and upvalue mutation).
        const ALL = Self::BUILTINS.bits() | Self::OS.bits()
                  | Self::IO.bits() | Self::STDIO.bits()
                  | Self::EXEC.bits() | Self::ENV.bits()
                  | Self::EXIT.bits() | Self::PACKAGE.bits()
                  | Self::LOAD.bits() | Self::TASK.bits()
                  | Self::REGEX.bits();
        /// Sandbox-safe subset (no OS, I/O, exec, env, exit, load,
        /// or debug introspection).
        const SANDBOXED = Self::BUILTINS.bits() | Self::REGEX.bits();
    }
}

impl std::str::FromStr for Libraries {
    type Err = String;

    /// Parse a comma-separated list of library names.
    ///
    /// Names are case-insensitive and correspond to the bitflag constant
    /// names: `builtins`, `os`, `io`, `stdio`, `exec`, `env`, `exit`,
    /// `debug`, `package`, `load`, `task`, `all`, `sandboxed`.
    ///
    /// Examples: `"os,io,stdio"`, `"all"`, `"sandboxed,package"`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut libs = Libraries::empty();
        for part in s.to_ascii_lowercase().split(',') {
            let name = part.trim();
            if name.is_empty() {
                continue;
            }
            let flag = match name {
                "builtins" => Libraries::BUILTINS,
                "os" => Libraries::OS,
                "io" => Libraries::IO,
                "stdio" => Libraries::STDIO,
                "exec" => Libraries::EXEC,
                "env" => Libraries::ENV,
                "exit" => Libraries::EXIT,
                "debug" => Libraries::DEBUG,
                "package" => Libraries::PACKAGE,
                "load" => Libraries::LOAD,
                "task" => Libraries::TASK,
                "regex" => Libraries::REGEX,
                "all" => Libraries::ALL,
                "sandboxed" => Libraries::SANDBOXED,
                _ => return Err(format!("unknown library: '{name}'")),
            };
            libs |= flag;
        }
        Ok(libs)
    }
}

/// Register the requested set of standard libraries into `env`.
///
/// Implicit dependencies are handled automatically: [`Libraries::STDIO`]
/// pulls in [`Libraries::IO`] since the stdio functions live in the `io`
/// table.
///
/// Call [`flush_stdio`] before process exit to ensure buffered stdio output
/// is flushed.  It is safe to call unconditionally — it is a no-op if stdio
/// was not registered.
pub fn register_libs(env: &GlobalEnv, mut libs: Libraries) -> Result<(), VmError> {
    use std::sync::Arc;
    // Resolve implicit dependencies.
    if libs.contains(Libraries::STDIO) || libs.contains(Libraries::EXEC) {
        libs |= Libraries::IO;
    }

    if libs.contains(Libraries::BUILTINS) {
        builtins::register_sandboxed(env)?;
    }
    if libs.contains(Libraries::OS) {
        os::register(env)?;
    }
    if libs.contains(Libraries::IO) {
        io::register(env)?;
        os::register_fs(env)?;
    }
    if libs.contains(Libraries::STDIO) {
        io::register_stdio(env)?;
    }
    if libs.contains(Libraries::EXEC) {
        io::register_popen(env)?;
        os::register_exec(env)?;
    }
    if libs.contains(Libraries::ENV) {
        os::register_env(env)?;
    }
    if libs.contains(Libraries::EXIT) {
        os::register_exit(env)?;
    }
    if libs.contains(Libraries::PACKAGE) {
        // Enable file-based require with a default search path.
        // The CLI overrides this with the script's parent directory;
        // embedders can call `env.set_package_path()` to customize.
        env.set_package_path(Some("./?.lua;./?.luau".to_string()));
        env.set_module_loader(Arc::new(module_loader::LuaModuleLoader::new(
            env.global_type_map(),
        )));
    }

    if libs.contains(Libraries::LOAD) {
        builtins::register_load(env)?;
    }

    if libs.contains(Libraries::TASK) {
        task::register(env)?;
    }

    if libs.contains(Libraries::REGEX) {
        regex_lib::register(env)?;
    }

    // Sandbox-safe debug functions are always present.
    debug::register(env)?;
    if libs.contains(Libraries::DEBUG) {
        debug::register_introspection(env)?;
    }

    // Populate `loaded` cache so `require("os")` etc. works for
    // libraries registered as globals.
    for name in ["os", "io", "debug", "regex"] {
        if let Some(v) = env.get_global(name) {
            env.set_loaded(name, v);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_libs_empty_is_noop() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::empty()).expect("register");
        // Nothing registered — io and os should be absent.
        assert!(env.get_global("io").is_none());
        assert!(env.get_global("os").is_none());
    }

    #[test]
    fn register_libs_builtins_only() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::BUILTINS).expect("register");
        // Core globals present, io/os absent.
        assert!(env.get_global("print").is_some());
        assert!(env.get_global("io").is_none());
        assert!(env.get_global("os").is_none());
    }

    #[test]
    fn register_libs_stdio_implies_io() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::BUILTINS | Libraries::STDIO).expect("register");
        // STDIO should have pulled in IO.
        assert!(env.get_global("io").is_some());
    }

    #[test]
    fn register_libs_exec_implies_io() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::BUILTINS | Libraries::EXEC).expect("register");
        // EXEC should have pulled in IO.
        assert!(env.get_global("io").is_some());
    }

    #[test]
    fn register_libs_all() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::ALL).expect("register");
        assert!(env.get_global("io").is_some());
        assert!(env.get_global("os").is_some());
        assert!(env.get_global("print").is_some());
    }

    #[test]
    fn register_libs_sandboxed() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::SANDBOXED).expect("register");
        assert!(env.get_global("print").is_some());
        assert!(env.get_global("io").is_none());
        assert!(env.get_global("os").is_none());
        assert!(env.get_global("regex").is_some());
    }

    #[test]
    fn register_libs_io_without_stdio() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::BUILTINS | Libraries::IO).expect("register");
        // IO registered but no stdio handles.
        let io = env.get_global("io");
        assert!(io.is_some());
        match io {
            Some(Value::Table(t)) => {
                let stdin = t.raw_get(&Value::string("stdin")).expect("raw_get");
                k9::assert_equal!(stdin, Value::Nil);
            }
            other => panic!("expected io table, got {:?}", other),
        }
    }

    #[test]
    fn libraries_all_equals_individual_flags() {
        k9::assert_equal!(
            Libraries::ALL,
            Libraries::BUILTINS
                | Libraries::OS
                | Libraries::IO
                | Libraries::STDIO
                | Libraries::EXEC
                | Libraries::ENV
                | Libraries::EXIT
                | Libraries::PACKAGE
                | Libraries::LOAD
                | Libraries::TASK
                | Libraries::REGEX
        );
    }

    #[test]
    fn libraries_all_does_not_include_debug() {
        assert!(!Libraries::ALL.contains(Libraries::DEBUG));
    }

    #[test]
    fn debug_table_always_present() {
        // Even with empty libs, the sandbox-safe debug table is registered.
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::empty()).expect("register");
        assert!(env.get_global("debug").is_some());
    }

    #[test]
    fn debug_table_present_in_sandboxed() {
        let env = GlobalEnv::new();
        register_libs(&env, Libraries::SANDBOXED).expect("register");
        assert!(env.get_global("debug").is_some());
    }

    #[test]
    fn libraries_from_str_single() {
        let libs: Libraries = "os".parse().expect("parse");
        k9::assert_equal!(libs, Libraries::OS);
    }

    #[test]
    fn libraries_from_str_multiple() {
        let libs: Libraries = "os,io,stdio".parse().expect("parse");
        k9::assert_equal!(libs, Libraries::OS | Libraries::IO | Libraries::STDIO);
    }

    #[test]
    fn libraries_from_str_all() {
        let libs: Libraries = "all".parse().expect("parse");
        k9::assert_equal!(libs, Libraries::ALL);
    }

    #[test]
    fn libraries_from_str_sandboxed() {
        let libs: Libraries = "sandboxed".parse().expect("parse");
        k9::assert_equal!(libs, Libraries::SANDBOXED);
    }

    #[test]
    fn libraries_from_str_case_insensitive() {
        let libs: Libraries = "OS,Io,STDIO".parse().expect("parse");
        k9::assert_equal!(libs, Libraries::OS | Libraries::IO | Libraries::STDIO);
    }

    #[test]
    fn libraries_from_str_with_spaces() {
        let libs: Libraries = "os , io , stdio".parse().expect("parse");
        k9::assert_equal!(libs, Libraries::OS | Libraries::IO | Libraries::STDIO);
    }

    #[test]
    fn libraries_from_str_unknown_errors() {
        let err = "banana".parse::<Libraries>().unwrap_err();
        k9::assert_equal!(err, "unknown library: 'banana'");
    }

    #[test]
    fn libraries_from_str_empty_is_empty() {
        let libs: Libraries = "".parse().expect("parse");
        k9::assert_equal!(libs, Libraries::empty());
    }
}
