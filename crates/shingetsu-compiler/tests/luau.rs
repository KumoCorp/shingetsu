mod common;

use common::{new_env, run_all, run_one};
use shingetsu::valuevec;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::types::{GenericTypeParam, ParamSpec};
use shingetsu_vm::{Bytes, Value};

// ---------------------------------------------------------------------------
// Compound assignments (LuaU)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compound_plus_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x += 5; return x").await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn compound_minus_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x -= 3; return x").await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn compound_star_equal() {
    k9::assert_equal!(
        run_one("local x = 6; x *= 7; return x").await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn compound_slash_equal() {
    k9::assert_equal!(
        run_one("local x = 10.0; x /= 4; return x").await,
        Value::Float(2.5)
    );
}

#[tokio::test]
async fn compound_double_slash_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x //= 3; return x").await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn compound_percent_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x %= 3; return x").await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn compound_caret_equal() {
    k9::assert_equal!(
        run_one("local x = 2.0; x ^= 10; return x").await,
        Value::Float(1024.0)
    );
}

#[tokio::test]
async fn compound_two_dots_equal() {
    k9::assert_equal!(
        run_one(r#"local s = "hello"; s ..= " world"; return s"#).await,
        Value::string("hello world")
    );
}

#[tokio::test]
async fn compound_global() {
    k9::assert_equal!(run_one("x = 5; x += 3; return x").await, Value::Integer(8));
}

#[tokio::test]
async fn compound_table_field() {
    k9::assert_equal!(
        run_one("local t = {n=10}; t.n += 5; return t.n").await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn compound_table_index() {
    k9::assert_equal!(
        run_one("local t = {[1]=100}; t[1] -= 1; return t[1]").await,
        Value::Integer(99)
    );
}

// ---------------------------------------------------------------------------
// if expressions (LuaU)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_expr_true_branch() {
    k9::assert_equal!(
        run_one("return if true then 1 else 2").await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn if_expr_false_branch() {
    k9::assert_equal!(
        run_one("return if false then 1 else 2").await,
        Value::Integer(2)
    );
}

#[tokio::test]
async fn if_expr_elseif() {
    k9::assert_equal!(
        run_one(
            "local x = 2; return if x == 1 then \"one\" elseif x == 2 then \"two\" else \"other\""
        )
        .await,
        Value::string("two")
    );
}

#[tokio::test]
async fn if_expr_nested() {
    k9::assert_equal!(
        run_one("local x = 5; local y = if x > 3 then if x > 4 then \"big\" else \"mid\" else \"small\"; return y").await,
        Value::string("big")
    );
}

#[tokio::test]
async fn if_expr_in_assignment() {
    k9::assert_equal!(
        run_one("local cond = true; local t = {v = if cond then 42 else 0}; return t.v").await,
        Value::Integer(42)
    );
}

// ---------------------------------------------------------------------------
// LuaU type annotation parsing
// ---------------------------------------------------------------------------

/// Compile a LuaU snippet and return the top-level Proto.
async fn compile_proto(src: &str) -> std::sync::Arc<shingetsu_vm::proto::Proto> {
    Compiler::new(CompileOptions::default(), Default::default())
        .compile(src)
        .await
        .expect("compile failed")
        .top_level
}

#[tokio::test]
async fn luau_type_annotation_param_basic() {
    use shingetsu_vm::types::LuaType;
    // The top-level proto's first constant closure should have the annotated
    // param types.
    let proto = compile_proto("function add(x: number, y: number): number return x + y end").await;
    // The function is in a nested proto (closure constant).
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(
        sig.params,
        vec![
            ParamSpec {
                name: Some(Bytes::from("x")),
                lua_type: Some(LuaType::Number),
                runtime_type: Some(shingetsu_vm::types::ValueType::Number),
            },
            ParamSpec {
                name: Some(Bytes::from("y")),
                lua_type: Some(LuaType::Number),
                runtime_type: Some(shingetsu_vm::types::ValueType::Number),
            },
        ]
    );
    k9::assert_equal!(sig.lua_returns, Some(vec![LuaType::Number]));
}

#[tokio::test]
async fn luau_type_annotation_param_optional() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: string, y: number?) end").await;
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(
        sig.params,
        vec![
            ParamSpec {
                name: Some(Bytes::from("x")),
                lua_type: Some(LuaType::String),
                runtime_type: Some(shingetsu_vm::types::ValueType::String),
            },
            ParamSpec {
                name: Some(Bytes::from("y")),
                lua_type: Some(LuaType::Optional(Box::new(LuaType::Number))),
                runtime_type: None,
            },
        ]
    );
}

#[tokio::test]
async fn luau_type_annotation_return_tuple() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(): (boolean, string) return true, 'ok' end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.lua_returns,
        Some(vec![LuaType::Boolean, LuaType::String])
    );
}

#[tokio::test]
async fn luau_type_annotation_no_annotation() {
    // Without annotations, lua_type should be None.
    let proto = compile_proto("function f(x, y) return x + y end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, None);
    k9::assert_equal!(child.signature.params[1].lua_type, None);
    k9::assert_equal!(child.signature.lua_returns, None);
}

#[tokio::test]
async fn luau_type_annotation_named_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: Foo) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Named(Bytes::from("Foo")))
    );
}

#[tokio::test]
async fn luau_type_annotation_union() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: string | number) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![LuaType::String, LuaType::Number]))
    );
}

#[tokio::test]
async fn luau_type_annotation_callback() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(cb: (number) -> string) end").await;
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Function(flt) => {
            k9::assert_equal!(flt.params, vec![(None, LuaType::Number)]);
            k9::assert_equal!(flt.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_type_annotation_table_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(t: { x: number, y: string }) end").await;
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Table(tlt) => {
            k9::assert_equal!(
                tlt.fields,
                vec![
                    (Bytes::from("x"), LuaType::Number),
                    (Bytes::from("y"), LuaType::String)
                ]
            );
            k9::assert_equal!(tlt.indexer, None);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_type_annotation_table_indexer() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(t: { [string]: number }) end").await;
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Table(tlt) => {
            k9::assert_equal!(tlt.fields, vec![]);
            k9::assert_equal!(
                tlt.indexer,
                Some((Box::new(LuaType::String), Box::new(LuaType::Number)))
            );
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_type_annotation_generic_type() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    let proto = compile_proto("function f(t: Map<string, number>) end").await;
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Map")));
            k9::assert_equal!(
                *args,
                vec![
                    LuaTypeArg::Type(LuaType::String),
                    LuaTypeArg::Type(LuaType::Number)
                ]
            );
        }
        other => panic!("expected Generic, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_type_annotation_array_shorthand() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    let proto = compile_proto("function f(t: { number }) end").await;
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Array")));
            k9::assert_equal!(*args, vec![LuaTypeArg::Type(LuaType::Number)]);
        }
        other => panic!("expected Generic(Array), got {:?}", other),
    }
}

#[tokio::test]
async fn luau_type_annotation_intersection() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: Readable & Writable) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Intersection(vec![
            LuaType::Named(Bytes::from("Readable")),
            LuaType::Named(Bytes::from("Writable")),
        ]))
    );
}

#[tokio::test]
async fn luau_type_annotation_basic_primitives() {
    use shingetsu_vm::types::LuaType;
    let proto =
        compile_proto("function f(a: nil, b: boolean, c: any, d: integer, e: float): never end")
            .await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Nil));
    k9::assert_equal!(child.signature.params[1].lua_type, Some(LuaType::Boolean));
    k9::assert_equal!(child.signature.params[2].lua_type, Some(LuaType::Any));
    k9::assert_equal!(child.signature.params[3].lua_type, Some(LuaType::Integer));
    k9::assert_equal!(child.signature.params[4].lua_type, Some(LuaType::Float));
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::Never]));
}

#[tokio::test]
async fn luau_type_annotation_typeof() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: typeof({})) end").await;
    let child = &proto.protos[0];
    // typeof is opaque at compile time — treated as Any.
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Any));
}

