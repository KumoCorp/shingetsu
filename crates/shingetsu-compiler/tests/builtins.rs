mod common;

use common::{new_env, run_all, run_one, run_with_env};
use shingetsu_vm::Value;

// Vararg / select / collectgarbage / string length
// ---------------------------------------------------------------------------

#[test]
fn string_length() {
    k9::assert_equal!(run_one("return #'hello'"), Value::Integer(5));
}

#[test]
fn string_length_empty() {
    k9::assert_equal!(run_one("return #''"), Value::Integer(0));
}

#[test]
fn vararg_single_value() {
    // `...` in single-value context takes only the first vararg.
    k9::assert_equal!(
        run_one(
            "local function f(...)
    local x = ...
    return x
end
return f(42)"
        ),
        Value::Integer(42)
    );
}

#[test]
fn vararg_return_all() {
    k9::assert_equal!(
        run_all(
            "local function f(...)
    return ...
end
return f(1, 2, 3)"
        ),
        vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[test]
fn vararg_local_multi() {
    // `local a, b = ...` expands varargs into both slots.
    k9::assert_equal!(
        run_all(
            "local function f(...)
    local a, b = ...
    return a, b
end
return f(10, 20)"
        ),
        vec![Value::Integer(10), Value::Integer(20)]
    );
}

#[test]
fn vararg_pass_to_call() {
    // Passing `...` as the last argument to another function.
    k9::assert_equal!(
        run_all(
            "local function sum(a, b) return a + b end
local function proxy(...)
    return sum(...)
end
return proxy(3, 4)"
        ),
        vec![Value::Integer(7)]
    );
}

#[test]
fn vararg_count_via_select() {
    k9::assert_equal!(
        run_one(
            "local function count(...)
    return select('#', ...)
end
return count(1, 2, 3)"
        ),
        Value::Integer(3)
    );
}

#[test]
fn vararg_expands_into_table_constructor() {
    // `{...}` as the last field in a constructor expands all varargs
    // into the array part (Lua §3.4.9).
    k9::assert_equal!(
        run_one(
            "local function f(...) local arr = {...}; return #arr end
return f(10, 20, 30, 40)"
        ),
        Value::Integer(4)
    );
}

#[test]
fn vararg_table_constructor_values() {
    k9::assert_equal!(
        run_all(
            "local function f(...) local t = {...}; return t[1], t[2], t[3] end
return f('a', 'b', 'c')"
        ),
        vec![Value::string("a"), Value::string("b"), Value::string("c"),]
    );
}

#[test]
fn vararg_table_constructor_empty() {
    k9::assert_equal!(
        run_one(
            "local function f(...) return #{...} end
return f()"
        ),
        Value::Integer(0)
    );
}

#[test]
fn vararg_table_constructor_mixed_static_fields() {
    // Static fields preceding `...` occupy their own array slots;
    // the trailing `...` expands into the rest.
    k9::assert_equal!(
        run_all(
            "local function f(...) local t = {'first', 'second', ...}; return #t, t[1], t[3] end
return f('a', 'b')"
        ),
        vec![
            Value::Integer(4),
            Value::string("first"),
            Value::string("a"),
        ]
    );
}

#[test]
fn call_expands_into_table_constructor() {
    // A function call as the last field expands its multiple returns
    // into the array part.
    k9::assert_equal!(
        run_all(
            "local function two() return 10, 20 end
local t = {two()}
return #t, t[1], t[2]"
        ),
        vec![Value::Integer(2), Value::Integer(10), Value::Integer(20)]
    );
}

#[test]
fn call_not_last_does_not_expand_in_table_constructor() {
    // Only the final field expands; a call earlier in the list is
    // truncated to a single value.
    k9::assert_equal!(
        run_all(
            "local function two() return 10, 20 end
local t = {two(), 99}
return #t, t[1], t[2]"
        ),
        vec![Value::Integer(2), Value::Integer(10), Value::Integer(99)]
    );
}

#[test]
fn vararg_table_constructor_ipairs() {
    // `{...}` expanded values are iterable via ipairs.
    k9::assert_equal!(
        run_one(
            "local function sum(...)
    local s = 0
    for _, v in ipairs({...}) do s = s + v end
    return s
end
return sum(1, 2, 3, 4, 5)"
        ),
        Value::Integer(15)
    );
}

#[test]
fn select_hash() {
    k9::assert_equal!(run_one("return select('#', 10, 20, 30)"), Value::Integer(3));
}

#[test]
fn select_index() {
    k9::assert_equal!(
        run_all("return select(2, 'a', 'b', 'c')"),
        vec![Value::string("b"), Value::string("c"),]
    );
}

#[test]
fn select_negative_index() {
    k9::assert_equal!(
        run_one("return select(-1, 'a', 'b', 'c')"),
        Value::string("c")
    );
}

#[test]
fn collectgarbage_collect() {
    k9::assert_equal!(
        run_one("return collectgarbage('collect')"),
        Value::Integer(0)
    );
}

#[test]
fn collectgarbage_count() {
    k9::assert_equal!(run_one("return collectgarbage('count')"), Value::Float(0.0));
}

// ---------------------------------------------------------------------------
// type / tostring / tonumber
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Runtime type validation (ParamSpec / validate_args)
// ---------------------------------------------------------------------------

#[test]
fn validate_args_rawget_rejects_non_table() {
    // rawget(table, key) — first arg must be a table.
    let res = run_all(
        "local ok, err = pcall(rawget, 'not a table', 'k')
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'rawget' (table expected, got string)"),
        ]
    );
}

#[test]
fn validate_args_string_len_rejects_non_string() {
    // string.len(s) — s must be a string.
    let res = run_all(
        "local ok, err = pcall(string.len, 123)
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'len' (string expected, got number)"),
        ]
    );
}

