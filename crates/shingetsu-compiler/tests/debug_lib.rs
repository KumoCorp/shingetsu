mod common;

use shingetsu::Libraries;
use shingetsu_vm::{Value, ValueVec};

// `register_libs` always installs the sandbox-safe debug functions, so
// `BUILTINS | OS` matches the surface the old `debug_env` provided.
const DEBUG_LIBS: Libraries = Libraries::BUILTINS.union(Libraries::OS);

async fn run_debug(src: &str) -> ValueVec {
    common::run_with(DEBUG_LIBS, src, |_| {})
        .await
        .unwrap_or_else(|diag| panic!("script failed:\n{diag}"))
}

/// Compile and run, returning the first value.
async fn run_debug_one(src: &str) -> Value {
    run_debug(src)
        .await
        .into_iter()
        .next()
        .unwrap_or(Value::Nil)
}

// ===========================================================================
// debug.traceback
// ===========================================================================

#[tokio::test]
async fn traceback_returns_string() {
    let val = run_debug_one("return type(debug.traceback())").await;
    k9::assert_equal!(val, Value::string("string"));
}

#[tokio::test]
async fn traceback_from_main_chunk() {
    let val = run_debug_one("return debug.traceback()").await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \ttest.lua:1: in main chunk"
        )
    );
}

#[tokio::test]
async fn traceback_with_message() {
    let val = run_debug_one(r#"return debug.traceback("oops")"#).await;
    k9::assert_equal!(
        val,
        Value::string(
            "oops\n\
            stack traceback:\n\
            \ttest.lua:1: in main chunk"
        )
    );
}

#[tokio::test]
async fn traceback_non_string_message_passthrough() {
    // Non-string, non-nil, non-numeric message is returned as-is.
    let val = run_debug_one("return debug.traceback(true)").await;
    k9::assert_equal!(val, Value::Boolean(true));
}

#[tokio::test]
async fn traceback_nil_message_no_prefix() {
    let val = run_debug_one("return debug.traceback(nil)").await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \ttest.lua:1: in main chunk"
        )
    );
}

#[tokio::test]
async fn traceback_level_skips_frames() {
    // Level 0 includes the native traceback frame and all Lua frames.
    let val = run_debug_one(
        r#"
local function inner()
    return debug.traceback(nil, 0)
end
return inner()
"#,
    )
    .await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \ttest.lua:3: in function inner()\n\
            \ttest.lua:5: in main chunk"
        )
    );
}

#[tokio::test]
async fn traceback_from_nested_call_shows_chain() {
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
    )
    .await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \ttest.lua:3: in function a()\n\
            \ttest.lua:6: in function b()\n\
            \ttest.lua:8: in main chunk"
        )
    );
}

#[tokio::test]
async fn traceback_typed_function_shows_signature() {
    let val = run_debug_one(
        r#"
local function add(x: number, y: number): number
    return debug.traceback()
end
return add(1, 2)
"#,
    )
    .await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \ttest.lua:3: in function add(x: number, y: number): number\n\
            \ttest.lua:5: in main chunk"
        )
    );
}

#[tokio::test]
async fn traceback_default_level_is_one() {
    // Default level=1 skips the native traceback frame.
    let val = run_debug_one("return debug.traceback()").await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \ttest.lua:1: in main chunk"
        )
    );
}

#[tokio::test]
async fn traceback_level_zero_includes_traceback_frame() {
    let val = run_debug_one("return debug.traceback(nil, 0)").await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \ttest.lua:1: in main chunk"
        )
    );
}

// Gap #1: numeric-only first arg (level as integer, no message).
#[tokio::test]
async fn traceback_integer_first_arg_is_level() {
    let val = run_debug_one("return debug.traceback(0)").await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \ttest.lua:1: in main chunk"
        )
    );
}

// Gap #1b: float first arg treated as level.
#[tokio::test]
async fn traceback_float_first_arg_is_level() {
    let val = run_debug_one("return debug.traceback(0.0)").await;
    k9::assert_equal!(
        val,
        Value::string(
            "stack traceback:\n\
            \t[Native]: in function traceback\n\
            \ttest.lua:1: in main chunk"
        )
    );
}