#[tokio::test]
async fn luau_type_annotation_method_self() {
    use shingetsu_vm::types::LuaType;
    // Method syntax: implicit self has no annotation.
    let proto = compile_proto("local t = {}; function t:m(x: number) end").await;
    let child = &proto.protos[0];
    let sig = &child.signature;
    // self is param[0], x is param[1]
    k9::assert_equal!(
        sig.params,
        vec![
            ParamSpec {
                name: Some(Bytes::from("self")),
                lua_type: None,
                runtime_type: None,
            },
            ParamSpec {
                name: Some(Bytes::from("x")),
                lua_type: Some(LuaType::Number),
                runtime_type: Some(shingetsu_vm::types::ValueType::Number),
            },
        ]
    );
}

#[tokio::test]
async fn luau_type_annotation_mixed_annotated_unannotated() {
    use shingetsu_vm::types::LuaType;
    // Some params annotated, some not.
    let proto = compile_proto("function f(a: number, b, c: string) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(child.signature.params[1].lua_type, None);
    k9::assert_equal!(child.signature.params[2].lua_type, Some(LuaType::String));
}

#[tokio::test]
async fn luau_type_annotation_variadic_param() {
    use shingetsu_vm::types::LuaType;
    // Variadic params don't get a ParamSpec entry, but should not break parsing.
    let proto = compile_proto("function f(x: number, ...): string end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params,
        vec![ParamSpec {
            name: Some(Bytes::from("x")),
            lua_type: Some(LuaType::Number),
            runtime_type: Some(shingetsu_vm::types::ValueType::Number),
        }]
    );
    k9::assert_equal!(child.signature.variadic, true);
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::String]));
}

// ---------------------------------------------------------------------------
// LuaU runtime type enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn luau_runtime_type_check_rejects_wrong_type() {
    // Annotated Lua function rejects wrong argument type at call boundary.
    let res = run_all(
        "function add(x: number, y: number): number return x + y end
         local ok, err = pcall(add, 1, 'two')
         return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #2 to 'add' (number expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn luau_runtime_type_check_accepts_correct_type() {
    // Annotated Lua function accepts correct types.
    let res = run_one(
        "function add(x: number, y: number): number return x + y end
         return add(3, 4)",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(7));
}

#[tokio::test]
async fn luau_runtime_type_check_string_param() {
    let res = run_all(
        "function greet(name: string) return 'hi ' .. name end
         local ok, err = pcall(greet, 42)
         return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'greet' (string expected, got number)"),
        ]
    );
}

#[tokio::test]
async fn luau_runtime_type_check_table_param() {
    let res = run_all(
        "function keys(t: {[string]: number}) return next(t) end
         local ok, err = pcall(keys, 'not a table')
         return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'keys' (table expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn luau_runtime_type_check_boolean_param() {
    let res = run_all(
        "function toggle(b: boolean) return not b end
         local ok, err = pcall(toggle, 'yes')
         return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'toggle' (boolean expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn luau_runtime_type_check_optional_allows_nil() {
    // Optional params should NOT reject nil.
    let res = run_one(
        "function f(x: number?) return x or 0 end
         return f(nil)",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(0));
}

#[tokio::test]
async fn luau_runtime_type_check_unannotated_no_check() {
    // Unannotated params should accept any type (no runtime check).
    let res = run_one(
        "function f(x) return type(x) end
         return f({})",
    )
    .await;
    k9::assert_equal!(res, Value::string("table"));
}

#[tokio::test]
async fn luau_runtime_type_check_function_param() {
    let res = run_all(
        "function apply(cb: (number) -> number) return cb(5) end
         local ok, err = pcall(apply, 'not a function')
         return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'apply' (function expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn luau_runtime_type_check_function_param_accepts() {
    let res = run_one(
        "function apply(cb: (number) -> number) return cb(5) end
         return apply(function(x) return x * 2 end)",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(10));
}

#[tokio::test]
async fn luau_runtime_type_check_integer_rejects_float() {
    let res = run_all(
        "function f(x: integer) return x end
         local ok, err = pcall(f, 1.5)
         return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'f' (integer expected, got number)"),
        ]
    );
}

#[tokio::test]
async fn luau_runtime_type_check_integer_accepts_integer() {
    let res = run_one(
        "function f(x: integer) return x + 1 end
         return f(10)",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(11));
}

#[tokio::test]
async fn luau_runtime_type_check_any_accepts_all() {
    // `any` annotation should accept any value.
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f(42)").await,
        Value::string("number")
    );
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f('s')").await,
        Value::string("string")
    );
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f(nil)").await,
        Value::string("nil")
    );
}

#[tokio::test]
async fn luau_runtime_type_check_direct_call_fails() {
    // Direct call (not pcall) with wrong type should produce an error
    // from the initial task entry validation.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{CompileOptions, Compiler};

    let compiler = Compiler::new(
        CompileOptions {
            ..CompileOptions::default()
        },
        Default::default(),
    );
    // Compile a chunk that defines a typed function then calls it wrong.
    let bc = compiler
        .compile("function f(x: number) return x end; return f('bad')")
        .await
        .expect("compile");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, valuevec![]).await.unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'f' (number expected, got string)"
    );
}

#[tokio::test]
async fn luau_type_annotation_string_literal() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto(r#"function f(x: "hello") end"#).await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::StringLiteral(Bytes::from("hello")))
    );
}

#[tokio::test]
async fn luau_type_annotation_boolean_literal() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: true) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::BoolLiteral(true))
    );
}

// ---------------------------------------------------------------------------
// Generic type parameter declarations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn luau_generic_function_type_params() {
    let proto = compile_proto("function identity<T>(x: T): T return x end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.type_params,
        vec![GenericTypeParam {
            name: Bytes::from("T"),
            is_pack: false,
            constraint: None,
            default: None,
        }]
    );
}

#[tokio::test]
async fn luau_generic_function_param_is_type_param() {
    use shingetsu_vm::types::LuaType;
    // Inside a generic function, `T` in parameter annotations should be
    // `LuaType::TypeParam`, not `LuaType::Named`.
    let proto = compile_proto("function identity<T>(x: T): T return x end").await;
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

#[tokio::test]
async fn luau_generic_multiple_type_params() {
    let proto =
        compile_proto("function map<T, U>(list: {T}, f: (T) -> U): {U} return {} end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.type_params,
        vec![
            GenericTypeParam {
                name: Bytes::from("T"),
                is_pack: false,
                constraint: None,
                default: None,
            },
            GenericTypeParam {
                name: Bytes::from("U"),
                is_pack: false,
                constraint: None,
                default: None,
            },
        ]
    );
}

#[tokio::test]
async fn luau_generic_variadic_pack() {
    let proto = compile_proto("function first<T...>(...: T...): T... return ... end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.type_params,
        vec![GenericTypeParam {
            name: Bytes::from("T"),
            is_pack: true,
            constraint: None,
            default: None,
        }]
    );
}

#[tokio::test]
async fn luau_generic_with_default_on_type_alias() {
    // Default type params are supported on type aliases, not functions.
    // Verify they parse correctly via a callback type that uses one.
    // full_moon does not support `<T = number>` on function generics,
    // so we test default parsing indirectly via the GenericDeclaration
    // on a type alias (tested in G2). For now, just verify that
    // function generics without defaults work.
    let proto = compile_proto("function f<T>(x: T): T return x end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params[0].default, None);
}

#[tokio::test]
async fn luau_generic_non_generic_name_stays_named() {
    use shingetsu_vm::types::LuaType;
    // `Foo` is not a declared type param, so it should be `LuaType::Named`.
    let proto = compile_proto("function f<T>(x: T, y: Foo): T return x end").await;
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

#[tokio::test]
async fn luau_generic_erased_at_runtime() {
    // A generic param like `T` should not produce a runtime_type
    // (it's erased — treated as `any`).
    let proto = compile_proto("function identity<T>(x: T): T return x end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].runtime_type, None);
}

#[tokio::test]
async fn luau_generic_function_still_runs() {
    // Generic function should compile and execute normally.
    k9::assert_equal!(
        run_one("function identity<T>(x: T): T return x end\nreturn identity(42)").await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn luau_generic_type_param_in_callback() {
    use shingetsu_vm::types::LuaType;
    // T inside a callback parameter should be TypeParam.
    let proto = compile_proto("function f<T>(cb: (T) -> T) end").await;
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

#[tokio::test]
async fn luau_generic_type_param_in_optional() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: T?) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::TypeParam(
            Bytes::from("T")
        ))))
    );
}

#[tokio::test]
async fn luau_generic_type_param_in_union() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: T | string) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![
            LuaType::TypeParam(Bytes::from("T")),
            LuaType::String,
        ]))
    );
}

