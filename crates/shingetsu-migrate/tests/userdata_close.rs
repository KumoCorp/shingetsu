//! Confirms `#[lua_metamethod(Close)]` registers on both engines
//! from a single `#[shingetsu_migrate::userdata]` impl.  Lua 5.4
//! `<close>` semantics fire the metamethod when the local goes out
//! of scope.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use parking_lot::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock};

static CLOSED: LazyLock<AtomicI64> = LazyLock::new(|| AtomicI64::new(0));
static CLOSED_LOCK: Mutex<()> = Mutex::new(());

struct Resource;

#[shingetsu_migrate::userdata]
impl Resource {
    /// `<close>`-driven close metamethod.  Receives no extra args;
    /// the optional Lua 5.4 error parameter is ignored here.
    /// Records the close via interior mutability so we can verify
    /// it fired on scope exit.
    #[lua_metamethod(Close)]
    fn close(&self) {
        CLOSED.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn shingetsu_close_metamethod_fires_on_scope_exit() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let _guard = CLOSED_LOCK.lock();
    CLOSED.store(0, Ordering::SeqCst);

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    env.set_global("r", Value::Userdata(Arc::new(Resource)));

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile(
        r#"
        do
            local x <close> = r
        end
        return 1
        "#,
    )
    .await
    .expect("compile");
    let func = shingetsu::Function::lua(bc.top_level, vec![]);
    let _ = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(CLOSED.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mlua_close_metamethod_fires_on_scope_exit() {
    use mlua::Lua;

    let _guard = CLOSED_LOCK.lock();
    CLOSED.store(0, Ordering::SeqCst);

    let lua = Lua::new();
    lua.globals().set("r", Resource).expect("set");
    lua.load(
        r#"
        do
            local x <close> = r
        end
        "#,
    )
    .exec()
    .expect("exec");
    k9::assert_equal!(CLOSED.load(Ordering::SeqCst), 1);
}

// ---------------------------------------------------------------------------
// Async `__close` (the kumo-jsonl pattern: an async resource that
// awaits cleanup work in its close handler).
// ---------------------------------------------------------------------------

static ASYNC_CLOSED: LazyLock<AtomicI64> = LazyLock::new(|| AtomicI64::new(0));
static ASYNC_CLOSED_LOCK: Mutex<()> = Mutex::new(());

struct AsyncResource;

#[shingetsu_migrate::userdata]
impl AsyncResource {
    /// Async close: the body awaits before recording the close.
    /// Mirrors kumo-jsonl's `add_async_meta_method_mut(Close, ...)`
    /// pattern, rewritten to `&self` + interior mutability per the
    /// migration convention.
    #[lua_metamethod(Close)]
    async fn close(&self) {
        tokio::task::yield_now().await;
        ASYNC_CLOSED.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn shingetsu_async_close_fires_on_scope_exit() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let _guard = ASYNC_CLOSED_LOCK.lock();
    ASYNC_CLOSED.store(0, Ordering::SeqCst);

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    env.set_global("r", Value::Userdata(Arc::new(AsyncResource)));

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile(
        r#"
        do
            local x <close> = r
        end
        return 1
        "#,
    )
    .await
    .expect("compile");
    let func = shingetsu::Function::lua(bc.top_level, vec![]);
    let _ = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(ASYNC_CLOSED.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mlua_async_close_fires_on_scope_exit() {
    use mlua::Lua;

    let _guard = ASYNC_CLOSED_LOCK.lock();
    ASYNC_CLOSED.store(0, Ordering::SeqCst);

    let lua = Lua::new();
    lua.globals().set("r", AsyncResource).expect("set");
    lua.load(
        r#"
        do
            local x <close> = r
        end
        "#,
    )
    .exec_async()
    .await
    .expect("exec_async");
    k9::assert_equal!(ASYNC_CLOSED.load(Ordering::SeqCst), 1);
}
