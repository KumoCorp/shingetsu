mod common;

use bytes::Bytes;
use common::{new_env, run_all, run_one};
use shingetsu_compiler::{compile, CompileOptions};
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// Compound assignments (LuaU)
// ---------------------------------------------------------------------------

#[test]
fn compound_plus_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x += 5; return x"),
        Value::Integer(15)
    );
}

#[test]
fn compound_minus_equal() {
    k9::assert_equal!(run_one("local x = 10; x -= 3; return x"), Value::Integer(7));
}

#[test]
fn compound_star_equal() {
    k9::assert_equal!(run_one("local x = 6; x *= 7; return x"), Value::Integer(42));
}

#[test]
fn compound_slash_equal() {
    k9::assert_equal!(
        run_one("local x = 10.0; x /= 4; return x"),
        Value::Float(2.5)
    );
}

#[test]
fn compound_double_slash_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x //= 3; return x"),
        Value::Integer(3)
    );
}

#[test]
fn compound_percent_equal() {
    k9::assert_equal!(run_one("local x = 10; x %= 3; return x"), Value::Integer(1));
}

#[test]
fn compound_caret_equal() {
    k9::assert_equal!(
        run_one("local x = 2.0; x ^= 10; return x"),
        Value::Float(1024.0)
    );
}

#[test]
fn compound_two_dots_equal() {
    k9::assert_equal!(
        run_one(r#"local s = "hello"; s ..= " world"; return s"#),
        Value::string("hello world")
    );
}

#[test]
fn compound_global() {
    k9::assert_equal!(run_one("x = 5; x += 3; return x"), Value::Integer(8));
}

#[test]
fn compound_table_field() {
    k9::assert_equal!(
        run_one("local t = {n=10}; t.n += 5; return t.n"),
        Value::Integer(15)
    );
}

#[test]
fn compound_table_index() {
    k9::assert_equal!(
        run_one("local t = {[1]=100}; t[1] -= 1; return t[1]"),
        Value::Integer(99)
    );
}

// ---------------------------------------------------------------------------
// if expressions (LuaU)
// ---------------------------------------------------------------------------

#[test]
fn if_expr_true_branch() {
    k9::assert_equal!(run_one("return if true then 1 else 2"), Value::Integer(1));
}

#[test]
fn if_expr_false_branch() {
    k9::assert_equal!(run_one("return if false then 1 else 2"), Value::Integer(2));
}

#[test]
fn if_expr_elseif() {
    k9::assert_equal!(
        run_one(
            "local x = 2; return if x == 1 then \"one\" elseif x == 2 then \"two\" else \"other\""
        ),
        Value::string("two")
    );
}

#[test]
fn if_expr_nested() {
    k9::assert_equal!(
        run_one("local x = 5; local y = if x > 3 then if x > 4 then \"big\" else \"mid\" else \"small\"; return y"),
        Value::string("big")
    );
}

#[test]
fn if_expr_in_assignment() {
    k9::assert_equal!(
        run_one("local cond = true; local t = {v = if cond then 42 else 0}; return t.v"),
        Value::Integer(42)
    );
}

// ---------------------------------------------------------------------------
// LuaU type annotation parsing
// ---------------------------------------------------------------------------

/// Compile a LuaU snippet and return the top-level Proto.
fn compile_proto(src: &str) -> std::sync::Arc<shingetsu_vm::proto::Proto> {
    compile(src, &CompileOptions::default())
        .expect("compile failed")
        .top_level
}

#[test]
fn luau_type_annotation_param_basic() {
    use shingetsu_vm::types::LuaType;
    // The top-level proto's first constant closure should have the annotated
    // param types.
    let proto = compile_proto("function add(x: number, y: number): number return x + y end");
    // The function is in a nested proto (closure constant).
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(sig.params.len(), 2);
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(sig.params[1].lua_type, Some(LuaType::Number));
    k9::assert_equal!(sig.lua_returns, Some(vec![LuaType::Number]));
    // runtime_type should be derived from lua_type.
    k9::assert_equal!(
        sig.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Number)
    );
    k9::assert_equal!(
        sig.params[1].runtime_type,
        Some(shingetsu_vm::types::ValueType::Number)
    );
}

#[test]
fn luau_type_annotation_param_optional() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: string, y: number?) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(sig.params.len(), 2);
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::String));
    k9::assert_equal!(
        sig.params[1].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

#[test]
fn luau_type_annotation_return_tuple() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(): (boolean, string) return true, 'ok' end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.lua_returns,
        Some(vec![LuaType::Boolean, LuaType::String])
    );
}

#[test]
fn luau_type_annotation_no_annotation() {
    // Without annotations, lua_type should be None.
    let proto = compile_proto("function f(x, y) return x + y end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, None);
    k9::assert_equal!(child.signature.params[1].lua_type, None);
    k9::assert_equal!(child.signature.lua_returns, None);
}

#[test]
fn luau_type_annotation_named_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: Foo) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Named(Bytes::from("Foo")))
    );
}

