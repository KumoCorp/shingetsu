mod common;

use shingetsu::{
    valuevec, Bytes, FromLua, IntoLua, IntoLuaMulti, LuaRepr, LuaTyped, Value, Variadic,
};

// ---------------------------------------------------------------------------
// Basic enum: disjoint types
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum IntOrStr {
    Int(i64),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn from_lua_enum_integer() {
    let v = IntOrStr::from_lua(Value::Integer(42), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, IntOrStr::Int(42));
}

#[test]
fn from_lua_enum_string() {
    let v =
        IntOrStr::from_lua(Value::string("hello"), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, IntOrStr::Str(Bytes::from("hello")));
}

#[test]
fn from_lua_enum_wrong_type() {
    let err = IntOrStr::from_lua(Value::Boolean(true), &shingetsu::GlobalEnv::new()).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "bad argument #0 to '' (string | number expected, got boolean)"
    );
}

#[test]
fn into_lua_enum_integer() {
    let v = IntOrStr::Int(99).into_lua();
    k9::assert_equal!(v, Value::Integer(99));
}

#[test]
fn into_lua_enum_string() {
    let v = IntOrStr::Str(Bytes::from("world")).into_lua();
    k9::assert_equal!(v, Value::string("world"));
}

// ---------------------------------------------------------------------------
// LuaTyped: union type
// ---------------------------------------------------------------------------

#[test]
fn lua_typed_union() {
    let ty = IntOrStr::lua_type();
    k9::assert_equal!(ty.to_string(), "number | string");
}

// ---------------------------------------------------------------------------
// Auto-ordering by discriminant size
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum StringOrNum {
    // Declared number first, but string should be tried first because
    // {String} doesn't overlap with {Integer, Float}.
    Num(f64),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn auto_order_string_tried_first() {
    // A string value should match the Str variant.
    let v =
        StringOrNum::from_lua(Value::string("hi"), &shingetsu::GlobalEnv::new()).expect("from_lua");
    assert!(matches!(v, StringOrNum::Str(_)));
}

#[test]
fn auto_order_number_matches_num() {
    let v =
        StringOrNum::from_lua(Value::Float(3.14), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, StringOrNum::Num(3.14));
}

#[test]
fn auto_order_lua_typed() {
    let ty = StringOrNum::lua_type();
    k9::assert_equal!(ty.to_string(), "number | string");
}

// ---------------------------------------------------------------------------
// Value variant (catch-all, must be last)
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug)]
enum StringOrAny {
    Str(shingetsu_vm::Bytes),
    Any(Value),
}

#[test]
fn value_variant_is_last() {
    // String values should match Str, not Any.
    let v = StringOrAny::from_lua(Value::string("test"), &shingetsu::GlobalEnv::new())
        .expect("from_lua");
    assert!(matches!(v, StringOrAny::Str(_)));
}

#[test]
fn value_variant_catches_rest() {
    // A boolean doesn't match Str, so it falls through to Any.
    let v = StringOrAny::from_lua(Value::Boolean(true), &shingetsu::GlobalEnv::new())
        .expect("from_lua");
    assert!(matches!(v, StringOrAny::Any(Value::Boolean(true))));
}

// ---------------------------------------------------------------------------
// Table-backed struct inside an enum
// ---------------------------------------------------------------------------

