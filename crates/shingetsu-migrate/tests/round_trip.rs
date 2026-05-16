//! Property test for the conversion-derive facade: a corpus of
//! structs round-trips through both backends and produces identical
//! observable behavior.
//!
//! Each fixture uses a single `#[derive(shingetsu_migrate::LuaRepr)]`
//! that emits both shingetsu-side and mlua-side conversion impls.  No
//! parallel `#[serde(...)]` annotations are needed; the same
//! `#[lua(...)]` attributes drive both engines via shared codegen.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::{FromLua as FromLuaDerive, IntoLua as IntoLuaDerive, LuaRepr};

// Untagged newtype enum: a string decodes to `Str`, a table to
// `Typed` (the `StringOr<T>`-style accessor-setter shape).  Only
// `FromLua` is needed (setter parameter position).
#[derive(Debug, PartialEq, FromLuaDerive)]
enum StrOrPoint {
    Str(String),
    Typed(PointMsg),
}

#[derive(Debug, PartialEq, LuaRepr)]
struct PointMsg {
    px: i64,
    py: i64,
}

// Untagged newtype enum exercising the **IntoLua** mlua-side
// mirror (symmetric to `StrOrPoint`'s FromLua): each variant
// delegates to its inner type's IntoLua — a scalar stays a scalar,
// a struct stays a table.
#[derive(Debug, PartialEq, IntoLuaDerive, FromLuaDerive)]
enum IntOrPoint {
    Num(i64),
    Typed(PointMsg),
}

// ---------------------------------------------------------------------------
// Test corpus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, LuaRepr)]
struct Simple {
    name: String,
    count: i64,
}

#[derive(Debug, Clone, PartialEq, LuaRepr)]
struct WithOptional {
    label: String,
    note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, LuaRepr)]
struct Renamed {
    #[lua(rename = "x-pos")]
    x: i64,
    #[lua(default = 7)]
    y: i64,
}

#[derive(Debug, Clone, PartialEq, LuaRepr)]
struct Outer {
    name: String,
    pos_x: f64,
    pos_y: f64,
}

#[derive(Debug, Clone, PartialEq, LuaRepr)]
enum Strategy {
    TimerWheel,
    SkipList,
    #[lua(rename = "singleton_v2")]
    SingletonTimerWheelV2,
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
fn unit_enum_round_trips_and_uses_string_repr_on_both_engines() {
    for original in [
        Strategy::TimerWheel,
        Strategy::SkipList,
        Strategy::SingletonTimerWheelV2,
    ] {
        k9::assert_equal!(round_trip_through_shingetsu(original.clone()), original.clone());
        k9::assert_equal!(round_trip_through_mlua(&original), original.clone());
    }

    // serde-default repr is the variant name; `#[lua(rename)]` overrides.
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(Strategy::TimerWheel);
    k9::assert_equal!(v, shingetsu_migrate::shingetsu::Value::string("TimerWheel"));
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(Strategy::SingletonTimerWheelV2);
    k9::assert_equal!(v, shingetsu_migrate::shingetsu::Value::string("singleton_v2"));

    // mlua side honors the rename too.
    let lua = ::mlua::Lua::new();
    let s: ::mlua::String = lua
        .unpack(::mlua::IntoLua::into_lua(Strategy::SingletonTimerWheelV2, &lua).unwrap())
        .unwrap();
    k9::assert_equal!(s.as_bytes().as_ref(), b"singleton_v2");

    // unknown variant is an error on both engines.
    shingetsu_migrate::shingetsu::FromLua::from_lua(
        shingetsu_migrate::shingetsu::Value::string("bogus"),
    )
    .map(|_: Strategy| ())
    .unwrap_err();
    <Strategy as ::mlua::FromLua>::from_lua(
        ::mlua::Value::String(lua.create_string("bogus").unwrap()),
        &lua,
    )
    .unwrap_err();
}

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
fn untagged_newtype_enum_from_lua_shingetsu() {
    use shingetsu_migrate::shingetsu::{FromLua, Table, Value};

    let s: StrOrPoint =
        FromLua::from_lua(Value::string("hello")).expect("string -> Str");
    k9::assert_equal!(s, StrOrPoint::Str("hello".to_owned()));

    let t = Table::new();
    t.raw_set(Value::string("px"), Value::Integer(3)).unwrap();
    t.raw_set(Value::string("py"), Value::Integer(4)).unwrap();
    let p: StrOrPoint =
        FromLua::from_lua(Value::Table(t)).expect("table -> Typed");
    k9::assert_equal!(p, StrOrPoint::Typed(PointMsg { px: 3, py: 4 }));

    <StrOrPoint as FromLua>::from_lua(Value::Boolean(true))
        .map(|_| ())
        .unwrap_err();
}

#[test]
fn untagged_newtype_enum_from_lua_mlua() {
    let lua = ::mlua::Lua::new();

    let s: StrOrPoint = <StrOrPoint as ::mlua::FromLua>::from_lua(
        ::mlua::Value::String(lua.create_string("hello").unwrap()),
        &lua,
    )
    .expect("string -> Str");
    k9::assert_equal!(s, StrOrPoint::Str("hello".to_owned()));

    let t = lua.create_table().unwrap();
    t.set("px", 3).unwrap();
    t.set("py", 4).unwrap();
    let p: StrOrPoint =
        <StrOrPoint as ::mlua::FromLua>::from_lua(::mlua::Value::Table(t), &lua)
            .expect("table -> Typed");
    k9::assert_equal!(p, StrOrPoint::Typed(PointMsg { px: 3, py: 4 }));

    <StrOrPoint as ::mlua::FromLua>::from_lua(::mlua::Value::Boolean(true), &lua)
        .map(|_| ())
        .unwrap_err();
}

#[test]
fn untagged_newtype_enum_into_lua_shingetsu() {
    use shingetsu_migrate::shingetsu::{FromLua, IntoLua, Value};

    let v = IntoLua::into_lua(IntOrPoint::Num(7));
    k9::assert_equal!(v, Value::Integer(7));
    let back: IntOrPoint = FromLua::from_lua(v).expect("Num round-trip");
    k9::assert_equal!(back, IntOrPoint::Num(7));

    let v = IntoLua::into_lua(IntOrPoint::Typed(PointMsg { px: 1, py: 2 }));
    assert!(matches!(v, Value::Table(_)));
    let back: IntOrPoint = FromLua::from_lua(v).expect("Typed round-trip");
    k9::assert_equal!(back, IntOrPoint::Typed(PointMsg { px: 1, py: 2 }));
}

#[test]
fn untagged_newtype_enum_into_lua_mlua() {
    let lua = ::mlua::Lua::new();

    let v = ::mlua::IntoLua::into_lua(IntOrPoint::Num(7), &lua).expect("into_lua Num");
    assert!(matches!(v, ::mlua::Value::Integer(7)));
    let back: IntOrPoint =
        <IntOrPoint as ::mlua::FromLua>::from_lua(v, &lua).expect("Num round-trip");
    k9::assert_equal!(back, IntOrPoint::Num(7));

    let v = ::mlua::IntoLua::into_lua(
        IntOrPoint::Typed(PointMsg { px: 1, py: 2 }),
        &lua,
    )
    .expect("into_lua Typed");
    assert!(matches!(v, ::mlua::Value::Table(_)));
    let back: IntOrPoint =
        <IntOrPoint as ::mlua::FromLua>::from_lua(v, &lua).expect("Typed round-trip");
    k9::assert_equal!(back, IntOrPoint::Typed(PointMsg { px: 1, py: 2 }));
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
