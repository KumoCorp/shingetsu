//! Verifies the runtime-engine wrapper construction, accessor, and
//! backend-label surface.  `Engine` is a pure wrapper: there is no
//! cross-engine `eval` or `run` here yet -- callers reach into the
//! variant-specific engine state via `as_shingetsu()` / `as_mlua()`
//! to load scripts with full control over `CompileOptions`,
//! `lua.load(...).set_name(...)`, and any other engine-native
//! configuration.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::shingetsu::GlobalEnv;
use shingetsu_migrate::Engine;

fn shingetsu_engine_with_builtins() -> Engine {
    let env = GlobalEnv::new();
    shingetsu_migrate::shingetsu::builtins::register(&env).expect("register builtins");
    Engine::from_shingetsu(env)
}

#[tokio::test]
async fn shingetsu_engine_backend_label() {
    let engine = shingetsu_engine_with_builtins();
    k9::assert_equal!(engine.backend_name(), "shingetsu");
}

#[tokio::test]
async fn mlua_engine_backend_label() {
    let engine = Engine::from_mlua(mlua::Lua::new());
    k9::assert_equal!(engine.backend_name(), "mlua");
}

#[tokio::test]
async fn as_accessors_only_match_active_variant() {
    let s_engine = shingetsu_engine_with_builtins();
    assert!(s_engine.as_shingetsu().is_some());
    assert!(s_engine.as_mlua().is_none());

    let m_engine = Engine::from_mlua(mlua::Lua::new());
    assert!(m_engine.as_mlua().is_some());
    assert!(m_engine.as_shingetsu().is_none());
}

#[tokio::test]
async fn shingetsu_accessor_lets_caller_drive_native_compile_path() {
    use shingetsu_migrate::shingetsu::compiler::{CompileOptions, Compiler};
    use shingetsu_migrate::shingetsu::{Task, Value};

    // Real hosts load scripts via the engine's native API to get
    // full control over CompileOptions (source_name, type_check)
    // and access to Bytecode.diagnostics for surfacing warnings.
    // This test exercises that path through the accessor.
    let engine = shingetsu_engine_with_builtins();
    let env = engine.as_shingetsu().expect("shingetsu env");

    let opts = CompileOptions {
        source_name: std::sync::Arc::new("=demo.lua".to_owned()),
        ..Default::default()
    };
    let bc = Compiler::new(opts, env.global_type_map())
        .compile("answer = 42")
        .await
        .expect("compile");
    let func = bc.into_function();
    let _ = Task::new(env.clone(), func, shingetsu_migrate::shingetsu::valuevec![])
        .await
        .expect("task");

    k9::assert_equal!(env.get_global("answer"), Some(Value::Integer(42)));
}

#[tokio::test]
async fn mlua_accessor_lets_caller_drive_native_load_path() {
    let engine = Engine::from_mlua(mlua::Lua::new());
    let lua = engine.as_mlua().expect("mlua state");

    // Same idea on mlua: real hosts use Lua::load(...).set_name(...)
    // and friends for full control.  Exercising the accessor
    // confirms the wrapper passes the underlying state through
    // unchanged.
    lua.load("answer = 42")
        .set_name("=demo.lua")
        .exec_async()
        .await
        .expect("exec");
    let val: i64 = lua.globals().get("answer").expect("get global");
    k9::assert_equal!(val, 42);
}