#[derive(LuaRepr, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(FromLua, IntoLua, LuaTyped, Debug)]
enum PointOrStr {
    Pt(Point),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn struct_variant_from_table() {
    let table = shingetsu::Table::new();
    table
        .raw_set(Value::string("x"), Value::Float(1.0))
        .expect("set");
    table
        .raw_set(Value::string("y"), Value::Float(2.0))
        .expect("set");
    let v =
        PointOrStr::from_lua(Value::Table(table), &shingetsu::GlobalEnv::new()).expect("from_lua");
    match v {
        PointOrStr::Pt(p) => {
            k9::assert_equal!(p.x, 1.0);
            k9::assert_equal!(p.y, 2.0);
        }
        _ => panic!("expected Pt variant"),
    }
}

#[test]
fn struct_variant_string_still_works() {
    let v = PointOrStr::from_lua(Value::string("hello"), &shingetsu::GlobalEnv::new())
        .expect("from_lua");
    assert!(matches!(v, PointOrStr::Str(_)));
}

// ---------------------------------------------------------------------------
// IntoLua round-trip
// ---------------------------------------------------------------------------

#[test]
fn round_trip_int() {
    let original = IntOrStr::Int(123);
    let lua_val = original.into_lua();
    let back = IntOrStr::from_lua(lua_val, &shingetsu_vm::GlobalEnv::new()).expect("round trip");
    k9::assert_equal!(back, IntOrStr::Int(123));
}

#[test]
fn round_trip_string() {
    let original = IntOrStr::Str(Bytes::from("abc"));
    let lua_val = original.into_lua();
    let back = IntOrStr::from_lua(lua_val, &shingetsu_vm::GlobalEnv::new()).expect("round trip");
    k9::assert_equal!(back, IntOrStr::Str(Bytes::from("abc")));
}

// ---------------------------------------------------------------------------
// Function variant
// ---------------------------------------------------------------------------

#[derive(FromLua)]
#[allow(dead_code)]
enum LevelOrFn {
    Level(i64),
    Func(shingetsu::Function),
}

#[test]
fn function_variant() {
    let func = shingetsu::Function::wrap("test", || -> Result<(), shingetsu::VmError> { Ok(()) });
    let v =
        LevelOrFn::from_lua(Value::Function(func), &shingetsu::GlobalEnv::new()).expect("from_lua");
    assert!(matches!(v, LevelOrFn::Func(_)));
}

#[test]
fn level_variant() {
    let v = LevelOrFn::from_lua(Value::Integer(3), &shingetsu::GlobalEnv::new()).expect("from_lua");
    assert!(matches!(v, LevelOrFn::Level(3)));
}

// ---------------------------------------------------------------------------
// Bool variant
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum BoolOrStr {
    Bool(bool),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn bool_variant() {
    let v =
        BoolOrStr::from_lua(Value::Boolean(true), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, BoolOrStr::Bool(true));
}

// ---------------------------------------------------------------------------
// Table variant (explicit Table type)
// ---------------------------------------------------------------------------

#[derive(FromLua)]
#[allow(dead_code)]
enum TableOrStr {
    Tbl(shingetsu::Table),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn table_variant() {
    let t = shingetsu::Table::new();
    let v = TableOrStr::from_lua(Value::Table(t), &shingetsu::GlobalEnv::new()).expect("from_lua");
    assert!(matches!(v, TableOrStr::Tbl(_)));
}

// ---------------------------------------------------------------------------
// Integration: use enum as a function parameter via Lua
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq, Clone)]
enum NumOrStr {
    Num(i64),
    Str(shingetsu_vm::Bytes),
}

// ---------------------------------------------------------------------------
// Single-variant enum (degenerate case)
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum SingleVariant {
    Only(i64),
}

#[test]
fn single_variant_matches() {
    let v = SingleVariant::from_lua(Value::Integer(42), &shingetsu::GlobalEnv::new())
        .expect("from_lua");
    k9::assert_equal!(v, SingleVariant::Only(42));
}

#[test]
fn single_variant_rejects() {
    let err =
        SingleVariant::from_lua(Value::string("nope"), &shingetsu::GlobalEnv::new()).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(msg, "bad argument #0 to '' (number expected, got string)");
}

#[test]
fn single_variant_into_lua() {
    let v = SingleVariant::Only(7).into_lua();
    k9::assert_equal!(v, Value::Integer(7));
}

#[test]
fn single_variant_lua_typed() {
    // Single-element union is still Union([Integer]).
    let ty = SingleVariant::lua_type();
    k9::assert_equal!(ty.to_string(), "number");
}

// ---------------------------------------------------------------------------
// Three+ variants with different set sizes
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum ThreeWay {
    // Str ({String}) declared first, Num ({Integer, Float}) second,
    // Any ({all}) last — sorted to: Str, Num, Any.
    Num(f64),
    Str(shingetsu_vm::Bytes),
    Any(Value),
}

#[test]
fn three_way_string_matches_narrowest() {
    let v =
        ThreeWay::from_lua(Value::string("hi"), &shingetsu::GlobalEnv::new()).expect("from_lua");
    assert!(matches!(v, ThreeWay::Str(_)));
}

#[test]
fn three_way_number_matches_mid() {
    let v = ThreeWay::from_lua(Value::Float(2.5), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, ThreeWay::Num(2.5));
}

#[test]
fn three_way_other_matches_any() {
    let v =
        ThreeWay::from_lua(Value::Boolean(false), &shingetsu::GlobalEnv::new()).expect("from_lua");
    assert!(matches!(v, ThreeWay::Any(Value::Boolean(false))));
}

#[test]
fn three_way_lua_typed() {
    let ty = ThreeWay::lua_type();
    // Declaration order: Num(f64), Str(Bytes), Any(Value).
    k9::assert_equal!(ty.to_string(), "number | string | any");
}

// ---------------------------------------------------------------------------
// f64 without i64 sibling — integer coerces to float
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum FloatOrStr {
    Num(f64),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn f64_alone_accepts_integer() {
    // f64's FromLua accepts Value::Integer, so this should work.
    let v =
        FloatOrStr::from_lua(Value::Integer(10), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, FloatOrStr::Num(10.0));
}

#[test]
fn f64_alone_accepts_float() {
    let v =
        FloatOrStr::from_lua(Value::Float(1.5), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, FloatOrStr::Num(1.5));
}

// ---------------------------------------------------------------------------
// nil passed to enum without Value catch-all
// ---------------------------------------------------------------------------

#[test]
fn nil_rejected_without_catchall() {
    let err = IntOrStr::from_lua(Value::Nil, &shingetsu_vm::GlobalEnv::new()).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "bad argument #0 to '' (string | number expected, got nil)"
    );
}

// ---------------------------------------------------------------------------
// IntoLua only (without FromLua) — independent derive
// ---------------------------------------------------------------------------

#[derive(IntoLua, Debug)]
enum IntoOnly {
    Int(i64),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn into_lua_only_integer() {
    let v = IntoOnly::Int(42).into_lua();
    k9::assert_equal!(v, Value::Integer(42));
}

#[test]
fn into_lua_only_string() {
    let v = IntoOnly::Str(Bytes::from("hi")).into_lua();
    k9::assert_equal!(v, Value::string("hi"));
}

// ---------------------------------------------------------------------------
// Arc<dyn Userdata> variant
// ---------------------------------------------------------------------------

#[derive(shingetsu::UserData)]
struct MyUserdata;

#[derive(FromLua)]
#[allow(dead_code)]
enum UserdataOrStr {
    Ud(std::sync::Arc<dyn shingetsu::Userdata>),
    Str(shingetsu_vm::Bytes),
}

#[test]
fn userdata_variant_matches() {
    let ud: std::sync::Arc<dyn shingetsu::Userdata> = std::sync::Arc::new(MyUserdata);
    let v = UserdataOrStr::from_lua(Value::Userdata(ud), &shingetsu::GlobalEnv::new())
        .expect("from_lua");
    assert!(matches!(v, UserdataOrStr::Ud(_)));
}

#[test]
fn userdata_variant_string_falls_through() {
    let v = UserdataOrStr::from_lua(Value::string("hi"), &shingetsu::GlobalEnv::new())
        .expect("from_lua");
    assert!(matches!(v, UserdataOrStr::Str(_)));
}

#[test]
fn userdata_variant_rejects_other() {
    let result = UserdataOrStr::from_lua(Value::Integer(1), &shingetsu::GlobalEnv::new());
    let err = result.map(|_| ()).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "bad argument #0 to '' (userdata | string expected, got number)"
    );
}

// ---------------------------------------------------------------------------
// Struct variant IntoLua round-trip
// ---------------------------------------------------------------------------

#[test]
fn struct_variant_into_lua_round_trip() {
    let original = PointOrStr::Pt(Point { x: 3.0, y: 4.0 });
    let lua_val = original.into_lua();
    // Should be a table with x=3.0, y=4.0.
    let back = PointOrStr::from_lua(lua_val, &shingetsu_vm::GlobalEnv::new()).expect("round trip");
    match back {
        PointOrStr::Pt(p) => {
            k9::assert_equal!(p.x, 3.0);
            k9::assert_equal!(p.y, 4.0);
        }
        _ => panic!("expected Pt variant"),
    }
}

// ---------------------------------------------------------------------------
// Value catch-all IntoLua
// ---------------------------------------------------------------------------

#[test]
fn value_catchall_into_lua() {
    let v = StringOrAny::Any(Value::Boolean(true)).into_lua();
    k9::assert_equal!(v, Value::Boolean(true));
}

#[test]
fn value_catchall_into_lua_str() {
    let v = StringOrAny::Str(Bytes::from("test")).into_lua();
    k9::assert_equal!(v, Value::string("test"));
}

// ---------------------------------------------------------------------------
// Error message from function call context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enum_error_has_function_name_and_position() {
    use shingetsu::Function;

