use std::sync::Arc;
mod common;

use common::{new_env, run_err, run_one, run_with_env};
use shingetsu_vm::Value;

// error / assert / pcall / xpcall
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pcall_success() {
    k9::assert_equal!(
        run_one("local ok, v = pcall(function() return 42 end) return ok").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn pcall_success_result() {
    k9::assert_equal!(
        run_one("local ok, v = pcall(function() return 42 end) return v").await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn pcall_error_caught() {
    k9::assert_equal!(
        run_one(
            "local ok, msg = pcall(function() error('boom') end)
return ok"
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn pcall_error_message() {
    k9::assert_equal!(
        run_one(
            "local ok, msg = pcall(function() error('boom') end)
return msg"
        )
        .await,
        Value::string(
            "\
test.lua:1: boom"
        )
    );
}

#[tokio::test]
async fn pcall_error_value() {
    // error() can throw any value; pcall preserves it.
    k9::assert_equal!(
        run_one(
            "local ok, v = pcall(function() error(99) end)
return v"
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn pcall_nested() {
    // Inner pcall catches its error; outer pcall succeeds.
    k9::assert_equal!(
        run_one(
            "local function inner()
    local ok, msg = pcall(function() error('inner') end)
    return ok
end
local ok, v = pcall(inner)
return v"
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn assert_pass() {
    k9::assert_equal!(run_one("return assert(42)").await, Value::Integer(42));
}

#[tokio::test]
async fn assert_fail() {
    k9::assert_equal!(
        run_one(
            "local ok, msg = pcall(function() assert(false, 'bad') end)
return msg"
        )
        .await,
        Value::string("bad")
    );
}

#[tokio::test]
async fn xpcall_success() {
    k9::assert_equal!(
        run_one(
            "local ok, v = xpcall(function() return 7 end, function(e) return 'handled' end)
return ok"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn xpcall_handler_called() {
    k9::assert_equal!(
        run_one(
            "local ok, v = xpcall(
    function() error('oops') end,
    function(e) return 'caught: ' .. e end
)
return v"
        )
        .await,
        Value::string(
            "\
caught: test.lua:2: oops"
        )
    );
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// error() level argument
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_level_zero_no_position() {
    // level=0: message is passed through unchanged.
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(function()
    error("raw msg", 0)
end)
return err"#
        )
        .await,
        Value::string("raw msg")
    );
}

#[tokio::test]
async fn error_level_default_string() {
    // Default level=1: error value is still a string (may have position prefix).
    // We just check it contains the original message.
    let result = run_one(
        r#"local ok, err = pcall(function()
    error("boom")
end)
return type(err)"#,
    )
    .await;
    k9::assert_equal!(result, Value::string("string"));
}

#[tokio::test]
async fn error_non_string_preserved() {
    // Non-string errors are returned as-is regardless of level.
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(function()
    error(42)
end)
return err"#
        )
        .await,
        Value::Integer(42)
    );
}

// ---------------------------------------------------------------------------
// BadArgument context fixup tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bad_argument_context_module_function_arg1() {
    // Passing the wrong type to argument #1 of a module function surfaces
    // the correct position and function name via with_arg_and_call_context.
    use shingetsu::{module, valuevec, Function, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};

    #[module]
    mod ctx_test {
        #[function]
        fn greet(name: String) -> String {
            format!("hello {name}")
        }
    }

    let env = new_env();
    ctx_test::register_global_module(&env).expect("register");
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: Arc::new("@test".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    // Pass a boolean where a string is expected.
    let bc = compiler
        .compile("return ctx_test.greet(true)")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'greet' (string expected, got boolean)"
    );
}

#[tokio::test]
async fn bad_argument_context_module_function_arg2() {
    // Position tracking: the error should say #2 for the second argument.
    use shingetsu::{module, valuevec, Function, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};

    #[module]
    mod ctx_test2 {
        #[function]
        fn add(a: i64, b: i64) -> i64 {
            a + b
        }
    }

    let env = new_env();
    ctx_test2::register_global_module(&env).expect("register");
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: Arc::new("@test".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    // First arg is fine, second arg is wrong type.
    let bc = compiler
        .compile("return ctx_test2.add(1, 'oops')")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to 'add' (number expected, got string)"
    );
}

#[tokio::test]
async fn bad_argument_context_userdata_method() {
    // Userdata method dispatch also gets the correct function name and
    // argument position via the proc-macro generated fixup.
    use shingetsu::{userdata, valuevec, Function, Task, Value};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use std::sync::Arc;

    struct Acc(i64);

    #[userdata]
    impl Acc {
        #[lua_method]
        fn add(&self, n: i64) -> i64 {
            self.0 + n
        }
    }

    let env = new_env();
    env.set_global("acc", Value::Userdata(Arc::new(Acc(10))));
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: Arc::new("@test".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    // Pass a table where an integer is expected.
    let bc = compiler
        .compile("return acc:add({})")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'add' (number expected, got table)"
    );
}

#[tokio::test]
async fn bad_argument_context_require() {
    // The hand-written require() builtin uses FromLuaMulti + with_arg_and_call_context.
    use shingetsu::{valuevec, Function, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};

    let env = new_env();
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: Arc::new("@test".to_string()),
            type_check: false,
        },
        Default::default(),
    );
    // Pass a number where a string is expected.
    let bc = compiler.compile("require(42)").await.expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'require' (string expected, got number)"
    );
}

#[tokio::test]
async fn bad_argument_context_tuple_return_type_mismatch() {
    // A module function returns (i64, i64) but Lua-side we try to extract
    // the result as (i64, String) via FromLuaMulti.  The second element
    // should produce a BadArgument with position 2.
    use shingetsu::FromLuaMulti;

    let env = new_env();
    // divmod returns two integers; try to unpack the second as String.
    let res = run_with_env(env, "return 10, 42").await;
    let err = <(i64, String)>::from_lua_multi(res.into()).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to '' (string expected, got number)"
    );
}

#[tokio::test]
async fn require_via_register_global_and_preload() {
    // register_global_module exposes the module as a global AND
    // register_preload makes it require()-able; both work independently.
    use shingetsu::{module, Value};

    #[module(name = "util")]
    mod util_impl {
        #[function]
        fn double(n: i64) -> i64 {
            n * 2
        }
    }

    let env = new_env();
    // Register both ways.
    util_impl::register_global_module(&env).expect("global");
    util_impl::register_preload(&env);

    // Direct global access.
    let res = run_with_env(env.clone(), "return util.double(3)").await;
    k9::assert_equal!(res[0], Value::Integer(6));

    // require() access — different table instance but same functions.
    let res = run_with_env(env, "local u = require('util'); return u.double(5)").await;
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Contextual error messages — variable names in errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_index_nil_global() {
    k9::assert_equal!(
        run_err("return nil_global.field").await,
        "\
error: attempt to index global 'nil_global' (a nil value) with key 'field'
 --> test.lua:1:1
  |
1 | return nil_global.field
  | ^^^^^^^^^^^^^^^^^^^^^^^ attempt to index global 'nil_global' (a nil value) with key 'field'
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_index_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil
            return x.field"
        )
        .await,
        "\
error: attempt to index local 'x' (a nil value) with key 'field'
 --> test.lua:2:13
  |
1 | local x = nil
  |       - defined here
2 |             return x.field
  |             ^^^^^^^^^^^^^^ attempt to index local 'x' (a nil value) with key 'field'
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_call_nil_global() {
    k9::assert_equal!(
        run_err("nil_global()").await,
        "\
error: attempt to call global 'nil_global' (a nil value)
 --> test.lua:1:1
  |
1 | nil_global()
  | ^^^^^^^^^^^^ attempt to call global 'nil_global' (a nil value)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_call_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local f = nil
            f()"
        )
        .await,
        "\
error: attempt to call local 'f' (a nil value)
 --> test.lua:2:13
  |
1 | local f = nil
  |       - defined here
2 |             f()
  |             ^^^ attempt to call local 'f' (a nil value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_call_number() {
    k9::assert_equal!(
        run_err(
            "\
            local n = 42
            n()"
        )
        .await,
        "\
error: attempt to call local 'n' (a number value)
 --> test.lua:2:13
  |
1 | local n = 42
  |       - defined here
2 |             n()
  |             ^^^ attempt to call local 'n' (a number value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_index_number_local() {
    k9::assert_equal!(
        run_err(
            "\
            local n = 42
            return n.field"
        )
        .await,
        "\
error: attempt to index local 'n' (a number value) with key 'field'
 --> test.lua:2:13
  |
1 | local n = 42
  |       - defined here
2 |             return n.field
  |             ^^^^^^^^^^^^^^ attempt to index local 'n' (a number value) with key 'field'
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_index_boolean_local() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true
            return b.field"
        )
        .await,
        "\
error: attempt to index local 'b' (a boolean value) with key 'field'
 --> test.lua:2:13
  |
1 | local b = true
  |       - defined here
2 |             return b.field
  |             ^^^^^^^^^^^^^^ attempt to index local 'b' (a boolean value) with key 'field'
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_method_on_nil_global() {
    // obj:method() desugars to GetTable + Call; the error should mention
    // the object being indexed.
    k9::assert_equal!(
        run_err("nil_global:some_method()").await,
        "\
error: attempt to index global 'nil_global' (a nil value) with key 'some_method'
 --> test.lua:1:1
  |
1 | nil_global:some_method()
  | ^^^^^^^^^^^^^^^^^^^^^^^^ attempt to index global 'nil_global' (a nil value) with key 'some_method'
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_index_without_name() {
    // When the value comes from an expression rather than a named variable,
    // we fall back to the type-only message.
    k9::assert_equal!(
        run_err("return (nil).field").await,
        "\
error: attempt to index a nil value with key 'field'
 --> test.lua:1:1
  |
1 | return (nil).field
  | ^^^^^^^^^^^^^^^^^^ attempt to index a nil value with key 'field'
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Length operator error messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_length_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return #x"
        )
        .await,
        "\
error: attempt to get length of local 'x' (a nil value)
 --> test.lua:2:8
  |
1 | local x = nil
  |       - defined here
2 | return #x
  |        ^^ attempt to get length of local 'x' (a nil value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_length_boolean_local() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true\n\
            return #b"
        )
        .await,
        "\
error: attempt to get length of local 'b' (a boolean value)
 --> test.lua:2:8
  |
1 | local b = true
  |       - defined here
2 | return #b
  |        ^^ attempt to get length of local 'b' (a boolean value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_length_number_local() {
    k9::assert_equal!(
        run_err(
            "\
            local n = 42\n\
            return #n"
        )
        .await,
        "\
error: attempt to get length of local 'n' (a number value)
 --> test.lua:2:8
  |
1 | local n = 42
  |       - defined here
2 | return #n
  |        ^^ attempt to get length of local 'n' (a number value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_length_nil_global() {
    k9::assert_equal!(
        run_err("return #nil_global").await,
        "\
error: attempt to get length of global 'nil_global' (a nil value)
 --> test.lua:1:8
  |
1 | return #nil_global
  |        ^^^^^^^^^^^ attempt to get length of global 'nil_global' (a nil value)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_length_no_name() {
    k9::assert_equal!(
        run_err("return #true").await,
        "\
error: attempt to get length of a boolean value
 --> test.lua:1:8
  |
1 | return #true
  |        ^^^^^ attempt to get length of a boolean value
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Table key errors (nil / NaN)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_table_key_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local t = {}\n\
            t[nil] = 1"
        )
        .await,
        "\
error: table index is nil (table is local 't')
 --> test.lua:2:1
  |
1 | local t = {}
  |       - defined here
2 | t[nil] = 1
  | ^^^^^^^^^^ table index is nil (table is local 't')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_table_key_nil_global() {
    k9::assert_equal!(
        run_err(
            "\
            g = {}\n\
            g[nil] = 1"
        )
        .await,
        "\
error: table index is nil (table is global 'g')
 --> test.lua:2:1
  |
2 | g[nil] = 1
  | ^^^^^^^^^^ table index is nil (table is global 'g')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_table_key_nil_no_name() {
    k9::assert_equal!(
        run_err("({})[ nil] = 1").await,
        "\
error: table index is nil
 --> test.lua:1:1
  |
1 | ({})[ nil] = 1
  | ^^^^^^^^^^^^^^ table index is nil
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_table_key_nan() {
    k9::assert_equal!(
        run_err(
            "\
            local t = {}\n\
            t[0/0] = 1"
        )
        .await,
        "\
error: table index is NaN (table is local 't')
 --> test.lua:2:3
  |
1 | local t = {}
  |       - defined here
2 | t[0/0] = 1
  |   ^^^ table index is NaN (table is local 't')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Arithmetic error messages with variable names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_arith_local_nil() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x + 1"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 'x' (a nil value)
 --> test.lua:2:8
  |
1 | local x = nil
  |       - defined here
2 | return x + 1
  |        ^^^^^ attempt to perform arithmetic on local 'x' (a nil value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_arith_global_nil() {
    k9::assert_equal!(
        run_err("return g + 1").await,
        "\
error: attempt to perform arithmetic on global 'g' (a nil value)
 --> test.lua:1:8
  |
1 | return g + 1
  |        ^^^^^ attempt to perform arithmetic on global 'g' (a nil value)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_arith_string_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s - 1"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 's' (a string value)
 --> test.lua:2:8
  |
1 | local s = 'hello'
  |       - defined here
2 | return s - 1
  |        ^^^^^ attempt to perform arithmetic on local 's' (a string value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_arith_rhs_is_bad() {
    // When the left operand is fine but the right is not, name the right.
    k9::assert_equal!(
        run_err(
            "\
            local y = true\n\
            return 1 + y"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 'y' (a boolean value)
 --> test.lua:2:8
  |
1 | local y = true
  |       - defined here
2 | return 1 + y
  |        ^^^^^ attempt to perform arithmetic on local 'y' (a boolean value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_arith_no_name() {
    // Expression without a named variable falls back to type-only.
    k9::assert_equal!(
        run_err("return nil + 1").await,
        "\
error: attempt to perform arithmetic on a nil value
 --> test.lua:1:8
  |
1 | return nil + 1
  |        ^^^^^^^ attempt to perform arithmetic on a nil value
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_negate_local() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true\n\
            return -b"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 'b' (a boolean value)
 --> test.lua:2:8
  |
1 | local b = true
  |       - defined here
2 | return -b
  |        ^^ attempt to perform arithmetic on local 'b' (a boolean value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_bitwise_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s & 1"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 's' (a string value)
 --> test.lua:2:8
  |
1 | local s = 'hello'
  |       - defined here
2 | return s & 1
  |        ^^^^^ attempt to perform arithmetic on local 's' (a string value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_bitnot_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return ~s"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 's' (a string value)
 --> test.lua:2:8
  |
1 | local s = 'hello'
  |       - defined here
2 | return ~s
  |        ^^ attempt to perform arithmetic on local 's' (a string value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Concatenation error messages with variable names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_concat_local_nil() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return 'hello' .. x"
        )
        .await,
        "\
error: attempt to concatenate local 'x' (a nil value)
 --> test.lua:2:8
  |
1 | local x = nil
  |       - defined here
2 | return 'hello' .. x
  |        ^^^^^^^^^^^^ attempt to concatenate local 'x' (a nil value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_concat_global() {
    k9::assert_equal!(
        run_err("return 'hello' .. g").await,
        "\
error: attempt to concatenate global 'g' (a nil value)
 --> test.lua:1:8
  |
1 | return 'hello' .. g
  |        ^^^^^^^^^^^^ attempt to concatenate global 'g' (a nil value)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_concat_boolean_local() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true\n\
            return b .. 'world'"
        )
        .await,
        "\
error: attempt to concatenate local 'b' (a boolean value)
 --> test.lua:2:8
  |
1 | local b = true
  |       - defined here
2 | return b .. 'world'
  |        ^^^^^^^^^^^^ attempt to concatenate local 'b' (a boolean value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_concat_no_name() {
    k9::assert_equal!(
        run_err("return true .. 'x'").await,
        "\
error: attempt to concatenate a boolean value
 --> test.lua:1:8
  |
1 | return true .. 'x'
  |        ^^^^^^^^^^^ attempt to concatenate a boolean value
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Comparison error messages with variable names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_compare_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x < 1"
        )
        .await,
        "\
error: attempt to compare nil with number (local 'x')
 --> test.lua:2:8
  |
1 | local x = nil
  |       - defined here
2 | return x < 1
  |        ^^^^^ attempt to compare nil with number (local 'x')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_compare_global() {
    k9::assert_equal!(
        run_err("return g < 1").await,
        "\
error: attempt to compare nil with number (global 'g')
 --> test.lua:1:8
  |
1 | return g < 1
  |        ^^^^^ attempt to compare nil with number (global 'g')
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_compare_different_types() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s < 1"
        )
        .await,
        "\
error: attempt to compare string with number (local 's')
 --> test.lua:2:8
  |
1 | local s = 'hello'
  |       - defined here
2 | return s < 1
  |        ^^^^^ attempt to compare string with number (local 's')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_compare_no_name() {
    k9::assert_equal!(
        run_err("return nil < 1").await,
        "\
error: attempt to compare nil with number
 --> test.lua:1:8
  |
1 | return nil < 1
  |        ^^^^^^^ attempt to compare nil with number
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn error_compare_gt_names_lhs() {
    // `a > b` is compiled as `compare_lt(b, a)` — verify lhs name still appears.
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x > 1"
        )
        .await,
        "\
error: attempt to compare number with nil (local 'x')
 --> test.lua:2:8
  |
1 | local x = nil
  |       - defined here
2 | return x > 1
  |        ^^^^^ attempt to compare number with nil (local 'x')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_compare_ge_names_lhs() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x >= 1"
        )
        .await,
        "\
error: attempt to compare number with nil (local 'x')
 --> test.lua:2:8
  |
1 | local x = nil
  |       - defined here
2 | return x >= 1
  |        ^^^^^^ attempt to compare number with nil (local 'x')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_compare_rhs_named() {
    // Only rhs is a named variable — should still appear in message.
    k9::assert_equal!(
        run_err(
            "\
            local y = nil\n\
            return 1 < y"
        )
        .await,
        "\
error: attempt to compare number with nil (local 'y')
 --> test.lua:2:8
  |
2 | return 1 < y
  |        ^^^^^ attempt to compare number with nil (local 'y')
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_bitwise_rhs_bad() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true\n\
            return 1 & b"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 'b' (a boolean value)
 --> test.lua:2:8
  |
1 | local b = true
  |       - defined here
2 | return 1 & b
  |        ^^^^^ attempt to perform arithmetic on local 'b' (a boolean value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_shift_left_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s << 1"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 's' (a string value)
 --> test.lua:2:8
  |
1 | local s = 'hello'
  |       - defined here
2 | return s << 1
  |        ^^^^^^ attempt to perform arithmetic on local 's' (a string value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_shift_right_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s >> 1"
        )
        .await,
        "\
error: attempt to perform arithmetic on local 's' (a string value)
 --> test.lua:2:8
  |
1 | local s = 'hello'
  |       - defined here
2 | return s >> 1
  |        ^^^^^^ attempt to perform arithmetic on local 's' (a string value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn error_concat_literal_true() {
    k9::assert_equal!(
        run_err("return 'string' .. true").await,
        "\
error: attempt to concatenate a boolean value
 --> test.lua:1:8
  |
1 | return 'string' .. true
  |        ^^^^^^^^^^^^^^^^ attempt to concatenate a boolean value
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn var_context_definition_site() {
    // When a runtime error references a local variable, the RuntimeError
    // should include a var_context with the definition site.
    use shingetsu_compiler::{CompileOptions, Compiler};
    use shingetsu_vm::{valuevec, Function, GlobalEnv, Task};

    let src = "\
local config = nil
config.timeout = 30
";
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    let ctx = err.var_context.expect("var_context should be populated");
    let def = ctx.definition.expect("definition should be populated");
    // "local config = nil" is on line 1.
    k9::assert_equal!(def.line, 1);
}

#[tokio::test]
async fn error_concat_string_and_variable() {
    k9::assert_equal!(
        run_err(
            "\
            local some_variable = true\n\
            return 'string' .. some_variable"
        )
        .await,
        "\
error: attempt to concatenate local 'some_variable' (a boolean value)
 --> test.lua:2:8
  |
1 | local some_variable = true
  |       ------------- defined here
2 | return 'string' .. some_variable
  |        ^^^^^^^^^^^^^^^^^^^^^^^^^ attempt to concatenate local 'some_variable' (a boolean value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}