#[test]
fn validate_args_optional_param_accepts_nil() {
    // string.sub(s, i [, j]) — j is optional, nil should be accepted.
    let res = run_one("return string.sub('hello', 2, nil)");
    k9::assert_equal!(res, Value::string("ello"));
}

#[test]
fn validate_args_table_concat_accepts_optional_sep() {
    // table.concat(t [, sep]) — sep is optional.
    let res = run_one("return table.concat({1, 2, 3})");
    k9::assert_equal!(res, Value::string("123"));
}

#[test]
fn validate_args_math_floor_rejects_string() {
    // math.floor(x) takes a Value (unconstrained), so this should
    // pass validate_args but fail inside the function.
    // NOTE: position=0 and empty function name because the error is
    // raised inside to_float() after FromLua succeeds, so the
    // proc-macro's with_arg_and_call_context patch doesn't apply.
    // TODO: propagate position/name into internal helpers like to_float.
    let res = run_all(
        "local ok, err = pcall(math.floor, 'abc')
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #0 to '' (number expected, got string)"),
        ]
    );
}

// print
// ---------------------------------------------------------------------------

#[test]
fn print_exists_and_returns_nil() {
    // print() returns no values.
    let res = run_all("return print('hello')");
    k9::assert_equal!(res, vec![]);
}

#[test]
fn print_type_is_function() {
    let res = run_one("return type(print)");
    k9::assert_equal!(res, Value::string("function"));
}

