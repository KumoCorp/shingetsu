mod common;

use shingetsu_compiler::{compile, CompileOptions};
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
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed")
}

// ===========================================================================
// debug.info — option 's' (source)
// ===========================================================================

#[test]
fn info_s_from_main_chunk() {
    let results = run_debug("return debug.info(1, 's')");
    k9::assert_equal!(results, vec![Value::string("@<string>")]);
}

#[test]
fn info_s_from_named_function() {
    let results = run_debug(
        r#"
local function foo()
    return debug.info(1, "s")
end
return foo()
"#,
    );
    k9::assert_equal!(results, vec![Value::string("@<string>")]);
}

#[test]
fn info_s_level_zero_is_native() {
    let results = run_debug("return debug.info(0, 's')");
    k9::assert_equal!(results, vec![Value::string("=[Native]")]);
}

// ===========================================================================
// debug.info — option 'l' (currentline)
// ===========================================================================

#[test]
fn info_l_from_main_chunk() {
    let results = run_debug("return debug.info(1, 'l')");
    k9::assert_equal!(results, vec![Value::Integer(1)]);
}

#[test]
fn info_l_native_is_negative_one() {
    let results = run_debug("return debug.info(0, 'l')");
    k9::assert_equal!(results, vec![Value::Integer(-1)]);
}

// ===========================================================================
// debug.info — option 'n' (name)
// ===========================================================================

#[test]
fn info_n_from_main_chunk_is_nil() {
    let results = run_debug("return debug.info(1, 'n')");
    k9::assert_equal!(results, vec![Value::Nil]);
}

#[test]
fn info_n_from_named_function() {
    let results = run_debug(
        r#"
local function foo()
    return debug.info(1, "n")
end
return foo()
"#,
    );
    k9::assert_equal!(results, vec![Value::string("foo")]);
}

#[test]
fn info_n_native_at_level_zero() {
    let results = run_debug("return debug.info(0, 'n')");
    k9::assert_equal!(results, vec![Value::string("info")]);
}

// ===========================================================================
// debug.info — option 'a' (arity, is_vararg)
// ===========================================================================

#[test]
fn info_a_from_main_chunk() {
    // Main chunk: 0 params, variadic.
    let results = run_debug("return debug.info(1, 'a')");
    k9::assert_equal!(results, vec![Value::Integer(0), Value::Boolean(true)]);
}

#[test]
fn info_a_from_function_with_params() {
    let results = run_debug(
        r#"
local function bar(x, y, z)
    return debug.info(1, "a")
end
return bar(1, 2, 3)
"#,
    );
    k9::assert_equal!(results, vec![Value::Integer(3), Value::Boolean(false)]);
}

#[test]
fn info_a_from_variadic_function() {
    let results = run_debug(
        r#"
local function va(...)
    return debug.info(1, "a")
end
return va(1, 2)
"#,
    );
    k9::assert_equal!(results, vec![Value::Integer(0), Value::Boolean(true)]);
}

#[test]
fn info_a_native_at_level_zero() {
    // Native: 0 params, variadic.
    let results = run_debug("return debug.info(0, 'a')");
    k9::assert_equal!(results, vec![Value::Integer(0), Value::Boolean(true)]);
}

// ===========================================================================
// debug.info — combined options (ordering preserved)
// ===========================================================================

#[test]
fn info_sln_from_named_function() {
    let results = run_debug(
        r#"
local function foo()
    return debug.info(1, "sln")
end
return foo()
"#,
    );
    k9::assert_equal!(
        results,
        vec![
            Value::string("@<string>"),
            Value::Integer(3),
            Value::string("foo")
        ]
    );
}

#[test]
fn info_nls_ordering() {
    // n, l, s — ordering should match option string.
    let results = run_debug("return debug.info(1, 'nls')");
    k9::assert_equal!(
        results,
        vec![Value::Nil, Value::Integer(1), Value::string("@<string>")]
    );
}

#[test]
fn info_slna_combined() {
    let results = run_debug(
        r#"
local function two_params(a, b)
    return debug.info(1, "slna")
end
return two_params(1, 2)
"#,
    );
    k9::assert_equal!(
        results,
        vec![
            Value::string("@<string>"),
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

#[test]
fn info_out_of_range_returns_no_values() {
    let results = run_debug("return debug.info(99, 'sln')");
    k9::assert_equal!(results, vec![] as Vec<Value>);
}

// ===========================================================================
// debug.info — function argument form
// ===========================================================================

#[test]
fn info_function_arg_form() {
    let results = run_debug(
        r#"
local function typed(a: number, b: string): boolean
end
return debug.info(typed, "sna")
"#,
    );
    k9::assert_equal!(
        results,
        vec![
            Value::string("@<string>"),
            Value::string("typed"),
            Value::Integer(2),
            Value::Boolean(false),
        ]
    );
}

#[test]
fn info_function_arg_l_is_negative_one() {
    // No activation, so currentline is -1.
    let results = run_debug(
        r#"
local function f() end
return debug.info(f, "l")
"#,
    );
    k9::assert_equal!(results, vec![Value::Integer(-1)]);
}

// ===========================================================================
// debug.info — native function source from module name
// ===========================================================================

#[test]
fn info_native_function_source_from_module() {
    // Function-argument form on a native from #[module(name = "debug")]
    let results = run_debug(r#"return debug.info(debug.traceback, "sn")"#);
    k9::assert_equal!(
        results,
        vec![Value::string("=[debug]"), Value::string("traceback")]
    );
}

#[test]
fn info_builtin_function_source() {
    let results = run_debug(r#"return debug.info(print, "sn")"#);
    k9::assert_equal!(
        results,
        vec![Value::string("=[builtins]"), Value::string("print")]
    );
}

// ===========================================================================
// debug.info — error cases
// ===========================================================================

#[test]
fn info_invalid_option_errors() {
    let opts = CompileOptions::default();
    let bc = compile("return debug.info(1, 'x')", &opts).expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt.block_on(task).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to 'info' (invalid option 'x')"
    );
}

#[test]
fn info_missing_options_string_errors() {
    let opts = CompileOptions::default();
    let bc = compile("return debug.info(1)", &opts).expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt.block_on(task).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to 'info' (string expected, got nil)"
    );
}

#[test]
fn info_bad_first_arg_errors() {
    let opts = CompileOptions::default();
    let bc = compile(r#"return debug.info(true, "s")"#, &opts).expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt.block_on(task).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'info' (function | number expected, got boolean)"
    );
}

// ===========================================================================
// debug.info — float level (resolve_frame Float branch)
// ===========================================================================

#[test]
fn info_float_level_resolves_frame() {
    // 1.0 should behave identically to integer 1.
    let results = run_debug("return debug.info(1.0, 's')");
    k9::assert_equal!(results, vec![Value::string("@<string>")]);
}

// ===========================================================================
// debug.info — option 'f' (function value, currently nil)
// ===========================================================================

#[test]
fn info_f_option_returns_nil() {
    let results = run_debug("return debug.info(1, 'f')");
    k9::assert_equal!(results, vec![Value::Nil]);
}