#[tokio::test]
async fn luau_generic_type_param_in_table() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: { val: T }) end").await;
    let child = &proto.protos[0];
    match child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type")
    {
        LuaType::Table(t) => {
            k9::assert_equal!(
                t.fields,
                vec![(Bytes::from("val"), LuaType::TypeParam(Bytes::from("T")))]
            );
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_generic_type_param_in_generic_instantiation() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // T used as a type argument: Array<T>
    let proto = compile_proto("function f<T>(x: Array<T>) end").await;
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

#[tokio::test]
async fn luau_generic_type_param_in_array_shorthand() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // {T} is array shorthand — T inside should be TypeParam.
    let proto = compile_proto("function f<T>(x: {T}) end").await;
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

#[tokio::test]
async fn luau_generic_does_not_leak_to_sibling_function() {
    use shingetsu_vm::types::LuaType;
    // T is declared on f but not on g — in g, T should be Named.
    let proto = compile_proto("function f<T>(x: T) end\nfunction g(x: T) end").await;
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

#[tokio::test]
async fn luau_generic_local_function() {
    use shingetsu_vm::types::LuaType;
    // local function should go through the same generic path.
    let proto = compile_proto("local function f<T>(x: T): T return x end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.type_params,
        vec![GenericTypeParam {
            name: Bytes::from("T"),
            is_pack: false,
            constraint: None,
            default: None,
        }]
    );
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
}

#[tokio::test]
async fn luau_generic_multiple_params_execution() {
    // Multi-param generic function should execute correctly.
    k9::assert_equal!(
        run_one("function swap<A, B>(a: A, b: B): (B, A) return b, a end\nreturn swap(1, 'hello')")
            .await,
        Value::string("hello")
    );
}

// ---------------------------------------------------------------------------
// Type alias declarations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn luau_type_alias_simple() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Meters = number").await;
    let alias = proto
        .type_aliases
        .get(b"Meters" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.params, vec![]);
    k9::assert_equal!(alias.body, LuaType::Number);
    k9::assert_equal!(alias.exported, false);
}

#[tokio::test]
async fn luau_type_alias_exported() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("export type Meters = number").await;
    let alias = proto
        .type_aliases
        .get(b"Meters" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.params, vec![]);
    k9::assert_equal!(alias.body, LuaType::Number);
    k9::assert_equal!(alias.exported, true);
}

#[tokio::test]
async fn luau_type_alias_exported_and_local() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto(
        "export type Public = number\n\
         type Private = string",
    )
    .await;
    let public = proto
        .type_aliases
        .get(b"Public" as &[u8])
        .expect("public alias exists");
    k9::assert_equal!(public.exported, true);
    k9::assert_equal!(public.body, LuaType::Number);
    let private = proto
        .type_aliases
        .get(b"Private" as &[u8])
        .expect("private alias exists");
    k9::assert_equal!(private.exported, false);
    k9::assert_equal!(private.body, LuaType::String);
}

#[tokio::test]
async fn module_type_info_exported_types() {
    use shingetsu_vm::types::LuaType;
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "export type Point = { x: number, y: number }\n\
             type Internal = string\n\
             export type Id = number",
        )
        .await
        .expect("compile");
    let info = &bc.module_type_info;
    // Only exported types appear in module_type_info.
    let mut keys: Vec<&[u8]> = info.exported_types.keys().map(|k| k.as_ref()).collect();
    keys.sort();
    k9::assert_equal!(keys, vec![b"Id" as &[u8], b"Point"]);
    k9::assert_equal!(
        info.exported_types
            .get(b"Point" as &[u8])
            .expect("Point")
            .body,
        LuaType::Table(Box::new(shingetsu_vm::types::TableLuaType {
            fields: vec![
                (Bytes::from("x"), LuaType::Number),
                (Bytes::from("y"), LuaType::Number),
            ],
            indexer: None,
        }))
    );
    k9::assert_equal!(
        info.exported_types.get(b"Id" as &[u8]).expect("Id").body,
        LuaType::Number
    );
    k9::assert_equal!(
        info.exported_types.contains_key(b"Internal" as &[u8]),
        false
    );
    // Return type is not determined (no type annotation on return).
    k9::assert_equal!(info.return_type, None);
}

#[tokio::test]
async fn module_type_info_return_type_from_annotation() {
    use shingetsu_vm::types::LuaType;
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "export type MyMod = { x: number }\n\
             local M: MyMod = { x = 42 }\n\
             return M",
        )
        .await
        .expect("compile");
    let info = &bc.module_type_info;
    k9::assert_equal!(
        info.return_type,
        Some(LuaType::Table(Box::new(
            shingetsu_vm::types::TableLuaType {
                fields: vec![(Bytes::from("x"), LuaType::Number)],
                indexer: None,
            }
        )))
    );
}

#[tokio::test]
async fn module_type_info_return_type_table_without_annotation() {
    use shingetsu_vm::types::LuaType;
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local M = { x = 42 }\n\
             return M",
        )
        .await
        .expect("compile");
    // Table constructor seeds an empty table type on the local.
    // Constructor field inference is future work.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(
            shingetsu_vm::types::TableLuaType {
                fields: vec![],
                indexer: None,
            }
        )))
    );
}

