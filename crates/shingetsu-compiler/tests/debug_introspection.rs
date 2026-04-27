mod common;

use shingetsu::Libraries;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{valuevec, Function, GlobalEnv, Task, Value, ValueVec};

const DEBUG_LIBS: Libraries = Libraries::BUILTINS
    .union(Libraries::OS)
    .union(Libraries::DEBUG);

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
// debug.getlocal
// ===========================================================================

#[tokio::test]
async fn getlocal_returns_name_and_value() {
    let results = run_debug(
        r#"
local x = 42
return debug.getlocal(1, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("x"));
    k9::assert_equal!(results[1], Value::Integer(42));
}

#[tokio::test]
async fn getlocal_second_local() {
    let results = run_debug(
        r#"
local a = "hello"
local b = "world"
return debug.getlocal(1, 2)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("b"));
    k9::assert_equal!(results[1], Value::string("world"));
}

#[tokio::test]
async fn getlocal_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
return debug.getlocal(1, 99)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn getlocal_native_frame_returns_nil() {
    // Level 0 is getlocal itself (native), should return nil.
    let results = run_debug(
        r#"
return debug.getlocal(0, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn getlocal_invalid_level_returns_nil() {
    let results = run_debug(
        r#"
return debug.getlocal(99, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn getlocal_function_param() {
    // Function parameters are locals visible in the frame.
    let results = run_debug(
        r#"
local function foo(x)
    return debug.getlocal(1, 1)
end
return foo("hello")
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("x"));
    k9::assert_equal!(results[1], Value::string("hello"));
}

// ===========================================================================
// debug.getupvalue
// ===========================================================================

#[tokio::test]
async fn getupvalue_captures_local() {
    let results = run_debug(
        r#"
local captured = 100
local function foo()
    return captured
end
return debug.getupvalue(foo, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("captured"));
    k9::assert_equal!(results[1], Value::Integer(100));
}

#[tokio::test]
async fn getupvalue_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local function foo() end
return debug.getupvalue(foo, 99)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn getupvalue_zero_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function foo() return x end
return debug.getupvalue(foo, 0)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn getupvalue_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function foo() return x end
return debug.getupvalue(foo, -1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.setupvalue
// ===========================================================================

#[tokio::test]
async fn setupvalue_modifies_upvalue() {
    let results = run_debug(
        r#"
local captured = 100
local function foo()
    return captured
end
local name = debug.setupvalue(foo, 1, 999)
return name, foo()
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("captured"));
    k9::assert_equal!(results[1], Value::Integer(999));
}

#[tokio::test]
async fn setupvalue_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local function foo() end
return debug.setupvalue(foo, 99, "val")
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn setupvalue_shared_upvalue_visible_to_sibling() {
    // Two closures sharing the same upvalue; setupvalue on one should
    // be visible when calling the other.
    let results = run_debug(
        r#"
local shared = "original"
local function reader() return shared end
local function writer() shared = "unused" end
debug.setupvalue(writer, 1, "modified")
return reader()
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::string("modified"));
}

// ===========================================================================
// debug.getupvalue + debug.setupvalue round-trip
// ===========================================================================

#[tokio::test]
async fn getupvalue_after_setupvalue() {
    let results = run_debug(
        r#"
local val = "before"
local function foo() return val end
debug.setupvalue(foo, 1, "after")
return debug.getupvalue(foo, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("val"));
    k9::assert_equal!(results[1], Value::string("after"));
}

// ===========================================================================
// debug.upvalueid
// ===========================================================================

#[tokio::test]
async fn upvalueid_shared_upvalue_same_id() {
    // Two closures capturing the same variable should return the same id.
    let results = run_debug(
        r#"
local x = 0
local function inc() x = x + 1 end
local function get() return x end
return debug.upvalueid(inc, 1) == debug.upvalueid(get, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Boolean(true));
}

#[tokio::test]
async fn upvalueid_different_upvalues_different_id() {
    // Two closures capturing different variables should return different ids.
    let results = run_debug(
        r#"
local a = 1
local b = 2
local function fa() return a end
local function fb() return b end
return debug.upvalueid(fa, 1) == debug.upvalueid(fb, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Boolean(false));
}

#[tokio::test]
async fn upvalueid_returns_integer() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return type(debug.upvalueid(f, 1))
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::string("number"));
}

#[tokio::test]
async fn upvalueid_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local function f() end
return debug.upvalueid(f, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn upvalueid_zero_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.upvalueid(f, 0)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn upvalueid_multiple_upvalues_distinct() {
    // A function capturing two variables should have distinct ids for each.
    let results = run_debug(
        r#"
local a = 1
local b = 2
local function f() return a + b end
return debug.upvalueid(f, 1) == debug.upvalueid(f, 2)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Boolean(false));
}

// ===========================================================================
// debug.upvalueid — native function
// ===========================================================================

#[tokio::test]
async fn upvalueid_native_function_returns_nil() {
    let results = run_debug(
        r#"
return debug.upvalueid(print, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn upvalueid_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.upvalueid(f, -1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.getlocal — function-argument form (param names, nil values)
// ===========================================================================

#[tokio::test]
async fn getlocal_function_arg_form_returns_param_name() {
    let results = run_debug(
        r#"
local function foo(a, b, c) end
return debug.getlocal(foo, 2)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("b"));
    k9::assert_equal!(results[1], Value::Nil);
}

#[tokio::test]
async fn getlocal_function_arg_form_out_of_range() {
    let results = run_debug(
        r#"
local function foo(a) end
return debug.getlocal(foo, 5)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn getlocal_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
return debug.getlocal(1, -1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.getupvalue / debug.setupvalue — native functions
// ===========================================================================

#[tokio::test]
async fn getupvalue_native_function_returns_nil() {
    let results = run_debug(
        r#"
return debug.getupvalue(print, 1)
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn setupvalue_native_function_returns_nil() {
    let results = run_debug(
        r#"
return debug.setupvalue(print, 1, "x")
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn setupvalue_zero_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.setupvalue(f, 0, "new")
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[tokio::test]
async fn setupvalue_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.setupvalue(f, -1, "new")
"#,
    )
    .await;
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.getlocal — bad first arg type
// ===========================================================================

#[tokio::test]
async fn getlocal_bad_first_arg_errors() {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(r#"return debug.getlocal(true, 1)"#)
        .await
        .expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    let err = task.await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'getlocal' (function | number expected, got boolean)"
    );
}

// ===========================================================================
// Introspection not available without Libraries::DEBUG
// ===========================================================================

#[tokio::test]
async fn introspection_not_in_sandbox_env() {
    // Build env WITHOUT register_introspection.
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::debug_lib::register(&env).expect("register debug");
    // debug.traceback should exist, but debug.getlocal should not.
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(r#"return type(debug.traceback), type(debug.getlocal)"#)
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    let results = task.await.expect("task failed");
    k9::assert_equal!(results[0], Value::string("function"));
    k9::assert_equal!(results[1], Value::string("nil"));
}
