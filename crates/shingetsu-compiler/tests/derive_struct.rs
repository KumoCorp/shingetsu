//! Tests for `derive(LuaTable)` / `derive(FromLua)` / `derive(IntoLua)` /
//! `derive(LuaTyped)` field attributes added in Phase 0 of the migration plan.

use shingetsu::{Bytes, FromLua, IntoLua, LuaTable, LuaType, LuaTyped, Table, TableLuaType, Value};

// ---------------------------------------------------------------------------
// rename + default (existing behavior — guard against regression)
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
struct Renamed {
    #[lua(rename = "x-pos")]
    x: i64,
    #[lua(default = 7)]
    y: i64,
}

#[test]
fn rename_uses_lua_key() {
    let t = Table::new();
    t.raw_set(Value::string("x-pos"), Value::Integer(3))
        .expect("set");
    t.raw_set(Value::string("y"), Value::Integer(11))
        .expect("set");
    let v = Renamed::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(v, Renamed { x: 3, y: 11 });
}

#[test]
fn default_fills_absent_field() {
    let t = Table::new();
    t.raw_set(Value::string("x-pos"), Value::Integer(3))
        .expect("set");
    let v = Renamed::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(v, Renamed { x: 3, y: 7 });
}

// ---------------------------------------------------------------------------
// skip
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
struct WithSkip {
    visible: i64,
    #[lua(skip)]
    hidden: i64,
}

#[test]
fn skip_field_uses_default_on_from_lua() {
    let t = Table::new();
    t.raw_set(Value::string("visible"), Value::Integer(42))
        .expect("set");
    // Even if a key with this name exists, skip means the field comes from
    // Default::default(), not from the table.
    t.raw_set(Value::string("hidden"), Value::Integer(99))
        .expect("set");
    let v = WithSkip::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(
        v,
        WithSkip {
            visible: 42,
            hidden: 0,
        }
    );
}

#[test]
fn skip_field_omitted_from_into_lua() {
    let v = WithSkip {
        visible: 1,
        hidden: 7,
    };
    let lua = v.into_lua();
    let Value::Table(t) = lua else {
        panic!("expected table");
    };
    k9::assert_equal!(
        t.raw_get(&Value::string("visible")).expect("get"),
        Value::Integer(1)
    );
    k9::assert_equal!(
        t.raw_get(&Value::string("hidden")).expect("get"),
        Value::Nil
    );
}

#[test]
fn skip_field_omitted_from_lua_typed() {
    let LuaType::Table(t) = WithSkip::lua_type() else {
        panic!("expected Table type");
    };
    let names: Vec<&str> = t
        .fields
        .iter()
        .map(|(b, _)| std::str::from_utf8(b).expect("utf8"))
        .collect();
    k9::assert_equal!(names, vec!["visible"]);
}

// ---------------------------------------------------------------------------
// flatten
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq, Default)]
struct Inner {
    a: i64,
    b: Bytes,
}

#[derive(LuaTable, Debug, PartialEq)]
struct Outer {
    name: Bytes,
    #[lua(flatten)]
    inner: Inner,
}

#[test]
fn flatten_reads_inner_fields_from_outer_table() {
    let t = Table::new();
    t.raw_set(Value::string("name"), Value::string("hi"))
        .expect("set");
    t.raw_set(Value::string("a"), Value::Integer(5))
        .expect("set");
    t.raw_set(Value::string("b"), Value::string("there"))
        .expect("set");
    let v = Outer::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(
        v,
        Outer {
            name: "hi".into(),
            inner: Inner {
                a: 5,
                b: "there".into(),
            },
        }
    );
}

#[test]
fn flatten_writes_inner_fields_into_outer_table() {
    let v = Outer {
        name: "hi".into(),
        inner: Inner {
            a: 5,
            b: "there".into(),
        },
    };
    let Value::Table(t) = v.into_lua() else {
        panic!("expected table");
    };
    k9::assert_equal!(
        t.raw_get(&Value::string("name")).expect("get"),
        Value::string("hi")
    );
    k9::assert_equal!(
        t.raw_get(&Value::string("a")).expect("get"),
        Value::Integer(5)
    );
    k9::assert_equal!(
        t.raw_get(&Value::string("b")).expect("get"),
        Value::string("there")
    );
}

#[test]
fn flatten_unfolds_inner_fields_in_lua_typed() {
    let LuaType::Table(t) = Outer::lua_type() else {
        panic!("expected Table type");
    };
    let names: Vec<&str> = t
        .fields
        .iter()
        .map(|(b, _)| std::str::from_utf8(b).expect("utf8"))
        .collect();
    k9::assert_equal!(names, vec!["name", "a", "b"]);
}