#[test]
fn print_calls_tostring_metamethod() {
    // Verify print calls __tostring by capturing the side effect.
    let res = run_one(
        "\
        local called = false
        local mt = { __tostring = function(t) called = true; return 'custom' end }
        local obj = setmetatable({}, mt)
        print(obj)
        return called",
    );
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn print_multiple_args() {
    // print accepts multiple arguments without error.
    let res = run_all("return print(1, 'two', true, nil)");
    k9::assert_equal!(res, vec![]);
}

#[test]
fn print_no_args() {
    // print with no args just prints a newline, no error.
    let res = run_all("return print()");
    k9::assert_equal!(res, vec![]);
}

#[test]
fn tonumber_int() {
    k9::assert_equal!(run_one("return tonumber('42')"), Value::Integer(42));
}

#[test]
fn tonumber_float() {
    k9::assert_equal!(run_one("return tonumber('3.14')"), Value::Float(3.14));
}

#[test]
fn tonumber_base() {
    k9::assert_equal!(run_one("return tonumber('ff', 16)"), Value::Integer(255));
}

#[test]
fn tonumber_non_numeric() {
    k9::assert_equal!(run_one("return tonumber('hello')"), Value::Nil);
}

// Lua's `l_str2d` rejects any string containing `n`/`N`, so `"nan"`,
// `"inf"`, `"Inf"`, and `"NaN"` must not be accepted as numbers even
// though Rust's `f64::parse` would happily produce NaN/inf from them.
#[test]
fn tonumber_rejects_nan_string() {
    k9::assert_equal!(run_one("return tonumber('nan')"), Value::Nil);
}

#[test]
fn tonumber_rejects_inf_string() {
    k9::assert_equal!(run_one("return tonumber('inf')"), Value::Nil);
}

#[test]
fn tonumber_rejects_capitalized_nan() {
    k9::assert_equal!(run_one("return tonumber('NaN')"), Value::Nil);
}

#[test]
fn tonumber_rejects_capitalized_inf() {
    k9::assert_equal!(run_one("return tonumber('Inf')"), Value::Nil);
}

// ---------------------------------------------------------------------------

// pairs / ipairs / next
// ---------------------------------------------------------------------------

#[test]
fn pairs_iteration() {
    k9::assert_equal!(
        run_one(
            "local t = {a=1, b=2, c=3}
local count = 0
for k, v in pairs(t) do
    count = count + 1
end
return count"
        ),
        Value::Integer(3)
    );
}

#[test]
fn ipairs_iteration() {
    k9::assert_equal!(
        run_one(
            "local t = {10, 20, 30}
local sum = 0
for i, v in ipairs(t) do
    sum = sum + v
end
return sum"
        ),
        Value::Integer(60)
    );
}

#[test]
fn ipairs_stops_at_nil() {
    k9::assert_equal!(
        run_one(
            "local t = {1, 2, nil, 4}
local count = 0
for i, v in ipairs(t) do
    count = count + 1
end
return count"
        ),
        Value::Integer(2)
    );
}

#[test]
fn next_basic() {
    k9::assert_equal!(
        run_one(
            "local t = {x=42}
local k, v = next(t)
return v"
        ),
        Value::Integer(42)
    );
}

#[test]
fn next_nil_at_end() {
    k9::assert_equal!(
        run_one(
            "local t = {x=1}
local k = next(t)  -- gets 'x'
return next(t, k)  -- should be nil"
        ),
        Value::Nil
    );
}

#[test]
fn pairs_mixed_integer_and_string_keys() {
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
        ),
        Value::string("1=10,2=20,x=hello,y=world")
    );
}

#[test]
fn next_mixed_keys_manual() {
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
        ),
        Value::Integer(4)
    );
}

// ---------------------------------------------------------------------------

// generic for: break
// ---------------------------------------------------------------------------

#[test]
fn generic_for_break() {
    k9::assert_equal!(
        run_one(
            "local t = {1, 2, 3, 4, 5}
local sum = 0
for i, v in ipairs(t) do
    if v > 3 then break end
    sum = sum + v
end
return sum"
        ),
        Value::Integer(6)
    );
}

// ---------------------------------------------------------------------------

// continue statement
// ---------------------------------------------------------------------------

#[test]
fn continue_in_while() {
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
        ),
        Value::Integer(25)
    );
}

#[test]
fn continue_in_numeric_for() {
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
        ),
        Value::Integer(37)
    );
}

#[test]
fn continue_in_generic_for() {
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
        ),
        Value::Integer(4)
    );
}

#[test]
fn continue_in_repeat() {
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
        ),
        Value::Integer(12)
    );
}

// Multi-value return: Variadic and 2-tuple
// ---------------------------------------------------------------------------

