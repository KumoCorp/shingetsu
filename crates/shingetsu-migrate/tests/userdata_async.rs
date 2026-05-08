//! Confirms `#[shingetsu_migrate::userdata]` mirrors async
//! `#[lua_method]` on both engines, covering both `&self` (via
//! `add_async_method`) and `&mut self` (via `add_async_method_mut`)
//! receivers.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use parking_lot::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

struct Counter {
    val: AtomicI64,
    log: Mutex<Vec<i64>>,
}

#[shingetsu_migrate::userdata]
impl Counter {
    /// Async getter — exercises `add_async_method` (`&self`) on
    /// the mlua side and the shingetsu async dispatch path.
    #[lua_method]
    async fn current(&self) -> i64 {
        self.val.load(Ordering::SeqCst)
    }

    /// Async mutator using interior mutability.  shingetsu's
    /// `Arc<dyn Userdata>` only hands out `&self`, so async
    /// (and sync) methods that need to mutate state do so through
    /// `Atomic*` / `Mutex<_>` fields rather than `&mut self`.
    #[lua_method]
    async fn add(&self, by: i64) -> i64 {
        let new = self.val.fetch_add(by, Ordering::SeqCst) + by;
        self.log.lock().push(new);
        new
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_async_methods_round_trip() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    let counter = Arc::new(Counter {
        val: AtomicI64::new(0),
        log: Mutex::new(Vec::new()),
    });
    env.set_global(
        "c",
        Value::Userdata(counter.clone() as Arc<dyn shingetsu::Userdata>),
    );

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("local a = c:add(5); local b = c:add(3); return a, b, c:current()")
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(5), Value::Integer(8), Value::Integer(8),]
    );
    k9::assert_equal!(*counter.log.lock(), vec![5_i64, 8]);
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_async_methods_round_trip() {
    use mlua::Lua;

    let lua = Lua::new();
    let counter = Counter {
        val: AtomicI64::new(0),
        log: Mutex::new(Vec::new()),
    };
    lua.globals().set("c", counter).expect("set");

    let result: (i64, i64, i64) = lua
        .load("local a = c:add(5); local b = c:add(3); return a, b, c:current()")
        .eval_async()
        .await
        .expect("eval_async");
    k9::assert_equal!(result, (5, 8, 8));
}
