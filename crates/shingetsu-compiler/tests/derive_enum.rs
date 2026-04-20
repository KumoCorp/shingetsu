mod common;

use shingetsu::{FromLua, IntoLua, IntoLuaMulti, LuaTable, LuaTyped, Value, Variadic};

// ---------------------------------------------------------------------------
// Basic enum: disjoint types
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum IntOrStr {
    Int(i64),
    Str(bytes::Bytes),
}

#[test]
fn from_lua_enum_integer() {
    let v = IntOrStr::from_lua(Value::Integer(42)).expect("from_lua");
    k9::assert_equal!(v, IntOrStr::Int(42));
}

#[test]
fn from_lua_enum_string() {
    let v = IntOrStr::from_lua(Value::string("hello")).expect("from_lua");
    k9::assert_equal!(v, IntOrStr::Str(bytes::Bytes::from_static(b"hello")));
}

#[test]
fn from_lua_enum_wrong_type() {
    let err = IntOrStr::from_lua(Value::Boolean(true)).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "bad argument #0 to '' (integer | string expected, got boolean)"
    );
}

#[test]
fn into_lua_enum_integer() {
    let v = IntOrStr::Int(99).into_lua();
    k9::assert_equal!(v, Value::Integer(99));
}

#[test]
fn into_lua_enum_string() {
    let v = IntOrStr::Str(bytes::Bytes::from_static(b"world")).into_lua();
    k9::assert_equal!(v, Value::string("world"));
}

// ---------------------------------------------------------------------------
// LuaTyped: union type
// ---------------------------------------------------------------------------

#[test]
fn lua_typed_union() {
    let ty = IntOrStr::lua_type();
    k9::assert_equal!(ty.to_string(), "integer | string");
}

// ---------------------------------------------------------------------------
// Auto-ordering: i64 before f64
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum IntOrFloat {
    // Declared float first, but i64 should be tried first because
    // {Integer} ⊂ {Integer, Float}.
    Num(f64),
    Int(i64),
}

#[test]
fn auto_order_integer_tried_first() {
    // An integer value should match the i64 variant, not f64,
    // even though f64 is declared first.
    let v = IntOrFloat::from_lua(Value::Integer(7)).expect("from_lua");
    k9::assert_equal!(v, IntOrFloat::Int(7));
}

#[test]
fn auto_order_float_matches_float() {
    let v = IntOrFloat::from_lua(Value::Float(3.14)).expect("from_lua");
    k9::assert_equal!(v, IntOrFloat::Num(3.14));
}

#[test]
fn auto_order_lua_typed() {
    let ty = IntOrFloat::lua_type();
    // Declaration order: Num(f64) first, then Int(i64).
    k9::assert_equal!(ty.to_string(), "float | integer");
}

// ---------------------------------------------------------------------------
// Value variant (catch-all, must be last)
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug)]
enum StringOrAny {
    Str(bytes::Bytes),
    Any(Value),
}

#[test]
fn value_variant_is_last() {
    // String values should match Str, not Any.
    let v = StringOrAny::from_lua(Value::string("test")).expect("from_lua");
    assert!(matches!(v, StringOrAny::Str(_)));
}

#[test]
fn value_variant_catches_rest() {
    // A boolean doesn't match Str, so it falls through to Any.
    let v = StringOrAny::from_lua(Value::Boolean(true)).expect("from_lua");
    assert!(matches!(v, StringOrAny::Any(Value::Boolean(true))));
}

// ---------------------------------------------------------------------------
// Table-backed struct inside an enum
// ---------------------------------------------------------------------------

#[derive(LuaTable, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(FromLua, IntoLua, LuaTyped, Debug)]
enum PointOrStr {
    Pt(Point),
    Str(bytes::Bytes),
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
    let v = PointOrStr::from_lua(Value::Table(table)).expect("from_lua");
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
    let v = PointOrStr::from_lua(Value::string("hello")).expect("from_lua");
    assert!(matches!(v, PointOrStr::Str(_)));
}

// ---------------------------------------------------------------------------
// IntoLua round-trip
// ---------------------------------------------------------------------------

#[test]
fn round_trip_int() {
    let original = IntOrStr::Int(123);
    let lua_val = original.into_lua();
    let back = IntOrStr::from_lua(lua_val).expect("round trip");
    k9::assert_equal!(back, IntOrStr::Int(123));
}

