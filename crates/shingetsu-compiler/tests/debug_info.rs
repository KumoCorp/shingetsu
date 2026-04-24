mod common;

use shingetsu::valuevec;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{Function, GlobalEnv, Task, Value, ValueVec};

/// Create an env with builtins + sandbox-safe debug library.
fn debug_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::debug_lib::register(&env).expect("register debug");
    env
}

/// Compile and run a Lua snippet with debug library, returning all values.
async fn run_debug(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    task.await.expect("task failed")
}

// ===========================================================================
// debug.info — option 's' (source)
// ===========================================================================

#[tokio::test]
async fn info_s_from_main_chunk() {
    let results = run_debug("return debug.info(1, 's')").await;
    k9::assert_equal!(results, valuevec![Value::string("=<string>")]);
}

#[tokio::test]
async fn info_s_from_named_function() {
    let results = run_debug(
        r#"
local function foo()
    return debug.info(1, "s")
end
return foo()
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::string("=<string>")]);
}

#[tokio::test]
async fn info_s_level_zero_is_native() {
    let results = run_debug("return debug.info(0, 's')").await;
    k9::assert_equal!(results, valuevec![Value::string("=[Native]")]);
}

// ===========================================================================
// debug.info — option 'l' (currentline)
// ===========================================================================

#[tokio::test]
async fn info_l_from_main_chunk() {
    let results = run_debug("return debug.info(1, 'l')").await;
    k9::assert_equal!(results, valuevec![Value::Integer(1)]);
}

#[tokio::test]
async fn info_l_native_is_negative_one() {
    let results = run_debug("return debug.info(0, 'l')").await;
    k9::assert_equal!(results, valuevec![Value::Integer(-1)]);
}

// ===========================================================================
// debug.info — option 'n' (name)
// ===========================================================================

#[tokio::test]
async fn info_n_from_main_chunk_is_nil() {
    let results = run_debug("return debug.info(1, 'n')").await;
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn info_n_from_named_function() {
    let results = run_debug(
        r#"
local function foo()
    return debug.info(1, "n")
end
return foo()
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::string("foo")]);
}

#[tokio::test]
async fn info_n_native_at_level_zero() {
    let results = run_debug("return debug.info(0, 'n')").await;
    k9::assert_equal!(results, valuevec![Value::string("info")]);
}

// ===========================================================================
// debug.info — option 'a' (arity, is_vararg)
// ===========================================================================

#[tokio::test]
async fn info_a_from_main_chunk() {
    // Main chunk: 0 params, variadic.
    let results = run_debug("return debug.info(1, 'a')").await;
    k9::assert_equal!(results, valuevec![Value::Integer(0), Value::Boolean(true)]);
}

#[tokio::test]
async fn info_a_from_function_with_params() {
    let results = run_debug(
        r#"
local function bar(x, y, z)
    return debug.info(1, "a")
end
return bar(1, 2, 3)
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(3), Value::Boolean(false)]);
}

#[tokio::test]
async fn info_a_from_variadic_function() {
    let results = run_debug(
        r#"
local function va(...)
    return debug.info(1, "a")
end
return va(1, 2)
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(0), Value::Boolean(true)]);
}

#[tokio::test]
async fn info_a_native_at_level_zero() {
    // Native: 0 params, variadic.
    let results = run_debug("return debug.info(0, 'a')").await;
    k9::assert_equal!(results, valuevec![Value::Integer(0), Value::Boolean(true)]);
}

// ===========================================================================
// debug.info — combined options (ordering preserved)
// ===========================================================================

#[tokio::test]
async fn info_sln_from_named_function() {
    let results = run_debug(
        r#"
local function foo()
    return debug.info(1, "sln")
end
return foo()
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("=<string>"),
            Value::Integer(3),
            Value::string("foo")
        ]
    );
}

#[tokio::test]
async fn info_nls_ordering() {
    // n, l, s — ordering should match option string.
    let results = run_debug("return debug.info(1, 'nls')").await;
    k9::assert_equal!(
        results,
        valuevec![Value::Nil, Value::Integer(1), Value::string("=<string>")]
    );
}

#[tokio::test]
async fn info_slna_combined() {
    let results = run_debug(
        r#"
local function two_params(a, b)
    return debug.info(1, "slna")
end
return two_params(1, 2)
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("=<string>"),
            Value::Integer(3),
            Value::string("two_params"),
            Value::Integer(2),
            Value::Boolean(false),
        ]
    );
}

// ===========================================================================
// debug.info — level out of range
// ===========================================================================

#[tokio::test]
async fn info_out_of_range_returns_no_values() {
    let results = run_debug("return debug.info(99, 'sln')").await;
    k9::assert_equal!(results, valuevec![]);
}

// ===========================================================================
// debug.info — function argument form
// ===========================================================================

#[tokio::test]
async fn info_function_arg_form() {
    let results = run_debug(
        r#"
local function typed(a: number, b: string): boolean
end
return debug.info(typed, "sna")
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("=<string>"),
            Value::string("typed"),
            Value::Integer(2),
            Value::Boolean(false),
        ]
    );
}

#[tokio::test]
async fn info_function_arg_l_is_negative_one() {
    // No activation, so currentline is -1.
    let results = run_debug(
        r#"
local function f() end
return debug.info(f, "l")
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(-1)]);
}

// ===========================================================================
// debug.info — native function source from module name
// ===========================================================================

#[tokio::test]
async fn info_native_function_source_from_module() {
    // Function-argument form on a native from #[module(name = "debug")]
    let results = run_debug(r#"return debug.info(debug.traceback, "sn")"#).await;
    k9::assert_equal!(
        results,
        valuevec![Value::string("=[debug]"), Value::string("traceback")]
    );
}

#[tokio::test]
async fn info_builtin_function_source() {
    let results = run_debug(r#"return debug.info(print, "sn")"#).await;
    k9::assert_equal!(
        results,
        valuevec![Value::string("=[builtins]"), Value::string("print")]
    );
}

// ===========================================================================
// debug.info — error cases
// ===========================================================================

#[tokio::test]
async fn info_invalid_option_errors() {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile("return debug.info(1, 'x')")
        .await
        .expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    let err = task.await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to 'info' (invalid option 'x')"
    );
}

#[tokio::test]
async fn info_missing_options_string_errors() {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile("return debug.info(1)")
        .await
        .expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    let err = task.await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to 'info' (value expected, got no value)"
    );
}

#[tokio::test]
async fn info_bad_first_arg_errors() {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(r#"return debug.info(true, "s")"#)
        .await
        .expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    let err = task.await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'info' (function | number expected, got boolean)"
    );
}

// ===========================================================================
// debug.info — float level (resolve_frame Float branch)
// ===========================================================================

#[tokio::test]
async fn info_float_level_resolves_frame() {
    // 1.0 should behave identically to integer 1.
    let results = run_debug("return debug.info(1.0, 's')").await;
    k9::assert_equal!(results, valuevec![Value::string("=<string>")]);
}

// ===========================================================================
// debug.info — option 'f' (function value, currently nil)
// ===========================================================================

#[tokio::test]
async fn info_f_option_returns_nil() {
    let results = run_debug("return debug.info(1, 'f')").await;
    k9::assert_equal!(results, valuevec![Value::Nil]);
}
