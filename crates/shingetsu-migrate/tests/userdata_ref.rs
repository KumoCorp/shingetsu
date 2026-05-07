//! Confirms `shingetsu_migrate::UserDataRef<T>` decodes a userdata
//! operand on both engines so a single metamethod body can compare
//! or operate between two of the same userdata type.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::UserDataRef;
use std::sync::Arc;

struct Pair {
    x: i64,
    y: i64,
}

#[shingetsu_migrate::userdata]
impl Pair {
    /// `a + b` collapsed to a single integer sum.  Lua's `+`
    /// operator only consumes one return value from `__add`, so a
    /// tuple-returning metamethod would silently drop everything
    /// past the first.
    #[lua_metamethod(Add)]
    fn add_mm(&self, other: UserDataRef<Self>) -> i64 {
        self.x + other.x + self.y + other.y
    }

    /// `a < b` between two `Pair` userdata; lexicographic on
    /// `(x, y)`.  `__lt` exercises the same `UserDataRef<T>`
    /// operand path on both engines.
    #[lua_metamethod(Lt)]
    fn lt_mm(&self, other: UserDataRef<Self>) -> bool {
        (self.x, self.y) < (other.x, other.y)
    }

    /// `a == b` between two `Pair` userdata; named `eq_mm` to
    /// avoid shadowing `PartialEq::eq` if the type ever derives
    /// `PartialEq`.
    #[lua_metamethod(Eq)]
    fn eq_mm(&self, other: UserDataRef<Self>) -> bool {
        self.x == other.x && self.y == other.y
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_userdata_ref_add_and_lt() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::Value;

    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    env.set_global("a", Value::Userdata(Arc::new(Pair { x: 1, y: 2 })));
    env.set_global("b", Value::Userdata(Arc::new(Pair { x: 3, y: 4 })));

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("return a + b, a < b, b < a")
    .await
    .expect("compile");
    let func = shingetsu::Function::lua(bc.top_level, vec![]);
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![
            Value::Integer(10),
            Value::Boolean(true),
            Value::Boolean(false),
        ]
    );
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_userdata_ref_add_and_lt() {
    use mlua::Lua;

    let lua = Lua::new();
    lua.globals().set("a", Pair { x: 1, y: 2 }).expect("set a");
    lua.globals().set("b", Pair { x: 3, y: 4 }).expect("set b");
    lua.globals()
        .set("clone", Pair { x: 1, y: 2 })
        .expect("set clone");

    let result: (i64, bool, bool, bool, bool) = lua
        .load("return a + b, a < b, b < a, a == clone, a == b")
        .eval()
        .expect("eval");
    k9::assert_equal!(result, (10, true, false, true, false));
}
