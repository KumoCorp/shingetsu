// `common.rs` is shared by every integration-test crate via `mod common;`,
// so individual test files only exercise a subset of these helpers.  The
// file-level allow keeps that from producing per-crate dead-code /
// unused-import warnings without requiring an annotation on every item.
#![allow(dead_code, unused_imports, unused_macros)]
use futures::executor::block_on;
use similar::TextDiff;

use shingetsu::diagnostic::{
    assert_diagnostics, render_compile_error, render_runtime_error, render_warnings, RenderStyle,
};
use shingetsu::lint_plugin::{load_plugin, LoadedPlugins};
use shingetsu::Libraries;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{
    valuevec, Function, GlobalEnv, GlobalTypeMap, SharedRegistry, Task, Value, ValueVec,
};
use std::sync::Arc;
use tempfile::NamedTempFile;

/// CompileOptions used by every test helper.  Debug info is on (so
/// rendered diagnostics carry source context and a useful traceback)
/// and the source name is `@test.lua` — both matching what real
/// embedders configure, so test assertions show what users will see.
pub fn test_compile_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
        type_check: false,
    }
}

/// Build a [`GlobalEnv`] populated with the requested [`Libraries`].
pub fn build_env(libs: Libraries) -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, libs).expect("register libs");
    env
}

/// Core run helper.
///
/// Builds a fresh env populated with `libs`, runs `fixup` on it (for
/// tests that need to register additional functions or globals), then
/// compiles and executes `src`.
///
/// On success returns the full result vector.  On failure returns the
/// fully rendered runtime error diagnostic — including source context
/// and stack traceback — so tests can assert on the complete output.
pub async fn run_with(
    libs: Libraries,
    src: &str,
    fixup: impl FnOnce(&GlobalEnv),
) -> Result<ValueVec, String> {
    run_with_keep_env(libs, src, fixup).await.1
}

/// Like [`run_with`] but also returns the [`GlobalEnv`] so tests can
/// inspect it after the run completes (e.g. checking GC / finalizer
/// behaviour by reading shared `Arc` strong counts).
pub async fn run_with_keep_env(
    libs: Libraries,
    src: &str,
    fixup: impl FnOnce(&GlobalEnv),
) -> (GlobalEnv, Result<ValueVec, String>) {
    let env = build_env(libs);
    fixup(&env);
    let result = run_in_env(&env, src).await.map_err(|e| render(&e));
    (env, result)
}

/// Compile and run `src` against an existing env, returning either the
/// result vector or the structured runtime error.  Panics with the
/// rendered compile-error diagnostic if the script fails to compile.
pub async fn run_in_env(
    env: &GlobalEnv,
    src: &str,
) -> Result<ValueVec, shingetsu_vm::error::RuntimeError> {
    let bc = compile_or_panic(env, src).await;
    let func = bc.into_function();
    Task::new(env.clone(), func, valuevec![]).await
}

/// Compile `src` against `env`, panicking with a rendered compile-error
/// diagnostic if compilation fails.
pub async fn compile_or_panic(env: &GlobalEnv, src: &str) -> shingetsu_compiler::Bytecode {
    let compiler = Compiler::new(test_compile_opts(), env.global_type_map());
    match compiler.compile(src).await {
        Ok(bc) => bc,
        Err(err) => panic!(
            "compile failed:\n{}",
            render_compile_error(&err, src, RenderStyle::Plain)
        ),
    }
}

/// Render a [`RuntimeError`] to the plain-style diagnostic string used
/// throughout the test suite.
fn render(err: &shingetsu_vm::error::RuntimeError) -> String {
    render_runtime_error(err, RenderStyle::Plain)
}

// ---------------------------------------------------------------------------
// Convenience wrappers — builtins-only success/failure paths.
// ---------------------------------------------------------------------------

/// Compile and run a Lua snippet, returning the first return value.
pub async fn run_one(src: &str) -> Value {
    run_all(src).await.into_iter().next().unwrap_or(Value::Nil)
}