#[tokio::test]
async fn luau_type_alias_with_generics() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Pair<A, B> = { first: A, second: B }").await;
    let alias = proto
        .type_aliases
        .get(b"Pair" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(
        alias.params,
        vec![
            GenericTypeParam {
                name: Bytes::from("A"),
                is_pack: false,
                constraint: None,
                default: None,
            },
            GenericTypeParam {
                name: Bytes::from("B"),
                is_pack: false,
                constraint: None,
                default: None,
            },
        ]
    );
    // The body should use TypeParam for A and B.
    match &alias.body {
        LuaType::Table(t) => {
            k9::assert_equal!(
                t.fields,
                vec![
                    (Bytes::from("first"), LuaType::TypeParam(Bytes::from("A"))),
                    (Bytes::from("second"), LuaType::TypeParam(Bytes::from("B"))),
                ]
            );
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_type_alias_function_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Predicate = (number) -> boolean").await;
    let alias = proto
        .type_aliases
        .get(b"Predicate" as &[u8])
        .expect("alias exists");
    match &alias.body {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params, vec![(None, LuaType::Number)]);
            k9::assert_equal!(ft.returns, vec![LuaType::Boolean]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_type_alias_no_runtime_effect() {
    // Type aliases produce no instructions.
    k9::assert_equal!(
        run_one("type Meters = number\nreturn 42").await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn luau_exported_type_alias() {
    use shingetsu_vm::types::LuaType;
    // `export type` should be stored the same as `type`.
    let proto = compile_proto("export type ID = string").await;
    let alias = proto
        .type_aliases
        .get(b"ID" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.body, LuaType::String);
}

#[tokio::test]
async fn luau_type_alias_union_body() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type StringOrNumber = string | number").await;
    let alias = proto
        .type_aliases
        .get(b"StringOrNumber" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(
        alias.body,
        LuaType::Union(vec![LuaType::String, LuaType::Number])
    );
}

#[tokio::test]
async fn luau_type_alias_optional_body() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type MaybeString = string?").await;
    let alias = proto
        .type_aliases
        .get(b"MaybeString" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.body, LuaType::Optional(Box::new(LuaType::String)));
}

#[tokio::test]
async fn luau_type_alias_multiple_in_chunk() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type A = number\ntype B = string").await;
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

#[tokio::test]
async fn luau_type_alias_overwrite() {
    use shingetsu_vm::types::LuaType;
    // Last declaration wins.
    let proto = compile_proto("type X = number\ntype X = string").await;
    k9::assert_equal!(
        proto
            .type_aliases
            .get(b"X" as &[u8])
            .expect("X exists")
            .body,
        LuaType::String
    );
}

#[tokio::test]
async fn luau_type_alias_references_named_type() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // `User` is not a generic param, so it stays Named.
    let proto = compile_proto("type UserList = Array<User>").await;
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

#[tokio::test]
async fn luau_type_alias_generic_params_dont_leak() {
    use shingetsu_vm::types::LuaType;
    // T is a generic param on Foo but not on Bar.
    let proto = compile_proto("type Foo<T> = T\ntype Bar = T").await;
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

#[tokio::test]
async fn luau_alias_resolution_simple() {
    use shingetsu_vm::types::LuaType;
    // `type Meters = number` then a function using Meters should resolve to Number.
    let proto = compile_proto("type Meters = number\nfunction f(x: Meters) end").await;
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(
        sig.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Number)
    );
}

#[tokio::test]
async fn luau_alias_resolution_string_alias() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Name = string\nfunction greet(who: Name) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::String));
    k9::assert_equal!(
        child.signature.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::String)
    );
}

#[tokio::test]
async fn luau_alias_resolution_generic_table() {
    use shingetsu_vm::types::LuaType;
    // Generic alias `Pair<A, B>` with concrete args `number, string`.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string>) end",
    )
    .await;
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(
                t.fields,
                vec![
                    (Bytes::from("first"), LuaType::Number),
                    (Bytes::from("second"), LuaType::String)
                ]
            );
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_alias_resolution_generic_table_has_runtime_type() {
    // Expanded table alias has Table runtime type.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string>) end",
    )
    .await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Table)
    );
}

