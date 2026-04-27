// `common.rs` is shared by every integration-test crate via `mod common;`,
// so individual test files only exercise a subset of these helpers.  The
// file-level allow keeps that from producing per-crate dead-code /
// unused-import warnings without requiring an annotation on every item.
#![allow(dead_code, unused_imports)]

use shingetsu::diagnostic::{render_compile_error, render_runtime_error, RenderStyle};
use shingetsu::Libraries;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{valuevec, Function, GlobalEnv, Task, Value, ValueVec};
use std::sync::Arc;

/// CompileOptions used by every test helper.  Debug info is on (so
/// rendered diagnostics carry source context and a useful traceback)
/// and the source name is `@test.lua` — both matching what real
/// embedders configure, so test assertions show what users will see.
fn test_compile_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
        type_check: false,
    }
}

/// Build a [`GlobalEnv`] populated with the requested [`Libraries`].
fn build_env(libs: Libraries) -> GlobalEnv {
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
    let env = build_env(libs);
    fixup(&env);
    run_in_env(&env, src).await.map_err(|e| render(&e))
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
    let func = Function::lua(bc.top_level, vec![]);
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

/// Compile and run a Lua snippet, returning the fully rendered runtime
/// error diagnostic (with source context and stack traceback).
pub async fn run_err(src: &str) -> String {
    run_err_with_env(new_env(), src).await
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

/// Run a pre-built env against `src`, panicking on error with the full
/// rendered diagnostic.
pub async fn run_with_env(env: GlobalEnv, src: &str) -> ValueVec {
    match run_in_env(&env, src).await {
        Ok(vv) => vv,
        Err(err) => panic!("script failed:\n{}", render(&err)),
    }
}

/// Run a pre-built env against `src`, returning the fully rendered
/// runtime error diagnostic.
pub async fn run_err_with_env(env: GlobalEnv, src: &str) -> String {
    match run_in_env(&env, src).await {
        Ok(vv) => panic!("expected error, got: {vv:?}"),
        Err(err) => render(&err),
    }
}
