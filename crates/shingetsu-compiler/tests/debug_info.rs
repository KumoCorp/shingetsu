mod common;

use shingetsu::{valuevec, Libraries};
use shingetsu_vm::{GlobalEnv, Value, ValueVec};

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
// debug.info — option 's' (source)
// ===========================================================================

#[tokio::test]
async fn info_s_from_main_chunk() {
    let results = run_debug("return debug.info(1, 's')").await;
    k9::assert_equal!(results, valuevec![Value::string("@test.lua")]);
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
    k9::assert_equal!(results, valuevec![Value::string("@test.lua")]);
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
            Value::string("@test.lua"),
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
        valuevec![Value::Nil, Value::Integer(1), Value::string("@test.lua")]
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
            Value::string("@test.lua"),
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
            Value::string("@test.lua"),
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
    k9::assert_equal!(
        common::run_err_with_env(debug_env(), "return debug.info(1, 'x')").await,
        "\
error: bad argument #2 to 'info' (invalid option 'x')
 --> test.lua:1:8
  |
1 | return debug.info(1, 'x')
  |        ^^^^^^^^^^ bad argument #2 to 'info' (invalid option 'x')
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn info_missing_options_string_errors() {
    k9::assert_equal!(
        common::run_err_with_env(debug_env(), "return debug.info(1)").await,
        "\
error: bad argument #2 to 'info' (value expected, got no value)
 --> test.lua:1:8
  |
1 | return debug.info(1)
  |        ^^^^^^^^^^ bad argument #2 to 'info' (value expected, got no value)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn info_bad_first_arg_errors() {
    k9::assert_equal!(
        common::run_err_with_env(debug_env(), r#"return debug.info(true, "s")"#).await,
        "\
error: bad argument #1 to 'info' (function | number expected, got boolean)
 --> test.lua:1:8
  |
1 | return debug.info(true, \"s\")
  |        ^^^^^^^^^^ bad argument #1 to 'info' (function | number expected, got boolean)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ===========================================================================
// debug.info — float level (resolve_frame Float branch)
// ===========================================================================

#[tokio::test]
async fn info_float_level_resolves_frame() {
    // 1.0 should behave identically to integer 1.
    let results = run_debug("return debug.info(1.0, 's')").await;
    k9::assert_equal!(results, valuevec![Value::string("@test.lua")]);
}

// ===========================================================================
// debug.info — option 'f' (function value, currently nil)
// ===========================================================================

#[tokio::test]
async fn info_f_option_returns_nil() {
    let results = run_debug("return debug.info(1, 'f')").await;
    k9::assert_equal!(results, valuevec![Value::Nil]);
}
