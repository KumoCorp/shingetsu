// Allow proc-macro generated code (which references `::shingetsu::*`) to
// resolve within this crate.
extern crate self as shingetsu;

// Re-export the VM public API so embedders only need to depend on `shingetsu`.
pub use shingetsu_vm::*;

pub mod builtins;
pub mod debug_lib;
pub mod diagnostic;
pub mod io_lib;
pub mod lua_pattern;
pub mod math_lib;
pub mod module_loader;
pub mod os_lib;
pub mod popen;
pub mod project_config;
pub mod string_lib;
pub mod string_pack;
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
        /// File I/O (`io.open`, `io.tmpfile`, …) plus the filesystem
        /// subset of the `os` table (`os.remove`, `os.rename`,
        /// `os.tmpname`).
        const IO       = 1 << 2;
        /// Stdio handles (`io.stdin`, `io.stdout`, `io.stderr`,
        /// `io.read`, `io.write`, `io.flush`).
        ///
        /// Call [`io_lib::flush_stdio`] before process exit to ensure
        /// buffered output is flushed (safe to call unconditionally).
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

        /// Everything enabled except debug introspection (which
        /// requires an explicit `Libraries::DEBUG` opt-in because it
        /// exposes frame locals and upvalue mutation).
        const ALL = Self::BUILTINS.bits() | Self::OS.bits()
                  | Self::IO.bits() | Self::STDIO.bits()
                  | Self::EXEC.bits() | Self::ENV.bits()
                  | Self::EXIT.bits() | Self::PACKAGE.bits()
                  | Self::LOAD.bits();
        /// Sandbox-safe subset (no OS, I/O, exec, env, exit, load,
        /// or debug introspection).
        const SANDBOXED = Self::BUILTINS.bits();
    }
}

impl std::str::FromStr for Libraries {
    type Err = String;

    /// Parse a comma-separated list of library names.
    ///
    /// Names are case-insensitive and correspond to the bitflag constant
    /// names: `builtins`, `os`, `io`, `stdio`, `exec`, `env`, `exit`,
    /// `debug`, `package`, `load`, `all`, `sandboxed`.
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
/// Call [`io_lib::flush_stdio`] before process exit to ensure
/// buffered stdio output is flushed.  It is safe to call
/// unconditionally — it is a no-op if stdio was not registered.
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
        os_lib::register(env)?;
    }
    if libs.contains(Libraries::IO) {
        io_lib::register(env)?;
        os_lib::register_fs(env)?;
    }
    if libs.contains(Libraries::STDIO) {
        io_lib::register_stdio(env)?;
    }
    if libs.contains(Libraries::EXEC) {
        io_lib::register_popen(env)?;
        os_lib::register_exec(env)?;
    }
    if libs.contains(Libraries::ENV) {
        os_lib::register_env(env)?;
    }
    if libs.contains(Libraries::EXIT) {
        os_lib::register_exit(env)?;
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

    // Sandbox-safe debug functions are always present.
    debug_lib::register(env)?;
    if libs.contains(Libraries::DEBUG) {
        debug_lib::register_introspection(env)?;
    }

    // Populate `loaded` cache so `require("os")` etc. works for
    // libraries registered as globals.
    for name in ["os", "io", "debug"] {
        if let Some(v) = env.get_global(name) {
            env.set_loaded(name, v);
        }
    }

    Ok(())
}

// Re-export downcast_rs so proc-macro generated code can reference
// `::shingetsu::downcast_rs::impl_downcast!` without the embedder having
// to add a direct dependency on downcast-rs.
pub use downcast_rs;

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

// Re-export proc macros so users only need `shingetsu` as a dependency.
pub use shingetsu_derive::{
    module, userdata, FromLua, FromLuaMulti, IntoLua, IntoLuaMulti, LuaTable, LuaTyped, UserData,
};

// Re-export external crates referenced in proc-macro generated code so that
// generated code can use `::shingetsu::bytes` / `::shingetsu::async_trait`
// without the embedder needing direct dependencies on those crates.
pub use async_trait;
pub use bytes;
