//! Confirms `#[lua_pairs]` mirrors onto both engines from a single
//! `#[shingetsu_migrate::userdata]` impl block.  The user method
//! returns a Rust iterator; the macro emits the iterator-stashing
//! glue and registers `__pairs` on each engine.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use std::sync::Arc;

#[derive(Clone)]
struct Map(Vec<(String, i64)>);

#[shingetsu_migrate::userdata]
impl Map {
    #[lua_pairs]
    fn pairs_impl(&self) -> impl Iterator<Item = (String, i64)> + Send + 'static {
        self.0.clone().into_iter()
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_lua_pairs_iterates_in_order() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    env.set_global(
        "m",
        Value::Userdata(Arc::new(Map(vec![
            ("a".into(), 1),
            ("b".into(), 2),
            ("c".into(), 3),
        ]))),
    );

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile(
        r#"
        local seen = {}
        for k, v in pairs(m) do
            seen[#seen + 1] = k .. '=' .. tostring(v)
        end
        return seen[1], seen[2], seen[3], #seen
        "#,
    )
    .await
    .expect("compile");
    let func = shingetsu::Function::lua(bc.top_level, vec![]);
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![
            Value::string("a=1"),
            Value::string("b=2"),
            Value::string("c=3"),
            Value::Integer(3),
        ]
    );
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_lua_pairs_iterates_in_order() {
    use mlua::Lua;

    let lua = Lua::new();
    lua.globals()
        .set(
            "m",
            Map(vec![("a".into(), 1), ("b".into(), 2), ("c".into(), 3)]),
        )
        .expect("set userdata");
    let result: (String, String, String, i64) = lua
        .load(
            r#"
            local seen = {}
            for k, v in pairs(m) do
                seen[#seen + 1] = k .. '=' .. tostring(v)
            end
            return seen[1], seen[2], seen[3], #seen
            "#,
        )
        .eval()
        .expect("eval");
    k9::assert_equal!(
        result,
        ("a=1".to_owned(), "b=2".to_owned(), "c=3".to_owned(), 3,)
    );
}
