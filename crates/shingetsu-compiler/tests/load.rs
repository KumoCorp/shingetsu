mod common;

use shingetsu::valuevec;
use shingetsu_vm::{GlobalEnv, Value, ValueVec};

async fn run_load(src: &str) -> ValueVec {
    common::run_with_env(common::new_env_with_load(), src).await
}

async fn run_load_one(src: &str) -> Value {
    run_load(src).await.into_iter().next().unwrap_or(Value::Nil)
}

// -----------------------------------------------------------------------
// Basic load() from string
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_string_returns_function() {
    let v = run_load_one(r#"return type(load("return 42"))"#).await;
    k9::assert_equal!(v, Value::string("function"));
}

#[tokio::test]
async fn load_and_call_returns_value() {
    let v = run_load_one(
        r#"
        local f = load("return 42")
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(42));
}

#[tokio::test]
async fn load_string_with_chunkname() {
    let v = run_load_one(
        r#"
        local f, err = load("error('boom')", "mychunk")
        local ok, msg = pcall(f)
        return msg
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("[string \"mychunk\"]:1: boom"));
}

#[tokio::test]
async fn load_syntax_error_returns_nil_and_message() {
    let results = run_load(
        r#"
        local f, err = load("function(")
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::Nil,
            Value::string(
                "[string \"function(\"]:1:9: unexpected token `(`, expected function name"
            )
        ]
    );
}

#[tokio::test]
async fn load_no_args_errors() {
    common::assert_runtime_error_with_env!(
        common::new_env_with_load(),
        r#"
        load()
    "#,
        "\
error: bad argument #1 to 'load' (value expected, got no value)
 --> test.lua:2:9
  |
2 |         load()
  |         ^^^^ bad argument #1 to 'load' (value expected, got no value)
stack traceback:
\ttest.lua:2: in main chunk",
    );
}

// -----------------------------------------------------------------------
// load() with mode parameter
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_mode_t_works() {
    let v = run_load_one(
        r#"
        local f = load("return 1", nil, "t")
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(1));
}

#[tokio::test]
async fn load_mode_b_rejects_text() {
    let results = run_load(
        r#"
        local f, err = load("return 1", nil, "b")
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::Nil,
            Value::string("attempt to load a text chunk (mode is 'b')")
        ]
    );
}

// -----------------------------------------------------------------------
// load() with env parameter
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_with_custom_env() {
    let v = run_load_one(
        r#"
        local env = { x = 99 }
        local f = load("return x", "test", "t", env)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(99));
}

#[tokio::test]
async fn load_env_isolates_globals() {
    let v = run_load_one(
        r#"
        local env = {}
        local f = load("x = 42", "test", "t", env)
        f()
        return env.x
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(42));
}

#[tokio::test]
async fn load_env_does_not_see_outer_globals() {
    let v = run_load_one(
        r#"
        x = 100
        local env = {}
        local f = load("return x", "test", "t", env)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Nil);
}

// -----------------------------------------------------------------------
// load() with function reader
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_function_reader() {
    let v = run_load_one(
        r#"
        local parts = {"ret", "urn ", "42"}
        local i = 0
        local f = load(function()
            i = i + 1
            return parts[i]
        end)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(42));
}

// -----------------------------------------------------------------------
// _ENV access
// -----------------------------------------------------------------------