/// Compile and run a Lua snippet, returning all return values.
pub async fn run_all(src: &str) -> ValueVec {
    run_with_env(new_env(), src).await
}

// ---------------------------------------------------------------------------
// Env constructors — for tests that need a pre-built env (gc.rs,
// native_calls.rs) before the script even compiles.
// ---------------------------------------------------------------------------

/// Build a [`GlobalEnv`] with all builtins registered (matches the old
/// `shingetsu::builtins::register` surface: builtins + os).
pub fn new_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    env
}

/// Build a [`GlobalEnv`] with builtins + `load()` registered.
pub fn new_env_with_load() -> GlobalEnv {
    let env = new_env();
    shingetsu::builtins::register_load(&env).expect("register load");
    env
}

/// Build a [`GlobalEnv`] with builtins + the `task` module registered,
/// and a fresh [`SharedRegistry`] installed so named `task.mutex` /
/// `task.rwlock` / etc. created in this env do not leak into
/// (or collide with) other tests running in parallel.
pub fn task_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    let installed = env.install_shared_registry(Arc::new(SharedRegistry::new()));
    assert!(installed, "freshly constructed env must accept registry");
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::debug::register(&env).expect("register debug");
    shingetsu::task::register(&env).expect("register task");
    env
}

/// Run a pre-built env against `src`, panicking on error with the full
/// rendered diagnostic.
pub async fn run_with_env(env: GlobalEnv, src: &str) -> ValueVec {
    match run_in_env(&env, src).await {
        Ok(vv) => vv,
        Err(err) => panic!("script failed:\n{}", render(&err)),
    }
}

/// Compile `src` against a fresh builtins-only env, asserting that
/// compilation must fail and returning the fully rendered compile-error
/// diagnostic (with source context, caret, and any help message).
pub async fn compile_err(src: &str) -> String {
    compile_err_with_env(&new_env(), src).await
}

/// Compile `src` against a pre-built env, asserting that compilation
/// must fail and returning the fully rendered compile-error diagnostic.
pub async fn compile_err_with_env(env: &GlobalEnv, src: &str) -> String {
    let compiler = Compiler::new(test_compile_opts(), env.global_type_map());
    match compiler.compile(src).await {
        Ok(_) => panic!("expected compile error, got success"),
        Err(err) => render_compile_error(&err, src, RenderStyle::Plain),
    }
}

/// Compile `src` against a fresh builtins-only env and return the rendered
/// non-fatal diagnostics (warnings + lint-style errors collected during
/// compilation, as opposed to fatal `CompileError`s).
pub async fn compile_diagnostics(src: &str) -> String {
    compile_diagnostics_with_env(&new_env(), src).await
}

/// Like [`compile_diagnostics`] but uses a pre-built env so tests can
/// register custom global types / event registrars / event signatures
/// before compilation.  Type checking is enabled so global-type-driven
/// diagnostics fire.  Lint directives in the source are honoured.
pub async fn compile_diagnostics_with_env(env: &GlobalEnv, src: &str) -> String {
    compile_diagnostics_with_globals(env.global_type_map(), src).await
}

/// Compile `src` against a synthesized [`GlobalTypeMap`] (no
/// `GlobalEnv` needed) and return the fully rendered non-fatal
/// diagnostics.  Tests that exercise the type checker against
/// hand-built global types (e.g. `derive(LuaRepr)` round-trips,
/// `--types`-style scenarios) use this to skip env / library
/// setup entirely.  Type checking is enabled and lint directives
/// in the source are honoured.
pub async fn compile_diagnostics_with_globals(globals: GlobalTypeMap, src: &str) -> String {
    let opts = CompileOptions {
        type_check: true,
        ..test_compile_opts()
    };
    let compiler = Compiler::new(opts, globals);
    let bc = compiler.compile(src).await.expect("compile");
    let filtered = bc.lint_directives.filter(bc.diagnostics);
    render_warnings(&filtered, src, RenderStyle::Plain)
}

