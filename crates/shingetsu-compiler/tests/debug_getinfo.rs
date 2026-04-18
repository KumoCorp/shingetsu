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
    let bc = compiler.compile(src).expect("compile failed");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed")
}

// ===========================================================================
// debug.getinfo — default what from main chunk
// ===========================================================================

#[test]
fn getinfo_default_what_from_main() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1)
return t.source, t.what, t.linedefined, t.lastlinedefined,
       t.currentline, t.name, t.nparams, t.isvararg, t.nups, t.istailcall
"#,
    );
    k9::assert_equal!(
        results,
        vec![
            Value::string("@<string>"),
            Value::string("main"),
            Value::Integer(0),
            Value::Integer(5),
            Value::Integer(2),
            Value::Nil,
            Value::Integer(0),
            Value::Boolean(true),
            Value::Integer(0),
            Value::Boolean(false),
        ]
    );
}

// ===========================================================================
// debug.getinfo — 'S' option
// ===========================================================================

#[test]
fn getinfo_s_from_named_function() {
    let results = run_debug(
        r#"
local function foo(x, y)
    local t = debug.getinfo(1, "S")
    return t.source, t.what, t.linedefined, t.lastlinedefined
end
return foo(1, 2)
"#,
    );
    k9::assert_equal!(
        results,
        vec![
            Value::string("@<string>"),
            Value::string("Lua"),
            Value::Integer(2),
            Value::Integer(5),
        ]
    );
}

#[test]
fn getinfo_s_main_chunk_what_is_main() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "S")
return t.what
"#,
    );
    k9::assert_equal!(results, vec![Value::string("main")]);
}

// ===========================================================================
// debug.getinfo — 'n' option
// ===========================================================================

#[test]
fn getinfo_n_from_named_function() {
    let results = run_debug(
        r#"
local function bar()
    local t = debug.getinfo(1, "n")
    return t.name, t.namewhat
end
return bar()
"#,
    );
    k9::assert_equal!(results, vec![Value::string("bar"), Value::string("")]);
}

#[test]
fn getinfo_n_from_main_chunk_name_is_nil() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "n")
return t.name
"#,
    );
    k9::assert_equal!(results, vec![Value::Nil]);
}

// ===========================================================================
// debug.getinfo — 'l' option
// ===========================================================================

#[test]
fn getinfo_l_currentline() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "l")
return t.currentline
"#,
    );
    // Line 2 is where `debug.getinfo(1, "l")` executes.
    k9::assert_equal!(results, vec![Value::Integer(2)]);
}

// ===========================================================================
// debug.getinfo — 't' option
// ===========================================================================

#[test]
fn getinfo_t_istailcall() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "t")
return t.istailcall
"#,
    );
    k9::assert_equal!(results, vec![Value::Boolean(false)]);
}

// ===========================================================================
// debug.getinfo — 'u' option
// ===========================================================================

#[test]
fn getinfo_u_from_function_with_upvalues() {
    let results = run_debug(
        r#"
local x = 1
local function f(a, b, c)
    local _ = x
    local t = debug.getinfo(1, "u")
    return t.nups, t.nparams, t.isvararg
end
return f(1, 2, 3)
"#,
    );
    k9::assert_equal!(
        results,
        vec![Value::Integer(1), Value::Integer(3), Value::Boolean(false)]
    );
}

#[test]
fn getinfo_u_variadic_function() {
    let results = run_debug(
        r#"
local function va(...)
    local t = debug.getinfo(1, "u")
    return t.nparams, t.isvararg
end
return va(1, 2)
"#,
    );
    k9::assert_equal!(results, vec![Value::Integer(0), Value::Boolean(true)]);
}

// ===========================================================================
// debug.getinfo — level 0 (native getinfo itself)
// ===========================================================================

#[test]
fn getinfo_level_zero_is_native() {
    let results = run_debug(
        r#"
local t = debug.getinfo(0, "Sn")
return t.source, t.what, t.name
"#,
    );
    k9::assert_equal!(
        results,
        vec![
            Value::string("=[Native]"),
            Value::string("Native"),
            Value::string("getinfo"),
        ]
    );
}

// ===========================================================================
// debug.getinfo — out of range returns nil
// ===========================================================================

#[test]
fn getinfo_out_of_range_returns_nil() {
    let results = run_debug("return debug.getinfo(99)");
    k9::assert_equal!(results, vec![Value::Nil]);
}

// ===========================================================================
// debug.getinfo — function argument form
// ===========================================================================

#[test]
fn getinfo_function_arg_form() {
    let results = run_debug(
        r#"
local function typed(a: number): string end
local t = debug.getinfo(typed, "Snu")
return t.source, t.what, t.name, t.nparams, t.isvararg,
       t.linedefined, t.lastlinedefined
"#,
    );
    k9::assert_equal!(
        results,
        vec![
            Value::string("@<string>"),
            Value::string("Lua"),
            Value::string("typed"),
            Value::Integer(1),
            Value::Boolean(false),
            Value::Integer(2),
            Value::Integer(2),
        ]
    );
}

// ===========================================================================
// debug.getinfo — 'L' option (activelines)
// ===========================================================================

#[test]
fn getinfo_l_upper_activelines_is_table() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "L")
return type(t.activelines)
"#,
    );
    k9::assert_equal!(results, vec![Value::string("table")]);
}

// ===========================================================================
// debug.getinfo — native frame with 'u' option
// ===========================================================================

#[test]
fn getinfo_u_native_frame() {
    let results = run_debug(
        r#"
local t = debug.getinfo(0, "u")
return t.nups, t.nparams, t.isvararg
"#,
    );
    k9::assert_equal!(
        results,
        vec![Value::Integer(0), Value::Integer(0), Value::Boolean(true)]
    );
}

// ===========================================================================
// debug.getinfo — short_src field
// ===========================================================================

#[test]
fn getinfo_short_src_matches_source() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "S")
return t.short_src, t.source
"#,
    );
    k9::assert_equal!(
        results,
        vec![Value::string("@<string>"), Value::string("@<string>")]
    );
}

// ===========================================================================
// debug.getinfo — float level
// ===========================================================================

#[test]
fn getinfo_float_level() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1.0, "S")
return t.what
"#,
    );
    k9::assert_equal!(results, vec![Value::string("main")]);
}

// ===========================================================================
// debug.getinfo — bad first arg type
// ===========================================================================

#[test]
fn getinfo_bad_first_arg_errors() {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(r#"return debug.getinfo(true, "S")"#)
        .expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt.block_on(task).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'getinfo' (function | number expected, got boolean)"
    );
}

// ===========================================================================
// debug.getinfo — error cases
// ===========================================================================

#[test]
fn getinfo_invalid_what_option_errors() {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile("return debug.getinfo(1, 'x')")
        .expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt.block_on(task).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to 'getinfo' (invalid option 'x')"
    );
}
