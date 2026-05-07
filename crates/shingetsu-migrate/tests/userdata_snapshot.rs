//! Confirms `#[shingetsu_migrate::userdata(snapshot)]` registers
//! both shingetsu's `Userdata::snapshot()` hook and the mlua
//! `__memoize` metamethod from a single decorated impl block,
//! sharing the same `Self: Clone + IntoLua` requirement on both
//! engines.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use std::sync::Arc;

#[derive(Clone)]
struct Counter {
    value: i64,
}

#[shingetsu_migrate::userdata(snapshot)]
impl Counter {
    #[lua_method]
    fn get(&self) -> i64 {
        self.value
    }
}

// shingetsu's `IntoLua` isn't blanket-impl'd for `Userdata` types,
// so the cloned `Counter` needs an explicit conversion that the
// auto-snapshot closure can call.  mlua's blanket
// `IntoLua for T: UserData` covers the mlua side automatically once
// the macro emits `impl ::mlua::UserData for Counter`.
impl shingetsu_migrate::shingetsu::IntoLua for Counter {
    fn into_lua(self) -> shingetsu_migrate::shingetsu::Value {
        shingetsu_migrate::shingetsu::Value::Userdata(Arc::new(self))
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_snapshot_rebuilds_in_fresh_env() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    let counter: Arc<dyn shingetsu::Userdata> = Arc::new(Counter { value: 42 });

    let snapshot = counter.snapshot().expect("snapshot present");

    // Rebuild in a fresh env to mimic mod-memoize's cross-context use.
    let env2 = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env2).expect("builtins");
    let rebuilt = snapshot.rebuild(&env2).expect("rebuild");
    env2.set_global("c", rebuilt);

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env2.global_type_map(),
    )
    .compile("return c:get()")
    .await
    .expect("compile");
    let func = shingetsu::Function::lua(bc.top_level, vec![]);
    let res = shingetsu::Task::new(env2, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(res, shingetsu::valuevec![Value::Integer(42)]);
    let _ = env;
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

/// Walk the userdata's metatable from Rust the way kumomta's
/// `mod-memoize::CacheValue::from_lua` does: pull the `__memoize`
/// function off the metatable and call it with the userdata.  mlua
/// hides metatables from Lua-side `getmetatable` by default, so
/// this is the supported access path.
fn fetch_memoized(
    _lua: &mlua::Lua,
    ud: &mlua::AnyUserData,
) -> mlua::Result<shingetsu_migrate::Memoized> {
    let mt = ud.metatable()?;
    let func: mlua::Function = mt.get("__memoize")?;
    func.call(mlua::Value::UserData(ud.clone()))
}

#[tokio::test]
async fn mlua_memoize_metamethod_returns_memoized() {
    use mlua::Lua;

    let lua = Lua::new();
    let ud = lua
        .create_userdata(Counter { value: 7 })
        .expect("create_userdata");
    let memoized = fetch_memoized(&lua, &ud).expect("fetch");

    let lua2 = Lua::new();
    let rebuilt = (memoized.to_value)(&lua2).expect("rebuild");
    lua2.globals().set("c2", rebuilt).expect("set rebuilt");

    let result: i64 = lua2.load("return c2:get()").eval().expect("eval");
    k9::assert_equal!(result, 7);
}

#[tokio::test]
async fn mlua_memoize_round_trip_preserves_state() {
    use mlua::Lua;

    let lua = Lua::new();
    let ud = lua
        .create_userdata(Counter { value: 100 })
        .expect("create_userdata");

    // Two consecutive __memoize calls produce independent Memoized
    // instances pointing at the same captured Counter clone.
    let snap1 = fetch_memoized(&lua, &ud).expect("snap1");
    let snap2 = fetch_memoized(&lua, &ud).expect("snap2");

    let v1 = (snap1.to_value)(&lua).expect("rebuild snap1");
    let v2 = (snap2.to_value)(&lua).expect("rebuild snap2");

    lua.globals().set("v1", v1).expect("set v1");
    lua.globals().set("v2", v2).expect("set v2");
    let result: (i64, i64) = lua.load("return v1:get(), v2:get()").eval().expect("eval");
    k9::assert_equal!(result, (100, 100));
}
