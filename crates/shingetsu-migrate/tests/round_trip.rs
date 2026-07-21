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

#[derive(Debug, Clone, Copy, PartialEq, Eq, LuaRepr)]
#[lua(rename_all = "kebab-case")]
enum ResizePolicy {
    No,
    SmallestWins,
    #[lua(rename = "custom-wins")]
    CustomOverride,
}

#[derive(Debug, Clone, PartialEq, LuaRepr)]
#[lua(rename_all = "kebab-case")]
struct RetryPolicy {
    max_retries: i64,
    backoff_ms: i64,
    #[lua(rename = "jitter%")]
    jitter_pct: i64,
}

// ---------------------------------------------------------------------------
// Round-trip helpers
// ---------------------------------------------------------------------------

fn round_trip_through_shingetsu<T>(input: T) -> T
where
    T: shingetsu_migrate::shingetsu::FromLua + shingetsu_migrate::shingetsu::IntoLua,
{
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(input);
    shingetsu_migrate::shingetsu::FromLua::from_lua(
        v,
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .expect("shingetsu round-trip from_lua")
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
        k9::assert_equal!(
            round_trip_through_shingetsu(original.clone()),
            original.clone()
        );
        k9::assert_equal!(round_trip_through_mlua(&original), original.clone());
    }

    // serde-default repr is the variant name; `#[lua(rename)]` overrides.
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(Strategy::TimerWheel);
    k9::assert_equal!(v, shingetsu_migrate::shingetsu::Value::string("TimerWheel"));
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(Strategy::SingletonTimerWheelV2);
    k9::assert_equal!(
        v,
        shingetsu_migrate::shingetsu::Value::string("singleton_v2")
    );

    // mlua side honors the rename too.
    let lua = ::mlua::Lua::new();
    let s: ::mlua::String = lua
        .unpack(::mlua::IntoLua::into_lua(Strategy::SingletonTimerWheelV2, &lua).unwrap())
        .unwrap();
    k9::assert_equal!(s.as_bytes().as_ref(), b"singleton_v2");

    // An unknown string and a wrong type both fail on each engine, and
    // each engine names the enum and lists its variants.  The shingetsu
    // side reports a bad-argument error whose position and function name
    // the call machinery fills in at a real call site; the mlua side
    // reports its native conversion error, which mlua's own argument
    // machinery wraps with the call context.
    let env = shingetsu_migrate::shingetsu::GlobalEnv::new();
    let sh_unknown = shingetsu_migrate::shingetsu::FromLua::from_lua(
        shingetsu_migrate::shingetsu::Value::string("bogus"),
        &env,
    )
    .map(|_: Strategy| ())
    .unwrap_err();
    k9::assert_equal!(
        sh_unknown.to_string(),
        "bad argument #0 to '' (one of `TimerWheel`, `SkipList`, `singleton_v2` expected, got `bogus`)"
    );
    let sh_wrong = shingetsu_migrate::shingetsu::FromLua::from_lua(
        shingetsu_migrate::shingetsu::Value::Boolean(true),
        &env,
    )
    .map(|_: Strategy| ())
    .unwrap_err();
    k9::assert_equal!(
        sh_wrong.to_string(),
        "bad argument #0 to '' (one of `TimerWheel`, `SkipList`, `singleton_v2` expected, got boolean)"
    );

    let ml_unknown = <Strategy as ::mlua::FromLua>::from_lua(
        ::mlua::Value::String(lua.create_string("bogus").unwrap()),
        &lua,
    )
    .map(|_| ())
    .unwrap_err();
    k9::assert_equal!(
        ml_unknown.to_string(),
        "error converting Lua string to Strategy (unknown Strategy variant `bogus`; expected one of `TimerWheel`, `SkipList`, `singleton_v2`)"
    );
    let ml_wrong = <Strategy as ::mlua::FromLua>::from_lua(::mlua::Value::Boolean(true), &lua)
        .map(|_| ())
        .unwrap_err();
    k9::assert_equal!(
        ml_wrong.to_string(),
        "error converting Lua boolean to Strategy (expected one of `TimerWheel`, `SkipList`, `singleton_v2`)"
    );
}

