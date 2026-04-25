#[allow(unused_imports)]
use shingetsu_compiler::{CompileOptions, Compiler};
#[allow(unused_imports)]
use shingetsu_vm::{valuevec, Function, GlobalEnv, Task, Value, ValueVec};
use std::sync::Arc;

/// Create a [`GlobalEnv`] with all builtins registered (both the VM-internal
/// ones and the macro-generated ones from `shingetsu::builtins`).
#[allow(dead_code)]
pub fn new_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    env
}

/// Create a [`GlobalEnv`] with all builtins plus `load()` registered.
#[allow(dead_code)]
pub fn new_env_with_load() -> GlobalEnv {
    let env = new_env();
    shingetsu::builtins::register_load(&env).expect("register load");
    env
}

/// Compile and run a Lua snippet, returning the first return value.
#[allow(dead_code)]
pub async fn run_one(src: &str) -> Value {
    run_all(src).await.into_iter().next().unwrap_or(Value::Nil)
}

/// Compile and run a Lua snippet, returning all return values.
#[allow(dead_code)]
pub async fn run_all(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, valuevec![])
        .await
        .expect("task failed")
}

/// Compile and run a Lua snippet, returning the error message string.
#[allow(dead_code)]
pub async fn run_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    err.to_string()
}

/// Compile and run a Lua snippet, returning the fully rendered runtime error
/// diagnostic (with source context and stack traceback).
#[allow(dead_code)]
pub async fn run_err_rendered(src: &str) -> String {
    use shingetsu::diagnostic::{render_runtime_error, RenderStyle};
    let opts = CompileOptions {
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
        type_check: false,
    };
    let compiler = Compiler::new(opts, Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    render_runtime_error(&err, RenderStyle::Plain)
}

/// Run a Lua snippet against the provided env, returning the fully rendered
/// runtime error diagnostic (with source context and stack traceback).
#[allow(dead_code)]
pub async fn run_err_with_env(env: GlobalEnv, src: &str) -> String {
    use shingetsu::diagnostic::{render_runtime_error, RenderStyle};
    let opts = CompileOptions {
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
        type_check: false,
    };
    let compiler = Compiler::new(opts, Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    render_runtime_error(&err, RenderStyle::Plain)
}

/// Run a Lua snippet against the provided env, returning all return values.
#[allow(dead_code)]
pub async fn run_with_env(env: GlobalEnv, src: &str) -> ValueVec {
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: Arc::new("@test".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, valuevec![]).await.expect("run")
}