#[test]
fn luau_type_annotation_union() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: string | number) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![LuaType::String, LuaType::Number]))
    );
}

#[test]
fn luau_type_annotation_callback() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(cb: (number) -> string) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Function(flt) => {
            k9::assert_equal!(flt.params.len(), 1);
            k9::assert_equal!(flt.params[0].1, LuaType::Number);
            k9::assert_equal!(flt.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_table_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(t: { x: number, y: string }) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Table(tlt) => {
            k9::assert_equal!(tlt.fields.len(), 2);
            k9::assert_equal!(tlt.fields[0], (Bytes::from("x"), LuaType::Number));
            k9::assert_equal!(tlt.fields[1], (Bytes::from("y"), LuaType::String));
            k9::assert_equal!(tlt.indexer, None);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_table_indexer() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(t: { [string]: number }) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Table(tlt) => {
            k9::assert_equal!(tlt.fields.len(), 0);
            k9::assert_equal!(
                tlt.indexer,
                Some((Box::new(LuaType::String), Box::new(LuaType::Number)))
            );
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_generic_type() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    let proto = compile_proto("function f(t: Map<string, number>) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Map")));
            k9::assert_equal!(args.len(), 2);
            k9::assert_equal!(args[0], LuaTypeArg::Type(LuaType::String));
            k9::assert_equal!(args[1], LuaTypeArg::Type(LuaType::Number));
        }
        other => panic!("expected Generic, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_array_shorthand() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    let proto = compile_proto("function f(t: { number }) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Array")));
            k9::assert_equal!(args.len(), 1);
            k9::assert_equal!(args[0], LuaTypeArg::Type(LuaType::Number));
        }
        other => panic!("expected Generic(Array), got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_intersection() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: Readable & Writable) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Intersection(vec![
            LuaType::Named(Bytes::from("Readable")),
            LuaType::Named(Bytes::from("Writable")),
        ]))
    );
}

#[test]
fn luau_type_annotation_basic_primitives() {
    use shingetsu_vm::types::LuaType;
    let proto =
        compile_proto("function f(a: nil, b: boolean, c: any, d: integer, e: float): never end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Nil));
    k9::assert_equal!(child.signature.params[1].lua_type, Some(LuaType::Boolean));
    k9::assert_equal!(child.signature.params[2].lua_type, Some(LuaType::Any));
    k9::assert_equal!(child.signature.params[3].lua_type, Some(LuaType::Integer));
    k9::assert_equal!(child.signature.params[4].lua_type, Some(LuaType::Float));
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::Never]));
}

#[test]
fn luau_type_annotation_typeof() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: typeof({})) end");
    let child = &proto.protos[0];
    // typeof is opaque at compile time — treated as Any.
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Any));
}

#[test]
fn luau_type_annotation_method_self() {
    use shingetsu_vm::types::LuaType;
    // Method syntax: implicit self has no annotation.
    let proto = compile_proto("local t = {}; function t:m(x: number) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    // self is param[0], x is param[1]
    k9::assert_equal!(sig.params.len(), 2);
    k9::assert_equal!(sig.params[0].name, Some(Bytes::from("self")));
    k9::assert_equal!(sig.params[0].lua_type, None);
    k9::assert_equal!(sig.params[1].lua_type, Some(LuaType::Number));
}

#[test]
fn luau_type_annotation_mixed_annotated_unannotated() {
    use shingetsu_vm::types::LuaType;
    // Some params annotated, some not.
    let proto = compile_proto("function f(a: number, b, c: string) end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(child.signature.params[1].lua_type, None);
    k9::assert_equal!(child.signature.params[2].lua_type, Some(LuaType::String));
}

#[test]
fn luau_type_annotation_variadic_param() {
    use shingetsu_vm::types::LuaType;
    // Variadic params don't get a ParamSpec entry, but should not break parsing.
    let proto = compile_proto("function f(x: number, ...): string end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params.len(), 1);
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(child.signature.variadic, true);
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::String]));
}

// ---------------------------------------------------------------------------
// LuaU runtime type enforcement
// ---------------------------------------------------------------------------

#[test]
fn luau_runtime_type_check_rejects_wrong_type() {
    // Annotated Lua function rejects wrong argument type at call boundary.
    let res = run_all(
        "function add(x: number, y: number): number return x + y end
         local ok, err = pcall(add, 1, 'two')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'add' (number expected, got string)"),
        ]
    );
}

#[test]
fn luau_runtime_type_check_accepts_correct_type() {
    // Annotated Lua function accepts correct types.
    let res = run_one(
        "function add(x: number, y: number): number return x + y end
         return add(3, 4)",
    );
    k9::assert_equal!(res, Value::Integer(7));
}

