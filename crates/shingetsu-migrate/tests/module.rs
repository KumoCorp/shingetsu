//! Confirms `#[shingetsu_migrate::module]` registers the same
//! functions on both engines from a single decorated module body.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

#[shingetsu_migrate::module(name = "demo")]
mod demo {
    /// Add two integers.
    #[function]
    fn add(a: i64, b: i64) -> i64 {
        a + b
    }

    /// Concat two strings.
    #[function]
    fn join(left: String, right: String) -> String {
        format!("{left}{right}")
    }

    /// Async function exercising the create_async_function path.
    #[function]
    async fn double(n: i64) -> i64 {
        n * 2
    }

    /// Eager field evaluated once at registration time.
    #[field]
    fn version() -> String {
        "1.0".to_owned()
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_engine_calls_module_function() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    demo::register_global_module(&env).expect("register");

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("return demo.add(2, 3), demo.version, demo.join('hello, ', 'world')")
    .await
    .expect("compile");

    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");

    k9::assert_equal!(
        res,
        shingetsu::valuevec![
            Value::Integer(5),
            Value::string("1.0"),
            Value::string("hello, world"),
        ]
    );
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_engine_calls_module_function() {
    use mlua::Lua;

    let lua = Lua::new();
    demo::register_mlua_module(&lua).expect("register");

    let result: (i64, String, String) = lua
        .load("return demo.add(2, 3), demo.version, demo.join('hello, ', 'world')")
        .eval()
        .expect("eval");

    k9::assert_equal!(result.0, 5);
    k9::assert_equal!(result.1, "1.0".to_owned());
    k9::assert_equal!(result.2, "hello, world".to_owned());
}

#[tokio::test]
async fn mlua_engine_calls_async_module_function() {
    use mlua::Lua;

    let lua = Lua::new();
    demo::register_mlua_module(&lua).expect("register");

    let result: i64 = lua
        .load("return demo.double(21)")
        .eval_async()
        .await
        .expect("eval_async");
    k9::assert_equal!(result, 42);
}

// ---------------------------------------------------------------------------
// Accessors: lazy_field / getter / setter on both engines
// ---------------------------------------------------------------------------

use shingetsu::sync::{Mutex, MutexGuard};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::LazyLock;

static LAZY_COUNTER: LazyLock<AtomicI64> = LazyLock::new(|| AtomicI64::new(0));
static GETSET_SLOT: LazyLock<AtomicI64> = LazyLock::new(|| AtomicI64::new(7));

// Tests in this file share `LAZY_COUNTER` / `GETSET_SLOT` because
// the `#[shingetsu_migrate::module]` macro requires its accessor
// functions to reference statics rather than per-test state.  We
// serialize the accessor tests through a test-only mutex so each
// run sees a clean slate, regardless of the test runner's
// concurrency.
static ACCESSOR_TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_accessor_state() -> MutexGuard<'static, ()> {
    ACCESSOR_TEST_LOCK.lock()
}

#[shingetsu_migrate::module(name = "counters")]
mod counters {
    use super::{GETSET_SLOT, LAZY_COUNTER};
    use std::sync::atomic::Ordering;

    #[lazy_field]
    fn tick() -> i64 {
        LAZY_COUNTER.fetch_add(1, Ordering::SeqCst) + 1
    }

    #[getter("value")]
    fn get_value() -> i64 {
        GETSET_SLOT.load(Ordering::SeqCst)
    }

    #[setter("value")]
    fn set_value(v: i64) {
        GETSET_SLOT.store(v, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn shingetsu_lazy_field_recomputes_each_access() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let _guard = lock_accessor_state();
    LAZY_COUNTER.store(0, Ordering::SeqCst);
    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    counters::register_global_module(&env).expect("register");

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("return counters.tick, counters.tick, counters.tick")
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[tokio::test]
async fn mlua_lazy_field_recomputes_each_access() {
    use mlua::Lua;

    let _guard = lock_accessor_state();
    LAZY_COUNTER.store(0, Ordering::SeqCst);
    let lua = Lua::new();
    counters::register_mlua_module(&lua).expect("register");
    let res: (i64, i64, i64) = lua
        .load("return counters.tick, counters.tick, counters.tick")
        .eval()
        .expect("eval");
    k9::assert_equal!(res, (1, 2, 3));
}

#[tokio::test]
async fn shingetsu_getter_setter_round_trips() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let _guard = lock_accessor_state();
    GETSET_SLOT.store(7, Ordering::SeqCst);
    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    counters::register_global_module(&env).expect("register");

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("local before = counters.value; counters.value = 100; return before, counters.value")
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(7), Value::Integer(100)]
    );
}

#[tokio::test]
async fn mlua_getter_setter_round_trips() {
    use mlua::Lua;

    let _guard = lock_accessor_state();
    GETSET_SLOT.store(7, Ordering::SeqCst);
    let lua = Lua::new();
    counters::register_mlua_module(&lua).expect("register");
    let res: (i64, i64) = lua
        .load("local before = counters.value; counters.value = 100; return before, counters.value")
        .eval()
        .expect("eval");
    k9::assert_equal!(res, (7, 100));
}

#[tokio::test]
async fn build_mlua_module_table_returns_populated_table() {
    use mlua::Lua;

    // Confirms the host can place the module under a sub-module
    // (kumomta-style) by attaching the returned table.
    let lua = Lua::new();
    let parent = lua.create_table().expect("create parent");
    let demo_tbl = demo::build_mlua_module_table(&lua).expect("build");
    parent.set("demo", demo_tbl).expect("attach");
    lua.globals().set("kumo", parent).expect("set kumo");

    let result: i64 = lua
        .load("return kumo.demo.add(40, 2)")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, 42);
}
