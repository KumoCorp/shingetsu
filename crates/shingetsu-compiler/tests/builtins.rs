mod common;

use common::{new_env, run_all, run_one, run_with_env};
use shingetsu::valuevec;
use shingetsu_vm::Value;

// Vararg / select / collectgarbage / string length
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_length() {
    k9::assert_equal!(run_one("return #'hello'").await, Value::Integer(5));
}

#[tokio::test]
async fn string_length_empty() {
    k9::assert_equal!(run_one("return #''").await, Value::Integer(0));
}

#[tokio::test]
async fn vararg_single_value() {
    // `...` in single-value context takes only the first vararg.
    k9::assert_equal!(
        run_one(
            "local function f(...)
    local x = ...
    return x
end
return f(42)"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn vararg_return_all() {
    k9::assert_equal!(
        run_all(
            "local function f(...)
    return ...
end
return f(1, 2, 3)"
        )
        .await,
        valuevec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[tokio::test]
async fn vararg_local_multi() {
    // `local a, b = ...` expands varargs into both slots.
    k9::assert_equal!(
        run_all(
            "local function f(...)
    local a, b = ...
    return a, b
end
return f(10, 20)"
        )
        .await,
        valuevec![Value::Integer(10), Value::Integer(20)]
    );
}

#[tokio::test]
async fn vararg_pass_to_call() {
    // Passing `...` as the last argument to another function.
    k9::assert_equal!(
        run_all(
            "local function sum(a, b) return a + b end
local function proxy(...)
    return sum(...)
end
return proxy(3, 4)"
        )
        .await,
        valuevec![Value::Integer(7)]
    );
}

#[tokio::test]
async fn vararg_count_via_select() {
    k9::assert_equal!(
        run_one(
            "local function count(...)
    return select('#', ...)
end
return count(1, 2, 3)"
        )
        .await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn vararg_expands_into_table_constructor() {
    // `{...}` as the last field in a constructor expands all varargs
    // into the array part (Lua §3.4.9).
    k9::assert_equal!(
        run_one(
            "local function f(...) local arr = {...}; return #arr end
return f(10, 20, 30, 40)"
        )
        .await,
        Value::Integer(4)
    );
}

#[tokio::test]
async fn vararg_table_constructor_values() {
    k9::assert_equal!(
        run_all(
            "local function f(...) local t = {...}; return t[1], t[2], t[3] end
return f('a', 'b', 'c')"
        )
        .await,
        valuevec![Value::string("a"), Value::string("b"), Value::string("c"),]
    );
}

#[tokio::test]
async fn vararg_table_constructor_empty() {
    k9::assert_equal!(
        run_one(
            "local function f(...) return #{...} end
return f()"
        )
        .await,
        Value::Integer(0)
    );
}

#[tokio::test]
async fn vararg_table_constructor_mixed_static_fields() {
    // Static fields preceding `...` occupy their own array slots;
    // the trailing `...` expands into the rest.
    k9::assert_equal!(
        run_all(
            "local function f(...) local t = {'first', 'second', ...}; return #t, t[1], t[3] end
return f('a', 'b')"
        )
        .await,
        valuevec![
            Value::Integer(4),
            Value::string("first"),
            Value::string("a"),
        ]
    );
}

#[tokio::test]
async fn call_expands_into_table_constructor() {
    // A function call as the last field expands its multiple returns
    // into the array part.
    k9::assert_equal!(
        run_all(
            "local function two() return 10, 20 end
local t = {two()}
return #t, t[1], t[2]"
        )
        .await,
        valuevec![Value::Integer(2), Value::Integer(10), Value::Integer(20)]
    );
}

#[tokio::test]
async fn call_not_last_does_not_expand_in_table_constructor() {
    // Only the final field expands; a call earlier in the list is
    // truncated to a single value.
    k9::assert_equal!(
        run_all(
            "local function two() return 10, 20 end
local t = {two(), 99}
return #t, t[1], t[2]"
        )
        .await,
        valuevec![Value::Integer(2), Value::Integer(10), Value::Integer(99)]
    );
}

#[tokio::test]
async fn vararg_table_constructor_ipairs() {
    // `{...}` expanded values are iterable via ipairs.
    k9::assert_equal!(
        run_one(
            "local function sum(...)
    local s = 0
    for _, v in ipairs({...}) do s = s + v end
    return s
end
return sum(1, 2, 3, 4, 5)"
        )
        .await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn select_hash() {
    k9::assert_equal!(
        run_one("return select('#', 10, 20, 30)").await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn select_index() {
    k9::assert_equal!(
        run_all("return select(2, 'a', 'b', 'c')").await,
        valuevec![Value::string("b"), Value::string("c"),]
    );
}

#[tokio::test]
async fn select_negative_index() {
    k9::assert_equal!(
        run_one("return select(-1, 'a', 'b', 'c')").await,
        Value::string("c")
    );
}

#[tokio::test]
async fn collectgarbage_collect() {
    k9::assert_equal!(
        run_one("return collectgarbage('collect')").await,
        Value::Integer(0)
    );
}

#[tokio::test]
async fn collectgarbage_count() {
    k9::assert_equal!(
        run_one("return collectgarbage('count')").await,
        Value::Float(0.0)
    );
}

#[tokio::test]
async fn collectgarbage_default_opt() {
    // No argument defaults to "collect"
    k9::assert_equal!(run_one("return collectgarbage()").await, Value::Integer(0));
}

#[tokio::test]
async fn collectgarbage_isrunning() {
    k9::assert_equal!(
        run_one("return collectgarbage('isrunning')").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn collectgarbage_stop() {
    // "stop" and other unrecognized opts return 0
    k9::assert_equal!(
        run_one("return collectgarbage('stop')").await,
        Value::Integer(0)
    );
}

#[tokio::test]
async fn collectgarbage_step() {
    k9::assert_equal!(
        run_one("return collectgarbage('step')").await,
        Value::Integer(0)
    );
}

#[tokio::test]
async fn collectgarbage_runs_gc_finalizer() {
    // A table with __gc should have its finalizer called during collect
    k9::assert_equal!(
        run_one(
            "\
            local flag = false
            do
                local t = setmetatable({}, { __gc = function() flag = true end })
                t = nil
            end
            collectgarbage('collect')
            return flag"
        )
        .await,
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// type / tostring / tonumber
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Runtime type validation (ParamSpec / validate_args)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_args_rawget_rejects_non_table() {
    // rawget(table, key) — first arg must be a table.
    let res = run_all(
        "local ok, err = pcall(rawget, 'not a table', 'k')
        return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'rawget' (table expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn validate_args_string_len_rejects_non_string() {
    // string.len(s) — s must be a string.
    let res = run_all(
        "local ok, err = pcall(string.len, 123)
        return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'len' (string expected, got number)"),
        ]
    );
}

#[tokio::test]
async fn validate_args_optional_param_accepts_nil() {
    // string.sub(s, i [, j]) — j is optional, nil should be accepted.
    let res = run_one("return string.sub('hello', 2, nil)").await;
    k9::assert_equal!(res, Value::string("ello"));
}

#[tokio::test]
async fn validate_args_table_concat_accepts_optional_sep() {
    // table.concat(t [, sep]) — sep is optional.
    let res = run_one("return table.concat({1, 2, 3})").await;
    k9::assert_equal!(res, Value::string("123"));
}

#[tokio::test]
async fn validate_args_math_floor_rejects_string() {
    // math.floor(x) takes a Value (unconstrained), so this should
    // pass validate_args but fail inside the function.
    // NOTE: position=0 and empty function name because the error is
    // raised inside to_float() after FromLua succeeds, so the
    // proc-macro's with_arg_and_call_context patch doesn't apply.
    // TODO: propagate position/name into internal helpers like to_float.
    let res = run_all(
        "local ok, err = pcall(math.floor, 'abc')
        return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #0 to '' (number expected, got string)"),
        ]
    );
}

// print
// ---------------------------------------------------------------------------

#[tokio::test]
async fn print_exists_and_returns_nil() {
    // print() returns no values.
    let res = run_all("return print('hello')").await;
    k9::assert_equal!(res, valuevec![]);
}

#[tokio::test]
async fn print_type_is_function() {
    let res = run_one("return type(print)").await;
    k9::assert_equal!(res, Value::string("function"));
}

#[tokio::test]
async fn print_calls_tostring_metamethod() {
    // Verify print calls __tostring by capturing the side effect.
    let res = run_one(
        "\
        local called = false
        local mt = { __tostring = function(t) called = true; return 'custom' end }
        local obj = setmetatable({}, mt)
        print(obj)
        return called",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn print_multiple_args() {
    // print accepts multiple arguments without error.
    let res = run_all("return print(1, 'two', true, nil)").await;
    k9::assert_equal!(res, valuevec![]);
}

#[tokio::test]
async fn print_no_args() {
    // print with no args just prints a newline, no error.
    let res = run_all("return print()").await;
    k9::assert_equal!(res, valuevec![]);
}

#[tokio::test]
async fn tonumber_int() {
    k9::assert_equal!(run_one("return tonumber('42')").await, Value::Integer(42));
}

#[tokio::test]
async fn tonumber_float() {
    k9::assert_equal!(run_one("return tonumber('3.14')").await, Value::Float(3.14));
}

#[tokio::test]
async fn tonumber_base() {
    k9::assert_equal!(
        run_one("return tonumber('ff', 16)").await,
        Value::Integer(255)
    );
}

#[tokio::test]
async fn tonumber_non_numeric() {
    k9::assert_equal!(run_one("return tonumber('hello')").await, Value::Nil);
}

// Lua's `l_str2d` rejects any string containing `n`/`N`, so `"nan"`,
// `"inf"`, `"Inf"`, and `"NaN"` must not be accepted as numbers even
// though Rust's `f64::parse` would happily produce NaN/inf from them.
#[tokio::test]
async fn tonumber_rejects_nan_string() {
    k9::assert_equal!(run_one("return tonumber('nan')").await, Value::Nil);
}

#[tokio::test]
async fn tonumber_rejects_inf_string() {
    k9::assert_equal!(run_one("return tonumber('inf')").await, Value::Nil);
}

#[tokio::test]
async fn tonumber_rejects_capitalized_nan() {
    k9::assert_equal!(run_one("return tonumber('NaN')").await, Value::Nil);
}

#[tokio::test]
async fn tonumber_rejects_capitalized_inf() {
    k9::assert_equal!(run_one("return tonumber('Inf')").await, Value::Nil);
}

// ---------------------------------------------------------------------------

// pairs / ipairs / next
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pairs_iteration() {
    k9::assert_equal!(
        run_one(
            "local t = {a=1, b=2, c=3}
local count = 0
for k, v in pairs(t) do
    count = count + 1
end
return count"
        )
        .await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn ipairs_iteration() {
    k9::assert_equal!(
        run_one(
            "local t = {10, 20, 30}
local sum = 0
for i, v in ipairs(t) do
    sum = sum + v
end
return sum"
        )
        .await,
        Value::Integer(60)
    );
}

#[tokio::test]
async fn ipairs_iterator_wraps_at_max_integer() {
    // Lua 5.4 §6.1: when the iterator's counter reaches
    // `math.maxinteger`, the next index wraps to `math.mininteger`.
    // Verify the wrap step lands on the key that exists, then
    // terminates on the following step when the wrapped key is
    // absent.
    let res = run_all(
        r#"
        local t = {[math.mininteger] = 10}
        local f = ipairs{}
        local k1, v1 = f(t, math.maxinteger)  -- wraps to mininteger
        local k2, v2 = f(t, k1)               -- mininteger+1 absent
        return k1, v1, k2, v2
    "#,
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Integer(i64::MIN),
            Value::Integer(10),
            Value::Nil,
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn ipairs_stops_at_nil() {
    k9::assert_equal!(
        run_one(
            "local t = {1, 2, nil, 4}
local count = 0
for i, v in ipairs(t) do
    count = count + 1
end
return count"
        )
        .await,
        Value::Integer(2)
    );
}

#[tokio::test]
async fn next_basic() {
    k9::assert_equal!(
        run_one(
            "local t = {x=42}
local k, v = next(t)
return v"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn next_nil_at_end() {
    k9::assert_equal!(
        run_one(
            "local t = {x=1}
local k = next(t)  -- gets 'x'
return next(t, k)  -- should be nil"
        )
        .await,
        Value::Nil
    );
}

#[tokio::test]
async fn pairs_mixed_integer_and_string_keys() {
    // Verify that pairs/next visits all entries in a table with both
    // integer (sequence) keys and string (hash) keys.  We collect all
    // key-value pairs into a sorted string so iteration order doesn't matter.
    k9::assert_equal!(
        run_one(
            "local t = {10, 20, x='hello', y='world'}
local entries = {}
for k, v in pairs(t) do
    entries[#entries + 1] = tostring(k) .. '=' .. tostring(v)
end
table.sort(entries)
return table.concat(entries, ',')"
        )
        .await,
        Value::string("1=10,2=20,x=hello,y=world")
    );
}

#[tokio::test]
async fn next_mixed_keys_manual() {
    // Manually walk a mixed table via next() and verify all 4 entries are seen.
    k9::assert_equal!(
        run_one(
            "local t = {10, 20, x='hello', y='world'}
local count = 0
local k = nil
while true do
    k = next(t, k)
    if k == nil then break end
    count = count + 1
end
return count"
        )
        .await,
        Value::Integer(4)
    );
}

// ---------------------------------------------------------------------------

// generic for: break
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generic_for_break() {
    k9::assert_equal!(
        run_one(
            "local t = {1, 2, 3, 4, 5}
local sum = 0
for i, v in ipairs(t) do
    if v > 3 then break end
    sum = sum + v
end
return sum"
        )
        .await,
        Value::Integer(6)
    );
}

// ---------------------------------------------------------------------------

// continue statement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn continue_in_while() {
    // Sum only odd numbers 1..10 using continue to skip evens.
    k9::assert_equal!(
        run_one(
            "local sum = 0
local i = 0
while i < 10 do
    i = i + 1
    if i % 2 == 0 then
        continue
    end
    sum = sum + i
end
return sum"
        )
        .await,
        Value::Integer(25)
    );
}

#[tokio::test]
async fn continue_in_numeric_for() {
    // Sum 1..10 skipping multiples of 3.
    k9::assert_equal!(
        run_one(
            "local sum = 0
for i = 1, 10 do
    if i % 3 == 0 then
        continue
    end
    sum = sum + i
end
return sum"
        )
        .await,
        Value::Integer(37)
    );
}

#[tokio::test]
async fn continue_in_generic_for() {
    // Collect values from pairs, skipping key "b".
    k9::assert_equal!(
        run_one(
            "local t = {a=1, b=2, c=3}
local sum = 0
for k, v in pairs(t) do
    if k == 'b' then
        continue
    end
    sum = sum + v
end
return sum"
        )
        .await,
        Value::Integer(4)
    );
}

#[tokio::test]
async fn continue_in_repeat() {
    // Sum 1..5 skipping 3.
    k9::assert_equal!(
        run_one(
            "local sum = 0
local i = 0
repeat
    i = i + 1
    if i == 3 then
        continue
    end
    sum = sum + i
until i >= 5
return sum"
        )
        .await,
        Value::Integer(12)
    );
}

// Multi-value return: Variadic and 2-tuple
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_macro_variadic_return() {
    // A function can return Variadic to produce an arbitrary number of values.
    // We verify arity on the raw Vec and then use FromLuaMulti for typed extraction.
    use shingetsu::{module, FromLuaMulti, Value, Variadic};

    #[module]
    mod swapmod {
        use shingetsu::{valuevec, Value, Variadic};

        #[function]
        fn swap(a: i64, b: i64) -> Variadic {
            Variadic(valuevec![Value::Integer(b), Value::Integer(a)])
        }
    }

    let env = new_env();
    swapmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return swapmod.swap(1, 2)").await;

    k9::assert_equal!(res, valuevec![Value::Integer(2), Value::Integer(1)]);

    // Typed extraction via FromLuaMulti.
    let Variadic(vals) = Variadic::from_lua_multi(res.into()).expect("from_lua_multi");
    k9::assert_equal!(vals, valuevec![Value::Integer(2), Value::Integer(1)]);
}

#[tokio::test]
async fn module_macro_tuple_return() {
    // A function can return a tuple to produce a fixed number of values.
    // We verify arity on the raw Vec and then use FromLuaMulti for typed extraction.
    use shingetsu::{module, FromLuaMulti};

    #[module]
    mod divmod {
        #[function]
        fn divmod(a: i64, b: i64) -> (i64, i64) {
            (a / b, a % b)
        }
    }

    let env = new_env();
    divmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return divmod.divmod(10, 3)").await;

    k9::assert_equal!(res, valuevec![Value::Integer(3), Value::Integer(1)]);

    // Typed extraction via FromLuaMulti.
    let (q, r) = <(i64, i64)>::from_lua_multi(res.into()).expect("from_lua_multi");
    k9::assert_equal!(q, 3);
    k9::assert_equal!(r, 1);
}

// ---------------------------------------------------------------------------

// require() builtin + register_preload
// ---------------------------------------------------------------------------

#[tokio::test]
async fn require_basic() {
    // require("name") calls the registered preload opener once and returns its table.
    use shingetsu::{module, Value};

    #[module(name = "mylib")]
    mod mylib_impl {
        #[function]
        fn answer() -> i64 {
            42
        }
    }

    let env = new_env();
    mylib_impl::register_preload(&env);

    let res = run_with_env(env, "local m = require('mylib'); return m.answer()").await;
    k9::assert_equal!(res[0], Value::Integer(42));
}

#[tokio::test]
async fn require_caches_result() {
    // A second require() call returns the same (cached) table value — the
    // opener is only called once.

    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let env = new_env();
    let call_count = Arc::new(AtomicU32::new(0));
    let cc = Arc::clone(&call_count);
    env.register_preload("counted", move |_env| {
        cc.fetch_add(1, Ordering::Relaxed);
        Ok(shingetsu::Table::new())
    });

    run_with_env(env.clone(), "require('counted')").await;
    run_with_env(env.clone(), "require('counted')").await;
    run_with_env(env, "require('counted')").await;

    k9::assert_equal!(call_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn require_missing_module_errors() {
    // require() on an unregistered name returns a VmError.
    let env = new_env();
    common::assert_runtime_error_with_env!(
        env,
        "require('notfound')",
        "\
error: error in 'require': module 'notfound' not found
 --> test.lua:1:1
  |
1 | require('notfound')
  | ^^^^^^^ error in 'require': module 'notfound' not found
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

// ---------------------------------------------------------------------------
// File-based require
// ---------------------------------------------------------------------------

#[tokio::test]
async fn require_file_basic() {
    use shingetsu::{Libraries, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use shingetsu_vm::GlobalEnv;

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("mymod.lua"), "return { answer = 42 }").expect("write");

    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::BUILTINS | Libraries::PACKAGE).expect("register");
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    env.set_package_path(Some(search));

    let compiler = Compiler::new(CompileOptions::default(), env.global_type_map());
    let bc = compiler
        .compile("local m = require('mymod'); return m.answer")
        .await
        .expect("compile");
    let func = bc.into_function();
    let results = Task::new(env, func, valuevec![]).await.expect("run");
    k9::assert_equal!(results[0], Value::Integer(42));
}

#[tokio::test]
async fn require_file_caches_result() {
    use shingetsu::{Libraries, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use shingetsu_vm::GlobalEnv;

    let dir = tempfile::tempdir().expect("tempdir");
    // The module increments a global counter each time it runs.
    std::fs::write(
        dir.path().join("counter.lua"),
        "count = (count or 0) + 1; return count",
    )
    .expect("write");

    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::BUILTINS | Libraries::PACKAGE).expect("register");
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    env.set_package_path(Some(search));

    let compiler = Compiler::new(CompileOptions::default(), env.global_type_map());
    let bc = compiler
        .compile("require('counter'); require('counter'); return require('counter')")
        .await
        .expect("compile");
    let func = bc.into_function();
    let results = Task::new(env, func, valuevec![]).await.expect("run");
    // Module only executes once; subsequent requires return cached value.
    k9::assert_equal!(results[0], Value::Integer(1));
}

#[tokio::test]
async fn require_file_not_found_error() {
    use shingetsu::Libraries;
    use shingetsu_vm::GlobalEnv;

    let dir = tempfile::tempdir().expect("tempdir");

    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::BUILTINS | Libraries::PACKAGE).expect("register");
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    env.set_package_path(Some(search.clone()));

    common::assert_runtime_error_with_env!(
        env,
        "require('nosuch')",
        "\
error: error in 'require': module 'nosuch' not found:
           no field package.preload['nosuch']
           TMPDIR/nosuch.lua: No such file or directory
 --> test.lua:1:1
  |
1 | require('nosuch')
  | ^^^^^^^ error in 'require': module 'nosuch' not found: ...
stack traceback:
\ttest.lua:1: in main chunk",
        dir.path() => "TMPDIR",
    );
}

#[tokio::test]
async fn require_file_dotted_name() {
    use shingetsu::{Libraries, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use shingetsu_vm::GlobalEnv;

    let dir = tempfile::tempdir().expect("tempdir");
    let subdir = dir.path().join("foo");
    std::fs::create_dir(&subdir).expect("mkdir");
    std::fs::write(subdir.join("bar.lua"), "return { x = 99 }").expect("write");

    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::BUILTINS | Libraries::PACKAGE).expect("register");
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    env.set_package_path(Some(search));

    let compiler = Compiler::new(CompileOptions::default(), env.global_type_map());
    let bc = compiler
        .compile("local m = require('foo.bar'); return m.x")
        .await
        .expect("compile");
    let func = bc.into_function();
    let results = Task::new(env, func, valuevec![]).await.expect("run");
    k9::assert_equal!(results[0], Value::Integer(99));
}

#[tokio::test]
async fn require_file_preload_takes_priority() {
    use shingetsu::{module, Libraries, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use shingetsu_vm::GlobalEnv;

    #[module(name = "prio")]
    mod prio_impl {
        #[function]
        fn source() -> String {
            "preload".to_string()
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("prio.lua"),
        "return { source = function() return 'file' end }",
    )
    .expect("write");

    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::BUILTINS | Libraries::PACKAGE).expect("register");
    prio_impl::register_preload(&env);
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    env.set_package_path(Some(search));

    let compiler = Compiler::new(CompileOptions::default(), env.global_type_map());
    let bc = compiler
        .compile("local m = require('prio'); return m.source()")
        .await
        .expect("compile");
    let func = bc.into_function();
    let results = Task::new(env, func, valuevec![]).await.expect("run");
    // Preload should win over file.
    k9::assert_equal!(results[0], Value::string("preload"));
}

// ===========================================================================
// ipairs identity
// ===========================================================================

#[tokio::test]
async fn ipairs_returns_same_iterator_function() {
    let res = run_one("return ipairs{} == ipairs{}").await;
    k9::assert_equal!(res, Value::Boolean(true));
}

// ===========================================================================
// require for built-in libraries
// ===========================================================================

#[tokio::test]
async fn require_builtin_math() {
    let res = run_one("return require('math').pi").await;
    k9::assert_equal!(res, Value::Float(std::f64::consts::PI));
}

#[tokio::test]
async fn require_builtin_string() {
    let res = run_one("return require('string').upper('hello')").await;
    k9::assert_equal!(res, Value::string("HELLO"));
}

#[tokio::test]
async fn require_builtin_table() {
    let res = run_one("return require('table').concat({'a','b'}, ',')").await;
    k9::assert_equal!(res, Value::string("a,b"));
}

#[tokio::test]
async fn require_builtin_utf8() {
    let res = run_one("return type(require('utf8').charpattern)").await;
    k9::assert_equal!(res, Value::string("string"));
}

#[tokio::test]
async fn require_builtin_os() {
    let res = run_one("return type(require('os').clock())").await;
    k9::assert_equal!(res, Value::string("number"));
}

#[tokio::test]
async fn require_builtin_io() {
    let env = new_env();
    shingetsu::io::register(&env).expect("register io");
    shingetsu::io::register_stdio(&env).expect("register stdio");
    // re-populate loaded cache after io registration
    if let Some(v) = env.get_global("io") {
        env.set_loaded("io", v);
    }
    let res = run_with_env(env, "return type(require('io'))").await;
    k9::assert_equal!(res, valuevec![Value::string("table")]);
}

// ===========================================================================
// tonumber with hex floats and hex integers
// ===========================================================================

#[tokio::test]
async fn tonumber_hex_integer() {
    let res = run_all("return tonumber('0xFF'), math.type(tonumber('0xFF'))").await;
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(255), Value::string("integer")]
    );
}

#[tokio::test]
async fn tonumber_hex_float() {
    let res = run_one("return tonumber('0x0.41')").await;
    k9::assert_equal!(res, Value::Float(0.25390625));
}

#[tokio::test]
async fn tonumber_hex_float_with_exponent() {
    let res = run_one("return tonumber('0xABCp-3')").await;
    k9::assert_equal!(res, Value::Float(343.5));
}

#[tokio::test]
async fn tonumber_hex_float_signed() {
    let res = run_one("return tonumber('+0x.41')").await;
    k9::assert_equal!(res, Value::Float(0.25390625));
}

#[tokio::test]
async fn tonumber_hex_float_negative() {
    let res = run_one("return tonumber('-0xABC')").await;
    k9::assert_equal!(res, Value::Integer(-2748));
}

#[tokio::test]
async fn tonumber_oversized_hex_wraps_to_integer() {
    // Per Lua 5.4 §3.1, hex literals (and `tonumber` on hex strings)
    // wrap modularly to i64.  The 26-digit literal
    // `0x13121110090807060504030201` keeps its low 64 bits as a
    // signed integer: the bottom 16 hex digits are
    // `0x0807060504030201` = 578437695752307201.
    let res = run_all(
        "return tonumber('0x13121110090807060504030201'), \
         math.type(tonumber('0x13121110090807060504030201'))",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(0x0807060504030201), Value::string("integer"),]
    );
}

// ===========================================================================
// Hex float literals in source code
// ===========================================================================

#[tokio::test]
async fn hex_float_literal() {
    let res = run_one("return 0X0.41").await;
    k9::assert_equal!(res, Value::Float(0.25390625));
}

#[tokio::test]
async fn hex_float_literal_with_exponent() {
    let res = run_one("return 0xABCp-3").await;
    k9::assert_equal!(res, Value::Float(343.5));
}

#[tokio::test]
async fn hex_float_literal_integer_dot_zero() {
    let res = run_one("return 0xF0.0").await;
    k9::assert_equal!(res, Value::Float(240.0));
}

#[tokio::test]
async fn oversized_hex_integer_literal_wraps() {
    // Hex integer literals wrap modularly to i64 (Lua 5.4 §3.1).
    // 26-digit literal → low 64 bits = 0x0807060504030201.
    let res = run_one("return 0x13121110090807060504030201").await;
    k9::assert_equal!(res, Value::Integer(0x0807060504030201));
}

// ---------------------------------------------------------------------------
// type()
// ---------------------------------------------------------------------------

#[tokio::test]
async fn type_no_args_errors() {
    common::assert_runtime_error!(
        "type()",
        r#"error: bad argument #1 to 'type' (value expected, got no value)
 --> test.lua:1:1
  |
1 | type()
  | ^^^^ bad argument #1 to 'type' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

#[tokio::test]
async fn type_nil_returns_nil() {
    let res = run_one("return type(nil)").await;
    k9::assert_equal!(res, Value::string("nil"));
}

#[tokio::test]
async fn type_basic_types() {
    let results = run_all(
        r#"
        return type(true), type(42), type(3.14), type("hi"), type(print)
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("boolean"),
            Value::string("number"),
            Value::string("number"),
            Value::string("string"),
            Value::string("function"),
        ]
    );
}

// ---------------------------------------------------------------------------
// Missing required arguments — typed params (caught by validate_args)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rawget_no_args() {
    common::assert_runtime_error!(
        "rawget()",
        r#"error: bad argument #1 to 'rawget' (value expected, got no value)
 --> test.lua:1:1
  |
1 | rawget()
  | ^^^^^^ bad argument #1 to 'rawget' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

#[tokio::test]
async fn rawset_missing_third_arg() {
    common::assert_runtime_error!(
        r#"rawset({}, "k")"#,
        r#"error: bad argument #3 to 'rawset' (value expected, got no value)
 --> test.lua:1:1
  |
1 | rawset({}, "k")
  | ^^^^^^ bad argument #3 to 'rawset' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

#[tokio::test]
async fn string_len_no_args() {
    common::assert_runtime_error!(
        "string.len()",
        r#"error: bad argument #1 to 'len' (value expected, got no value)
 --> test.lua:1:1
  |
1 | string.len()
  | ^^^^^^^^^^ bad argument #1 to 'len' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

#[tokio::test]
async fn math_fmod_missing_second_arg() {
    common::assert_runtime_error!(
        "math.fmod(10)",
        r#"error: bad argument #2 to 'fmod' (value expected, got no value)
 --> test.lua:1:1
  |
1 | math.fmod(10)
  | ^^^^^^^^^ bad argument #2 to 'fmod' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

// ---------------------------------------------------------------------------
// Missing required arguments — untyped params (caught by arg_fetch)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tostring_no_args() {
    common::assert_runtime_error!(
        "tostring()",
        r#"error: bad argument #1 to 'tostring' (value expected, got no value)
 --> test.lua:1:1
  |
1 | tostring()
  | ^^^^^^^^ bad argument #1 to 'tostring' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

#[tokio::test]
async fn getmetatable_no_args() {
    common::assert_runtime_error!(
        "getmetatable()",
        r#"error: bad argument #1 to 'getmetatable' (value expected, got no value)
 --> test.lua:1:1
  |
1 | getmetatable()
  | ^^^^^^^^^^^^ bad argument #1 to 'getmetatable' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

#[tokio::test]
async fn rawlen_no_args() {
    common::assert_runtime_error!(
        "rawlen()",
        r#"error: bad argument #1 to 'rawlen' (value expected, got no value)
 --> test.lua:1:1
  |
1 | rawlen()
  | ^^^^^^ bad argument #1 to 'rawlen' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#,
    );
}

// ---------------------------------------------------------------------------
// Option<T> params still accept missing args
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tonumber_optional_base_omitted() {
    let res = run_one(r#"return tonumber("42")"#).await;
    k9::assert_equal!(res, Value::Integer(42));
}

#[tokio::test]
async fn string_sub_optional_j_omitted() {
    let res = run_one(r#"return string.sub("hello", 2)"#).await;
    k9::assert_equal!(res, Value::string("ello"));
}

#[tokio::test]
async fn table_remove_optional_pos_omitted() {
    let res = run_one(
        r#"
        local t = {10, 20, 30}
        return table.remove(t)
    "#,
    )
    .await;
    k9::assert_equal!(res, Value::Integer(30));
}

// ---------------------------------------------------------------------------
// `error()` accepts an optional message (Lua 5.4 semantics)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_no_args_propagates_nil() {
    // `error()` with no message produces nil as the error value, not
    // a "value expected" arg-error.  Matches the assertion at
    // `errors.lua:49`: `assert(doit("error()") == nil)`.
    let res = run_all(r#"return pcall(error)"#).await;
    k9::assert_equal!(res, valuevec![Value::Boolean(false), Value::Nil]);
}

#[tokio::test]
async fn error_explicit_nil_propagates_nil() {
    let res = run_all(r#"return pcall(error, nil)"#).await;
    k9::assert_equal!(res, valuevec![Value::Boolean(false), Value::Nil]);
}

#[tokio::test]
async fn error_string_message_gets_location_prefix() {
    // String messages with the default level=1 are prefixed with
    // `source:line:` per Lua 5.4.
    let res = run_all(r#"return pcall(error, "boom")"#).await;
    k9::assert_equal!(
        res,
        valuevec![Value::Boolean(false), Value::string("test.lua:1: boom"),]
    );
}

#[tokio::test]
async fn error_level_zero_skips_location_prefix() {
    let res = run_all(r#"return pcall(error, "plain", 0)"#).await;
    k9::assert_equal!(
        res,
        valuevec![Value::Boolean(false), Value::string("plain")]
    );
}

#[tokio::test]
async fn error_non_string_message_passed_through() {
    // Non-string error values (tables, numbers) are propagated
    // verbatim to the pcall handler, no location prefix.
    let res = run_all(r#"return pcall(error, 42)"#).await;
    k9::assert_equal!(res, valuevec![Value::Boolean(false), Value::Integer(42)]);
}