#[tokio::test]
async fn luau_alias_resolution_optional() {
    use shingetsu_vm::types::LuaType;
    // `type Id = number` then `function f(x: Id?) end` should give Optional(Number).
    let proto = compile_proto("type Id = number\nfunction f(x: Id?) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

#[tokio::test]
async fn luau_alias_resolution_in_union() {
    use shingetsu_vm::types::LuaType;
    // Alias used as part of a union.
    let proto = compile_proto("type Id = number\nfunction f(x: Id | string) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![LuaType::Number, LuaType::String]))
    );
}

#[tokio::test]
async fn luau_alias_resolution_no_runtime_effect() {
    // Aliases have no runtime effect — the code still runs.
    k9::assert_equal!(
        run_one(
            "type Meters = number\n\
             function add(a: Meters, b: Meters): Meters\n\
             return a + b\n\
             end\n\
             return add(3, 4)"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn luau_alias_resolution_chained() {
    use shingetsu_vm::types::LuaType;
    // `type A = number`, `type B = A` — B should resolve to number too.
    let proto = compile_proto("type A = number\ntype B = A\nfunction f(x: B) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
}

#[tokio::test]
async fn luau_alias_resolution_in_return_type() {
    use shingetsu_vm::types::LuaType;
    // Alias should also resolve in return type annotations.
    let proto =
        compile_proto("type Meters = number\nfunction f(x: number): Meters return x end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::Number]));
}

#[tokio::test]
async fn luau_alias_resolution_generic_in_function_type() {
    use shingetsu_vm::types::LuaType;
    // `type Mapper<T, U> = (T) -> U` then `function f(m: Mapper<number, string>) end`
    let proto =
        compile_proto("type Mapper<T, U> = (T) -> U\nfunction f(m: Mapper<number, string>) end")
            .await;
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params, vec![(None, LuaType::Number)]);
            k9::assert_equal!(ft.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_alias_resolution_preserves_unrelated_generics() {
    use shingetsu_vm::types::LuaType;
    // A function with its own generic T that is NOT an alias should still produce TypeParam.
    let proto = compile_proto("type Meters = number\nfunction f<T>(x: Meters, y: T) end").await;
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

#[tokio::test]
async fn luau_alias_resolution_alias_in_alias_body() {
    use shingetsu_vm::types::LuaType;
    // `type A = number`, `type B = { x: A }` — alias body references another alias.
    let proto = compile_proto("type A = number\ntype B = { x: A }\nfunction f(p: B) end").await;
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields, vec![(Bytes::from("x"), LuaType::Number)]);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_alias_resolution_generic_fewer_args() {
    use shingetsu_vm::types::LuaType;
    // `Pair<number>` with only one arg — B defaults to Any.
    let proto =
        compile_proto("type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number>) end")
            .await;
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
            k9::assert_equal!(t.fields[1].1, LuaType::Any);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_alias_resolution_generic_extra_args() {
    use shingetsu_vm::types::LuaType;
    // `Pair<number, string, boolean>` — extra arg is silently ignored.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string, boolean>) end",
    ).await;
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

#[tokio::test]
async fn luau_alias_resolution_in_callback_param() {
    use shingetsu_vm::types::LuaType;
    // Alias used inside a callback parameter type.
    let proto = compile_proto("type Meters = number\nfunction f(cb: (Meters) -> string) end").await;
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params, vec![(None, LuaType::Number)]);
            k9::assert_equal!(ft.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_alias_resolution_in_table_field() {
    use shingetsu_vm::types::LuaType;
    // Alias used inside a table type annotation on a param.
    let proto = compile_proto("type Meters = number\nfunction f(p: { dist: Meters }) end").await;
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields, vec![(Bytes::from("dist"), LuaType::Number)]);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[tokio::test]
async fn luau_alias_resolution_nested_generic_optional() {
    use shingetsu_vm::types::LuaType;
    // `type Wrap<T> = T?` then `Wrap<number>` should give `Optional(Number)`.
    let proto = compile_proto("type Wrap<T> = T?\nfunction f(x: Wrap<number>) end").await;
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

// ---------------------------------------------------------------------------
// table.create / find / clear / freeze / isfrozen / clone (LuaU extensions)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn luau_table_create_default_nil() {
    // Without a value argument all slots are effectively nil —
    // assigning nil leaves the table empty, so #t is 0.
    let res = run_all(
        "\
        local t = table.create(3)\n\
        return #t, t[1], t[2], t[3]",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(0), Value::Nil, Value::Nil, Value::Nil,]
    );
}

#[tokio::test]
async fn luau_table_create_with_value() {
    let res = run_all(
        "\
        local t = table.create(4, 'x')\n\
        return #t, t[1], t[4]",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![Value::Integer(4), Value::string("x"), Value::string("x"),]
    );
}

#[tokio::test]
async fn luau_table_create_zero() {
    let res = run_one("local t = table.create(0, 'x'); return #t").await;
    k9::assert_equal!(res, Value::Integer(0));
}

#[tokio::test]
async fn luau_table_create_negative_errors() {
    let err = common::run_err("table.create(-1, 'x')").await;
    k9::assert_equal!(err, "bad argument #1 to 'create' (size out of range: -1)");
}

#[tokio::test]
async fn luau_table_find_present() {
    let res = run_one("return table.find({10, 20, 30}, 20)").await;
    k9::assert_equal!(res, Value::Integer(2));
}

#[tokio::test]
async fn luau_table_find_missing() {
    let res = run_one("return table.find({10, 20, 30}, 99)").await;
    k9::assert_equal!(res, Value::Nil);
}

#[tokio::test]
async fn luau_table_find_with_init() {
    // Two 20s; init=3 skips the first one.
    let res = run_one("return table.find({20, 10, 20, 10}, 20, 3)").await;
    k9::assert_equal!(res, Value::Integer(3));
}

#[tokio::test]
async fn luau_table_find_string() {
    let res = run_one("return table.find({'a', 'b', 'c'}, 'b')").await;
    k9::assert_equal!(res, Value::Integer(2));
}

#[tokio::test]
async fn luau_table_find_init_zero_errors() {
    let err = common::run_err("table.find({1,2,3}, 2, 0)").await;
    k9::assert_equal!(err, "bad argument #3 to 'find' (index out of range: 0)");
}

#[tokio::test]
async fn luau_table_clear() {
    let res = run_all(
        "\
        local t = {1, 2, 3, foo = 'bar'}\n\
        table.clear(t)\n\
        return #t, t.foo, t[1]",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::Integer(0), Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn luau_table_freeze_returns_table() {
    // table.freeze returns the same table so you can chain.
    let res = run_one(
        "\
        local t = {1, 2, 3}\n\
        local u = table.freeze(t)\n\
        return t == u",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn luau_table_isfrozen_default_false() {
    let res = run_one("return table.isfrozen({1,2,3})").await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn luau_table_isfrozen_after_freeze() {
    let res = run_one(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        return table.isfrozen(t)",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn luau_frozen_table_rejects_assignment() {
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        t[4] = 99",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[tokio::test]
async fn luau_frozen_table_rejects_insert() {
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        table.insert(t, 4)",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[tokio::test]
async fn luau_frozen_table_rejects_clear() {
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        table.clear(t)",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[tokio::test]
async fn luau_freeze_is_idempotent() {
    // Freezing an already-frozen table is fine.
    let res = run_one(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        table.freeze(t)\n\
        return table.isfrozen(t)",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(true));
}

#[tokio::test]
async fn luau_table_clone_shallow() {
    // Contents copied by value, nested tables shared by reference.
    let res = run_all(
        "\
        local inner = {10}\n\
        local t = {1, 2, inner}\n\
        local c = table.clone(t)\n\
        return c[1], c[2], c[3] == inner, c == t",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Boolean(true),
            Value::Boolean(false),
        ]
    );
}

#[tokio::test]
async fn luau_table_clone_of_frozen_is_not_frozen() {
    // Per LuaU: the clone is never frozen even if the source is.
    let res = run_one(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        local c = table.clone(t)\n\
        return table.isfrozen(c)",
    )
    .await;
    k9::assert_equal!(res, Value::Boolean(false));
}

#[tokio::test]
async fn luau_table_clone_allows_mutation() {
    // Sanity check that the clone really is writable.
    let res = run_all(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        local c = table.clone(t)\n\
        c[4] = 4\n\
        return #c, c[4]",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::Integer(4), Value::Integer(4)]);
}

#[tokio::test]
async fn luau_table_clone_copies_metatable() {
    // Shallow clone keeps the same metatable (shared by Arc ref).
    let res = run_one(
        "\
        local mt = {__index = function() return 42 end}\n\
        local t = setmetatable({}, mt)\n\
        local c = table.clone(t)\n\
        return c.anything",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(42));
}

// ---- Frozen rejection: remaining mutation paths ----------------------------

#[tokio::test]
async fn luau_frozen_table_rejects_setmetatable() {
    // `setmetatable` mutates the table's metatable slot — check_writable
    // fires before the assignment.
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        setmetatable(t, {})",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[tokio::test]
async fn luau_frozen_table_rejects_rawset() {
    // `rawset` goes straight to `raw_set`, bypassing __newindex.
    let err = common::run_err(
        "\
        local t = {1,2,3}\n\
        table.freeze(t)\n\
        rawset(t, 1, 99)",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[tokio::test]
async fn luau_frozen_table_rejects_sort() {
    // `table.sort` uses `swap_array`, which must propagate the frozen error.
    let err = common::run_err(
        "\
        local t = {3, 1, 2}\n\
        table.freeze(t)\n\
        table.sort(t)",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[tokio::test]
async fn luau_frozen_table_rejects_remove() {
    // `table.remove` on a non-empty frozen table hits `raw_remove`.
    let err = common::run_err(
        "\
        local t = {10, 20, 30}\n\
        table.freeze(t)\n\
        table.remove(t, 1)",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

#[tokio::test]
async fn luau_frozen_table_newindex_fires_for_new_key() {
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
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::Integer(99), Value::Nil]);
}

#[tokio::test]
async fn luau_frozen_table_existing_key_bypasses_newindex() {
    // When the key already exists, __newindex is skipped and raw_set runs
    // directly — which on a frozen table errors.
    let err = common::run_err(
        "\
        local t = {existing = 1}\n\
        setmetatable(t, {__newindex = function() end})\n\
        table.freeze(t)\n\
        t.existing = 99",
    )
    .await;
    k9::assert_equal!(err, "attempt to modify a readonly table");
}

// ---- table.find edge cases -------------------------------------------------

#[tokio::test]
async fn luau_table_find_init_past_end() {
    let res = run_one("return table.find({1,2,3}, 2, 5)").await;
    k9::assert_equal!(res, Value::Nil);
}

#[tokio::test]
async fn luau_table_find_init_at_end_plus_one() {
    // init == #t + 1 is a valid starting point that matches nothing.
    let res = run_one("return table.find({1,2,3}, 3, 4)").await;
    k9::assert_equal!(res, Value::Nil);
}

#[tokio::test]
async fn luau_table_find_empty_table() {
    let res = run_one("return table.find({}, 1)").await;
    k9::assert_equal!(res, Value::Nil);
}

#[tokio::test]
async fn luau_table_find_by_table_reference() {
    // Raw equality: tables compare by identity, so the same Arc matches.
    let res = run_one(
        "\
        local a = {}\n\
        return table.find({a, {}}, a)",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(1));
}

#[tokio::test]
async fn luau_table_find_bad_arg_not_table() {
    let res = run_all(
        "local ok, err = pcall(table.find, 'not a table', 1)\n\
        return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'find' (table expected, got string)"),
        ]
    );
}

// ---- table.clone edge cases ------------------------------------------------

#[tokio::test]
async fn luau_table_clone_preserves_hash_keys() {
    let res = run_all(
        "\
        local t = {foo = 'bar', baz = 42}\n\
        local c = table.clone(t)\n\
        return c.foo, c.baz",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("bar"), Value::Integer(42),]);
}

#[tokio::test]
async fn luau_table_clone_empty() {
    let res = run_one("local c = table.clone({}); return #c").await;
    k9::assert_equal!(res, Value::Integer(0));
}

#[tokio::test]
async fn luau_table_clone_array_is_independent() {
    // Mutating the clone's array slot must not affect the source.
    let res = run_all(
        "\
        local src = {1, 2, 3}\n\
        local cp = table.clone(src)\n\
        cp[1] = 99\n\
        return src[1], cp[1]",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::Integer(1), Value::Integer(99)]);
}

// ---- table.create edge cases -----------------------------------------------

#[tokio::test]
async fn luau_table_create_bad_arg_non_integer() {
    let res = run_all(
        "local ok, err = pcall(table.create, 'not int')\n\
        return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'create' (number expected, got string)"),
        ]
    );
}

#[tokio::test]
async fn luau_table_create_bad_arg_fractional() {
    // A float with a non-zero fraction is not coercible to integer.
    let res = run_all(
        "local ok, err = pcall(table.create, 2.5)\n\
        return ok, err",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Boolean(false),
            Value::string("bad argument #1 to 'create' (number has no integer representation)"),
        ]
    );
}

#[tokio::test]
async fn luau_table_create_count_one() {
    // Boundary between 0 and the loop path.
    let res = run_all(
        "\
        local t = table.create(1, 'z')\n\
        return #t, t[1]",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::Integer(1), Value::string("z")]);
}

// ---- Arg validation on the other new helpers -------------------------------

#[tokio::test]
async fn luau_table_helpers_reject_non_table() {
    // One combined test for the uniform FromLua-generated BadArgument paths.
    let res = run_all(
        "\
        local e1 = select(2, pcall(table.clear, 'x'))\n\
        local e2 = select(2, pcall(table.freeze, 42))\n\
        local e3 = select(2, pcall(table.isfrozen, true))\n\
        local e4 = select(2, pcall(table.clone, nil))\n\
        return e1, e2, e3, e4",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::string("bad argument #1 to 'clear' (table expected, got string)"),
            Value::string("bad argument #1 to 'freeze' (table expected, got number)"),
            Value::string("bad argument #1 to 'isfrozen' (table expected, got boolean)"),
            Value::string("bad argument #1 to 'clone' (table expected, got nil)"),
        ]
    );
}

// ---------------------------------------------------------------------------
// Incremental table type accumulation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_accumulation_dot_function() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.greet(name: string): string\n\
               return 'hello ' .. name\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("greet"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("name")), LuaType::String)],
                    variadic: None,
                    returns: vec![LuaType::String],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_colon_method() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod:setup(opts: string)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("setup"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![
                        (Some(Bytes::from("self")), LuaType::Any),
                        (Some(Bytes::from("opts")), LuaType::String),
                    ],
                    variadic: None,
                    returns: vec![],
                    is_method: true,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_multiple_functions() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.add(a: number, b: number): number\n\
               return a + b\n\
             end\n\
             function mod:name(): string\n\
               return 'mod'\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![
                (
                    Bytes::from("add"),
                    LuaType::Function(Box::new(FunctionLuaType {
                        type_params: vec![],
                        params: vec![
                            (Some(Bytes::from("a")), LuaType::Number),
                            (Some(Bytes::from("b")), LuaType::Number),
                        ],
                        variadic: None,
                        returns: vec![LuaType::Number],
                        is_method: false,
                        inferred_unannotated: false,
                    }))
                ),
                (
                    Bytes::from("name"),
                    LuaType::Function(Box::new(FunctionLuaType {
                        type_params: vec![],
                        params: vec![(Some(Bytes::from("self")), LuaType::Any),],
                        variadic: None,
                        returns: vec![LuaType::String],
                        is_method: true,
                        inferred_unannotated: false,
                    }))
                ),
            ],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_unannotated_function() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.greet(name)\n\
               return 'hello ' .. name\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("greet"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("name")), LuaType::Any)],
                    variadic: None,
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: true,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_no_accumulation_without_table_constructor() {
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = require('something')\n\
             function mod.greet(name)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    // Without a require resolution, mod has no inferred type,
    // so function declarations don't accumulate.
    k9::assert_equal!(bc.module_type_info.return_type, None);
}

#[tokio::test]
async fn table_accumulation_empty_table_no_functions() {
    use shingetsu_vm::types::{LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             return mod",
        )
        .await
        .expect("compile");
    // Empty table constructor seeds an empty table type.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_multi_level_does_not_accumulate() {
    use shingetsu_vm::types::{LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             mod.sub = {}\n\
             function mod.sub.deep(x: number)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    // Multi-level dotted path is not single-level, so no accumulation.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_annotation_takes_priority() {
    use shingetsu_vm::types::LuaType;
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "type MyMod = { x: number }\n\
             local mod: MyMod = {}\n\
             return mod",
        )
        .await
        .expect("compile");
    // The type annotation wins over table constructor seeding.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(
            shingetsu_vm::types::TableLuaType {
                fields: vec![(Bytes::from("x"), LuaType::Number)],
                indexer: None,
            }
        )))
    );
}

#[tokio::test]
async fn table_accumulation_variadic_function() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.log(fmt: string, ...)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("log"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("fmt")), LuaType::String)],
                    variadic: Some(Box::new(LuaType::Any)),
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_non_function_field_no_interference() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             mod.version = '1.0'\n\
             function mod.greet(name: string): string\n\
               return 'hello ' .. name\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    // Non-function field assignment doesn't interfere with accumulation.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("greet"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("name")), LuaType::String)],
                    variadic: None,
                    returns: vec![LuaType::String],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_local_function_does_not_leak() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             local function helper() end\n\
             function mod.greet(name: string)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    // Only mod.greet should appear, not the helper function.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("greet"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("name")), LuaType::String)],
                    variadic: None,
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_field_redefinition_replaces() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.f(x: number)\n\
             end\n\
             function mod.f(x: number, y: number)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    // Second definition replaces the first — no duplicates.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("f"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![
                        (Some(Bytes::from("x")), LuaType::Number),
                        (Some(Bytes::from("y")), LuaType::Number),
                    ],
                    variadic: None,
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_multiple_independent_locals() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    // Only the returned local's type matters for module_type_info.
    // But verify both locals accumulate independently by returning `a`.
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local a = {}\n\
             local b = {}\n\
             function a.foo(x: number)\n\
             end\n\
             function b.bar(s: string)\n\
             end\n\
             return a",
        )
        .await
        .expect("compile");
    // Only a.foo should appear, not b.bar.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("foo"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("x")), LuaType::Number)],
                    variadic: None,
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_on_global_does_not_accumulate() {
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "function globalmod.f(x: number)\n\
             end\n\
             return globalmod",
        )
        .await
        .expect("compile");
    // globalmod is not a local, so no table type is accumulated.
    // The return local lookup also fails (globalmod is not a local).
    k9::assert_equal!(bc.module_type_info.return_type, None);
}

#[tokio::test]
async fn table_accumulation_method_to_function_redefinition() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod:f()\n\
             end\n\
             function mod.f(x: number)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    // Second definition (dot) replaces the first (colon).
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("f"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("x")), LuaType::Number)],
                    variadic: None,
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_zero_param_unannotated() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.init()\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("init"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: None,
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: true,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_accumulation_vararg_only() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.log(...)\n\
             end\n\
             return mod",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("log"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: Some(Box::new(LuaType::Any)),
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: true,
                }))
            )],
            indexer: None,
        })))
    );
}

