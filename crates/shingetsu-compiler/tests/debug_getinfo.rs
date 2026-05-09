use std::sync::Arc;
mod common;

use shingetsu::{valuevec, Libraries};
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{GlobalEnv, Task, Value, ValueVec};

const DEBUG_LIBS: Libraries = Libraries::BUILTINS.union(Libraries::OS);

fn debug_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, DEBUG_LIBS).expect("register libs");
    env
}

async fn run_debug(src: &str) -> ValueVec {
    common::run_with(DEBUG_LIBS, src, |_| {})
        .await
        .unwrap_or_else(|diag| panic!("script failed:\n{diag}"))
}

// ===========================================================================
// debug.getinfo — default what from main chunk
// ===========================================================================

#[tokio::test]
async fn getinfo_default_what_from_main() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1)
return t.source, t.what, t.linedefined, t.lastlinedefined,
       t.currentline, t.name, t.nparams, t.isvararg, t.nups, t.istailcall
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("@test.lua"),
            Value::string("main"),
            Value::Integer(0),
            Value::Integer(5),
            Value::Integer(2),
            Value::Nil,
            Value::Integer(0),
            Value::Boolean(true),
            Value::Integer(1),
            Value::Boolean(false),
        ]
    );
}

// ===========================================================================
// debug.getinfo — 'S' option
// ===========================================================================

#[tokio::test]
async fn getinfo_s_from_named_function() {
    let results = run_debug(
        r#"
local function foo(x, y)
    local t = debug.getinfo(1, "S")
    return t.source, t.what, t.linedefined, t.lastlinedefined
end
return foo(1, 2)
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("@test.lua"),
            Value::string("Lua"),
            Value::Integer(2),
            Value::Integer(5),
        ]
    );
}

#[tokio::test]
async fn getinfo_s_main_chunk_what_is_main() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "S")
return t.what
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::string("main")]);
}

// ===========================================================================
// debug.getinfo — 'n' option
// ===========================================================================

#[tokio::test]
async fn getinfo_n_from_named_function() {
    let results = run_debug(
        r#"
local function bar()
    local t = debug.getinfo(1, "n")
    return t.name, t.namewhat
end
return bar()
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::string("bar"), Value::string("")]);
}

#[tokio::test]
async fn getinfo_n_from_main_chunk_name_is_nil() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "n")
return t.name
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

// ===========================================================================
// debug.getinfo — 'l' option
// ===========================================================================

#[tokio::test]
async fn getinfo_l_currentline() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "l")
return t.currentline
"#,
    )
    .await;
    // Line 2 is where `debug.getinfo(1, "l")` executes.
    k9::assert_equal!(results, valuevec![Value::Integer(2)]);
}

// ===========================================================================
// debug.getinfo — 't' option
// ===========================================================================

#[tokio::test]
async fn getinfo_t_istailcall() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "t")
return t.istailcall
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Boolean(false)]);
}

// ===========================================================================
// debug.getinfo — 'u' option
// ===========================================================================

#[tokio::test]
async fn getinfo_u_from_function_with_upvalues() {
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
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(2), Value::Integer(3), Value::Boolean(false)]
    );
}

#[tokio::test]
async fn getinfo_u_variadic_function() {
    let results = run_debug(
        r#"
local function va(...)
    local t = debug.getinfo(1, "u")
    return t.nparams, t.isvararg
end
return va(1, 2)
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(0), Value::Boolean(true)]);
}

// ===========================================================================
// debug.getinfo — level 0 (native getinfo itself)
// ===========================================================================

#[tokio::test]
async fn getinfo_level_zero_is_native() {
    let results = run_debug(
        r#"
local t = debug.getinfo(0, "Sn")
return t.source, t.what, t.name
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("=[Native]"),
            Value::string("Native"),
            Value::string("getinfo"),
        ]
    );
}

// ===========================================================================
// debug.getinfo — out of range returns nil
// ===========================================================================

#[tokio::test]
async fn getinfo_out_of_range_returns_nil() {
    let results = run_debug("return debug.getinfo(99)").await;
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

// ===========================================================================
// debug.getinfo — function argument form
// ===========================================================================

#[tokio::test]
async fn getinfo_function_arg_form() {
    let results = run_debug(
        r#"
local function typed(a: number): string end
local t = debug.getinfo(typed, "Snu")
return t.source, t.what, t.name, t.nparams, t.isvararg,
       t.linedefined, t.lastlinedefined
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("@test.lua"),
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

#[tokio::test]
async fn getinfo_l_upper_activelines_is_table() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "L")
return type(t.activelines)
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::string("table")]);
}

// ===========================================================================
// debug.getinfo — native frame with 'u' option
// ===========================================================================

#[tokio::test]
async fn getinfo_u_native_frame() {
    let results = run_debug(
        r#"
local t = debug.getinfo(0, "u")
return t.nups, t.nparams, t.isvararg
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(0), Value::Integer(0), Value::Boolean(true)]
    );
}

// ===========================================================================
// debug.getinfo — short_src field
// ===========================================================================

#[tokio::test]
async fn getinfo_short_src_matches_source() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1, "S")
return t.short_src, t.source
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::string("test.lua"), Value::string("@test.lua")]
    );
}

// ===========================================================================
// debug.getinfo — float level
// ===========================================================================

#[tokio::test]
async fn getinfo_float_level() {
    let results = run_debug(
        r#"
local t = debug.getinfo(1.0, "S")
return t.what
"#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::string("main")]);
}

// ===========================================================================
// debug.getinfo — bad first arg type
// ===========================================================================

#[tokio::test]
async fn getinfo_bad_first_arg_errors() {
    k9::assert_equal!(
        common::run_err_with_env(debug_env(), r#"return debug.getinfo(true, "S")"#).await,
        "\
error: bad argument #1 to 'getinfo' (function | number expected, got boolean)
 --> test.lua:1:8
  |
1 | return debug.getinfo(true, \"S\")
  |        ^^^^^^^^^^^^^ bad argument #1 to 'getinfo' (function | number expected, got boolean)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ===========================================================================
// debug.getinfo — error cases
// ===========================================================================

#[tokio::test]
async fn getinfo_short_src_strips_at_prefix() {
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: true,
            source_name: Arc::new("@myfile.lua".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler
        .compile(
            r#"
local t = debug.getinfo(1, "S")
return t.short_src, t.source
"#,
        )
        .await
        .expect("compile failed");
    let env = debug_env();
    let func = bc.into_function();
    let results = Task::new(env, func, valuevec![])
        .await
        .expect("task failed");
    k9::assert_equal!(
        results,
        valuevec![Value::string("myfile.lua"), Value::string("@myfile.lua")]
    );
}

#[tokio::test]
async fn getinfo_invalid_what_option_errors() {
    k9::assert_equal!(
        common::run_err_with_env(debug_env(), "return debug.getinfo(1, 'x')").await,
        "\
error: bad argument #2 to 'getinfo' (invalid option 'x')
 --> test.lua:1:8
  |
1 | return debug.getinfo(1, 'x')
  |        ^^^^^^^^^^^^^ bad argument #2 to 'getinfo' (invalid option 'x')
stack traceback:
\ttest.lua:1: in main chunk"
    );
}