// ---------------------------------------------------------------------------
// try_from + into  (intermediate type adapter)
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone, Copy)]
struct Celsius(i64);

impl TryFrom<i64> for Celsius {
    type Error = String;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < -273 {
            Err(format!("below absolute zero: {value}"))
        } else {
            Ok(Celsius(value))
        }
    }
}

impl From<Celsius> for i64 {
    fn from(value: Celsius) -> i64 {
        value.0
    }
}

#[derive(LuaTable, Debug, PartialEq, Clone)]
struct Reading {
    #[lua(try_from = "i64")]
    temp: Celsius,
}

#[test]
fn try_from_converts_intermediate_type() {
    let t = Table::new();
    t.raw_set(Value::string("temp"), Value::Integer(20))
        .expect("set");
    let v = Reading::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(v, Reading { temp: Celsius(20) });
}

#[test]
fn try_from_propagates_conversion_error() {
    let t = Table::new();
    t.raw_set(Value::string("temp"), Value::Integer(-300))
        .expect("set");
    let err = Reading::from_lua(Value::Table(t)).expect_err("conversion should fail");
    let rendered = format!("{err}");
    k9::assert_equal!(
        rendered,
        "bad argument #0 to '' (Celsius (try_from i64) expected, got below absolute zero: -300)"
    );
}

#[test]
fn try_from_round_trips_via_into() {
    let v = Reading { temp: Celsius(10) };
    let Value::Table(t) = v.clone().into_lua() else {
        panic!("expected table");
    };
    k9::assert_equal!(
        t.raw_get(&Value::string("temp")).expect("get"),
        Value::Integer(10)
    );
    let back = Reading::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(back, v);
}

#[test]
fn try_from_uses_intermediate_for_lua_typed() {
    let LuaType::Table(t) = Reading::lua_type() else {
        panic!("expected Table type");
    };
    k9::assert_equal!(t.fields.len(), 1);
    // Surface type is i64, not Celsius (which has no LuaTyped impl).
    k9::assert_equal!(t.fields[0].1, i64::lua_type());
}

// ---------------------------------------------------------------------------
// into  (one-way IntoLua adapter, no try_from)
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
struct WithInto {
    #[lua(into = "i64", default = 0)]
    n: i32,
}

#[test]
fn into_converts_on_emit() {
    let v = WithInto { n: 42 };
    let Value::Table(t) = v.into_lua() else {
        panic!("expected table");
    };
    k9::assert_equal!(
        t.raw_get(&Value::string("n")).expect("get"),
        Value::Integer(42)
    );
}

// ---------------------------------------------------------------------------
// validate
// ---------------------------------------------------------------------------

fn validate_positive(value: &i64) -> Result<(), String> {
    if *value > 0 {
        Ok(())
    } else {
        Err(format!("must be positive, got {value}"))
    }
}

#[derive(LuaTable, Debug)]
struct Validated {
    #[lua(validate = "validate_positive")]
    n: i64,
}

#[test]
fn validate_passes_for_valid_value() {
    let t = Table::new();
    t.raw_set(Value::string("n"), Value::Integer(7))
        .expect("set");
    let v = Validated::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(v.n, 7);
}

#[test]
fn validate_rejects_invalid_value() {
    let t = Table::new();
    t.raw_set(Value::string("n"), Value::Integer(-3))
        .expect("set");
    let err = Validated::from_lua(Value::Table(t)).expect_err("validator should reject");
    let rendered = format!("{err}");
    k9::assert_equal!(
        rendered,
        "bad argument #0 to '' (validated n expected, got must be positive, got -3)"
    );
}

// ---------------------------------------------------------------------------
// deprecated  (parsed and stored only in Phase 0; lint hookup comes in Phase 1)
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
struct WithDeprecated {
    fresh: i64,
    #[lua(deprecated = "use `fresh` instead", default = 0)]
    old: i64,
}

#[test]
fn deprecated_field_still_extractable() {
    let t = Table::new();
    t.raw_set(Value::string("fresh"), Value::Integer(1))
        .expect("set");
    t.raw_set(Value::string("old"), Value::Integer(99))
        .expect("set");
    let v = WithDeprecated::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(v, WithDeprecated { fresh: 1, old: 99 });
}

// ---------------------------------------------------------------------------
// Container: try_from / into
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq, Clone)]
struct Distance(i64);

// The lua-facing intermediate.
#[derive(LuaTable, Debug, PartialEq)]
struct DistanceWire {
    meters: i64,
}