#[test]
fn struct_rename_all_kebab_case_round_trips_on_both_engines() {
    let original = RetryPolicy {
        max_retries: 3,
        backoff_ms: 250,
        jitter_pct: 10,
    };
    k9::assert_equal!(round_trip_through_shingetsu(original.clone()), original);
    k9::assert_equal!(round_trip_through_mlua(&original), original);

    // The shingetsu side encodes snake_case fields as kebab-case keys;
    // an explicit per-field rename wins over the container default.
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(original.clone());
    let tbl = match v {
        shingetsu_migrate::shingetsu::Value::Table(t) => t,
        other => panic!("expected table, got {other:?}"),
    };
    let mut entries: Vec<(String, shingetsu_migrate::shingetsu::Value)> = Vec::new();
    let mut cursor = shingetsu_migrate::shingetsu::Value::Nil;
    while let Some((k, v)) = tbl.next(&cursor).unwrap() {
        let key = match &k {
            shingetsu_migrate::shingetsu::Value::String(b) => {
                String::from_utf8_lossy(b.as_ref()).into_owned()
            }
            other => panic!("expected string key, got {other:?}"),
        };
        entries.push((key, v));
        cursor = k;
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    k9::assert_equal!(
        entries,
        vec![
            (
                "backoff-ms".to_owned(),
                shingetsu_migrate::shingetsu::Value::Integer(250)
            ),
            (
                "jitter%".to_owned(),
                shingetsu_migrate::shingetsu::Value::Integer(10)
            ),
            (
                "max-retries".to_owned(),
                shingetsu_migrate::shingetsu::Value::Integer(3)
            ),
        ]
    );

    // The original snake_case spelling no longer decodes (the
    // kebab-cased form is canonical).  Default-less fields become a
    // FromLua error when their key is absent.
    let bad = shingetsu_migrate::shingetsu::Table::new();
    bad.raw_set(
        shingetsu_migrate::shingetsu::Value::string("max_retries"),
        shingetsu_migrate::shingetsu::Value::Integer(3),
    )
    .unwrap();
    bad.raw_set(
        shingetsu_migrate::shingetsu::Value::string("backoff_ms"),
        shingetsu_migrate::shingetsu::Value::Integer(250),
    )
    .unwrap();
    bad.raw_set(
        shingetsu_migrate::shingetsu::Value::string("jitter%"),
        shingetsu_migrate::shingetsu::Value::Integer(10),
    )
    .unwrap();
    shingetsu_migrate::shingetsu::FromLua::from_lua(
        shingetsu_migrate::shingetsu::Value::Table(bad),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .map(|_: RetryPolicy| ())
    .unwrap_err();
}

#[test]
fn rename_all_kebab_case_round_trips_on_both_engines() {
    for original in [
        ResizePolicy::No,
        ResizePolicy::SmallestWins,
        ResizePolicy::CustomOverride,
    ] {
        k9::assert_equal!(round_trip_through_shingetsu(original), original);
        k9::assert_equal!(round_trip_through_mlua(&original), original);
    }

    // shingetsu side encodes as the kebab-cased string; an explicit
    // `#[lua(rename)]` on a variant wins over the container default.
    k9::assert_equal!(
        shingetsu_migrate::shingetsu::IntoLua::into_lua(ResizePolicy::No),
        shingetsu_migrate::shingetsu::Value::string("no")
    );
    k9::assert_equal!(
        shingetsu_migrate::shingetsu::IntoLua::into_lua(ResizePolicy::SmallestWins),
        shingetsu_migrate::shingetsu::Value::string("smallest-wins")
    );
    k9::assert_equal!(
        shingetsu_migrate::shingetsu::IntoLua::into_lua(ResizePolicy::CustomOverride),
        shingetsu_migrate::shingetsu::Value::string("custom-wins")
    );

    // mlua side encodes the same strings.
    let lua = ::mlua::Lua::new();
    for (v, expected) in [
        (ResizePolicy::No, &b"no"[..]),
        (ResizePolicy::SmallestWins, &b"smallest-wins"[..]),
        (ResizePolicy::CustomOverride, &b"custom-wins"[..]),
    ] {
        let encoded: ::mlua::String = lua
            .unpack(::mlua::IntoLua::into_lua(v, &lua).unwrap())
            .unwrap();
        k9::assert_equal!(encoded.as_bytes().as_ref(), expected);
    }

    // The original PascalCase spelling is rejected on both engines
    // (it's the kebab-cased form that's now canonical).
    shingetsu_migrate::shingetsu::FromLua::from_lua(
        shingetsu_migrate::shingetsu::Value::string("SmallestWins"),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .map(|_: ResizePolicy| ())
    .unwrap_err();
    <ResizePolicy as ::mlua::FromLua>::from_lua(
        ::mlua::Value::String(lua.create_string("SmallestWins").unwrap()),
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
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
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

    let s: StrOrPoint = FromLua::from_lua(
        Value::string("hello"),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .expect("string -> Str");
    k9::assert_equal!(s, StrOrPoint::Str("hello".to_owned()));

    let t = Table::new();
    t.raw_set(Value::string("px"), Value::Integer(3)).unwrap();
    t.raw_set(Value::string("py"), Value::Integer(4)).unwrap();
    let p: StrOrPoint = FromLua::from_lua(
        Value::Table(t),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .expect("table -> Typed");
    k9::assert_equal!(p, StrOrPoint::Typed(PointMsg { px: 3, py: 4 }));

    <StrOrPoint as FromLua>::from_lua(
        Value::Boolean(true),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
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
    let p: StrOrPoint = <StrOrPoint as ::mlua::FromLua>::from_lua(::mlua::Value::Table(t), &lua)
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
    let back: IntOrPoint = FromLua::from_lua(v, &shingetsu_migrate::shingetsu::GlobalEnv::new())
        .expect("Num round-trip");
    k9::assert_equal!(back, IntOrPoint::Num(7));

    let v = IntoLua::into_lua(IntOrPoint::Typed(PointMsg { px: 1, py: 2 }));
    assert!(matches!(v, Value::Table(_)));
    let back: IntOrPoint = FromLua::from_lua(v, &shingetsu_migrate::shingetsu::GlobalEnv::new())
        .expect("Typed round-trip");
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

    let v = ::mlua::IntoLua::into_lua(IntOrPoint::Typed(PointMsg { px: 1, py: 2 }), &lua)
        .expect("into_lua Typed");
    assert!(matches!(v, ::mlua::Value::Table(_)));
    let back: IntOrPoint =
        <IntOrPoint as ::mlua::FromLua>::from_lua(v, &lua).expect("Typed round-trip");
    k9::assert_equal!(back, IntOrPoint::Typed(PointMsg { px: 1, py: 2 }));
}

// `#[lua(nil)]` unit variant in an untagged IntoLua enum (as used
// by `mod-redis`'s `RedisReply`): the unit variant projects to Lua
// nil while newtype variants delegate to their inner `IntoLua`.
#[derive(shingetsu_migrate::IntoLua, shingetsu_migrate::LuaTyped)]
enum NilOrInt {
    #[lua(nil)]
    Nothing,
    Num(i64),
}

#[test]
fn nil_unit_variant_into_lua_both_engines() {
    use shingetsu_migrate::shingetsu::{IntoLua, LuaType, LuaTyped, Value};

    k9::assert_equal!(IntoLua::into_lua(NilOrInt::Nothing), Value::Nil);
    k9::assert_equal!(IntoLua::into_lua(NilOrInt::Num(5)), Value::Integer(5));
    // The type surface is a union that includes nil.
    match <NilOrInt as LuaTyped>::lua_type() {
        LuaType::Union(parts) => assert!(parts.contains(&LuaType::Nil)),
        other => panic!("expected Union, got {other:?}"),
    }

    let lua = ::mlua::Lua::new();
    assert!(matches!(
        ::mlua::IntoLua::into_lua(NilOrInt::Nothing, &lua).expect("nil"),
        ::mlua::Value::Nil
    ));
    assert!(matches!(
        ::mlua::IntoLua::into_lua(NilOrInt::Num(5), &lua).expect("int"),
        ::mlua::Value::Integer(5)
    ));
}

// Mixed integer/string key, as used by `mod-regex`'s `captures`
// (numbered + named groups in one table).
#[derive(Debug, Clone, PartialEq, Eq, Hash, LuaRepr)]
enum MapKey {
    Int(i64),
    Str(String),
}

// A unit-string `LuaRepr` enum nested as a variant of an untagged
// `FromLua` enum (mod-filesystem's `SeekArg`): the string-repr
// inner must not be rejected by the mlua kind-guard, which models
// unknown paths as TABLE.
#[derive(Debug, Clone, PartialEq, LuaRepr)]
enum Whence {
    #[lua(rename = "set")]
    Set,
    #[lua(rename = "cur")]
    Cur,
    #[lua(rename = "end")]
    End,
}

#[derive(Debug, PartialEq, FromLuaDerive)]
enum WhenceOrPos {
    W(Whence),
    P(i64),
}

#[test]
fn nested_unit_string_enum_in_untagged_from_lua_both_engines() {
    use shingetsu_migrate::shingetsu::{FromLua, Value};

    let w: WhenceOrPos = FromLua::from_lua(
        Value::string("cur"),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .expect("string -> W");
    k9::assert_equal!(w, WhenceOrPos::W(Whence::Cur));
    let p: WhenceOrPos = FromLua::from_lua(
        Value::Integer(4),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .expect("int -> P");
    k9::assert_equal!(p, WhenceOrPos::P(4));
    <WhenceOrPos as FromLua>::from_lua(
        Value::string("bogus"),
        &shingetsu_migrate::shingetsu::GlobalEnv::new(),
    )
    .map(|_| ())
    .unwrap_err();

    let lua = ::mlua::Lua::new();
    let w: WhenceOrPos = <WhenceOrPos as ::mlua::FromLua>::from_lua(
        ::mlua::Value::String(lua.create_string("cur").unwrap()),
        &lua,
    )
    .expect("string -> W (mlua)");
    k9::assert_equal!(w, WhenceOrPos::W(Whence::Cur));
    let p: WhenceOrPos =
        <WhenceOrPos as ::mlua::FromLua>::from_lua(::mlua::Value::Integer(4), &lua)
            .expect("int -> P (mlua)");
    k9::assert_equal!(p, WhenceOrPos::P(4));
    <WhenceOrPos as ::mlua::FromLua>::from_lua(
        ::mlua::Value::String(lua.create_string("bogus").unwrap()),
        &lua,
    )
    .map(|_| ())
    .unwrap_err();
}

#[test]
fn enum_keyed_map_round_trips_through_both_engines() {
    use std::collections::HashMap;

    let mut original: HashMap<MapKey, String> = HashMap::new();
    original.insert(MapKey::Int(0), "whole".to_owned());
    original.insert(MapKey::Int(1), "first".to_owned());
    original.insert(MapKey::Str("name".to_owned()), "first".to_owned());

    let via_shingetsu = round_trip_through_shingetsu(original.clone());
    let via_mlua = round_trip_through_mlua(&original);
    k9::assert_equal!(via_shingetsu, original.clone());
    k9::assert_equal!(via_mlua, original);
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

// LuaCallback: a Lua function captured from policy and invoked from
// Rust via the engine-native handle (mlua side).
#[test]
fn lua_callback_mlua_invoke() {
    use shingetsu_migrate::LuaCallback;

    let lua = ::mlua::Lua::new();
    let func: ::mlua::Function = lua
        .load(r#"function(rec) return rec.type == "Delivery" end"#)
        .eval()
        .expect("compile filter");
    let cb = <LuaCallback as ::mlua::FromLua>::from_lua(::mlua::Value::Function(func), &lua)
        .expect("FromLua");

    let (lua_ref, func_ref) = cb.as_mlua().expect("mlua backend");
    let delivery = lua_ref.create_table().unwrap();
    delivery.set("type", "Delivery").unwrap();
    let reception = lua_ref.create_table().unwrap();
    reception.set("type", "Reception").unwrap();

    let is_delivery: bool = func_ref.call(delivery).expect("call delivery");
    assert!(is_delivery);
    let is_delivery: bool = func_ref.call(reception).expect("call reception");
    assert!(!is_delivery);
}

// ---------------------------------------------------------------------------
// Externally-tagged enum (inferred from mixed unit + newtype variants).
// Unit variants encode as bare strings; newtype variants encode as
// `{ tag = inner }`.  Mirrored on both engines via the facade derive.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, IntoLuaDerive, FromLuaDerive)]
#[lua(rename_all = "snake_case")]
enum Decision {
    Allow,
    Deny(String),
}

#[test]
fn externally_tagged_unit_round_trips_on_both_engines() {
    let original = Decision::Allow;
    k9::assert_equal!(round_trip_through_shingetsu(original.clone()), original);
    k9::assert_equal!(round_trip_through_mlua(&original), original);
}

#[test]
fn externally_tagged_newtype_round_trips_on_both_engines() {
    let original = Decision::Deny("nope".to_string());
    k9::assert_equal!(round_trip_through_shingetsu(original.clone()), original);
    k9::assert_equal!(round_trip_through_mlua(&original), original);
}

#[test]
fn externally_tagged_shingetsu_unit_emits_bare_string() {
    let v = shingetsu_migrate::shingetsu::IntoLua::into_lua(Decision::Allow);
    k9::assert_equal!(v, shingetsu_migrate::shingetsu::Value::string("allow"));
}

#[test]
fn externally_tagged_mlua_unit_emits_bare_string() {
    let lua = ::mlua::Lua::new();
    let v = ::mlua::IntoLua::into_lua(Decision::Allow, &lua).expect("mlua into_lua");
    let s = match v {
        ::mlua::Value::String(s) => s,
        other => panic!("expected string, got {other:?}"),
    };
    k9::assert_equal!(s.to_str().expect("utf8").to_owned(), "allow".to_string());
}

#[test]
fn externally_tagged_mlua_newtype_emits_single_key_table() {
    let lua = ::mlua::Lua::new();
    let v =
        ::mlua::IntoLua::into_lua(Decision::Deny("bad".to_string()), &lua).expect("mlua into_lua");
    let t = match v {
        ::mlua::Value::Table(t) => t,
        other => panic!("expected table, got {other:?}"),
    };
    let inner: String = t.raw_get("deny").expect("raw_get");
    k9::assert_equal!(inner, "bad".to_string());
}

#[test]
fn externally_tagged_shingetsu_accepts_string_or_table() {
    use shingetsu_migrate::shingetsu::{FromLua, GlobalEnv, Table, Value};
    let env = GlobalEnv::new();
    k9::assert_equal!(
        Decision::from_lua(Value::string("allow"), &env).expect("from_lua"),
        Decision::Allow
    );
    let t = Table::new();
    t.raw_set(Value::string("deny"), Value::string("x"))
        .expect("set");
    k9::assert_equal!(
        Decision::from_lua(Value::Table(t), &env).expect("from_lua"),
        Decision::Deny("x".to_string())
    );
}

#[test]
fn externally_tagged_mlua_accepts_string_or_table() {
    let lua = ::mlua::Lua::new();
    let v_str = ::mlua::Value::String(lua.create_string("allow").expect("string"));
    k9::assert_equal!(
        <Decision as ::mlua::FromLua>::from_lua(v_str, &lua).expect("from_lua"),
        Decision::Allow
    );
    let t = lua.create_table().expect("create_table");
    t.raw_set("deny", "x").expect("raw_set");
    k9::assert_equal!(
        <Decision as ::mlua::FromLua>::from_lua(::mlua::Value::Table(t), &lua).expect("from_lua"),
        Decision::Deny("x".to_string())
    );
}
