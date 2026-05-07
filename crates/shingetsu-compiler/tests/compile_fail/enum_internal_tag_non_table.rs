use shingetsu::{FromLua, IntoLua};

// Internally-tagged enums require every variant's inner type to implement
// `LuaTableShape` — i.e. produce a `Value::Table` from `IntoLua`.  `i64`
// produces `Value::Integer`, so this should fail to compile rather than
// panic at runtime.
#[derive(FromLua, IntoLua)]
#[lua(tag = "kind")]
enum Bad {
    Number(i64),
}

fn main() {}
