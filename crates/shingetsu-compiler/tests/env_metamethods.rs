mod common;

use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu::{valuevec, Libraries};
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{Function, GlobalEnv, Task, Value, ValueVec};

fn type_check_compiler() -> Compiler {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::ALL).expect("register libs");
    Compiler::new(
        CompileOptions {
            debug_info: true,
            source_name: std::sync::Arc::new("@test.lua".to_string()),
            type_check: true,
        },
        env.global_type_map(),
    )
}

async fn run(src: &str) -> ValueVec {
    common::run_with(Libraries::ALL, src, |_| {})
        .await
        .unwrap_or_else(|diag| panic!("script failed:\n{diag}"))
}

async fn run_one(src: &str) -> Value {
    run(src)
        .await
        .into_iter()
        .next()
        .expect("at least one return value")
}

// ---------------------------------------------------------------------------
// __index on a rebound _ENV
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rebound_env_table_index_falls_through_to_g() {
    let v = run_one(
        r#"
        local original_g = _G
        _ENV = setmetatable({}, {__index = original_g})
        return type(print)
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("function"));
}

#[tokio::test]
async fn rebound_env_function_index_returns_metamethod_value() {
    let v = run_one(
        r#"
        local probe
        _ENV = setmetatable({}, {__index = function(_, k)
            probe = k
            return 99
        end})
        local v = some_global
        return v, probe
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(99));
}

