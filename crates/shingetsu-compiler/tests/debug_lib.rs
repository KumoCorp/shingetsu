mod common;

use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{Function, GlobalEnv, Task, Value};

/// Create an env with builtins + sandbox-safe debug library.
fn debug_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::debug_lib::register(&env).expect("register debug");
    env
}

/// Compile and run a Lua snippet with debug library, returning all values.
fn run_debug(src: &str) -> Vec<Value> {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = tokio::runtime::Runtime::new()
        .expect("rt")
        .block_on(compiler.compile(src))
        .expect("compile failed");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed")
}

/// Compile and run, returning the first value.
fn run_debug_one(src: &str) -> Value {
    run_debug(src).into_iter().next().unwrap_or(Value::Nil)
}

// ===========================================================================
// debug.traceback
// ===========================================================================

#[test]
fn traceback_returns_string() {
    let val = run_debug_one("return type(debug.traceback())");
    k9::assert_equal!(val, Value::string("string"));
}

#[test]
fn traceback_from_main_chunk() {
    let val = run_debug_one("return debug.traceback()");
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t<string>:1: in main chunk"
        )
    );
}

#[test]
fn traceback_with_message() {
    let val = run_debug_one(r#"return debug.traceback("oops")"#);
    k9::assert_equal!(
        val,
        Value::string(
            "oops\n\
            stack traceback:\n\
            \t<string>:1: in main chunk"
        )
    );
}

#[test]
fn traceback_non_string_message_passthrough() {
    // Non-string, non-nil, non-numeric message is returned as-is.
    let val = run_debug_one("return debug.traceback(true)");
    k9::assert_equal!(val, Value::Boolean(true));
}

#[test]
fn traceback_nil_message_no_prefix() {
    let val = run_debug_one("return debug.traceback(nil)");
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t<string>:1: in main chunk"
        )
    );
}

#[test]
fn traceback_level_skips_frames() {
    // Level 0 includes the native traceback frame and all Lua frames.
    let val = run_debug_one(
        r#"
local function inner()
    return debug.traceback(nil, 0)
end
return inner()
"#,
    );
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \t<string>:3: in function inner()\n\
            \t<string>:5: in main chunk"
        )
    );
}

#[test]
fn traceback_from_nested_call_shows_chain() {
    let val = run_debug_one(
        r#"
local function a()
    return debug.traceback()
end
local function b()
    return a()
end
return b()
"#,
    );
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t<string>:3: in function a()\n\
            \t<string>:6: in function b()\n\
            \t<string>:8: in main chunk"
        )
    );
}

#[test]
fn traceback_typed_function_shows_signature() {
    let val = run_debug_one(
        r#"
local function add(x: number, y: number): number
    return debug.traceback()
end
return add(1, 2)
"#,
    );
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t<string>:3: in function add(x: number, y: number): number\n\
            \t<string>:5: in main chunk"
        )
    );
}

#[test]
fn traceback_default_level_is_one() {
    // Default level=1 skips the native traceback frame.
    let val = run_debug_one("return debug.traceback()");
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t<string>:1: in main chunk"
        )
    );
}

#[test]
fn traceback_level_zero_includes_traceback_frame() {
    let val = run_debug_one("return debug.traceback(nil, 0)");
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \t<string>:1: in main chunk"
        )
    );
}

// Gap #1: numeric-only first arg (level as integer, no message).
#[test]
fn traceback_integer_first_arg_is_level() {
    let val = run_debug_one("return debug.traceback(0)");
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \t<string>:1: in main chunk"
        )
    );
}

// Gap #1b: float first arg treated as level.
#[test]
fn traceback_float_first_arg_is_level() {
    let val = run_debug_one("return debug.traceback(0.0)");
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \t<string>:1: in main chunk"
        )
    );
}