#[test]
fn luau_runtime_type_check_string_param() {
    let res = run_all(
        "function greet(name: string) return 'hi ' .. name end
         local ok, err = pcall(greet, 42)
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'greet' (string expected, got number)"),
        ]
    );
}

#[test]
fn luau_runtime_type_check_table_param() {
    let res = run_all(
        "function keys(t: {[string]: number}) return next(t) end
         local ok, err = pcall(keys, 'not a table')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'keys' (table expected, got string)"),
        ]
    );
}

#[test]
fn luau_runtime_type_check_boolean_param() {
    let res = run_all(
        "function toggle(b: boolean) return not b end
         local ok, err = pcall(toggle, 'yes')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'toggle' (boolean expected, got string)"),
        ]
    );
}

#[test]
fn luau_runtime_type_check_optional_allows_nil() {
    // Optional params should NOT reject nil.
    let res = run_one(
        "function f(x: number?) return x or 0 end
         return f(nil)",
    );
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn luau_runtime_type_check_unannotated_no_check() {
    // Unannotated params should accept any type (no runtime check).
    let res = run_one(
        "function f(x) return type(x) end
         return f({})",
    );
    k9::assert_equal!(res, Value::string("table"));
}

#[test]
fn luau_runtime_type_check_function_param() {
    let res = run_all(
        "function apply(cb: (number) -> number) return cb(5) end
         local ok, err = pcall(apply, 'not a function')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'apply' (function expected, got string)"),
        ]
    );
}

#[test]
fn luau_runtime_type_check_function_param_accepts() {
    let res = run_one(
        "function apply(cb: (number) -> number) return cb(5) end
         return apply(function(x) return x * 2 end)",
    );
    k9::assert_equal!(res, Value::Integer(10));
}

#[test]
fn luau_runtime_type_check_integer_rejects_float() {
    let res = run_all(
        "function f(x: integer) return x end
         local ok, err = pcall(f, 1.5)
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'f' (integer expected, got number)"),
        ]
    );
}

#[test]
fn luau_runtime_type_check_integer_accepts_integer() {
    let res = run_one(
        "function f(x: integer) return x + 1 end
         return f(10)",
    );
    k9::assert_equal!(res, Value::Integer(11));
}

#[test]
fn luau_runtime_type_check_any_accepts_all() {
    // `any` annotation should accept any value.
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f(42)"),
        Value::string("number")
    );
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f('s')"),
        Value::string("string")
    );
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f(nil)"),
        Value::string("nil")
    );
}

#[test]
fn luau_runtime_type_check_direct_call_fails() {
    // Direct call (not pcall) with wrong type should produce an error
    // from the initial task entry validation.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};

    let opts = CompileOptions {
        ..CompileOptions::default()
    };
    // Compile a chunk that defines a typed function then calls it wrong.
    let bc =
        compile("function f(x: number) return x end; return f('bad')", &opts).expect("compile");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'f' (number expected, got string)"
    );
}

#[test]
fn luau_type_annotation_string_literal() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto(r#"function f(x: "hello") end"#);
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::StringLiteral(Bytes::from("hello")))
    );
}

#[test]
fn luau_type_annotation_boolean_literal() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: true) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::BoolLiteral(true))
    );
}

// ---------------------------------------------------------------------------
// Generic type parameter declarations
// ---------------------------------------------------------------------------