#[test]
fn round_trip_string() {
    let original = IntOrStr::Str(bytes::Bytes::from_static(b"abc"));
    let lua_val = original.into_lua();
    let back = IntOrStr::from_lua(lua_val).expect("round trip");
    k9::assert_equal!(back, IntOrStr::Str(bytes::Bytes::from_static(b"abc")));
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
    let v = LevelOrFn::from_lua(Value::Function(func)).expect("from_lua");
    assert!(matches!(v, LevelOrFn::Func(_)));
}

#[test]
fn level_variant() {
    let v = LevelOrFn::from_lua(Value::Integer(3)).expect("from_lua");
    assert!(matches!(v, LevelOrFn::Level(3)));
}

// ---------------------------------------------------------------------------
// Bool variant
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum BoolOrStr {
    Bool(bool),
    Str(bytes::Bytes),
}

#[test]
fn bool_variant() {
    let v = BoolOrStr::from_lua(Value::Boolean(true)).expect("from_lua");
    k9::assert_equal!(v, BoolOrStr::Bool(true));
}

// ---------------------------------------------------------------------------
// Table variant (explicit Table type)
// ---------------------------------------------------------------------------

#[derive(FromLua)]
#[allow(dead_code)]
enum TableOrStr {
    Tbl(shingetsu::Table),
    Str(bytes::Bytes),
}

#[test]
fn table_variant() {
    let t = shingetsu::Table::new();
    let v = TableOrStr::from_lua(Value::Table(t)).expect("from_lua");
    assert!(matches!(v, TableOrStr::Tbl(_)));
}

// ---------------------------------------------------------------------------
// Integration: use enum as a function parameter via Lua
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq, Clone)]
enum NumOrStr {
    Num(i64),
    Str(bytes::Bytes),
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
    let v = SingleVariant::from_lua(Value::Integer(42)).expect("from_lua");
    k9::assert_equal!(v, SingleVariant::Only(42));
}

#[test]
fn single_variant_rejects() {
    let err = SingleVariant::from_lua(Value::string("nope")).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(msg, "bad argument #0 to '' (integer expected, got string)");
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
    k9::assert_equal!(ty.to_string(), "integer");
}

// ---------------------------------------------------------------------------
// Three+ variants with different set sizes
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum ThreeWay {
    // f64 ({Integer, Float}) declared first, i64 ({Integer}) declared
    // second, Value ({all}) last — sorted to: i64, f64, Value.
    Num(f64),
    Int(i64),
    Any(Value),
}

#[test]
fn three_way_integer_matches_narrowest() {
    let v = ThreeWay::from_lua(Value::Integer(5)).expect("from_lua");
    k9::assert_equal!(v, ThreeWay::Int(5));
}

#[test]
fn three_way_float_matches_mid() {
    let v = ThreeWay::from_lua(Value::Float(2.5)).expect("from_lua");
    k9::assert_equal!(v, ThreeWay::Num(2.5));
}

#[test]
fn three_way_other_matches_any() {
    let v = ThreeWay::from_lua(Value::Boolean(false)).expect("from_lua");
    assert!(matches!(v, ThreeWay::Any(Value::Boolean(false))));
}

#[test]
fn three_way_lua_typed() {
    let ty = ThreeWay::lua_type();
    // Declaration order: Num(f64), Int(i64), Any(Value).
    k9::assert_equal!(ty.to_string(), "float | integer | any");
}

// ---------------------------------------------------------------------------
// f64 without i64 sibling — integer coerces to float
// ---------------------------------------------------------------------------

#[derive(FromLua, IntoLua, LuaTyped, Debug, PartialEq)]
enum FloatOrStr {
    Num(f64),
    Str(bytes::Bytes),
}

#[test]
fn f64_alone_accepts_integer() {
    // f64's FromLua accepts Value::Integer, so this should work.
    let v = FloatOrStr::from_lua(Value::Integer(10)).expect("from_lua");
    k9::assert_equal!(v, FloatOrStr::Num(10.0));
}

#[test]
fn f64_alone_accepts_float() {
    let v = FloatOrStr::from_lua(Value::Float(1.5)).expect("from_lua");
    k9::assert_equal!(v, FloatOrStr::Num(1.5));
}

// ---------------------------------------------------------------------------
// nil passed to enum without Value catch-all
// ---------------------------------------------------------------------------

#[test]
fn nil_rejected_without_catchall() {
    let err = IntOrStr::from_lua(Value::Nil).unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "bad argument #0 to '' (integer | string expected, got nil)"
    );
}

// ---------------------------------------------------------------------------
// IntoLua only (without FromLua) — independent derive
// ---------------------------------------------------------------------------

#[derive(IntoLua, Debug)]
enum IntoOnly {
    Int(i64),
    Str(bytes::Bytes),
}

