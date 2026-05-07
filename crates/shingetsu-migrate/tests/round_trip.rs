//! Property test for the conversion-derive facade: a corpus of
//! structs round-trips through both backends and produces identical
//! observable behavior.
//!
//! Each fixture uses a single `#[derive(shingetsu_migrate::LuaTable)]`
//! that emits both shingetsu-side and mlua-side conversion impls.  No
//! parallel `#[serde(...)]` annotations are needed; the same
//! `#[lua(...)]` attributes drive both engines via shared codegen.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::LuaTable;

// ---------------------------------------------------------------------------
// Test corpus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, LuaTable)]
struct Simple {
    name: String,
    count: i64,
}

#[derive(Debug, Clone, PartialEq, LuaTable)]
struct WithOptional {
    label: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, LuaTable)]
struct Renamed {
    #[lua(rename = "x-pos")]
    x: i64,
    #[lua(default = 7)]
    y: i64,
}

#[derive(Debug, Clone, PartialEq, LuaTable)]
struct Outer {
    name: String,
    pos_x: f64,
    pos_y: f64,
}

// ---------------------------------------------------------------------------
// Round-trip helpers
// ---------------------------------------------------------------------------

fn round_trip_through_shingetsu<T>(input: T) -> T
where
    T: shingetsu_migrate::shingetsu::FromLua + shingetsu_migrate::shingetsu::IntoLua,
{
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(input);
    shingetsu_migrate::shingetsu::FromLua::from_lua(v).expect("shingetsu round-trip from_lua")
}

fn round_trip_through_mlua<T>(input: &T) -> T
where
    T: ::mlua::IntoLua + ::mlua::FromLua + Clone,
{
    let lua = ::mlua::Lua::new();
    let v = ::mlua::IntoLua::into_lua(input.clone(), &lua).expect("mlua into_lua");
    <T as ::mlua::FromLua>::from_lua(v, &lua).expect("mlua from_lua")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn simple_struct_round_trips_through_both_engines() {
    let original = Simple {
        name: "alex".to_owned(),
        count: 42,
    };
    let via_shingetsu = round_trip_through_shingetsu(original.clone());
    let via_mlua = round_trip_through_mlua(&original);
    k9::assert_equal!(via_shingetsu, original);
    k9::assert_equal!(via_mlua, original);
}

#[test]
fn optional_some_round_trips_through_both_engines() {
    let original = WithOptional {
        label: "title".to_owned(),
        note: Some("hello".to_owned()),
    };
    let via_shingetsu = round_trip_through_shingetsu(original.clone());
    let via_mlua = round_trip_through_mlua(&original);
    k9::assert_equal!(via_shingetsu, original);
    k9::assert_equal!(via_mlua, original);
}

#[test]
fn optional_none_round_trips_through_both_engines() {
    // Both engines should treat a missing optional as None.
    let original = WithOptional {
        label: "title".to_owned(),
        note: None,
    };
    let via_shingetsu = round_trip_through_shingetsu(original.clone());
    let via_mlua = round_trip_through_mlua(&original);
    k9::assert_equal!(via_shingetsu, original);
    k9::assert_equal!(via_mlua, original);
}

#[test]
fn renamed_field_round_trips_through_both_engines() {
    // Confirms `#[lua(rename = ...)]` works on both engines from a
    // single attribute spelling on the user's type.
    let original = Renamed { x: 3, y: 11 };
    let via_shingetsu = round_trip_through_shingetsu(original.clone());
    let via_mlua = round_trip_through_mlua(&original);
    k9::assert_equal!(via_shingetsu, original);
    k9::assert_equal!(via_mlua, original);
}

#[test]
fn default_field_supplies_value_when_absent_via_shingetsu() {
    // Build a lua table missing the `y` field via shingetsu, decode,
    // and confirm the default fires.
    let table = shingetsu_migrate::shingetsu::Table::new();
    table
        .raw_set(
            shingetsu_migrate::shingetsu::Value::string("x-pos"),
            shingetsu_migrate::shingetsu::Value::Integer(5),
        )
        .expect("set x-pos");
    let v: Renamed = shingetsu_migrate::shingetsu::FromLua::from_lua(
        shingetsu_migrate::shingetsu::Value::Table(table),
    )
    .expect("from_lua");
    k9::assert_equal!(v, Renamed { x: 5, y: 7 });
}

#[test]
fn default_field_supplies_value_when_absent_via_mlua() {
    // Same scenario through the mlua engine.
    let lua = ::mlua::Lua::new();
    let table = lua.create_table().expect("create_table");
    table.set("x-pos", 5).expect("set x-pos");
    let v: Renamed = <Renamed as ::mlua::FromLua>::from_lua(::mlua::Value::Table(table), &lua)
        .expect("from_lua");
    k9::assert_equal!(v, Renamed { x: 5, y: 7 });
}

#[test]
fn struct_with_floats_round_trips_through_both_engines() {
    let original = Outer {
        name: "pos".to_owned(),
        pos_x: 1.5,
        pos_y: -2.25,
    };
    let via_shingetsu = round_trip_through_shingetsu(original.clone());
    let via_mlua = round_trip_through_mlua(&original);
    k9::assert_equal!(via_shingetsu, original);
    k9::assert_equal!(via_mlua, original);
}