// ---------------------------------------------------------------------------
// Table constructor return inference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_return_with_typed_locals() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local function greet(name: string): string\n\
               return 'hello ' .. name\n\
             end\n\
             local function add(a: number, b: number): number\n\
               return a + b\n\
             end\n\
             return { greet = greet, add = add }",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![
                (
                    Bytes::from("greet"),
                    LuaType::Function(Box::new(FunctionLuaType {
                        type_params: vec![],
                        params: vec![(Some(Bytes::from("name")), LuaType::String)],
                        variadic: None,
                        returns: vec![LuaType::String],
                        is_method: false,
                        inferred_unannotated: false,
                    }))
                ),
                (
                    Bytes::from("add"),
                    LuaType::Function(Box::new(FunctionLuaType {
                        type_params: vec![],
                        params: vec![
                            (Some(Bytes::from("a")), LuaType::Number),
                            (Some(Bytes::from("b")), LuaType::Number),
                        ],
                        variadic: None,
                        returns: vec![LuaType::Number],
                        is_method: false,
                        inferred_unannotated: false,
                    }))
                ),
            ],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_constructor_return_with_untyped_locals() {
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local function greet(name)\n\
               return 'hello ' .. name\n\
             end\n\
             return { greet = greet }",
        )
        .await
        .expect("compile");
    // Untyped local function has no inferred_type, so the field
    // is skipped and the constructor returns None (no inferrable fields).
    k9::assert_equal!(bc.module_type_info.return_type, None);
}

#[tokio::test]
async fn table_constructor_return_empty() {
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile("return {}")
        .await
        .expect("compile");
    // Empty constructor in return position has no named fields.
    k9::assert_equal!(bc.module_type_info.return_type, None);
}