#[test]
fn into_lua_only_integer() {
    let v = IntoOnly::Int(42).into_lua();
    k9::assert_equal!(v, Value::Integer(42));
}

#[test]
fn into_lua_only_string() {
    let v = IntoOnly::Str(bytes::Bytes::from_static(b"hi")).into_lua();
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
    Str(bytes::Bytes),
}

#[test]
fn userdata_variant_matches() {
    let ud: std::sync::Arc<dyn shingetsu::Userdata> = std::sync::Arc::new(MyUserdata);
    let v = UserdataOrStr::from_lua(Value::Userdata(ud)).expect("from_lua");
    assert!(matches!(v, UserdataOrStr::Ud(_)));
}

#[test]
fn userdata_variant_string_falls_through() {
    let v = UserdataOrStr::from_lua(Value::string("hi")).expect("from_lua");
    assert!(matches!(v, UserdataOrStr::Str(_)));
}

#[test]
fn userdata_variant_rejects_other() {
    let result = UserdataOrStr::from_lua(Value::Integer(1));
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
    let back = PointOrStr::from_lua(lua_val).expect("round trip");
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
    let v = StringOrAny::Str(bytes::Bytes::from_static(b"test")).into_lua();
    k9::assert_equal!(v, Value::string("test"));
}

// ---------------------------------------------------------------------------
// Error message from function call context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enum_error_has_function_name_and_position() {
    use shingetsu::Function;
    use shingetsu_compiler::{CompileOptions, Compiler};
    use shingetsu_vm::Task;

    let env = common::new_env();
    let func = Function::wrap(
        "myfunc",
        |_val: IntOrStr| -> Result<(), shingetsu::VmError> { Ok(()) },
    );
    env.set_global("myfunc", Value::Function(func));

    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler
        .compile("return myfunc(true)")
        .await
        .expect("compile");
    let f = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, f, vec![]).await.unwrap_err();
    let msg = err.to_string();
    k9::assert_equal!(
        msg,
        "bad argument #1 to 'myfunc' (integer | string expected, got boolean)"
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
    k9::assert_equal!(result, vec![Value::Nil]);
}

#[test]
fn into_lua_multi_tuple_variant() {
    let result = FindResult::Match(3, 7).into_lua_multi();
    k9::assert_equal!(result, vec![Value::Integer(3), Value::Integer(7)]);
}

#[test]
fn into_lua_multi_tuple_with_variadic() {
    let captures = Variadic(vec![Value::string("hello"), Value::string("world")]);
    let result = FindResult::MatchCaptures(1, 5, captures).into_lua_multi();
    k9::assert_equal!(
        result,
        vec![
            Value::Integer(1),
            Value::Integer(5),
            Value::string("hello"),
            Value::string("world"),
        ]
    );
}

#[test]
fn into_lua_multi_tuple_with_empty_variadic() {
    let result = FindResult::MatchCaptures(1, 5, Variadic(vec![])).into_lua_multi();
    k9::assert_equal!(result, vec![Value::Integer(1), Value::Integer(5)]);
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
    k9::assert_equal!(result, vec![Value::Integer(42)]);
}

#[test]
fn into_lua_multi_newtype_nil() {
    let result = SingleOrNil::Nil.into_lua_multi();
    k9::assert_equal!(result, vec![Value::Nil]);
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
    k9::assert_equal!(result, vec![Value::Nil, Value::Integer(42)]);
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
    let result =
        VarOrNil::Values(Variadic(vec![Value::Integer(1), Value::Integer(2)])).into_lua_multi();
    k9::assert_equal!(result, vec![Value::Integer(1), Value::Integer(2)]);
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

    let r = common::run_with_env(env.clone(), "return find(5)").await;
    k9::assert_equal!(r, vec![Value::Integer(1), Value::Integer(5)]);

    let r = common::run_with_env(env, "return find(-1)").await;
    k9::assert_equal!(r, vec![Value::Nil]);
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
    k9::assert_equal!(f.signature().lua_returns, Some(vec![LuaType::Integer]));
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
        Some(vec![LuaType::Integer, LuaType::String])
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
    // → (integer, integer) | (integer, integer, ...any) | nil
    k9::assert_equal!(
        f.signature().lua_returns,
        Some(vec![LuaType::Union(vec![
            LuaType::Tuple(vec![LuaType::Integer, LuaType::Integer]),
            LuaType::Tuple(vec![
                LuaType::Integer,
                LuaType::Integer,
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
        vec!["(integer, integer) | (integer, integer, ...any) | nil"]
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
        vec![LuaType::Tuple(vec![LuaType::Integer, LuaType::String])]
    );
}