/// Build a `LuaType::Function` shape suitable for registering as a
/// global in tests.  Free of generics and metadata flags; callers
/// supply just the named parameter types and the return types.
///
/// Use when a test needs to teach the type checker about a host
/// function (e.g. `some_func(cfg: HasDeprecatedField) ->
/// HasDeprecatedField`) without standing up a full module.
pub fn function_type(
    params: &[(&str, shingetsu_vm::types::LuaType)],
    returns: Vec<shingetsu_vm::types::LuaType>,
) -> shingetsu_vm::types::LuaType {
    use shingetsu_vm::types::{FunctionLuaType, TypedParam};
    shingetsu_vm::types::LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params: params
            .iter()
            .map(|(n, t)| TypedParam::new(Some(*n), t.clone()))
            .collect(),
        variadic: None,
        returns,
        is_method: false,
        inferred_unannotated: false,
        deprecated: None,
        must_use: None,
    }))
}

// ---------------------------------------------------------------------------
// Type-checking helpers (used by type_check_*.rs test files).
// ---------------------------------------------------------------------------

/// Shared sync core: compile `src` with `type_map` and the given flags,
/// then assert the resulting diagnostics match `expected`.
#[track_caller]
fn compile_and_check(
    type_map: GlobalTypeMap,
    src: &str,
    expected: &str,
    type_check: bool,
    filter: bool,
) {
    let opts = CompileOptions {
        type_check,
        ..test_compile_opts()
    };
    let compiler = Compiler::new(opts, type_map);
    let bc = block_on(compiler.compile(src)).expect("compile");
    let diags = if filter {
        bc.lint_directives.filter(bc.diagnostics)
    } else {
        bc.diagnostics
    };
    assert_diagnostics(&diags, src, expected);
}

#[track_caller]
pub fn type_check(src: &str, expected: &str) {
    compile_and_check(Default::default(), src, expected, true, false);
}

#[track_caller]
pub fn type_check_with_builtins(src: &str, expected: &str) {
    compile_and_check(
        build_env(Libraries::ALL).global_type_map(),
        src,
        expected,
        true,
        false,
    );
}

#[track_caller]
pub fn type_check_filtered(src: &str, expected: &str) {
    compile_and_check(Default::default(), src, expected, true, true);
}

macro_rules! assert_multi_line_output {
    ($actual:expr, $expected:expr $(,)?) => {
        common::assert_multi_line_output!($actual, $expected, "output mismatch")
    };
    ($actual:expr, $expected:expr, $msg:expr $(,)?) => {{
        let __actual: &str = ($actual).as_ref();
        let __expected: &str = ($expected).as_ref();
        if __actual != __expected {
            let __diff = ::similar::TextDiff::from_lines(__expected, __actual);
            panic!(
                "{}:\n\nexpected:\n{}\nactual:\n{}\ndiff:\n{}\n",
                $msg,
                __expected,
                __actual,
                __diff
                    .unified_diff()
                    .context_radius(3)
                    .missing_newline_hint(false)
                    .header("expected", "actual")
            );
        }
    }};
}
pub(crate) use assert_multi_line_output;

macro_rules! assert_runtime_error {
    ($src:expr, $expected:expr $(,)?) => {
        common::assert_runtime_error_with_env!(common::new_env(), $src, $expected)
    };
}
pub(crate) use assert_runtime_error;

macro_rules! assert_runtime_error_with_env {
    // Path sugar: `path_expr => "PLACEHOLDER"` expands to a replace closure.
    ($env:expr, $src:expr, $expected:expr, $path:expr => $placeholder:expr $(,)?) => {
        common::assert_runtime_error_with_env!($env, $src, $expected, |__s: &str| __s
            .replace(&($path).display().to_string(), $placeholder),)
    };
    // Explicit normalize function: applied to rendered output before comparing.
    ($env:expr, $src:expr, $expected:expr, $normalize:expr $(,)?) => {{
        let __err = common::run_in_env(&$env, $src)
            .await
            .expect_err("expected a runtime error");
        let __rendered = ::shingetsu::diagnostic::render_runtime_error(
            &__err,
            ::shingetsu::diagnostic::RenderStyle::Plain,
        );
        let __normalized = ($normalize)(&__rendered);
        common::assert_multi_line_output!(&__normalized, $expected, "error output mismatch");
    }};
    // No normalization: delegate to normalize arm with identity.
    ($env:expr, $src:expr, $expected:expr $(,)?) => {
        common::assert_runtime_error_with_env!($env, $src, $expected, |__s: &str| __s.to_owned())
    };
}
pub(crate) use assert_runtime_error_with_env;

