//! Confirms the `shingetsu_migrate::Variadic<T>` bridge type round-
//! trips a typed variadic parameter on both engines from a single
//! `#[shingetsu_migrate::module]` body.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::Variadic;

#[shingetsu_migrate::module(name = "varia")]
mod varia {
    use super::Variadic;

    /// Concatenate any number of strings with a separator.
    #[function(variadic)]
    fn join(sep: String, parts: Variadic<String>) -> String {
        parts.0.join(&sep)
    }

    /// Sum any number of integers.
    #[function(variadic)]
    fn sum(rest: Variadic<i64>) -> i64 {
        rest.0.iter().sum()
    }
}

#[tokio::test]
async fn shingetsu_typed_variadic_dispatches() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    varia::register_global_module(&env).expect("register");

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("return varia.sum(1, 2, 3, 4), varia.join('-', 'a', 'b', 'c')")
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(10), Value::string("a-b-c")]
    );
}

#[tokio::test]
async fn mlua_typed_variadic_dispatches() {
    use mlua::Lua;

    let lua = Lua::new();
    varia::register_mlua_module(&lua).expect("register");
    let result: (i64, String) = lua
        .load("return varia.sum(1, 2, 3, 4), varia.join('-', 'a', 'b', 'c')")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, (10, "a-b-c".to_owned()));
}

#[tokio::test]
async fn shingetsu_empty_variadic_dispatches() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    varia::register_global_module(&env).expect("register");

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("return varia.sum(), varia.join(',')")
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![Value::Integer(0), Value::string("")]
    );
}

#[tokio::test]
async fn mlua_empty_variadic_dispatches() {
    use mlua::Lua;

    let lua = Lua::new();
    varia::register_mlua_module(&lua).expect("register");
    let result: (i64, String) = lua
        .load("return varia.sum(), varia.join(',')")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, (0, "".to_owned()));
}