#[test]
fn luau_generic_function_type_params() {
    let proto = compile_proto("function identity<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 1);
    k9::assert_equal!(child.signature.type_params[0].name, Bytes::from("T"));
    k9::assert_equal!(child.signature.type_params[0].is_pack, false);
    k9::assert_equal!(child.signature.type_params[0].constraint, None);
    k9::assert_equal!(child.signature.type_params[0].default, None);
}

#[test]
fn luau_generic_function_param_is_type_param() {
    use shingetsu_vm::types::LuaType;
    // Inside a generic function, `T` in parameter annotations should be
    // `LuaType::TypeParam`, not `LuaType::Named`.
    let proto = compile_proto("function identity<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
    // Return type should also be TypeParam.
    k9::assert_equal!(
        child.signature.lua_returns,
        Some(vec![LuaType::TypeParam(Bytes::from("T"))])
    );
}

#[test]
fn luau_generic_multiple_type_params() {
    let proto = compile_proto("function map<T, U>(list: {T}, f: (T) -> U): {U} return {} end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 2);
    k9::assert_equal!(child.signature.type_params[0].name, Bytes::from("T"));
    k9::assert_equal!(child.signature.type_params[1].name, Bytes::from("U"));
}

#[test]
fn luau_generic_variadic_pack() {
    let proto = compile_proto("function first<T...>(...: T...): T... return ... end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 1);
    k9::assert_equal!(child.signature.type_params[0].name, Bytes::from("T"));
    k9::assert_equal!(child.signature.type_params[0].is_pack, true);
}

#[test]
fn luau_generic_with_default_on_type_alias() {
    // Default type params are supported on type aliases, not functions.
    // Verify they parse correctly via a callback type that uses one.
    // full_moon does not support `<T = number>` on function generics,
    // so we test default parsing indirectly via the GenericDeclaration
    // on a type alias (tested in G2). For now, just verify that
    // function generics without defaults work.
    let proto = compile_proto("function f<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params[0].default, None);
}

#[test]
fn luau_generic_non_generic_name_stays_named() {
    use shingetsu_vm::types::LuaType;
    // `Foo` is not a declared type param, so it should be `LuaType::Named`.
    let proto = compile_proto("function f<T>(x: T, y: Foo): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
    k9::assert_equal!(
        child.signature.params[1].lua_type,
        Some(LuaType::Named(Bytes::from("Foo")))
    );
}

#[test]
fn luau_generic_erased_at_runtime() {
    // A generic param like `T` should not produce a runtime_type
    // (it's erased — treated as `any`).
    let proto = compile_proto("function identity<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].runtime_type, None);
}

#[test]
fn luau_generic_function_still_runs() {
    // Generic function should compile and execute normally.
    k9::assert_equal!(
        run_one("function identity<T>(x: T): T return x end\nreturn identity(42)"),
        Value::Integer(42)
    );
}

#[test]
fn luau_generic_type_param_in_callback() {
    use shingetsu_vm::types::LuaType;
    // T inside a callback parameter should be TypeParam.
    let proto = compile_proto("function f<T>(cb: (T) -> T) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params[0].1, LuaType::TypeParam(Bytes::from("T")));
            k9::assert_equal!(ft.returns, vec![LuaType::TypeParam(Bytes::from("T"))]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_generic_type_param_in_optional() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: T?) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::TypeParam(
            Bytes::from("T")
        ))))
    );
}

#[test]
fn luau_generic_type_param_in_union() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: T | string) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![
            LuaType::TypeParam(Bytes::from("T")),
            LuaType::String,
        ]))
    );
}

#[test]
fn luau_generic_type_param_in_table() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: { val: T }) end");
    let child = &proto.protos[0];
    match child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type")
    {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 1);
            k9::assert_equal!(t.fields[0].1, LuaType::TypeParam(Bytes::from("T")));
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_generic_type_param_in_generic_instantiation() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // T used as a type argument: Array<T>
    let proto = compile_proto("function f<T>(x: Array<T>) end");
    let child = &proto.protos[0];
    match child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type")
    {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Array")));
            k9::assert_equal!(
                args[0],
                LuaTypeArg::Type(LuaType::TypeParam(Bytes::from("T")))
            );
        }
        other => panic!("expected Generic, got {:?}", other),
    }
}

#[test]
fn luau_generic_type_param_in_array_shorthand() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // {T} is array shorthand — T inside should be TypeParam.
    let proto = compile_proto("function f<T>(x: {T}) end");
    let child = &proto.protos[0];
    match child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type")
    {
        LuaType::Generic { args, .. } => {
            k9::assert_equal!(
                args[0],
                LuaTypeArg::Type(LuaType::TypeParam(Bytes::from("T")))
            );
        }
        other => panic!("expected Generic(Array), got {:?}", other),
    }
}