#[track_caller]
pub fn compile_diag(src: &str, expected: &str) {
    compile_and_check(Default::default(), src, expected, false, false);
}

// ---------------------------------------------------------------------------
// Lint-plugin test helpers
// ---------------------------------------------------------------------------

/// Write `contents` to a temporary file and return the handle.
/// The file is deleted when the handle is dropped.
pub fn write_temp_file(contents: &str) -> NamedTempFile {
    use std::io::Write;
    let mut file = NamedTempFile::new().expect("tempfile");
    file.write_all(contents.as_bytes()).expect("write");
    file.flush().expect("flush");
    file
}

/// Load `$plugin_src` as a lint plugin, compile `$test_src` through the
/// full compiler pipeline (type_check=true), dispatch all plugin events,
/// and assert the rendered warning diagnostics equal `$expected`.  The
/// plugin tempfile path is replaced with `<plugin>` before comparison.
/// Prints a unified diff on mismatch.
macro_rules! assert_plugin_diagnostics {
    ($plugin_src:expr, $test_src:expr, $expected:expr $(,)?) => {{
        let __plugin = common::write_temp_file($plugin_src);
        let __loaded = shingetsu::lint_plugin::LoadedPlugins::load_from_paths(
            &[__plugin.path()],
            None,
        )
        .await
        .expect("load plugin");
        let __opts = shingetsu_compiler::CompileOptions {
            type_check: true,
            ..common::test_compile_opts()
        };
        let __compiler = shingetsu_compiler::Compiler::new(
            __opts,
            shingetsu_vm::GlobalEnv::new().global_type_map(),
        );
        let __compiled = __compiler
            .compile_with_ast($test_src)
            .await
            .expect("compile target");
        let __lint_ir = __compiled
            .lint_ir
            .expect("lint_ir must be Some when type_check=true");
        let __source_name = ::std::sync::Arc::new("@test.lua".to_string());
        let __diags = __loaded
            .lint_chunk(__source_name, &__lint_ir)
            .await
            .expect("dispatch");
        let __rendered = shingetsu::diagnostic::render_warnings(
            &__diags,
            $test_src,
            shingetsu::diagnostic::RenderStyle::Plain,
        );
        let __normalized = __rendered.replace(&__plugin.path().display().to_string(), "<plugin>");
        let __expected_owned = $expected;
        let __expected: &str = __expected_owned.as_ref();
        common::assert_multi_line_output!(&__normalized, __expected, "plugin diagnostics mismatch");
    }};
}
pub(crate) use assert_plugin_diagnostics;

/// Assert that loading a lint plugin from `$plugin_src` fails with the
/// rendered error matching `$expected`.  The tempfile path is replaced
/// with `<plugin>` before comparison, matching the `<plugin>` tokens
/// you write in the expected string.  Prints a unified diff on mismatch.
macro_rules! assert_plugin_load_error {
    ($plugin_src:expr, $expected:expr $(,)?) => {{
        let __plugin = common::write_temp_file($plugin_src);
        let __env = shingetsu::lint_plugin::new_plugin_env().expect("new plugin env");
        let __err = shingetsu::lint_plugin::load_plugin(&__env, __plugin.path())
            .await
            .expect_err("expected plugin load to fail");
        let __normalized = __err.replace(&__plugin.path().display().to_string(), "<plugin>");
        let __expected_owned = $expected;
        let __expected: &str = __expected_owned.as_ref();
        common::assert_multi_line_output!(&__normalized, __expected, "plugin load error mismatch");
    }};
}
pub(crate) use assert_plugin_load_error;