    let env = common::new_env();
    let func = Function::wrap(
        "myfunc",
        |_val: IntOrStr| -> Result<(), shingetsu::VmError> { Ok(()) },
    );
    env.set_global("myfunc", Value::Function(func));

    common::assert_runtime_error_with_env!(
        env,
        "return myfunc(true)",
        "\
error: bad argument #1 to 'myfunc' (string | number expected, got boolean)
 --> test.lua:1:15
  |
1 | return myfunc(true)
  |               ^^^^ bad argument #1 to 'myfunc' (string | number expected, got boolean)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

// ---------------------------------------------------------------------------
// Integration: use enum as a function parameter via Lua
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enum_as_native_function_param() {
    use shingetsu::Function;

    let env = common::new_env();
    let classify = Function::wrap(
        "classify",
        |val: NumOrStr| -> Result<String, shingetsu::VmError> {
            match val {
                NumOrStr::Num(_) => Ok("number".to_string()),
                NumOrStr::Str(_) => Ok("string".to_string()),
            }
        },
    );
    env.set_global("classify", Value::Function(classify));

    let r = common::run_with_env(env.clone(), r#"return classify(42)"#).await;
    k9::assert_equal!(r[0], Value::string("number"));

    let r = common::run_with_env(env, r#"return classify("hello")"#).await;
    k9::assert_equal!(r[0], Value::string("string"));
}

// ===========================================================================
// IntoLuaMulti derive for enums
// ===========================================================================

#[derive(IntoLuaMulti)]
enum FindResult {
    Match(i64, i64),
    MatchCaptures(i64, i64, Variadic),
    NotFound,
}

#[test]
fn into_lua_multi_unit_variant() {
    let result = FindResult::NotFound.into_lua_multi();
    k9::assert_equal!(result, valuevec![Value::Nil]);
}

#[test]
fn into_lua_multi_tuple_variant() {
    let result = FindResult::Match(3, 7).into_lua_multi();
    k9::assert_equal!(result, valuevec![Value::Integer(3), Value::Integer(7)]);
}

#[test]
fn into_lua_multi_tuple_with_variadic() {
    let captures = Variadic(valuevec![Value::string("hello"), Value::string("world")]);
    let result = FindResult::MatchCaptures(1, 5, captures).into_lua_multi();
    k9::assert_equal!(
        result,
        valuevec![
            Value::Integer(1),
            Value::Integer(5),
            Value::string("hello"),
            Value::string("world"),
        ]
    );
}

#[test]
fn into_lua_multi_tuple_with_empty_variadic() {
    let result = FindResult::MatchCaptures(1, 5, Variadic(valuevec![])).into_lua_multi();
    k9::assert_equal!(result, valuevec![Value::Integer(1), Value::Integer(5)]);
}

// Single-field newtype variant
#[derive(IntoLuaMulti)]
enum SingleOrNil {
    Value(i64),
    Nil,
}

#[test]
fn into_lua_multi_newtype_variant() {
    let result = SingleOrNil::Value(42).into_lua_multi();
    k9::assert_equal!(result, valuevec![Value::Integer(42)]);
}

#[test]
fn into_lua_multi_newtype_nil() {
    let result = SingleOrNil::Nil.into_lua_multi();
    k9::assert_equal!(result, valuevec![Value::Nil]);
}

// Variant with nil placeholder + value (e.g. utf8.len error case)
#[derive(IntoLuaMulti)]
#[allow(dead_code)]
enum NilAndInt {
    Ok(i64),
    ErrAt(Value, i64),
}

#[test]
fn into_lua_multi_nil_placeholder() {
    let result = NilAndInt::ErrAt(Value::Nil, 42).into_lua_multi();
    k9::assert_equal!(result, valuevec![Value::Nil, Value::Integer(42)]);
}

// Standalone Variadic variant
#[derive(IntoLuaMulti)]
#[allow(dead_code)]
enum VarOrNil {
    Values(Variadic),
    Empty,
}

#[test]
fn into_lua_multi_standalone_variadic() {
    let result = VarOrNil::Values(Variadic(valuevec![Value::Integer(1), Value::Integer(2)]))
        .into_lua_multi();
    k9::assert_equal!(result, valuevec![Value::Integer(1), Value::Integer(2)]);
}

#[tokio::test]
async fn into_lua_multi_as_function_return() {
    use shingetsu::Function;

    let env = common::new_env();
    let find = Function::wrap("find", |n: i64| -> Result<FindResult, shingetsu::VmError> {
        if n > 0 {
            Ok(FindResult::Match(1, n))
        } else {
            Ok(FindResult::NotFound)
        }
    });
    env.set_global("find", Value::Function(find));

    let r: shingetsu::ValueVec = common::run_with_env(env.clone(), "return find(5)")
        .await
        .into();
    k9::assert_equal!(r, valuevec![Value::Integer(1), Value::Integer(5)]);

    let r: shingetsu::ValueVec = common::run_with_env(env, "return find(-1)").await.into();
    k9::assert_equal!(r, valuevec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// LuaTypedMulti — return type metadata
// ---------------------------------------------------------------------------

#[test]
fn lua_typed_multi_for_single_value() {
    use shingetsu::{Function, LuaType};

    // A function returning i64 should report lua_returns = Some([Integer]).
    let f = Function::wrap("add", |a: i64, b: i64| -> Result<i64, shingetsu::VmError> {
        Ok(a + b)
    });
    k9::assert_equal!(f.signature().lua_returns, Some(vec![LuaType::Number]));
}

#[test]
fn lua_typed_multi_for_tuple_return() {
    use shingetsu::{Function, LuaType};

    // A function returning (i64, String) should report both types.
    let f = Function::wrap(
        "pair",
        |x: i64| -> Result<(i64, String), shingetsu::VmError> { Ok((x, "hello".into())) },
    );
    k9::assert_equal!(
        f.signature().lua_returns,
        Some(vec![LuaType::Number, LuaType::String])
    );
}

#[test]
fn lua_typed_multi_for_unit_return() {
    use shingetsu::Function;

    let f = Function::wrap("noop", || -> Result<(), shingetsu::VmError> { Ok(()) });
    k9::assert_equal!(f.signature().lua_returns, Some(vec![]));
}

#[test]
fn lua_typed_multi_for_derived_enum() {
    use shingetsu::{Function, LuaType};

    let f = Function::wrap("find", |n: i64| -> Result<FindResult, shingetsu::VmError> {
        if n > 0 {
            Ok(FindResult::Match(1, n))
        } else {
            Ok(FindResult::NotFound)
        }
    });
    // FindResult { Match(i64, i64), MatchCaptures(i64, i64, Variadic), NotFound }
    // → (number, number) | (number, number, ...any) | nil
    k9::assert_equal!(
        f.signature().lua_returns,
        Some(vec![LuaType::Union(vec![
            LuaType::Tuple(vec![LuaType::Number, LuaType::Number]),
            LuaType::Tuple(vec![
                LuaType::Number,
                LuaType::Number,
                LuaType::Variadic(Box::new(LuaType::Any)),
            ]),
            LuaType::Nil,
        ])])
    );
}

#[test]
fn lua_typed_multi_display_rendering() {
    // FindResult's type should render as a readable union.
    let types = <FindResult as shingetsu::LuaTypedMulti>::lua_types();
    let rendered: Vec<String> = types.iter().map(|t| t.to_string()).collect();
    k9::assert_equal!(
        rendered,
        vec!["(number, number) | (number, number, ...any) | nil"]
    );
}

#[test]
fn lua_typed_multi_single_variant_no_union() {
    use shingetsu::LuaType;

    // An enum with a single variant should not produce a Union wrapper.
    #[derive(IntoLuaMulti)]
    #[allow(dead_code)]
    enum SingleReturn {
        Value(i64, String),
    }

    let types = <SingleReturn as shingetsu::LuaTypedMulti>::lua_types();
    k9::assert_equal!(
        types,
        vec![LuaType::Tuple(vec![LuaType::Number, LuaType::String])]
    );
}

// ===========================================================================
// Externally-tagged enums (inferred when an enum mixes unit + newtype
// variants).  Unit variants encode as a Lua string; newtype variants
// encode as `{ variant = inner }`.  Mirrors serde's default repr.
// ===========================================================================

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq, Clone)]
#[lua(rename_all = "snake_case")]
enum Decision {
    Allow,
    Deny(String),
}

#[test]
fn external_into_lua_unit_variant_is_string() {
    k9::assert_equal!(Decision::Allow.into_lua(), Value::string("allow"));
}

#[test]
fn external_into_lua_newtype_variant_is_single_key_table() {
    let v = Decision::Deny("nope".to_string()).into_lua();
    let table = match v {
        Value::Table(t) => t,
        other => panic!("expected table, got {other:?}"),
    };
    k9::assert_equal!(
        table.raw_get(&Value::string("deny")).expect("raw_get"),
        Value::string("nope")
    );
    k9::assert_equal!(table.raw_len(), 0);
}

#[test]
fn external_from_lua_unit_variant_from_string() {
    let v =
        Decision::from_lua(Value::string("allow"), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, Decision::Allow);
}

#[test]
fn external_from_lua_newtype_variant_from_table() {
    let t = shingetsu::Table::new();
    t.raw_set(Value::string("deny"), Value::string("bad reason"))
        .expect("set");
    let v = Decision::from_lua(Value::Table(t), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(v, Decision::Deny("bad reason".to_string()));
}

#[test]
fn external_round_trip_unit() {
    let original = Decision::Allow;
    let back = Decision::from_lua(original.clone().into_lua(), &shingetsu::GlobalEnv::new())
        .expect("round trip");
    k9::assert_equal!(back, original);
}

#[test]
fn external_round_trip_newtype() {
    let original = Decision::Deny("denied".to_string());
    let back = Decision::from_lua(original.clone().into_lua(), &shingetsu::GlobalEnv::new())
        .expect("round trip");
    k9::assert_equal!(back, original);
}

#[test]
fn external_from_lua_unknown_string_rejected() {
    let err = Decision::from_lua(Value::string("maybe"), &shingetsu::GlobalEnv::new()).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "error in '': unknown Decision variant `maybe`; expected one of: \"allow\" | { deny = ... }"
    );
}

#[test]
fn external_from_lua_table_without_known_tag_rejected() {
    let t = shingetsu::Table::new();
    t.raw_set(Value::string("approve"), Value::string("yes"))
        .expect("set");
    let err = Decision::from_lua(Value::Table(t), &shingetsu::GlobalEnv::new()).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "error in '': table did not contain any known variant tag for Decision; expected one of: \"allow\" | { deny = ... }"
    );
}

#[test]
fn external_from_lua_wrong_type_rejected() {
    let err = Decision::from_lua(Value::Integer(7), &shingetsu::GlobalEnv::new()).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "bad argument #0 to '' (\"allow\" | { deny = ... } expected, got number)"
    );
}

#[test]
fn external_lua_typed_is_union_of_string_literal_and_table() {
    use shingetsu::{Bytes, LuaType, TableField, TableLuaType};
    let ty = Decision::lua_type();
    k9::assert_equal!(
        ty,
        LuaType::Union(vec![
            LuaType::StringLiteral(Bytes::from("allow")),
            LuaType::Table(Box::new(TableLuaType {
                fields: vec![TableField::new(Bytes::from("deny"), LuaType::String,)],
                indexer: None,
            })),
        ])
    );
}

// Per-variant rename overrides the container rename_all.
#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
#[lua(rename_all = "snake_case")]
enum Mode {
    Auto,
    #[lua(rename = "custom!")]
    Custom(i64),
}

#[test]
fn external_per_variant_rename() {
    k9::assert_equal!(Mode::Auto.into_lua(), Value::string("auto"));
    let v = Mode::Custom(42).into_lua();
    let table = match v {
        Value::Table(t) => t,
        other => panic!("expected table, got {other:?}"),
    };
    k9::assert_equal!(
        table.raw_get(&Value::string("custom!")).expect("raw_get"),
        Value::Integer(42)
    );
}

// Multiple newtype + multiple unit variants in one enum.
#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq, Clone)]
#[lua(rename_all = "snake_case")]
enum Action {
    Pass,
    Skip,
    Tag(String),
    Score(i64),
}

#[test]
fn external_multi_unit_round_trip() {
    for original in [Action::Pass, Action::Skip] {
        let back = Action::from_lua(original.clone().into_lua(), &shingetsu::GlobalEnv::new())
            .expect("round trip");
        k9::assert_equal!(back, original);
    }
}

#[test]
fn external_multi_newtype_round_trip() {
    let cases = [Action::Tag("hello".to_string()), Action::Score(99)];
    for original in cases {
        let back = Action::from_lua(original.clone().into_lua(), &shingetsu::GlobalEnv::new())
            .expect("round trip");
        k9::assert_equal!(back, original);
    }
}

// `#[lua(nil)]` mixed with newtype variants should still go through
// the Untagged path, NOT trigger External inference.
#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum MaybeInt {
    Some(i64),
    #[lua(nil)]
    None,
}

#[test]
fn lua_nil_unit_keeps_untagged_behavior() {
    // None still maps to Lua nil, not to a string "None".
    k9::assert_equal!(MaybeInt::None.into_lua(), Value::Nil);
    let back =
        MaybeInt::from_lua(Value::Integer(7), &shingetsu::GlobalEnv::new()).expect("from_lua");
    k9::assert_equal!(back, MaybeInt::Some(7));
}
