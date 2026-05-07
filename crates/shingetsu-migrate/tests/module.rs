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

    let func = shingetsu::Function::lua(bc.top_level, vec![]);
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