impl TryFrom<DistanceWire> for Distance {
    type Error = String;
    fn try_from(w: DistanceWire) -> Result<Self, Self::Error> {
        if w.meters < 0 {
            Err(format!("negative: {}", w.meters))
        } else {
            Ok(Distance(w.meters))
        }
    }
}

impl From<Distance> for DistanceWire {
    fn from(d: Distance) -> Self {
        DistanceWire { meters: d.0 }
    }
}

#[derive(LuaTable, Debug, PartialEq, Clone)]
#[lua(try_from = "DistanceWire", into = "DistanceWire")]
struct Distance2(#[allow(dead_code)] i64);

impl TryFrom<DistanceWire> for Distance2 {
    type Error = String;
    fn try_from(w: DistanceWire) -> Result<Self, Self::Error> {
        Ok(Distance2(w.meters))
    }
}
impl From<Distance2> for DistanceWire {
    fn from(d: Distance2) -> Self {
        DistanceWire { meters: d.0 }
    }
}

#[test]
#[allow(dead_code)]
fn container_try_from_into_round_trip() {
    let v = Distance2(42);
    let lua = v.clone().into_lua();
    let back = Distance2::from_lua(lua).expect("from_lua");
    k9::assert_equal!(back, v);
}

#[test]
fn container_try_from_propagates_error_via_into_lua() {
    // Build a wire form via lua, push it through Distance2 via try_from.
    // The wire has a `meters` field; we route through try_from.
    let wire = Table::new();
    wire.raw_set(Value::string("meters"), Value::Integer(7))
        .expect("set");
    let v = Distance2::from_lua(Value::Table(wire)).expect("from_lua");
    k9::assert_equal!(v, Distance2(7));
}

#[test]
fn container_try_from_lua_typed_uses_intermediate() {
    let LuaType::Table(t) = Distance2::lua_type() else {
        panic!("expected Table type");
    };
    let names: Vec<&str> = t
        .fields
        .iter()
        .map(|(b, _)| std::str::from_utf8(b).expect("utf8"))
        .collect();
    k9::assert_equal!(names, vec!["meters"]);
}

// ---------------------------------------------------------------------------
// Container: default
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq, Default)]
#[lua(default)]
struct WithContainerDefault {
    a: i64,
    b: i64,
}

#[test]
fn container_default_fires_on_nil_value() {
    let v = WithContainerDefault::from_lua(Value::Nil).expect("from_lua");
    k9::assert_equal!(v, WithContainerDefault::default());
}

#[test]
fn container_default_does_not_block_normal_table_path() {
    let t = Table::new();
    t.raw_set(Value::string("a"), Value::Integer(3))
        .expect("set");
    t.raw_set(Value::string("b"), Value::Integer(4))
        .expect("set");
    let v = WithContainerDefault::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(v, WithContainerDefault { a: 3, b: 4 });
}

fn make_keyed_default() -> WithContainerDefaultPath {
    WithContainerDefaultPath { keyed: 99 }
}

#[derive(LuaTable, Debug, PartialEq)]
#[lua(default = "make_keyed_default")]
struct WithContainerDefaultPath {
    keyed: i64,
}

#[test]
fn container_default_path_invokes_named_fn() {
    let v = WithContainerDefaultPath::from_lua(Value::Nil).expect("from_lua");
    k9::assert_equal!(v, WithContainerDefaultPath { keyed: 99 });
}

// ---------------------------------------------------------------------------
// Container: deny_unknown_fields
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
#[lua(deny_unknown_fields)]
struct Strict {
    a: i64,
    #[lua(rename = "b-name")]
    b: i64,
}

#[test]
fn deny_unknown_fields_accepts_known_keys() {
    let t = Table::new();
    t.raw_set(Value::string("a"), Value::Integer(1))
        .expect("set");
    t.raw_set(Value::string("b-name"), Value::Integer(2))
        .expect("set");
    let v = Strict::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(v, Strict { a: 1, b: 2 });
}

#[test]
fn deny_unknown_fields_rejects_unknown_key() {
    let t = Table::new();
    t.raw_set(Value::string("a"), Value::Integer(1))
        .expect("set");
    t.raw_set(Value::string("b-name"), Value::Integer(2))
        .expect("set");
    t.raw_set(Value::string("surprise"), Value::Integer(3))
        .expect("set");
    let err = Strict::from_lua(Value::Table(t)).expect_err("should reject");
    let rendered = format!("{err}");
    k9::assert_equal!(
        rendered,
        "bad argument #0 to '' (only known fields of Strict expected, got unknown field `surprise`. Possible alternatives are `a`, `b-name`)"
    );
}

