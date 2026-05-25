//! Round-trips a representative `wezterm-dynamic` struct through the
//! `DynamicLua<T>` bridge on both engines.  The struct mirrors the
//! shape of `wezterm::config::Config` (nested struct + enum tag +
//! optional + array of structs) without depending on wezterm
//! itself.

#![cfg(all(
    feature = "mlua-backend",
    feature = "shingetsu-backend",
    feature = "dynamic"
))]

use shingetsu_migrate::DynamicLua;
use wezterm_dynamic::{FromDynamic, ToDynamic};

#[derive(Debug, Clone, PartialEq, FromDynamic, ToDynamic)]
struct Color {
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Debug, Clone, PartialEq, FromDynamic, ToDynamic)]
enum Shadow {
    None,
    Solid { color: Color, offset: i32 },
}

impl Default for Shadow {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, PartialEq, FromDynamic, ToDynamic)]
struct Pane {
    name: String,
    width: u32,
    fg: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, FromDynamic, ToDynamic)]
struct Layout {
    title: String,
    panes: Vec<Pane>,
    shadow: Shadow,
    debug: bool,
    line_height: f64,
}

fn fixture() -> Layout {
    Layout {
        title: "demo".to_owned(),
        panes: vec![
            Pane {
                name: "left".to_owned(),
                width: 80,
                fg: Some(Color { r: 255, g: 0, b: 0 }),
            },
            Pane {
                name: "right".to_owned(),
                width: 120,
                fg: None,
            },
        ],
        shadow: Shadow::Solid {
            color: Color {
                r: 10,
                g: 20,
                b: 30,
            },
            offset: 4,
        },
        debug: true,
        line_height: 1.25,
    }
}

// ---------------------------------------------------------------------------
// shingetsu engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_dynamic_lua_round_trips() {
    use shingetsu_migrate::shingetsu::{FromLua, IntoLua};

    let original = fixture();
    let lua_value = DynamicLua(original.clone()).into_lua();
    let decoded: DynamicLua<Layout> =
        FromLua::from_lua(lua_value, &shingetsu_migrate::shingetsu::GlobalEnv::new())
            .expect("from_lua");
    k9::assert_equal!(decoded.into_inner(), original);
}

#[tokio::test]
async fn shingetsu_dynamic_lua_round_trips_through_script() {
    use shingetsu_migrate::shingetsu;
    use shingetsu_migrate::shingetsu::{IntoLua, Value};

    // Round-trip through actual Lua execution: set the value as a
    // global, fetch it back via a script that just `return`s it.
    let env = shingetsu::GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");

    let original = fixture();
    env.set_global("layout", DynamicLua(original.clone()).into_lua());

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile(
        "return layout.title, layout.panes[1].name, layout.panes[2].fg, layout.shadow.Solid.offset",
    )
    .await
    .expect("compile");
    let func = bc.into_function();
    let res = shingetsu::Task::new(env, func, shingetsu::valuevec![])
        .await
        .expect("task");
    k9::assert_equal!(
        res,
        shingetsu::valuevec![
            Value::string("demo"),
            Value::string("left"),
            Value::Nil,
            Value::Integer(4),
        ]
    );
}

// ---------------------------------------------------------------------------
// mlua engine
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_dynamic_lua_round_trips() {
    use mlua::Lua;

    let lua = Lua::new();
    let original = fixture();
    let lua_value: mlua::Value =
        mlua::IntoLua::into_lua(DynamicLua(original.clone()), &lua).expect("into_lua");
    let decoded: DynamicLua<Layout> = mlua::FromLua::from_lua(lua_value, &lua).expect("from_lua");
    k9::assert_equal!(decoded.into_inner(), original);
}

#[tokio::test]
async fn mlua_dynamic_lua_round_trips_through_script() {
    use mlua::Lua;

    let lua = Lua::new();
    let original = fixture();
    lua.globals()
        .set("layout", DynamicLua(original.clone()))
        .expect("set");
    let result: (String, String, Option<String>, i32) = lua
        .load(
            "return layout.title, layout.panes[1].name, layout.panes[2].fg, layout.shadow.Solid.offset",
        )
        .eval()
        .expect("eval");
    k9::assert_equal!(result, ("demo".to_owned(), "left".to_owned(), None, 4));
}
