#[allow(unused_imports)]
use shingetsu_compiler::{CompileOptions, Compiler};
#[allow(unused_imports)]
use shingetsu_vm::{Function, GlobalEnv, Task, Value};

/// Create a [`GlobalEnv`] with all builtins registered (both the VM-internal
/// ones and the macro-generated ones from `shingetsu::builtins`).
#[allow(dead_code)]
pub fn new_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    env
}

/// Compile and run a Lua snippet, returning the first return value.
#[allow(dead_code)]
pub async fn run_one(src: &str) -> Value {
    run_all(src).await.into_iter().next().unwrap_or(Value::Nil)
}

/// Compile and run a Lua snippet, returning all return values.
#[allow(dead_code)]
pub async fn run_all(src: &str) -> Vec<Value> {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, vec![]).await.expect("task failed")
}

/// Compile and run a Lua snippet, returning the error message string.
#[allow(dead_code)]
pub async fn run_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    err.to_string()
}

/// Run a Lua snippet against the provided env, returning all return values.
#[allow(dead_code)]
pub async fn run_with_env(env: GlobalEnv, src: &str) -> Vec<Value> {
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: "test".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler.compile(src).await.expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, vec![]).await.expect("run")
}
