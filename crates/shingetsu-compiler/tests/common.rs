#[allow(unused_imports)]
use shingetsu_compiler::{CompileOptions, Compiler};
#[allow(unused_imports)]
use shingetsu_vm::{Function, GlobalEnv, Task, Value};

/// Run a future to completion, whether or not we're already inside a
/// tokio runtime (e.g. called from a `#[tokio::test]` async test).
#[allow(dead_code)]
fn block_on_compat<F: std::future::Future + Send>(f: F) -> F::Output
where
    F::Output: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // We're inside an existing runtime — spawn the future on a
            // separate thread to avoid "cannot block inside a runtime" panics.
            std::thread::scope(|s| s.spawn(|| handle.block_on(f)).join().unwrap())
        }
        Err(_) => {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            rt.block_on(f)
        }
    }
}

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
pub fn run_one(src: &str) -> Value {
    run_all(src).into_iter().next().unwrap_or(Value::Nil)
}

/// Compile and run a Lua snippet, returning all return values.
#[allow(dead_code)]
pub fn run_all(src: &str) -> Vec<Value> {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    block_on_compat(async {
        let bc = compiler.compile(src).await.expect("compile failed");
        let env = new_env();
        let func = Function::lua(bc.top_level, vec![]);
        Task::new(env, func, vec![]).await.expect("task failed")
    })
}

/// Compile and run a Lua snippet, returning the error message string.
#[allow(dead_code)]
pub fn run_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    block_on_compat(async {
        let bc = compiler.compile(src).await.expect("compile failed");
        let env = new_env();
        let func = Function::lua(bc.top_level, vec![]);
        let err = Task::new(env, func, vec![]).await.unwrap_err();
        err.to_string()
    })
}

/// Run a Lua snippet against the provided env, returning all return values.
#[allow(dead_code)]
pub fn run_with_env(env: GlobalEnv, src: &str) -> Vec<Value> {
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: "test".into(),
            type_check: false,
        },
        Default::default(),
    );
    block_on_compat(async {
        let bc = compiler.compile(src).await.expect("compile");
        let func = Function::lua(bc.top_level, vec![]);
        Task::new(env, func, vec![]).await.expect("run")
    })
}
