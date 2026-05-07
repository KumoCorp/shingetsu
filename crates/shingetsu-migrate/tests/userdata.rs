//! Confirms `#[shingetsu_migrate::userdata]` registers the same
//! methods and fields on both engines from a single decorated impl
//! block.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

struct Counter {
    val: AtomicI64,
}

#[shingetsu_migrate::userdata]
impl Counter {
    /// Read the current value.
    #[lua_method]
    fn get(&self) -> i64 {
        self.val.load(Ordering::SeqCst)
    }

    /// Add to the counter and return the new value.
    #[lua_method]
    fn incr(&self, by: i64) -> i64 {
        self.val.fetch_add(by, Ordering::SeqCst) + by
    }

    /// Read-only label exposed as a field.
    #[lua_field]
    fn label(&self) -> String {
        "counter".to_owned()
    }

    /// Write-through scale: setting `c.scale = N` multiplies the
    /// counter by N (exercises `add_field_method_set` on mlua and
    /// the shingetsu setter dispatch on `Userdata::newindex`).
    #[lua_field]
    fn set_scale(&self, factor: i64) {
        let cur = self.val.load(Ordering::SeqCst);
        self.val.store(cur * factor, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_engine_calls_userdata_method() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    let counter = Arc::new(Counter {
        val: AtomicI64::new(0),
    });
    env.set_global(
        "c",
        Value::Userdata(counter as Arc<dyn shingetsu::Userdata>),
    );

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("local a = c:incr(5); local b = c:incr(3); return a, b, c:get(), c.label")
    .await
    .expect("compile");
    let func = shingetsu::Function::lua(bc.top_level, vec![]);
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![
            Value::Integer(5),
            Value::Integer(8),
            Value::Integer(8),
            Value::string("counter"),
        ]
    );
}

#[tokio::test]
async fn shingetsu_engine_writes_userdata_field() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    let counter = Arc::new(Counter {
        val: AtomicI64::new(7),
    });
    env.set_global(
        "c",
        Value::Userdata(counter as Arc<dyn shingetsu::Userdata>),
    );

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("c.scale = 4; return c:get()")
    .await
    .expect("compile");
    let func = shingetsu::Function::lua(bc.top_level, vec![]);
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(res, shingetsu::valuevec![Value::Integer(28)]);
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_engine_calls_userdata_method() {
    use mlua::Lua;

    let lua = Lua::new();
    let counter = Counter {
        val: AtomicI64::new(0),
    };
    lua.globals().set("c", counter).expect("set userdata");

    let result: (i64, i64, i64, String) = lua
        .load("local a = c:incr(5); local b = c:incr(3); return a, b, c:get(), c.label")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, (5, 8, 8, "counter".to_owned()));
}

#[tokio::test]
async fn mlua_engine_writes_userdata_field() {
    use mlua::Lua;

    let lua = Lua::new();
    let counter = Counter {
        val: AtomicI64::new(7),
    };
    lua.globals().set("c", counter).expect("set userdata");

    let result: i64 = lua
        .load("c.scale = 4; return c:get()")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, 28);
}