#[test]
fn luau_generic_does_not_leak_to_sibling_function() {
    use shingetsu_vm::types::LuaType;
    // T is declared on f but not on g — in g, T should be Named.
    let proto = compile_proto("function f<T>(x: T) end\nfunction g(x: T) end");
    let f = &proto.protos[0];
    let g = &proto.protos[1];
    k9::assert_equal!(
        f.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
    k9::assert_equal!(
        g.signature.params[0].lua_type,
        Some(LuaType::Named(Bytes::from("T")))
    );
}

#[test]
fn luau_generic_local_function() {
    use shingetsu_vm::types::LuaType;
    // local function should go through the same generic path.
    let proto = compile_proto("local function f<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 1);
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
}

#[test]
fn luau_generic_multiple_params_execution() {
    // Multi-param generic function should execute correctly.
    k9::assert_equal!(
        run_one("function swap<A, B>(a: A, b: B): (B, A) return b, a end\nreturn swap(1, 'hello')"),
        Value::string("hello")
    );
}

// ---------------------------------------------------------------------------
// Type alias declarations
// ---------------------------------------------------------------------------

#[test]
fn luau_type_alias_simple() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Meters = number");
    let alias = proto
        .type_aliases
        .get(b"Meters" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.params.len(), 0);
    k9::assert_equal!(alias.body, LuaType::Number);
}

#[test]
fn luau_type_alias_with_generics() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Pair<A, B> = { first: A, second: B }");
    let alias = proto
        .type_aliases
        .get(b"Pair" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.params.len(), 2);
    k9::assert_equal!(alias.params[0].name, Bytes::from("A"));
    k9::assert_equal!(alias.params[1].name, Bytes::from("B"));
    // The body should use TypeParam for A and B.
    match &alias.body {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 2);
            k9::assert_equal!(t.fields[0].0, Bytes::from("first"));
            k9::assert_equal!(t.fields[0].1, LuaType::TypeParam(Bytes::from("A")));
            k9::assert_equal!(t.fields[1].0, Bytes::from("second"));
            k9::assert_equal!(t.fields[1].1, LuaType::TypeParam(Bytes::from("B")));
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_type_alias_function_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Predicate = (number) -> boolean");
    let alias = proto
        .type_aliases
        .get(b"Predicate" as &[u8])
        .expect("alias exists");
    match &alias.body {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params.len(), 1);
            k9::assert_equal!(ft.params[0].1, LuaType::Number);
            k9::assert_equal!(ft.returns, vec![LuaType::Boolean]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_type_alias_no_runtime_effect() {
    // Type aliases produce no instructions.
    k9::assert_equal!(
        run_one("type Meters = number\nreturn 42"),
        Value::Integer(42)
    );
}

#[test]
fn luau_exported_type_alias() {
    use shingetsu_vm::types::LuaType;
    // `export type` should be stored the same as `type`.
    let proto = compile_proto("export type ID = string");
    let alias = proto
        .type_aliases
        .get(b"ID" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.body, LuaType::String);
}

#[test]
fn luau_type_alias_union_body() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type StringOrNumber = string | number");
    let alias = proto
        .type_aliases
        .get(b"StringOrNumber" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(
        alias.body,
        LuaType::Union(vec![LuaType::String, LuaType::Number])
    );
}

#[test]
fn luau_type_alias_optional_body() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type MaybeString = string?");
    let alias = proto
        .type_aliases
        .get(b"MaybeString" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.body, LuaType::Optional(Box::new(LuaType::String)));
}

#[test]
fn luau_type_alias_multiple_in_chunk() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type A = number\ntype B = string");
    k9::assert_equal!(
        proto
            .type_aliases
            .get(b"A" as &[u8])
            .expect("A exists")
            .body,
        LuaType::Number
    );
    k9::assert_equal!(
        proto
            .type_aliases
            .get(b"B" as &[u8])
            .expect("B exists")
            .body,
        LuaType::String
    );
}

#[test]
fn luau_type_alias_overwrite() {
    use shingetsu_vm::types::LuaType;
    // Last declaration wins.
    let proto = compile_proto("type X = number\ntype X = string");
    k9::assert_equal!(
        proto
            .type_aliases
            .get(b"X" as &[u8])
            .expect("X exists")
            .body,
        LuaType::String
    );
}

#[test]
fn luau_type_alias_references_named_type() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // `User` is not a generic param, so it stays Named.
    let proto = compile_proto("type UserList = Array<User>");
    let alias = proto
        .type_aliases
        .get(b"UserList" as &[u8])
        .expect("alias exists");
    match &alias.body {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Array")));
            k9::assert_equal!(
                args[0],
                LuaTypeArg::Type(LuaType::Named(Bytes::from("User")))
            );
        }
        other => panic!("expected Generic, got {:?}", other),
    }
}

#[test]
fn luau_type_alias_generic_params_dont_leak() {
    use shingetsu_vm::types::LuaType;
    // T is a generic param on Foo but not on Bar.
    let proto = compile_proto("type Foo<T> = T\ntype Bar = T");
    k9::assert_equal!(
        proto.type_aliases.get(b"Foo" as &[u8]).expect("Foo").body,
        LuaType::TypeParam(Bytes::from("T"))
    );
    k9::assert_equal!(
        proto.type_aliases.get(b"Bar" as &[u8]).expect("Bar").body,
        LuaType::Named(Bytes::from("T"))
    );
}

// ---------------------------------------------------------------------------
// Type alias resolution in annotations
// ---------------------------------------------------------------------------

#[test]
fn luau_alias_resolution_simple() {
    use shingetsu_vm::types::LuaType;
    // `type Meters = number` then a function using Meters should resolve to Number.
    let proto = compile_proto("type Meters = number\nfunction f(x: Meters) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(
        sig.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Number)
    );
}

#[test]
fn luau_alias_resolution_string_alias() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Name = string\nfunction greet(who: Name) end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::String));
    k9::assert_equal!(
        child.signature.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::String)
    );
}

#[test]
fn luau_alias_resolution_generic_table() {
    use shingetsu_vm::types::LuaType;
    // Generic alias `Pair<A, B>` with concrete args `number, string`.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string>) end",
    );
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 2);
            k9::assert_equal!(t.fields[0].0, Bytes::from("first"));
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
            k9::assert_equal!(t.fields[1].0, Bytes::from("second"));
            k9::assert_equal!(t.fields[1].1, LuaType::String);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_generic_table_has_runtime_type() {
    // Expanded table alias has Table runtime type.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string>) end",
    );
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Table)
    );
}