#[tokio::test]
async fn env_is_accessible() {
    let v = run_load_one(r#"return type(_ENV)"#).await;
    k9::assert_equal!(v, Value::string("table"));
}

#[tokio::test]
async fn env_contains_globals() {
    let v = run_load_one(r#"return _ENV.type(42)"#).await;
    k9::assert_equal!(v, Value::string("number"));
}

#[tokio::test]
async fn env_self_reference() {
    let v = run_load_one(r#"return _ENV._ENV == _ENV"#).await;
    k9::assert_equal!(v, Value::Boolean(true));
}

// -----------------------------------------------------------------------
// _G global
// -----------------------------------------------------------------------

#[tokio::test]
async fn g_is_same_table_as_env() {
    let v = run_load_one(r#"return _G == _ENV"#).await;
    k9::assert_equal!(v, Value::Boolean(true));
}

#[tokio::test]
async fn g_contains_globals() {
    let v = run_load_one(r#"return _G.type(42)"#).await;
    k9::assert_equal!(v, Value::string("number"));
}

#[tokio::test]
async fn g_write_visible_as_global() {
    let v = run_load_one(
        r#"
        _G.mything = 123
        return mything
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(123));
}

#[tokio::test]
async fn g_self_reference() {
    let v = run_load_one(r#"return _G._G == _G"#).await;
    k9::assert_equal!(v, Value::Boolean(true));
}

// -----------------------------------------------------------------------
// _VERSION global
// -----------------------------------------------------------------------

#[tokio::test]
async fn version_is_string() {
    let v = run_load_one(r#"return type(_VERSION)"#).await;
    k9::assert_equal!(v, Value::string("string"));
}

#[tokio::test]
async fn version_value() {
    let v = run_load_one(r#"return _VERSION"#).await;
    k9::assert_equal!(v, Value::string("Shingetsu dev"));
}

// -----------------------------------------------------------------------
// load() is not available in sandboxed mode
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_not_in_sandboxed() {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::SANDBOXED).expect("register");
    k9::assert_equal!(env.get_global("load"), None);
}

#[tokio::test]
async fn load_available_with_flag() {
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::LOAD,
    )
    .expect("register");
    assert!(env.get_global("load").is_some());
}

// -----------------------------------------------------------------------
// UTF-8 validation
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_string_invalid_utf8_returns_error() {
    // Inject a global `raw_bytes` that contains invalid UTF-8.
    let env = common::new_env_with_load();
    env.set_global("raw_bytes", Value::string(b"return \xff\xfe"));
    let results = common::run_with_env(
        env,
        r#"
        local f, err = load(raw_bytes)
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::Nil, Value::string("load: chunk is not valid UTF-8")]
    );
}

#[tokio::test]
async fn load_reader_invalid_utf8_returns_error() {
    let results = run_load(
        r#"
        local done = false
        local f, err = load(function()
            if done then return nil end
            done = true
            return "\255\254"
        end)
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::Nil, Value::string("load: chunk is not valid UTF-8")]
    );
}

#[tokio::test]
async fn load_reader_multibyte_split_across_chunks() {
    let v = run_load_one(
        r#"
        -- U+00E9 (é) is 0xC3 0xA9 in UTF-8; split across two chunks.
        local chunks = {"return '\xC3", "\xA9'"}
        local i = 0
        local f = load(function()
            i = i + 1
            return chunks[i]
        end)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("é"));
}

// -----------------------------------------------------------------------
// Chunkname conventions (@file, =label)
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_chunkname_at_prefix_shows_as_path() {
    let v = run_load_one(
        r#"
        local f = load("error('boom')", "@myfile.lua")
        local ok, msg = pcall(f)
        return msg
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("myfile.lua:1: boom"));
}

#[tokio::test]
async fn load_chunkname_eq_prefix_shows_as_label() {
    let v = run_load_one(
        r#"
        local f = load("error('boom')", "=mylabel")
        local ok, msg = pcall(f)
        return msg
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("mylabel:1: boom"));
}

// -----------------------------------------------------------------------
// Mode parameter edge cases
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_mode_bt_allows_text() {
    let v = run_load_one(
        r#"
        local f = load("return 7", nil, "bt")
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(7));
}

#[tokio::test]
async fn load_mode_invalid_rejects() {
    let results = run_load(
        r#"
        local f, err = load("return 1", nil, "x")
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::Nil,
            Value::string("attempt to load a text chunk (mode is 'x')")
        ]
    );
}

// -----------------------------------------------------------------------
// Reader edge cases
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_reader_returns_empty_string_terminates() {
    let v = run_load_one(
        r#"
        local first = true
        local f = load(function()
            if first then
                first = false
                return ""
            end
        end)
        return type(f)
    "#,
    )
    .await;
    // Empty string signals end-of-input, so `load` gets "" source.
    // An empty chunk compiles to a function that returns nothing.
    k9::assert_equal!(v, Value::string("function"));
}

#[tokio::test]
async fn load_reader_nil_on_first_call_returns_function() {
    let v = run_load_one(
        r#"
        local f = load(function() return nil end)
        return type(f)
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("function"));
}

#[tokio::test]
async fn load_reader_non_string_terminates() {
    let v = run_load_one(
        r#"
        local calls = 0
        local f = load(function()
            calls = calls + 1
            if calls == 1 then return "return " end
            if calls == 2 then return 42 end
            return "should not reach"
        end)
        return f()
    "#,
    )
    .await;
    // Non-string (42) terminates reading; source is "return " which
    // compiles to a function returning nothing.
    k9::assert_equal!(v, Value::Nil);
}

// -----------------------------------------------------------------------
// env_override propagation through nested calls
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_env_propagates_to_nested_function() {
    let v = run_load_one(
        r#"
        local env = { x = 77 }
        local f = load([[
            local function inner()
                return x
            end
            return inner()
        ]], "test", "t", env)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(77));
}

#[tokio::test]
async fn load_env_nested_write_goes_to_custom_env() {
    let v = run_load_one(
        r#"
        local env = {}
        local f = load([[
            local function setter()
                y = 123
            end
            setter()
        ]], "test", "t", env)
        f()
        return env.y
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(123));
}

// -----------------------------------------------------------------------
// _ENV mutation from Lua
// -----------------------------------------------------------------------

#[tokio::test]
async fn env_write_then_read_via_global() {
    let v = run_load_one(
        r#"
        _ENV.foo = 99
        return foo
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(99));
}

#[tokio::test]
async fn env_field_hides_global() {
    let results = run_load(
        r#"
        local env = { type = function() return "custom" end }
        local f = load("return type(42)", nil, "t", env)
        local custom = f()
        local real = type(42)
        return custom, real
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::string("custom"), Value::string("number")]
    );
}

// -----------------------------------------------------------------------
// Default chunkname in error messages
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_default_chunkname_string_shows_in_error() {
    let v = run_load_one(
        r#"
        local f = load("error('oops')")
        local ok, msg = pcall(f)
        return msg
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("[string \"error('oops')\"]:1: oops"));
}

#[tokio::test]
async fn load_default_chunkname_reader_shows_in_error() {
    let v = run_load_one(
        r#"
        local done = false
        local f = load(function()
            if done then return nil end
            done = true
            return "error('oops')"
        end)
        local ok, msg = pcall(f)
        return msg
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("(load):1: oops"));
}

// -----------------------------------------------------------------------
// Reader that throws
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_reader_that_errors_returns_nil_and_message() {
    let results = run_load(
        r#"
        local f, err = load(function()
            error("reader broke")
        end)
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::Nil, Value::string("test.lua:3: reader broke")]
    );
}

// -----------------------------------------------------------------------
// _ENV edge cases
// -----------------------------------------------------------------------

#[tokio::test]
async fn env_set_nil_removes_global() {
    let v = run_load_one(
        r#"
        _ENV.myglobal = 42
        _ENV.myglobal = nil
        return myglobal
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Nil);
}

#[tokio::test]
async fn env_rawset_hides_builtin() {
    let v = run_load_one(
        r#"
        _ENV.type = nil
        return type
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Nil);
}

// -----------------------------------------------------------------------
// Tail call with env_override
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_env_propagates_through_tail_call() {
    let v = run_load_one(
        r#"
        local env = { val = 55 }
        local f = load([[
            local function tail()
                return val
            end
            -- tail-call position
            return tail()
        ]], "test", "t", env)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(55));
}

// -----------------------------------------------------------------------
// Metamethod dispatch with env_override
// -----------------------------------------------------------------------

#[tokio::test]
async fn load_env_propagates_through_metamethod() {
    let v = run_load_one(
        r#"
        local env = { marker = "found" }
        env.setmetatable = setmetatable
        local f = load([[
            local t = setmetatable({}, {
                __index = function(_, k)
                    return marker
                end
            })
            return t.anything
        ]], "test", "t", env)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("found"));
}

// -----------------------------------------------------------------------
// error() with format_source_name
// -----------------------------------------------------------------------

#[tokio::test]
async fn error_with_level_formats_source_name() {
    let v = run_load_one(
        r#"
        local f = load("error('kaboom', 1)", "@inner.lua")
        local ok, msg = pcall(f)
        return msg
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("inner.lua:1: kaboom"));
}

#[tokio::test]
async fn error_with_level_raw_source_text() {
    let v = run_load_one(
        r#"
        local f = load("error('kaboom', 1)", "mychunk")
        local ok, msg = pcall(f)
        return msg
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("[string \"mychunk\"]:1: kaboom"));
}

// -----------------------------------------------------------------------
// Nested load() with different envs
// -----------------------------------------------------------------------

#[tokio::test]
async fn nested_load_different_envs_are_isolated() {
    let results = run_load(
        r#"
        local env_a = { x = "aaa" }
        local env_b = { x = "bbb" }
        local fa = load("return x", nil, "t", env_a)
        local fb = load("return x", nil, "t", env_b)
        return fa(), fb()
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::string("aaa"), Value::string("bbb")]
    );
}

#[tokio::test]
async fn load_inside_loaded_chunk_with_env() {
    let v = run_load_one(
        r#"
        local outer_env = { load = load, x = 10 }
        local f = load([[
            local inner = load("return x")
            return inner()
        ]], nil, "t", outer_env)
        return f()
    "#,
    )
    .await;
    // The inner load() has no env arg, so it uses the global _ENV.
    // The outer_env has x=10 but the inner chunk runs against the
    // real global env which doesn't have x, so it should be nil.
    k9::assert_equal!(v, Value::Nil);
}

// -----------------------------------------------------------------------
// _ENV iteration
// -----------------------------------------------------------------------

#[tokio::test]
async fn env_is_iterable_with_pairs() {
    let v = run_load_one(
        r#"
        _ENV.testvar = 42
        local found = false
        for k, v in pairs(_ENV) do
            if k == "testvar" and v == 42 then
                found = true
            end
        end
        return found
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Boolean(true));
}