#[test]
fn module_macro_variadic_return() {
    // A function can return Variadic to produce an arbitrary number of values.
    // We verify arity on the raw Vec and then use FromLuaMulti for typed extraction.
    use shingetsu::{module, FromLuaMulti, Value, Variadic};

    #[module]
    mod swapmod {
        use shingetsu::{Value, Variadic};

        #[function]
        fn swap(a: i64, b: i64) -> Variadic {
            Variadic(vec![Value::Integer(b), Value::Integer(a)])
        }
    }

    let env = new_env();
    swapmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return swapmod.swap(1, 2)");

    // Arity check on the raw Vec.
    k9::assert_equal!(res.len(), 2);

    // Typed extraction via FromLuaMulti.
    let Variadic(vals) = Variadic::from_lua_multi(res).expect("from_lua_multi");
    k9::assert_equal!(vals.len(), 2);
    k9::assert_equal!(vals[0], Value::Integer(2));
    k9::assert_equal!(vals[1], Value::Integer(1));
}

#[test]
fn module_macro_tuple_return() {
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
    let res = run_with_env(env, "return divmod.divmod(10, 3)");

    // Arity check on the raw Vec.
    k9::assert_equal!(res.len(), 2);

    // Typed extraction via FromLuaMulti.
    let (q, r) = <(i64, i64)>::from_lua_multi(res).expect("from_lua_multi");
    k9::assert_equal!(q, 3);
    k9::assert_equal!(r, 1);
}

// ---------------------------------------------------------------------------

// require() builtin + register_preload
// ---------------------------------------------------------------------------

#[test]
fn require_basic() {
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

    let res = run_with_env(env, "local m = require('mylib'); return m.answer()");
    k9::assert_equal!(res[0], Value::Integer(42));
}

#[test]
fn require_caches_result() {
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

    run_with_env(env.clone(), "require('counted')");
    run_with_env(env.clone(), "require('counted')");
    run_with_env(env, "require('counted')");

    k9::assert_equal!(call_count.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn require_missing_module_errors() {
    // require() on an unregistered name returns a VmError.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};

    let env = new_env();
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: false,
            source_name: "test".into(),
            type_check: false,
        },
        Default::default(),
    );
    let bc = compiler
        .compile("require('notfound')")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "error in 'require': module 'notfound' not found"
    );
}

// ---------------------------------------------------------------------------
// File-based require
// ---------------------------------------------------------------------------

#[tokio::test]
async fn require_file_basic() {
    use shingetsu::{Function, Libraries, Task};
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
    let func = Function::lua(bc.top_level, vec![]);
    let results = Task::new(env, func, vec![]).await.expect("run");
    k9::assert_equal!(results[0], Value::Integer(42));
}

#[tokio::test]
async fn require_file_caches_result() {
    use shingetsu::{Function, Libraries, Task};
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
    let func = Function::lua(bc.top_level, vec![]);
    let results = Task::new(env, func, vec![]).await.expect("run");
    // Module only executes once; subsequent requires return cached value.
    k9::assert_equal!(results[0], Value::Integer(1));
}

#[tokio::test]
async fn require_file_not_found_error() {
    use shingetsu::{Function, Libraries, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};
    use shingetsu_vm::GlobalEnv;

    let dir = tempfile::tempdir().expect("tempdir");

    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::BUILTINS | Libraries::PACKAGE).expect("register");
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    env.set_package_path(Some(search.clone()));

    let compiler = Compiler::new(CompileOptions::default(), env.global_type_map());
    let bc = compiler
        .compile("require('nosuch')")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    let msg = err.to_string();
    // Should mention what was tried.
    let stable = msg.replace(&format!("{}", dir.path().display()), "TMPDIR");
    k9::assert_equal!(
        stable,
        "error in 'require': module 'nosuch' not found:\n\
         \tno field package.preload['nosuch']\n\
         \tTMPDIR/nosuch.lua: No such file or directory"
    );
}

#[tokio::test]
async fn require_file_dotted_name() {
    use shingetsu::{Function, Libraries, Task};
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
    let func = Function::lua(bc.top_level, vec![]);
    let results = Task::new(env, func, vec![]).await.expect("run");
    k9::assert_equal!(results[0], Value::Integer(99));
}

#[tokio::test]
async fn require_file_preload_takes_priority() {
    use shingetsu::{module, Function, Libraries, Task};
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
    let func = Function::lua(bc.top_level, vec![]);
    let results = Task::new(env, func, vec![]).await.expect("run");
    // Preload should win over file.
    k9::assert_equal!(results[0], Value::string("preload"));
}
