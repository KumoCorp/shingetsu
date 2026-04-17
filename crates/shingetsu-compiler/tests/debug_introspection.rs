mod common;

use shingetsu_compiler::{compile, CompileOptions};
use shingetsu_vm::{Function, GlobalEnv, Task, Value};

/// Create an env with builtins + sandbox-safe debug library + DEBUG introspection.
fn debug_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::debug_lib::register(&env).expect("register debug");
    shingetsu::debug_lib::register_introspection(&env).expect("register introspection");
    env
}

/// Compile and run a Lua snippet, returning all values.
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
// debug.getlocal
// ===========================================================================

#[test]
fn getlocal_returns_name_and_value() {
    let results = run_debug(
        r#"
local x = 42
return debug.getlocal(1, 1)
"#,
    );
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("x"));
    k9::assert_equal!(results[1], Value::Integer(42));
}

#[test]
fn getlocal_second_local() {
    let results = run_debug(
        r#"
local a = "hello"
local b = "world"
return debug.getlocal(1, 2)
"#,
    );
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("b"));
    k9::assert_equal!(results[1], Value::string("world"));
}

#[test]
fn getlocal_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
return debug.getlocal(1, 99)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn getlocal_native_frame_returns_nil() {
    // Level 0 is getlocal itself (native), should return nil.
    let results = run_debug(
        r#"
return debug.getlocal(0, 1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn getlocal_invalid_level_returns_nil() {
    let results = run_debug(
        r#"
return debug.getlocal(99, 1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn getlocal_function_param() {
    // Function parameters are locals visible in the frame.
    let results = run_debug(
        r#"
local function foo(x)
    return debug.getlocal(1, 1)
end
return foo("hello")
"#,
    );
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("x"));
    k9::assert_equal!(results[1], Value::string("hello"));
}

// ===========================================================================
// debug.getupvalue
// ===========================================================================

#[test]
fn getupvalue_captures_local() {
    let results = run_debug(
        r#"
local captured = 100
local function foo()
    return captured
end
return debug.getupvalue(foo, 1)
"#,
    );
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("captured"));
    k9::assert_equal!(results[1], Value::Integer(100));
}

#[test]
fn getupvalue_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local function foo() end
return debug.getupvalue(foo, 99)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn getupvalue_zero_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function foo() return x end
return debug.getupvalue(foo, 0)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn getupvalue_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function foo() return x end
return debug.getupvalue(foo, -1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.setupvalue
// ===========================================================================

#[test]
fn setupvalue_modifies_upvalue() {
    let results = run_debug(
        r#"
local captured = 100
local function foo()
    return captured
end
local name = debug.setupvalue(foo, 1, 999)
return name, foo()
"#,
    );
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("captured"));
    k9::assert_equal!(results[1], Value::Integer(999));
}

#[test]
fn setupvalue_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local function foo() end
return debug.setupvalue(foo, 99, "val")
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn setupvalue_shared_upvalue_visible_to_sibling() {
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
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::string("modified"));
}

// ===========================================================================
// debug.getupvalue + debug.setupvalue round-trip
// ===========================================================================

#[test]
fn getupvalue_after_setupvalue() {
    let results = run_debug(
        r#"
local val = "before"
local function foo() return val end
debug.setupvalue(foo, 1, "after")
return debug.getupvalue(foo, 1)
"#,
    );
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("val"));
    k9::assert_equal!(results[1], Value::string("after"));
}

// ===========================================================================
// debug.upvalueid
// ===========================================================================

#[test]
fn upvalueid_shared_upvalue_same_id() {
    // Two closures capturing the same variable should return the same id.
    let results = run_debug(
        r#"
local x = 0
local function inc() x = x + 1 end
local function get() return x end
return debug.upvalueid(inc, 1) == debug.upvalueid(get, 1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Boolean(true));
}

#[test]
fn upvalueid_different_upvalues_different_id() {
    // Two closures capturing different variables should return different ids.
    let results = run_debug(
        r#"
local a = 1
local b = 2
local function fa() return a end
local function fb() return b end
return debug.upvalueid(fa, 1) == debug.upvalueid(fb, 1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Boolean(false));
}

#[test]
fn upvalueid_returns_integer() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return type(debug.upvalueid(f, 1))
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::string("number"));
}

#[test]
fn upvalueid_out_of_range_returns_nil() {
    let results = run_debug(
        r#"
local function f() end
return debug.upvalueid(f, 1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn upvalueid_zero_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.upvalueid(f, 0)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn upvalueid_multiple_upvalues_distinct() {
    // A function capturing two variables should have distinct ids for each.
    let results = run_debug(
        r#"
local a = 1
local b = 2
local function f() return a + b end
return debug.upvalueid(f, 1) == debug.upvalueid(f, 2)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Boolean(false));
}

// ===========================================================================
// debug.upvalueid — native function
// ===========================================================================

#[test]
fn upvalueid_native_function_returns_nil() {
    let results = run_debug(
        r#"
return debug.upvalueid(print, 1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn upvalueid_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.upvalueid(f, -1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.getlocal — function-argument form (param names, nil values)
// ===========================================================================

#[test]
fn getlocal_function_arg_form_returns_param_name() {
    let results = run_debug(
        r#"
local function foo(a, b, c) end
return debug.getlocal(foo, 2)
"#,
    );
    k9::assert_equal!(results.len(), 2);
    k9::assert_equal!(results[0], Value::string("b"));
    k9::assert_equal!(results[1], Value::Nil);
}

#[test]
fn getlocal_function_arg_form_out_of_range() {
    let results = run_debug(
        r#"
local function foo(a) end
return debug.getlocal(foo, 5)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn getlocal_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
return debug.getlocal(1, -1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.getupvalue / debug.setupvalue — native functions
// ===========================================================================

#[test]
fn getupvalue_native_function_returns_nil() {
    let results = run_debug(
        r#"
return debug.getupvalue(print, 1)
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn setupvalue_native_function_returns_nil() {
    let results = run_debug(
        r#"
return debug.setupvalue(print, 1, "x")
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn setupvalue_zero_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.setupvalue(f, 0, "new")
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

#[test]
fn setupvalue_negative_index_returns_nil() {
    let results = run_debug(
        r#"
local x = 1
local function f() return x end
return debug.setupvalue(f, -1, "new")
"#,
    );
    k9::assert_equal!(results.len(), 1);
    k9::assert_equal!(results[0], Value::Nil);
}

// ===========================================================================
// debug.getlocal — bad first arg type
// ===========================================================================

#[test]
fn getlocal_bad_first_arg_errors() {
    let opts = CompileOptions::default();
    let bc = compile(r#"return debug.getlocal(true, 1)"#, &opts).expect("compile");
    let env = debug_env();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt.block_on(task).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'getlocal' (function or level expected)"
    );
}

// ===========================================================================
// Introspection not available without Libraries::DEBUG
// ===========================================================================

#[test]
fn introspection_not_in_sandbox_env() {
    // Build env WITHOUT register_introspection.
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    shingetsu::debug_lib::register(&env).expect("register debug");
    // debug.traceback should exist, but debug.getlocal should not.
    let opts = CompileOptions::default();
    let bc = compile(
        r#"return type(debug.traceback), type(debug.getlocal)"#,
        &opts,
    )
    .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let results = rt.block_on(task).expect("task failed");
    k9::assert_equal!(results[0], Value::string("function"));
    k9::assert_equal!(results[1], Value::string("nil"));
}