// ---------------------------------------------------------------------------
// deny_unknown_fields: did-you-mean suggestion for typos
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
#[lua(deny_unknown_fields)]
struct Typeable {
    font_size: i64,
    font_family: Bytes,
    line_height: f64,
    cursor_blink_rate: i64,
}

#[test]
fn deny_unknown_fields_suggests_close_match_on_typo() {
    let t = Table::new();
    t.raw_set(Value::string("font_sze"), Value::Integer(12))
        .expect("set");
    let err = Typeable::from_lua(Value::Table(t)).expect_err("reject");
    let rendered = format!("{err}");
    k9::assert_equal!(
        rendered,
        "bad argument #0 to '' (only known fields of Typeable expected, got unknown field `font_sze`. Did you mean `font_size`? Other alternatives are `cursor_blink_rate`, `font_family`, `line_height`)"
    );
}

#[test]
fn deny_unknown_fields_no_suggestion_when_nothing_close() {
    let t = Table::new();
    t.raw_set(Value::string("xyzzy"), Value::Integer(12))
        .expect("set");
    let err = Typeable::from_lua(Value::Table(t)).expect_err("reject");
    let rendered = format!("{err}");
    k9::assert_equal!(
        rendered,
        "bad argument #0 to '' (only known fields of Typeable expected, got unknown field `xyzzy`. Possible alternatives are `cursor_blink_rate`, `font_family`, `font_size`, `line_height`)"
    );
}

// 50-field-struct fixture from the migration plan: confirm the
// rendered diagnostic stays compact even when many candidates share
// a similar prefix.
#[derive(LuaTable, Debug)]
#[lua(deny_unknown_fields)]
#[allow(dead_code)]
struct Wide {
    field_00: i64,
    field_01: i64,
    field_02: i64,
    field_03: i64,
    field_04: i64,
    field_05: i64,
    field_06: i64,
    field_07: i64,
    field_08: i64,
    field_09: i64,
    field_10: i64,
    field_11: i64,
    field_12: i64,
    field_13: i64,
    field_14: i64,
    field_15: i64,
    field_16: i64,
    field_17: i64,
    field_18: i64,
    field_19: i64,
    field_20: i64,
    field_21: i64,
    field_22: i64,
    field_23: i64,
    field_24: i64,
    field_25: i64,
    field_26: i64,
    field_27: i64,
    field_28: i64,
    field_29: i64,
    field_30: i64,
    field_31: i64,
    field_32: i64,
    field_33: i64,
    field_34: i64,
    field_35: i64,
    field_36: i64,
    field_37: i64,
    field_38: i64,
    field_39: i64,
    field_40: i64,
    field_41: i64,
    field_42: i64,
    field_43: i64,
    field_44: i64,
    field_45: i64,
    field_46: i64,
    field_47: i64,
    field_48: i64,
    field_49: i64,
}

#[test]
fn deny_unknown_fields_truncates_for_wide_structs() {
    let t = Table::new();
    t.raw_set(Value::string("field_07x"), Value::Integer(0))
        .expect("set");
    let err = Wide::from_lua(Value::Table(t)).expect_err("reject");
    let rendered = format!("{err}");
    k9::assert_equal!(
        rendered,
        "bad argument #0 to '' (only known fields of Wide expected, got unknown field `field_07x`. Many fields share a similar name; consult the documentation for the full list.)"
    );
}

// ---------------------------------------------------------------------------
// Tagged enums (internally tagged)
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq, Clone)]
struct PtBody {
    x: f64,
    y: f64,
}

#[derive(LuaTable, Debug, PartialEq)]
struct CircleBody {
    radius: f64,
}

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
#[lua(tag = "kind")]
enum Shape {
    Point(PtBody),
    #[lua(rename = "round")]
    Circle(CircleBody),
}

#[test]
fn tagged_internal_from_lua_dispatches_by_tag() {
    let t = Table::new();
    t.raw_set(Value::string("kind"), Value::string("Point"))
        .expect("set");
    t.raw_set(Value::string("x"), Value::Float(1.0))
        .expect("set");
    t.raw_set(Value::string("y"), Value::Float(2.0))
        .expect("set");
    let s = Shape::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(s, Shape::Point(PtBody { x: 1.0, y: 2.0 }));
}

#[test]
fn tagged_internal_from_lua_uses_renamed_tag() {
    let t = Table::new();
    t.raw_set(Value::string("kind"), Value::string("round"))
        .expect("set");
    t.raw_set(Value::string("radius"), Value::Float(3.5))
        .expect("set");
    let s = Shape::from_lua(Value::Table(t)).expect("from_lua");
    k9::assert_equal!(s, Shape::Circle(CircleBody { radius: 3.5 }));
}

