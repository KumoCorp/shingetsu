//! Confirms `#[shingetsu_migrate::userdata]` mirrors
//! `#[lua_metamethod]` on both engines from a single decorated impl
//! block, covering non-binary (`ToString`, `Len`, `Unm`) and binary
//! (`Add`, `Sub`, `Lt`, `Concat`) metamethods.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use std::sync::Arc;

struct Num(i64);

#[shingetsu_migrate::userdata]
impl Num {
    /// `tostring(num)` — non-binary, returns a string.
    #[lua_metamethod(ToString)]
    fn ts(&self) -> String {
        format!("Num({})", self.0)
    }

    /// `-num` — non-binary unary negation.
    #[lua_metamethod(Unm)]
    fn neg(&self) -> i64 {
        -self.0
    }

    /// `num + n` / `n + num` — binary arithmetic, userdata may sit
    /// on either side of the operator.
    #[lua_metamethod(Add)]
    fn add_mm(&self, rhs: i64) -> i64 {
        self.0 + rhs
    }

    /// `num - n` / `n - num` — binary, exercises the same dispatch.
    #[lua_metamethod(Sub)]
    fn sub_mm(&self, rhs: i64) -> i64 {
        self.0 - rhs
    }

    /// `num < n` — binary comparison.
    #[lua_metamethod(Lt)]
    fn lt_mm(&self, rhs: i64) -> bool {
        self.0 < rhs
    }
}

struct Items(Vec<i64>);

#[shingetsu_migrate::userdata]
impl Items {
    /// `#items` — non-binary length.
    #[lua_metamethod(Len)]
    fn len(&self) -> i64 {
        self.0.len() as i64
    }
}

struct Label(String);

#[shingetsu_migrate::userdata]
impl Label {
    /// `lbl .. s` / `s .. lbl` — binary concatenation, exercises the
    /// userdata-on-right path with a string operand type.
    #[lua_metamethod(Concat)]
    fn concat_mm(&self, rhs: String) -> String {
        format!("{}{rhs}", self.0)
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_non_binary_metamethods() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    env.set_global("n", Value::Userdata(Arc::new(Num(7))));
    env.set_global("items", Value::Userdata(Arc::new(Items(vec![1, 2, 3, 4]))));

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("return tostring(n), -n, #items")
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![
            Value::string("Num(7)"),
            Value::Integer(-7),
            Value::Integer(4),
        ]
    );
}

#[tokio::test]
async fn shingetsu_binary_metamethods() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    env.set_global("n", Value::Userdata(Arc::new(Num(10))));
    env.set_global("lbl", Value::Userdata(Arc::new(Label("hi ".into()))));

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("return n + 5, 5 + n, n - 3, n < 20, n < 5, lbl .. 'world'")
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![
            Value::Integer(15),
            Value::Integer(15),
            Value::Integer(7),
            Value::Boolean(true),
            Value::Boolean(false),
            Value::string("hi world"),
        ]
    );
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_non_binary_metamethods() {
    use mlua::Lua;

    let lua = Lua::new();
    lua.globals().set("n", Num(7)).expect("set num");
    lua.globals()
        .set("items", Items(vec![1, 2, 3, 4]))
        .expect("set items");

    let result: (String, i64, i64) = lua
        .load("return tostring(n), -n, #items")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, ("Num(7)".to_owned(), -7, 4));
}

#[tokio::test]
async fn mlua_binary_metamethods() {
    use mlua::Lua;

    let lua = Lua::new();
    lua.globals().set("n", Num(10)).expect("set num");
    lua.globals()
        .set("lbl", Label("hi ".into()))
        .expect("set lbl");

    let result: (i64, i64, i64, bool, bool, String) = lua
        .load("return n + 5, 5 + n, n - 3, n < 20, n < 5, lbl .. 'world'")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, (15, 15, 7, true, false, "hi world".to_owned()));
}