#[tokio::test]
async fn table_constructor_return_mixed_typed_untyped() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local function typed(x: number): number return x end\n\
             local function untyped(x) return x end\n\
             return { typed = typed, untyped = untyped }",
        )
        .await
        .expect("compile");
    // Only the typed function contributes a field.
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("typed"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("x")), LuaType::Number)],
                    variadic: None,
                    returns: vec![LuaType::Number],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_constructor_return_preserves_field_order() {
    use shingetsu_vm::types::LuaType;
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local function beta(x: number) end\n\
             local function alpha(s: string) end\n\
             return { beta = beta, alpha = alpha }",
        )
        .await
        .expect("compile");
    // Fields appear in declaration order (beta before alpha),
    // not sorted alphabetically.
    let fields = match &bc.module_type_info.return_type {
        Some(LuaType::Table(t)) => &t.fields,
        other => panic!("expected Table, got {:?}", other),
    };
    k9::assert_equal!(fields[0].0, Bytes::from("beta"));
    k9::assert_equal!(fields[1].0, Bytes::from("alpha"));
}

#[tokio::test]
async fn table_constructor_return_with_accumulated_table() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local t = {}\n\
             function t.f(x: number): number\n\
               return x\n\
             end\n\
             return { utils = t }",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("utils"),
                LuaType::Table(Box::new(TableLuaType {
                    fields: vec![(
                        Bytes::from("f"),
                        LuaType::Function(Box::new(FunctionLuaType {
                            type_params: vec![],
                            params: vec![(Some(Bytes::from("x")), LuaType::Number)],
                            variadic: None,
                            returns: vec![LuaType::Number],
                            is_method: false,
                            inferred_unannotated: false,
                        }))
                    )],
                    indexer: None,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_constructor_return_positional_fields_ignored() {
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local function greet(name: string) end\n\
             return { greet }",
        )
        .await
        .expect("compile");
    // Positional (NoKey) fields don't contribute to the table type.
    k9::assert_equal!(bc.module_type_info.return_type, None);
}

#[tokio::test]
async fn table_constructor_return_dotted_local_access() {
    use shingetsu_vm::types::{FunctionLuaType, LuaType, TableLuaType};
    let bc = Compiler::new(CompileOptions::default(), Default::default())
        .compile(
            "local mod = {}\n\
             function mod.f(x: number): number\n\
               return x\n\
             end\n\
             return { f = mod.f }",
        )
        .await
        .expect("compile");
    k9::assert_equal!(
        bc.module_type_info.return_type,
        Some(LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                Bytes::from("f"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![(Some(Bytes::from("x")), LuaType::Number)],
                    variadic: None,
                    returns: vec![LuaType::Number],
                    is_method: false,
                    inferred_unannotated: false,
                }))
            )],
            indexer: None,
        })))
    );
}

#[tokio::test]
async fn table_constructor_return_dotted_global_access() {
    use shingetsu_vm::types::LuaType;
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    let bc = Compiler::new(CompileOptions::default(), env.global_type_map())
        .compile("return { abs = math.abs }")
        .await
        .expect("compile");
    // math.abs type is resolved from the global type map.
    let fields = match &bc.module_type_info.return_type {
        Some(LuaType::Table(t)) => &t.fields,
        other => panic!("expected Table, got {:?}", other),
    };
    match fields.as_slice() {
        [(name, LuaType::Function(f))] => {
            k9::assert_equal!(*name, Bytes::from("abs"));
            k9::assert_equal!(f.is_method, false);
        }
        other => panic!("expected single Function field, got {:?}", other),
    }
}

// ---- String interpolation -----------------------------------------------

#[tokio::test]
async fn interp_basic_variable() {
    let result = run_one(
        r#"local name = "world"
return `hello {name}`"#,
    )
    .await;
    k9::assert_equal!(result, Value::String(Bytes::from("hello world")));
}