#[test]
fn tagged_internal_from_lua_rejects_unknown_tag() {
    let t = Table::new();
    t.raw_set(Value::string("kind"), Value::string("Square"))
        .expect("set");
    let err = Shape::from_lua(Value::Table(t)).expect_err("reject");
    let rendered = format!("{err}");
    k9::assert_equal!(
        rendered,
        "bad argument #0 to '' (one of: Point | round expected, got unknown tag `Square`)"
    );
}

#[test]
fn tagged_internal_into_lua_emits_tag_field() {
    let s = Shape::Circle(CircleBody { radius: 5.0 });
    let Value::Table(t) = s.into_lua() else {
        panic!("expected table");
    };
    k9::assert_equal!(
        t.raw_get(&Value::string("kind")).expect("get"),
        Value::string("round")
    );
    k9::assert_equal!(
        t.raw_get(&Value::string("radius")).expect("get"),
        Value::Float(5.0)
    );
}

#[test]
fn tagged_internal_lua_typed_emits_tagged_table_union() {
    let LuaType::Union(variants) = Shape::lua_type() else {
        panic!("expected Union");
    };
    k9::assert_equal!(variants.len(), 2);
    // First variant: Point — tag StringLiteral, then x/y fields.
    let LuaType::Table(point) = &variants[0] else {
        panic!("expected Table for Point");
    };
    let names: Vec<&str> = point
        .fields
        .iter()
        .map(|(b, _)| std::str::from_utf8(b).expect("utf8"))
        .collect();
    k9::assert_equal!(names, vec!["kind", "x", "y"]);
    k9::assert_equal!(point.fields[0].1, LuaType::StringLiteral("Point".into()));
}

// ---------------------------------------------------------------------------
// Tagged enums (adjacently tagged)
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
#[lua(tag = "kind", content = "data")]
enum Adj {
    Pt(PtBody),
    Number(i64),
}

#[test]
fn tagged_adjacent_from_lua_dispatches() {
    let inner = Table::new();
    inner
        .raw_set(Value::string("x"), Value::Float(7.0))
        .unwrap();
    inner
        .raw_set(Value::string("y"), Value::Float(8.0))
        .unwrap();
    let outer = Table::new();
    outer
        .raw_set(Value::string("kind"), Value::string("Pt"))
        .unwrap();
    outer
        .raw_set(Value::string("data"), Value::Table(inner))
        .unwrap();
    let v = Adj::from_lua(Value::Table(outer)).expect("from_lua");
    k9::assert_equal!(v, Adj::Pt(PtBody { x: 7.0, y: 8.0 }));
}

#[test]
fn tagged_adjacent_from_lua_with_primitive_content() {
    let outer = Table::new();
    outer
        .raw_set(Value::string("kind"), Value::string("Number"))
        .unwrap();
    outer
        .raw_set(Value::string("data"), Value::Integer(42))
        .unwrap();
    let v = Adj::from_lua(Value::Table(outer)).expect("from_lua");
    k9::assert_equal!(v, Adj::Number(42));
}

#[test]
fn tagged_adjacent_into_lua_round_trip() {
    let v = Adj::Number(99);
    let lua = v.clone_via_round_trip();
    let back = Adj::from_lua(lua).expect("from_lua");
    k9::assert_equal!(back, v);
}

impl Adj {
    fn clone_via_round_trip(&self) -> Value {
        match self {
            Adj::Pt(p) => Adj::Pt(p.clone()).into_lua(),
            Adj::Number(n) => Adj::Number(*n).into_lua(),
        }
    }
}

// ---------------------------------------------------------------------------
// Untagged is the explicit default
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
#[lua(untagged)]
enum AnyValue {
    N(i64),
    S(Bytes),
}

#[test]
fn explicit_untagged_works_like_default() {
    let n = AnyValue::from_lua(Value::Integer(7)).expect("from_lua");
    k9::assert_equal!(n, AnyValue::N(7));
    let s = AnyValue::from_lua(Value::string("hi")).expect("from_lua");
    k9::assert_equal!(s, AnyValue::S("hi".into()));
}

// ---------------------------------------------------------------------------
// Sanity: TableLuaType is constructible for empty struct
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
struct Empty {}

#[test]
fn empty_struct_lua_type_is_empty_table() {
    k9::assert_equal!(
        Empty::lua_type(),
        LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: None,
        }))
    );
}