#[tokio::test]
async fn rebound_env_index_returns_probe_key() {
    let v = run(r#"
        _ENV = setmetatable({}, {__index = function(_, k) return k end})
        return some_global
    "#)
    .await;
    k9::assert_equal!(v, valuevec![Value::string("some_global")]);
}

// ---------------------------------------------------------------------------
// __newindex on a rebound _ENV
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rebound_env_newindex_intercepts_global_assignment() {
    // After rebinding _ENV with a __newindex metamethod, a free-name
    // assignment must invoke the metamethod and not write the raw slot.
    let v = run(r#"
        local captured_key, captured_val
        local sink = setmetatable({}, {__newindex = function(_, k, v)
            captured_key = k
            captured_val = v
        end})
        local saved_g = _G
        _ENV = sink
        new_global = 42
        -- Switch back to _G so we can return things through normal globals.
        _ENV = saved_g
        return captured_key, captured_val, rawget(sink, "new_global")
    "#)
    .await;
    k9::assert_equal!(
        v,
        valuevec![Value::string("new_global"), Value::Integer(42), Value::Nil,]
    );
}

#[tokio::test]
async fn rebound_env_newindex_table_redirects_writes() {
    // __newindex pointing at a table redirects raw writes to that table.
    let v = run(r#"
        local backing = {}
        local proxy = setmetatable({}, {__newindex = backing})
        local saved_g = _G
        _ENV = proxy
        x = 10
        y = 20
        _ENV = saved_g
        return backing.x, backing.y, rawget(proxy, "x"), rawget(proxy, "y")
    "#)
    .await;
    k9::assert_equal!(
        v,
        valuevec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Nil,
            Value::Nil,
        ]
    );
}

// ---------------------------------------------------------------------------
// rawget / rawset must continue to bypass metamethods
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rawget_bypasses_index_on_rebound_env() {
    let v = run_one(
        r#"
        local rawget = rawget
        local sink = setmetatable({}, {__index = function() return "via_mm" end})
        _ENV = sink
        return rawget(_ENV, "anything")
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Nil);
}

#[tokio::test]
async fn rawset_bypasses_newindex_on_rebound_env() {
    let v = run_one(
        r#"
        local intercepted = false
        local sink = setmetatable({}, {__newindex = function() intercepted = true end})
        rawset(sink, "key", 7)
        return rawget(sink, "key"), intercepted
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(7));
}

// ---------------------------------------------------------------------------
// `_ENV` as a source identifier resolves to the env upvalue
//
// These exercise Phase 3: the parser/compiler treats `_ENV` as a
// name with upvalue semantics rather than as a regular global.
// Order-independence (assignment to `_ENV` BEFORE any global access)
// is the key signal that resolution is no longer accidental.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn env_assignment_before_any_global_access_takes_effect() {
    // The chunk reassigns `_ENV` first, then reads a free name.  Phase
    // 1's accidental routing required a prior global access to
    // register `_ENV` as an upvalue; Phase 3 makes the lookup
    // order-independent.
    let v = run_one(
        r#"
        local saved_g = _G
        _ENV = setmetatable({}, {__index = function(_, k) return "new:" .. k end})
        return some_global
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("new:some_global"));
}

#[tokio::test]
async fn env_read_returns_current_env() {
    // Reading `_ENV` should yield the table currently bound to the
    // env upvalue, not `_G._ENV` (which the old GetGlobal-based path
    // would have produced).
    let v = run(r#"
        local saved_g = _G
        local custom = {marker = 42}
        _ENV = custom
        local v = _ENV
        _ENV = saved_g
        return v == custom, v.marker
    "#)
    .await;
    k9::assert_equal!(v, valuevec![Value::Boolean(true), Value::Integer(42)]);
}

#[tokio::test]
async fn env_set_to_nil_makes_globals_unreachable() {
    // After `_ENV = nil`, any free-name read raises rather than
    // silently falling back to `_G`.
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(
            r#"
        _ENV = nil
        return print
    "#,
        )
        .await
        .expect("compile failed");
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register libs");
    let func = bc.into_function();
    let err = Task::new(env, func, valuevec![])
        .await
        .expect_err("expected runtime error");
    k9::assert_equal!(
        err.error.to_string(),
        "attempt to index upvalue '_ENV' (a nil value) with key 'print'"
    );
}

#[tokio::test]
async fn env_dot_field_access_still_works() {
    // `_ENV.foo` is a regular table read of the env, distinct from a
    // free-name read of `foo`.  Verify it still resolves correctly.
    let v = run_one(
        r#"
        x = 17
        return _ENV.x
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(17));
}

#[tokio::test]
async fn env_propagates_to_nested_closure() {
    // A nested function created after `_ENV` was rebound captures the
    // new env (via the upvalue chain), so its free-name reads consult
    // the new env too.
    let v = run_one(
        r#"
        local saved_g = _G
        _ENV = setmetatable({}, {__index = function(_, k) return "hit:" .. k end})
        local function inner() return some_global end
        local r = inner()
        _ENV = saved_g
        return r
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("hit:some_global"));
}

// ---------------------------------------------------------------------------
// `local _ENV` shadowing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_env_routes_free_names_through_local() {
    // Inside a `local _ENV` scope, free-name reads consult the local
    // table rather than the enclosing chunk's env upvalue.
    let v = run_one(
        r#"
        local function sandbox()
            local _ENV = {greeting = "hello"}
            return greeting
        end
        return sandbox()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("hello"));
}

#[tokio::test]
async fn local_env_routes_free_name_writes_through_local() {
    // Free-name assignment in a `local _ENV` scope writes to the local
    // table, not to `_G`.
    let v = run(r#"
        local sink = {}
        local function setter()
            local _ENV = sink
            x = 42
            y = "text"
        end
        setter()
        return sink.x, sink.y, rawget(_G, "x"), rawget(_G, "y")
    "#)
    .await;
    k9::assert_equal!(
        v,
        valuevec![
            Value::Integer(42),
            Value::string("text"),
            Value::Nil,
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn local_env_metamethods_invoked() {
    // The local _ENV's __index/__newindex still drive free-name access.
    let v = run(r#"
        local backing = {}
        local function setter()
            local _ENV = setmetatable({}, {
                __index = function(_, k) return "read:" .. k end,
                __newindex = function(_, k, v) backing[k] = v end,
            })
            return missing, (function()
                written = 7
                return missing2
            end)()
        end
        local a, b = setter()
        return a, b, backing.written
    "#)
    .await;
    k9::assert_equal!(
        v,
        valuevec![
            Value::string("read:missing"),
            Value::string("read:missing2"),
            Value::Integer(7),
        ]
    );
}

#[tokio::test]
async fn local_env_captured_by_nested_closure() {
    // A function defined inside a `local _ENV` scope captures the
    // local _ENV and uses it for its own free-name access — even when
    // called outside the lexical scope.
    let v = run_one(
        r#"
        local function make_reader()
            local _ENV = {x = 99}
            return function() return x end
        end
        local reader = make_reader()
        return reader()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(99));
}

#[tokio::test]
async fn local_env_outer_globals_unaffected_after_scope() {
    // After a `local _ENV` scope exits, free names in the enclosing
    // chunk continue to resolve via the original env upvalue.
    let v = run_one(
        r#"
        outer = 100
        do
            local _ENV = {outer = 1}
            outer = outer + 1  -- writes to inner env, value=2
        end
        return outer  -- still original outer value
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(100));
}

#[tokio::test]
async fn local_env_in_main_chunk_redirects_globals() {
    // `local _ENV = ...` at chunk top-level redirects all subsequent
    // free-name access in the main chunk.
    let v = run_one(
        r#"
        local _ENV = {x = 5}
        return x
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(5));
}

// ---------------------------------------------------------------------------
// `load(chunk, name, mode, env)` with a metatable-bearing env
// ---------------------------------------------------------------------------

#[tokio::test]
async fn load_env_with_index_metamethod() {
    // `load(..., env)` honours `__index` on the supplied env table:
    // free names in the loaded chunk that aren't direct keys of the
    // env should fall through the metamethod.
    let v = run_one(
        r#"
        local saved_g = _G
        local sandbox = setmetatable({}, {__index = saved_g})
        local f = load("return type(print)", "chunk", "t", sandbox)
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("function"));
}

#[tokio::test]
async fn load_env_with_newindex_metamethod() {
    // Free-name assignment in a loaded chunk routes through the env's
    // `__newindex` metamethod.
    let v = run(r#"
        local backing = {}
        local sandbox = setmetatable({}, {__newindex = function(_, k, v)
            backing[k] = v
        end})
        local f = load("x = 11; y = 22", "chunk", "t", sandbox)
        f()
        return backing.x, backing.y, rawget(sandbox, "x"), rawget(sandbox, "y")
    "#)
    .await;
    k9::assert_equal!(
        v,
        valuevec![
            Value::Integer(11),
            Value::Integer(22),
            Value::Nil,
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn load_env_isolates_writes_from_g() {
    // A chunk loaded with a custom env writes to that env, not to _G.
    let v = run(r#"
        local sandbox = {}
        local f = load("isolated_write = 7", "chunk", "t", sandbox)
        f()
        return sandbox.isolated_write, rawget(_G, "isolated_write")
    "#)
    .await;
    k9::assert_equal!(v, valuevec![Value::Integer(7), Value::Nil]);
}

// ---------------------------------------------------------------------------
// Embedder API: `Function::lua_with_env`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn embedder_lua_with_env_installs_env_at_declared_slot() {
    // Drive `Function::lua_with_env` directly (no Lua-level `load`).
    // Verify the env table's metamethods drive free-name resolution.
    use shingetsu_vm::table::Table;
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile(r#"return some_global"#)
        .await
        .expect("compile failed");
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register libs");

    // Build a sandbox env with a __index metamethod that names every key.
    let sandbox = Table::new();
    let mt = Table::new();
    let probe_name = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let probe_clone = probe_name.clone();
    use shingetsu_vm::function::{NativeCall, NativeFunction};
    use shingetsu_vm::types::FunctionSignature;
    let probe_fn = shingetsu_vm::function::Function::native(NativeFunction {
        signature: std::sync::Arc::new(FunctionSignature {
            name: "__index".into(),
            source: "=[test]".into(),
            type_params: vec![],
            params: vec![],
            variadic: true,

            variadic_doc: None,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
            has_runtime_types: false,
            deprecated: None,
            must_use: None,
        }),
        call: NativeCall::SyncPlain(std::sync::Arc::new(move |args| {
            // args = [table, key]
            if let Some(Value::String(s)) = args.get(1) {
                *probe_clone.lock().unwrap() = String::from_utf8_lossy(s).into_owned();
            }
            Ok(shingetsu::valuevec![Value::Integer(123)])
        })),
    });
    mt.raw_set(Value::string("__index"), Value::Function(probe_fn))
        .expect("set __index");
    sandbox.set_metatable(Some(mt)).expect("set metatable");

    let func = Function::lua_with_env(bc.top_level, vec![], sandbox);
    let result = Task::new(env, func, valuevec![])
        .await
        .expect("task failed");
    k9::assert_equal!(result, valuevec![Value::Integer(123)]);
    k9::assert_equal!(probe_name.lock().unwrap().as_str(), "some_global");
}

// ---------------------------------------------------------------------------
// debug.getupvalue / debug.setupvalue on `_ENV`
// ---------------------------------------------------------------------------

/// Variant of `run` that registers the gated `Libraries::DEBUG`
/// introspection table.  Needed for tests that call
/// `debug.getupvalue` / `debug.setupvalue`.
async fn run_with_debug(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::ALL | shingetsu::Libraries::DEBUG,
    )
    .expect("register libs");
    let func = bc.into_function();
    Task::new(env, func, valuevec![])
        .await
        .expect("task failed")
}

#[tokio::test]
async fn debug_getupvalue_exposes_env_for_chunk_using_globals() {
    // A chunk that touches any global has `_ENV` as an upvalue.
    // `debug.getupvalue(f, 1)` should return the literal name
    // `"_ENV"` and the table currently bound to it.
    let v = run_with_debug(
        r#"
        local f = load("return print")
        local name, value = debug.getupvalue(f, 1)
        return name, value == _G
    "#,
    )
    .await;
    k9::assert_equal!(v, valuevec![Value::string("_ENV"), Value::Boolean(true)]);
}

#[tokio::test]
async fn debug_setupvalue_rebinds_env_for_subsequent_calls() {
    // Replacing the `_ENV` upvalue at runtime via `debug.setupvalue`
    // makes subsequent calls of the closure see the new env.  This is
    // how Lua-side sandbox helpers swap a chunk's globals after
    // loading.
    let v = run_with_debug(
        r#"
        local f = load("return some_value")
        local sandbox = {some_value = 77}
        local name = debug.setupvalue(f, 1, sandbox)
        return name, f()
    "#,
    )
    .await;
    k9::assert_equal!(v, valuevec![Value::string("_ENV"), Value::Integer(77)]);
}

// ---------------------------------------------------------------------------
// `_ENV` as a function parameter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn env_as_function_parameter_binds_env_for_body() {
    // When `_ENV` is named as a function parameter, free names in the
    // body resolve through the argument value, not through the
    // enclosing chunk's env.
    let v = run_one(
        r#"
        local function f(_ENV) return x end
        return f({x = 5})
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::Integer(5));
}

#[tokio::test]
async fn env_parameter_metamethod_invoked_on_free_name() {
    // The `_ENV` parameter's metamethods drive free-name resolution
    // exactly as a `local _ENV = ...` would.
    let v = run_one(
        r#"
        local function f(_ENV) return missing end
        return f(setmetatable({}, {__index = function(_, k) return "hit:" .. k end}))
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("hit:missing"));
}

// ---------------------------------------------------------------------------
// Module pattern: `_ENV = setmetatable(M, {__index = _G})`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_pattern_isolates_definitions_into_module_table() {
    // The canonical Lua module idiom: rebind `_ENV` to a fresh table
    // with `_G` fallback; define functions; return the table.  The
    // defined functions land in the module table (because
    // `function foo()` is sugar for `foo = function() ... end`,
    // which is a global write — routed through `_ENV`).
    let v = run(r#"
        local function build_module()
            local M = {}
            local _ENV = setmetatable(M, {__index = _G})
            function greet(name)
                return "hello, " .. name
            end
            value = 42
            return M
        end
        local M = build_module()
        return M.greet("world"), M.value, type(M.greet), rawget(_G, "greet")
    "#)
    .await;
    k9::assert_equal!(
        v,
        valuevec![
            Value::string("hello, world"),
            Value::Integer(42),
            Value::string("function"),
            Value::Nil,
        ]
    );
}

// ---------------------------------------------------------------------------
// Multi-level upvalue propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn env_propagates_through_three_function_levels() {
    // chunk → outer → middle → leaf.  The middle function rebinds
    // `_ENV`; the leaf reads a free name.  The leaf must see the
    // env that was in effect when it was created — i.e. the rebound
    // one, since it captures `_ENV` from middle's frame.
    let v = run_one(
        r#"
        local function outer()
            local function middle()
                _ENV = setmetatable({}, {__index = function(_, k)
                    return "deep:" .. k
                end})
                local function leaf()
                    return some_global
                end
                return leaf
            end
            return middle()
        end
        local leaf = outer()
        return leaf()
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("deep:some_global"));
}

// ---------------------------------------------------------------------------
// Reading `_ENV` in a chunk that performs no free-name access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reading_env_in_chunk_without_free_names_returns_g() {
    // A chunk that only mentions `_ENV` (no other free names) must
    // still resolve `_ENV` to the host's `_G`.  Exercises the
    // synthetic-root fallback in `resolve_upvalue` when no other
    // GetGlobal/SetGlobal would have triggered registration.
    let v = run_one(r#"return _ENV == _G"#).await;
    k9::assert_equal!(v, Value::Boolean(true));
}

// ---------------------------------------------------------------------------
// Type checker reconciliation: _ENV rebinding/shadowing silences
// global-type inference within the affected scope.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn type_check_global_call_unaffected_without_env_rebinding() {
    // Baseline: a wrong-arg-count call to a known global is flagged.
    let src = "math.abs()";
    let compiler = type_check_compiler();
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "error[arg_count]: expected 1 argument but got 0
 --> test.lua:1:9
  |
1 | math.abs()
  |         ^^ expected 1 argument but got 0"
    );
}

#[tokio::test]
async fn type_check_silenced_after_env_assignment() {
    // After `_ENV = ...`, the same call no longer triggers a
    // global-type diagnostic — we no longer know what `math` resolves
    // to.
    let src = "_ENV = setmetatable({}, {})\nmath.abs()";
    let compiler = type_check_compiler();
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn type_check_silenced_inside_local_env_scope() {
    // Inside a `local _ENV` block, free-name access can't be inferred.
    let src = "do
    local _ENV = {}
    math.abs()
end
";
    let compiler = type_check_compiler();
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn type_check_resumes_after_local_env_scope_exits() {
    // After a `do local _ENV = ... end` block ends, free-name
    // inference resumes — the local _ENV is out of scope and no
    // chunk-level _ENV reassignment has happened.
    let src = "do
    local _ENV = {}
end
math.abs()
";
    let compiler = type_check_compiler();
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "error[arg_count]: expected 1 argument but got 0
 --> test.lua:4:9
  |
4 | math.abs()
  |         ^^ expected 1 argument but got 0"
    );
}

#[tokio::test]
async fn type_check_taint_propagates_into_nested_function() {
    // After `_ENV = ...` at chunk level, a function defined later
    // inherits the taint — free-name access in its body isn't
    // inferred.  `_inner` (leading underscore) suppresses the
    // unused-function warning so the diagnostic stream is empty.
    let src = "_ENV = setmetatable({}, {})
local function _inner()
    math.abs()
end
";
    let compiler = type_check_compiler();
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

// ---------------------------------------------------------------------------
// __index on the original _G table (pre-existing global env)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn g_index_metamethod_reached_from_global_read() {
    // Install __index on the existing _G via a metatable.  Free-name
    // reads should consult it for missing keys.
    let v = run_one(
        r#"
        setmetatable(_G, {__index = function(_, k) return "fallback:" .. k end})
        local r = no_such_global
        setmetatable(_G, nil)
        return r
    "#,
    )
    .await;
    k9::assert_equal!(v, Value::string("fallback:no_such_global"));
}

#[tokio::test]
async fn g_newindex_metamethod_reached_from_global_assignment() {
    // Install __newindex on the existing _G.  Assigning a fresh free
    // name should invoke the metamethod.
    let v = run(r#"
        local captured_key, captured_val
        setmetatable(_G, {__newindex = function(_, k, v)
            captured_key = k
            captured_val = v
        end})
        brand_new_global = 100
        setmetatable(_G, nil)
        return captured_key, captured_val, rawget(_G, "brand_new_global")
    "#)
    .await;
    k9::assert_equal!(
        v,
        valuevec![
            Value::string("brand_new_global"),
            Value::Integer(100),
            Value::Nil,
        ]
    );
}