#[tokio::test]
async fn interp_number() {
    let result = run_one("local x = 42\nreturn `count: {x}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("count: 42")));
}

#[tokio::test]
async fn interp_float() {
    let result = run_one("return `pi: {3.14}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("pi: 3.14")));
}

#[tokio::test]
async fn interp_boolean() {
    let result = run_one("return `flag: {true}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("flag: true")));
}

#[tokio::test]
async fn interp_nil() {
    let result = run_one("return `val: {nil}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("val: nil")));
}

#[tokio::test]
async fn interp_expression() {
    let result = run_one("return `sum: {1 + 2}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("sum: 3")));
}

#[tokio::test]
async fn interp_multiple_segments() {
    let result = run_one(
        r#"local a = "x"
local b = "y"
local c = "z"
return `{a} and {b} and {c}`"#,
    )
    .await;
    k9::assert_equal!(result, Value::String(Bytes::from("x and y and z")));
}

#[tokio::test]
async fn interp_adjacent_expressions() {
    let result = run_one(r#"return `{"hello"}{"world"}`"#).await;
    k9::assert_equal!(result, Value::String(Bytes::from("helloworld")));
}

#[tokio::test]
async fn interp_no_expressions() {
    let result = run_one("return `just a string`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("just a string")));
}

#[tokio::test]
async fn interp_empty() {
    let result = run_one("return ``").await;
    k9::assert_equal!(result, Value::String(Bytes::from("")));
}

#[tokio::test]
async fn interp_escape_backtick() {
    let result = run_one(r"return `hello \` world`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("hello ` world")));
}

#[tokio::test]
async fn interp_escape_brace() {
    let result = run_one(r"return `hello \{ world`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("hello { world")));
}

#[tokio::test]
async fn interp_escape_backslash() {
    let result = run_one(r"return `hello \\ world`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("hello \\ world")));
}

#[tokio::test]
async fn interp_table_tostring() {
    let result = run_one(
        r#"local t = setmetatable({}, {
    __tostring = function() return "MyTable" end
})
return `value: {t}`"#,
    )
    .await;
    k9::assert_equal!(result, Value::String(Bytes::from("value: MyTable")));
}

#[tokio::test]
async fn interp_function_call_in_expr() {
    let result = run_one(
        r#"local function double(x: number): number
    return x * 2
end
return `result: {double(21)}`"#,
    )
    .await;
    k9::assert_equal!(result, Value::String(Bytes::from("result: 42")));
}

#[tokio::test]
async fn interp_nested_interpolation() {
    let result = run_one(
        r#"local x = 1
local y = 2
return `{`{x}`} + {`{y}`}`"#,
    )
    .await;
    k9::assert_equal!(result, Value::String(Bytes::from("1 + 2")));
}

#[tokio::test]
async fn interp_constant_fold_single_literal() {
    // No expressions — should compile to a single LoadK, no concat.
    let result = run_one("return `hello world`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("hello world")));
}

#[tokio::test]
async fn interp_register_batching() {
    // Burn 250 registers with locals so only ~5 remain, then use an
    // interpolated string with more parts than fit in one batch.
    let mut code = String::new();
    for i in 0..250 {
        code.push_str(&format!("local v{i} = {i}\n"));
    }
    // 10 expression segments → 10 parts (after constant folding removes
    // empty literals between adjacent expressions, we still get enough
    // parts to force multiple batches with only ~5 registers free).
    code.push_str("return `{v0}-{v1}-{v2}-{v3}-{v4}-{v5}-{v6}-{v7}-{v8}-{v9}`\n");
    let result = run_one(&code).await;
    k9::assert_equal!(result, Value::String(Bytes::from("0-1-2-3-4-5-6-7-8-9")));
}

#[tokio::test]
async fn interp_register_batching_tight() {
    // With 252 locals + 1 caller temp, only 2 registers remain per batch.
    // Each batch after the first carries one accumulated result, leaving
    // room for just 1 new part per batch.
    let mut code = String::new();
    for i in 0..252 {
        code.push_str(&format!("local v{i} = {i}\n"));
    }
    code.push_str("return `a{v0}b{v1}c{v2}d{v3}e`\n");
    let result = run_one(&code).await;
    k9::assert_equal!(result, Value::String(Bytes::from("a0b1c2d3e")));
}

#[tokio::test]
async fn interp_register_batching_overflow() {
    use shingetsu::diagnostic::{render_compile_error, RenderStyle};
    // With 253 locals the register window is too small for multi-part
    // interpolation; the compiler should report an error, not hang.
    let mut code = String::new();
    for i in 0..253 {
        code.push_str(&format!("local v{i} = {i}\n"));
    }
    code.push_str("return `a{v0}b{v1}c`\n");
    let compiler = shingetsu_compiler::Compiler::new(Default::default(), Default::default());
    let err = compiler.compile(&code).await.unwrap_err();
    let rendered = render_compile_error(&err, &code, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "error: string interpolation requires at least 2 free registers, but too many locals are in scope; consider refactoring into smaller functions
   --> <string>:254:8
    |
254 | return `a{v0}b{v1}c`
    |        ^^^^^^^^^^^^^ string interpolation requires at least 2 free registers, but too many locals are in scope; consider refactoring into smaller functions"
    );
}

#[tokio::test]
async fn interp_table_without_tostring() {
    // Table without __tostring falls back to "table: 0x..." representation.
    let result = run_one("local t = {} return `{t}`").await;
    match &result {
        Value::String(s) => {
            let s = std::str::from_utf8(s).expect("valid utf8");
            assert!(
                s.starts_with("table: 0x"),
                "expected 'table: 0x...' but got: {s}"
            );
        }
        other => panic!("expected string, got: {other:?}"),
    }
}

#[tokio::test]
async fn interp_function_value() {
    // A bare function value should stringify to "function".
    let result = run_one("local f = function() end\nreturn `fn: {f}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("fn: function")));
}

#[tokio::test]
async fn interp_userdata_tostring() {
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Widget(i64);

    #[userdata]
    impl Widget {
        #[lua_metamethod(ToString)]
        fn to_str(&self) -> String {
            format!("Widget({})", self.0)
        }
    }

    let env = common::new_env();
    env.set_global("w", Value::Userdata(Arc::new(Widget(42))));
    let res = common::run_with_env(env, "return `got: {w}`").await;
    k9::assert_equal!(res[0], Value::string("got: Widget(42)"));
}

#[tokio::test]
async fn interp_single_expr_no_literals() {
    // Degenerate case: single expression, no surrounding text.
    let result = run_one("local x = 99\nreturn `{x}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("99")));
}

#[tokio::test]
async fn interp_escape_newline_and_tab() {
    let result = run_one(r"return `line1\nline2\tend`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("line1\nline2\tend")));
}

#[tokio::test]
async fn interp_false_boolean() {
    let result = run_one("return `{false}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("false")));
}

#[tokio::test]
async fn interp_special_numbers() {
    let result = run_one("return `{-42}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("-42")));

    let result = run_one("return `{0}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("0")));

    let result = run_one("return `{1/0}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("inf")));

    let result = run_one("return `{-1/0}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("-inf")));

    let result = run_one("return `{0/0}`").await;
    k9::assert_equal!(result, Value::String(Bytes::from("nan")));
}

#[tokio::test]
async fn interp_as_argument() {
    // Interpolation used as a function argument, not in return position.
    let result = run_one(
        r#"local function id(s) return s end
local x = 10
return id(`val={x}`)"#,
    )
    .await;
    k9::assert_equal!(result, Value::String(Bytes::from("val=10")));
}

#[tokio::test]
async fn interp_assigned_to_variable() {
    let result = run_one(
        r#"local x = 5
local s = `x is {x}`
return s"#,
    )
    .await;
    k9::assert_equal!(result, Value::String(Bytes::from("x is 5")));
}

#[tokio::test]
async fn interp_type_check_infers_string() {
    // Type checker should accept interpolated string where string is expected.
    use shingetsu::diagnostic::{render_warnings, RenderStyle};
    let opts = CompileOptions {
        type_check: true,
        ..Default::default()
    };
    let compiler = Compiler::new(opts, Default::default());
    let bc = compiler
        .compile("function f(): string return `hello {42}` end")
        .await
        .expect("compile");
    let warnings = render_warnings(&bc.diagnostics, "", RenderStyle::Plain);
    k9::assert_equal!(warnings, "");
}

#[tokio::test]
async fn register_overflow_too_many_locals() {
    use shingetsu::diagnostic::{render_compile_error, RenderStyle};
    // 255 locals use slots 0-254; the 256th declaration should fail.
    let mut code = String::new();
    for i in 0..256 {
        code.push_str(&format!("local v{i} = {i}\n"));
    }
    code.push_str("return v0\n");
    let compiler = Compiler::new(Default::default(), Default::default());
    let err = compiler.compile(&code).await.unwrap_err();
    let rendered = render_compile_error(&err, &code, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "error: too many local variables (limit 255); consider refactoring into smaller functions
   --> <string>:256:7
    |
256 | local v255 = 255
    |       ^^^^ too many local variables (limit 255); consider refactoring into smaller functions"
    );
}

#[tokio::test]
async fn register_overflow_temp_exhaustion() {
    use shingetsu::diagnostic::{render_compile_error, RenderStyle};
    // 255 locals fill all registers; any expression needing a temp
    // should produce a clear error from alloc_temp.
    let mut code = String::new();
    for i in 0..255 {
        code.push_str(&format!("local v{i} = {i}\n"));
    }
    // A binary expression needs a temp for the LHS while evaluating RHS.
    code.push_str("return v0 + v1\n");
    let compiler = Compiler::new(Default::default(), Default::default());
    let err = compiler.compile(&code).await.unwrap_err();
    let rendered = render_compile_error(&err, &code, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "error: too many local variables or temporaries (register limit is 255); consider refactoring into smaller functions
   --> <string>:256:1
    |
256 | return v0 + v1
    | ^^^^^^^^^^^^^^ too many local variables or temporaries (register limit is 255); consider refactoring into smaller functions"
    );
}

#[tokio::test]
async fn register_limit_255_locals_ok() {
    // 255 locals (slots 0-254) is the maximum; should compile and run.
    let mut code = String::new();
    for i in 0..255 {
        code.push_str(&format!("local v{i} = {i}\n"));
    }
    code.push_str("return v254\n");
    let result = run_one(&code).await;
    k9::assert_equal!(result, Value::Integer(254));
}

// Type assertion (expr :: Type)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn type_assertion_passes_through_value() {
    k9::assert_equal!(run_one("return (42 :: number)").await, Value::Integer(42));
}

#[tokio::test]
async fn type_assertion_on_string() {
    k9::assert_equal!(
        run_one(r#"return ("hello" :: string)"#).await,
        Value::String(Bytes::from("hello"))
    );
}

#[tokio::test]
async fn type_assertion_on_expression() {
    k9::assert_equal!(
        run_one("local x = 10; return (x + 5 :: number)").await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn type_assertion_nested() {
    k9::assert_equal!(
        run_one("return ((1 + 2 :: number) :: number)").await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn type_assertion_in_assignment() {
    k9::assert_equal!(
        run_one("local x = (100 :: number); return x").await,
        Value::Integer(100)
    );
}

// Type instantiation (func<<T>>(args))
// ---------------------------------------------------------------------------
// Suffix::TypeInstantiation cannot be produced by the parser when both lua54
// and luau features are enabled, because the lexer tokenizes `<<` as the
// Lua 5.3 bitwise-shift operator (DoubleLessThan) before the parser can
// interpret it as a double-angle-bracket type instantiation.  The codegen
// support exists in lower.rs (apply_index_suffix) but is untestable in this
// configuration.
//
// Once full_moon resolves this ambiguity, this test should be updated to
// assert `Value::Integer(42)` instead of the runtime error.

#[tokio::test]
async fn type_instantiation_parsed_as_shift() {
    use common::run_err;
    let err = run_err(
        "\
        local function identity(x) return x end\n\
        return identity<<number>>(42)\
    ",
    )
    .await;
    k9::assert_equal!(
        err,
        "attempt to perform arithmetic on local 'identity' (a function value)"
    );
}