#[test]
fn luau_alias_resolution_optional() {
    use shingetsu_vm::types::LuaType;
    // `type Id = number` then `function f(x: Id?) end` should give Optional(Number).
    let proto = compile_proto("type Id = number\nfunction f(x: Id?) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

#[test]
fn luau_alias_resolution_in_union() {
    use shingetsu_vm::types::LuaType;
    // Alias used as part of a union.
    let proto = compile_proto("type Id = number\nfunction f(x: Id | string) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![LuaType::Number, LuaType::String]))
    );
}

#[test]
fn luau_alias_resolution_no_runtime_effect() {
    // Aliases have no runtime effect — the code still runs.
    k9::assert_equal!(
        run_one(
            "type Meters = number\n\
             function add(a: Meters, b: Meters): Meters\n\
             return a + b\n\
             end\n\
             return add(3, 4)"
        ),
        Value::Integer(7)
    );
}

#[test]
fn luau_alias_resolution_chained() {
    use shingetsu_vm::types::LuaType;
    // `type A = number`, `type B = A` — B should resolve to number too.
    let proto = compile_proto("type A = number\ntype B = A\nfunction f(x: B) end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
}

#[test]
fn luau_alias_resolution_in_return_type() {
    use shingetsu_vm::types::LuaType;
    // Alias should also resolve in return type annotations.
    let proto = compile_proto("type Meters = number\nfunction f(x: number): Meters return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::Number]));
}

#[test]
fn luau_alias_resolution_generic_in_function_type() {
    use shingetsu_vm::types::LuaType;
    // `type Mapper<T, U> = (T) -> U` then `function f(m: Mapper<number, string>) end`
    let proto =
        compile_proto("type Mapper<T, U> = (T) -> U\nfunction f(m: Mapper<number, string>) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params.len(), 1);
            k9::assert_equal!(ft.params[0].1, LuaType::Number);
            k9::assert_equal!(ft.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_preserves_unrelated_generics() {
    use shingetsu_vm::types::LuaType;
    // A function with its own generic T that is NOT an alias should still produce TypeParam.
    let proto = compile_proto("type Meters = number\nfunction f<T>(x: Meters, y: T) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    // Meters resolves to number.
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::Number));
    // T is a function generic param, stays as TypeParam.
    k9::assert_equal!(
        sig.params[1].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
}

#[test]
fn luau_alias_resolution_alias_in_alias_body() {
    use shingetsu_vm::types::LuaType;
    // `type A = number`, `type B = { x: A }` — alias body references another alias.
    let proto = compile_proto("type A = number\ntype B = { x: A }\nfunction f(p: B) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 1);
            k9::assert_equal!(t.fields[0].0, Bytes::from("x"));
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_generic_fewer_args() {
    use shingetsu_vm::types::LuaType;
    // `Pair<number>` with only one arg — B stays as TypeParam("B").
    let proto =
        compile_proto("type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number>) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
            k9::assert_equal!(t.fields[1].1, LuaType::TypeParam(Bytes::from("B")));
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_generic_extra_args() {
    use shingetsu_vm::types::LuaType;
    // `Pair<number, string, boolean>` — extra arg is silently ignored.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string, boolean>) end",
    );
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
            k9::assert_equal!(t.fields[1].1, LuaType::String);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_in_callback_param() {
    use shingetsu_vm::types::LuaType;
    // Alias used inside a callback parameter type.
    let proto = compile_proto("type Meters = number\nfunction f(cb: (Meters) -> string) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params.len(), 1);
            k9::assert_equal!(ft.params[0].1, LuaType::Number);
            k9::assert_equal!(ft.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_in_table_field() {
    use shingetsu_vm::types::LuaType;
    // Alias used inside a table type annotation on a param.
    let proto = compile_proto("type Meters = number\nfunction f(p: { dist: Meters }) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 1);
            k9::assert_equal!(t.fields[0].0, Bytes::from("dist"));
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_nested_generic_optional() {
    use shingetsu_vm::types::LuaType;
    // `type Wrap<T> = T?` then `Wrap<number>` should give `Optional(Number)`.
    let proto = compile_proto("type Wrap<T> = T?\nfunction f(x: Wrap<number>) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

// ---------------------------------------------------------------------------
// table.create / find / clear / freeze / isfrozen / clone (LuaU extensions)
// ---------------------------------------------------------------------------

#[test]
fn luau_table_create_default_nil() {
    // Without a value argument all slots are effectively nil —
    // assigning nil leaves the table empty, so #t is 0.
    let res = run_all(
        "\
        local t = table.create(3)\n\
        return #t, t[1], t[2], t[3]",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(0), Value::Nil, Value::Nil, Value::Nil,]
    );
}

#[test]
fn luau_table_create_with_value() {
    let res = run_all(
        "\
        local t = table.create(4, 'x')\n\
        return #t, t[1], t[4]",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(4), Value::string("x"), Value::string("x"),]
    );
}

#[test]
fn luau_table_create_zero() {
    let res = run_one("local t = table.create(0, 'x'); return #t");
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn luau_table_create_negative_errors() {
    let err = common::run_err("table.create(-1, 'x')");
    k9::assert_equal!(err, "bad argument #1 to 'create' (size out of range: -1)");
}

#[test]
fn luau_table_find_present() {
    let res = run_one("return table.find({10, 20, 30}, 20)");
    k9::assert_equal!(res, Value::Integer(2));
}

#[test]
fn luau_table_find_missing() {
    let res = run_one("return table.find({10, 20, 30}, 99)");
    k9::assert_equal!(res, Value::Nil);
}

#[test]
fn luau_table_find_with_init() {
    // Two 20s; init=3 skips the first one.
    let res = run_one("return table.find({20, 10, 20, 10}, 20, 3)");
    k9::assert_equal!(res, Value::Integer(3));
}

#[test]
fn luau_table_find_string() {
    let res = run_one("return table.find({'a', 'b', 'c'}, 'b')");
    k9::assert_equal!(res, Value::Integer(2));
}

#[test]
fn luau_table_find_init_zero_errors() {
    let err = common::run_err("table.find({1,2,3}, 2, 0)");
    k9::assert_equal!(err, "bad argument #3 to 'find' (index out of range: 0)");
}

#[test]
fn luau_table_clear() {
    let res = run_all(
        "\
        local t = {1, 2, 3, foo = 'bar'}\n\
        table.clear(t)\n\
        return #t, t.foo, t[1]",
    );
    k9::assert_equal!(res, vec![Value::Integer(0), Value::Nil, Value::Nil]);
}

#[test]
fn luau_table_freeze_returns_table() {
    // table.freeze returns the same table so you can chain.
    let res = run_one(
        "\
        local t = {1, 2, 3}\n\
        local u = table.freeze(t)\n\
        return t == u",
    );
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn luau_table_isfrozen_default_false() {
    let res = run_one("return table.isfrozen({1,2,3})");
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn luau_table_isfrozen_after_freeze() {
    let res = run_one(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        return table.isfrozen(t)",
    );
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn luau_frozen_table_rejects_assignment() {
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        t[4] = 99",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[test]
fn luau_frozen_table_rejects_insert() {
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        table.insert(t, 4)",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[test]
fn luau_frozen_table_rejects_clear() {
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        table.clear(t)",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[test]
fn luau_freeze_is_idempotent() {
    // Freezing an already-frozen table is fine.
    let res = run_one(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        table.freeze(t)\n\
        return table.isfrozen(t)",
    );
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn luau_table_clone_shallow() {
    // Contents copied by value, nested tables shared by reference.
    let res = run_all(
        "\
        local inner = {10}\n\
        local t = {1, 2, inner}\n\
        local c = table.clone(t)\n\
        return c[1], c[2], c[3] == inner, c == t",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Boolean(true),
            Value::Boolean(false),
        ]
    );
}

#[test]
fn luau_table_clone_of_frozen_is_not_frozen() {
    // Per LuaU: the clone is never frozen even if the source is.
    let res = run_one(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        local c = table.clone(t)\n\
        return table.isfrozen(c)",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn luau_table_clone_allows_mutation() {
    // Sanity check that the clone really is writable.
    let res = run_all(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        local c = table.clone(t)\n\
        c[4] = 4\n\
        return #c, c[4]",
    );
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Integer(4)]);
}

#[test]
fn luau_table_clone_copies_metatable() {
    // Shallow clone keeps the same metatable (shared by Arc ref).
    let res = run_one(
        "\
        local mt = {__index = function() return 42 end}\n\
        local t = setmetatable({}, mt)\n\
        local c = table.clone(t)\n\
        return c.anything",
    );
    k9::assert_equal!(res, Value::Integer(42));
}

// ---- Frozen rejection: remaining mutation paths ----------------------------

#[test]
fn luau_frozen_table_rejects_setmetatable() {
    // `setmetatable` mutates the table's metatable slot — check_writable
    // fires before the assignment.
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        setmetatable(t, {})",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[test]
fn luau_frozen_table_rejects_rawset() {
    // `rawset` goes straight to `raw_set`, bypassing __newindex.
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        rawset(t, 1, 99)",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[test]
fn luau_frozen_table_rejects_sort() {
    // `table.sort` uses `swap_array`, which must propagate the frozen error.
    let err = common::run_err(
        "\
        local t = {3, 1, 2}\n\
        table.freeze(t)\n\
        table.sort(t)",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[test]
fn luau_frozen_table_rejects_remove() {
    // `table.remove` on a non-empty frozen table hits `raw_remove`.
    let err = common::run_err(
        "\
        local t = {10, 20, 30}\n\
        table.freeze(t)\n\
        table.remove(t, 1)",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[test]
fn luau_frozen_table_newindex_fires_for_new_key() {
    // Assignments go through __newindex when the key is absent — the
    // source table isn't mutated, so freeze doesn't block the metamethod.
    let res = run_all(
        "\
        local log = {}\n\
        local t = {existing = 1}\n\
        setmetatable(t, {__newindex = function(_, k, v) log[k] = v end})\n\
        table.freeze(t)\n\
        t.newkey = 99\n\
        return log.newkey, rawget(t, 'newkey')",
    );
    k9::assert_equal!(res, vec![Value::Integer(99), Value::Nil]);
}

#[test]
fn luau_frozen_table_existing_key_bypasses_newindex() {
    // When the key already exists, __newindex is skipped and raw_set runs
    // directly — which on a frozen table errors.
    let err = common::run_err(
        "\
        local t = {existing = 1}\n\
        setmetatable(t, {__newindex = function() end})\n\
        table.freeze(t)\n\
        t.existing = 99",
    );
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

// ---- table.find edge cases -------------------------------------------------

#[test]
fn luau_table_find_init_past_end() {
    let res = run_one("return table.find({1,2,3}, 2, 5)");
    k9::assert_equal!(res, Value::Nil);
}

#[test]
fn luau_table_find_init_at_end_plus_one() {
    // init == #t + 1 is a valid starting point that matches nothing.
    let res = run_one("return table.find({1,2,3}, 3, 4)");
    k9::assert_equal!(res, Value::Nil);
}

#[test]
fn luau_table_find_empty_table() {
    let res = run_one("return table.find({}, 1)");
    k9::assert_equal!(res, Value::Nil);
}

#[test]
fn luau_table_find_by_table_reference() {
    // Raw equality: tables compare by identity, so the same Arc matches.
    let res = run_one(
        "\
        local a = {}\n\
        return table.find({a, {}}, a)",
    );
    k9::assert_equal!(res, Value::Integer(1));
}

#[test]
fn luau_table_find_bad_arg_not_table() {
    let res = run_all(
        "local ok, err = pcall(table.find, 'not a table', 1)\n\
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'find' (table expected, got string)"),
        ]
    );
}

// ---- table.clone edge cases ------------------------------------------------

#[test]
fn luau_table_clone_preserves_hash_keys() {
    let res = run_all(
        "\
        local t = {foo = 'bar', baz = 42}\n\
        local c = table.clone(t)\n\
        return c.foo, c.baz",
    );
    k9::assert_equal!(res, vec![Value::string("bar"), Value::Integer(42),]);
}

#[test]
fn luau_table_clone_empty() {
    let res = run_one("local c = table.clone({}); return #c");
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn luau_table_clone_array_is_independent() {
    // Mutating the clone's array slot must not affect the source.
    let res = run_all(
        "\
        local src = {1, 2, 3}\n\
        local cp = table.clone(src)\n\
        cp[1] = 99\n\
        return src[1], cp[1]",
    );
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(99)]);
}

// ---- table.create edge cases -----------------------------------------------

#[test]
fn luau_table_create_bad_arg_non_integer() {
    let res = run_all(
        "local ok, err = pcall(table.create, 'not int')\n\
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'create' (integer expected, got string)"),
        ]
    );
}

#[test]
fn luau_table_create_bad_arg_fractional() {
    // A float with a non-zero fraction is not coercible to integer.
    let res = run_all(
        "local ok, err = pcall(table.create, 2.5)\n\
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'create' (integer expected, got number)"),
        ]
    );
}

#[test]
fn luau_table_create_count_one() {
    // Boundary between 0 and the loop path.
    let res = run_all(
        "\
        local t = table.create(1, 'z')\n\
        return #t, t[1]",
    );
    k9::assert_equal!(res, vec![Value::Integer(1), Value::string("z")]);
}

// ---- Arg validation on the other new helpers -------------------------------

#[test]
fn luau_table_helpers_reject_non_table() {
    // One combined test for the uniform FromLua-generated BadArgument paths.
    let res = run_all(
        "\
        local e1 = select(2, pcall(table.clear, 'x'))\n\
        local e2 = select(2, pcall(table.freeze, 42))\n\
        local e3 = select(2, pcall(table.isfrozen, true))\n\
        local e4 = select(2, pcall(table.clone, nil))\n\
        return e1, e2, e3, e4",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::string("bad argument #1 to 'clear' (table expected, got string)"),
            Value::string("bad argument #1 to 'freeze' (table expected, got number)"),
            Value::string("bad argument #1 to 'isfrozen' (table expected, got boolean)"),
            Value::string("bad argument #1 to 'clone' (table expected, got nil)"),
        ]
    );
}
