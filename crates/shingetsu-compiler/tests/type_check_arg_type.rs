mod common;
use common::type_check;

use shingetsu::diagnostic::assert_diagnostics;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::types::TypedParam;
use std::sync::Arc;

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
        type_check: true,
    }
}

#[tokio::test]
async fn mismatch_string_for_number() {
    type_check(
        r#"
local function f(_x: number) end
f("hello")
"#,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"hello\")
  |   ^^^^^^^ expected 'number' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn compatible_integer_for_number() {
    type_check(
        "\
local function f(_x: number) end
f(42)",
        "",
    );
}

#[tokio::test]
async fn compatible_integer_for_float() {
    type_check(
        "\
local function f(_x: number, _y: number) end
f(1, 2.5)",
        "",
    );
}

#[tokio::test]
async fn nil_for_optional() {
    type_check(
        "\
local function f(_x: number?) end
f(nil)",
        "",
    );
}

#[tokio::test]
async fn wrong_for_optional() {
    type_check(
        r#"
local function f(_x: number?) end
f("oops")
"#,
        "\
error[arg_type]: expected 'number?' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"oops\")
  |   ^^^^^^ expected 'number?' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn any_param_accepts_anything() {
    type_check(
        r#"
local function f(_x: any) end
f("hello")
"#,
        "",
    );
}

#[tokio::test]
async fn boolean_for_string() {
    type_check(
        r#"
local function f(_x: string) end
f(true)
"#,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'boolean'
 --> test.lua:3:3
  |
3 | f(true)
  |   ^^^^ expected 'string' for parameter '_x' but got 'boolean'",
    );
}

#[tokio::test]
async fn skips_unannotated_params() {
    // Functions inferred without annotations have all-Any params,
    // so no arg_type diagnostics are produced.
    type_check(
        "\
local t = {}
function t.greet(_name) end
t.greet(42)",
        "",
    );
}

#[tokio::test]
async fn union_param() {
    type_check(
        r#"
local function f(_x: number | string) end
f("hello")
f(42)
f(true)
"#,
        "\
error[arg_type]: expected 'number | string' for parameter '_x' but got 'boolean'
 --> test.lua:5:3
  |
5 | f(true)
  |   ^^^^ expected 'number | string' for parameter '_x' but got 'boolean'",
    );
}

#[tokio::test]
async fn multiple_params() {
    type_check(
        r#"
local function f(_a: number, _b: string) end
f("wrong", 42)
"#,
        "\
error[arg_type]: expected 'number' for parameter '_a' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"wrong\", 42)
  |   ^^^^^^^ expected 'number' for parameter '_a' but got 'string'
error[arg_type]: expected 'string' for parameter '_b' but got 'integer'
 --> test.lua:3:12
  |
3 | f(\"wrong\", 42)
  |            ^^ expected 'string' for parameter '_b' but got 'integer'",
    );
}

#[tokio::test]
async fn table_for_table() {
    // Table arguments should be compatible with table parameters.
    type_check(
        "\
type Config = { name: string }
local function f(_cfg: Config) end
f({})",
        "",
    );
}

#[tokio::test]
async fn from_variable() {
    // Type checking should work when the argument is a typed variable.
    type_check(
        r#"
local function f(_x: number) end
local s: string = "hello"
f(s)
"#,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:4:3
  |
4 | f(s)
  |   ^ expected 'number' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn native_module() {
    // Native module functions should trigger arg type checks.
    use shingetsu::module;
    use shingetsu_vm::GlobalEnv;

    #[module(name = "typed_mod")]
    mod typed_mod_impl {
        #[function]
        fn greet(name: String) -> String {
            format!("Hello, {name}!")
        }
    }

    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register_libs");
    typed_mod_impl::register_preload(&env);

    let compiler = Compiler::new(type_check_opts(), env.global_type_map())
        .with_module_types(env.preload_module_types());

    let src = "\
local m = require('typed_mod')
m.greet(42)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:2:9
  |
2 | m.greet(42)
  |         ^^ expected 'string' for parameter 'name' but got 'integer'",
    );
}

#[tokio::test]
async fn unary_not_infers_boolean() {
    type_check(
        "\
local function f(_x: string) end
f(not true)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'boolean'
 --> test.lua:2:3
  |
2 | f(not true)
  |   ^^^^^^^^ expected 'string' for parameter '_x' but got 'boolean'",
    );
}

#[tokio::test]
async fn unary_len_infers_integer() {
    type_check(
        r#"
local function f(_x: string) end
local t = {}
f(#t)
"#,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'integer'
 --> test.lua:4:3
  |
4 | f(#t)
  |   ^^ expected 'string' for parameter '_x' but got 'integer'",
    );
}

#[tokio::test]
async fn unary_neg_infers_number() {
    type_check(
        "\
local function f(_x: string) end
f(-42)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'number'
 --> test.lua:2:3
  |
2 | f(-42)
  |   ^^^ expected 'string' for parameter '_x' but got 'number'",
    );
}

#[tokio::test]
async fn unary_bnot_infers_integer() {
    type_check(
        "\
local function f(_x: string) end
f(~0)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'number'
 --> test.lua:2:3
  |
2 | f(~0)
  |   ^^ expected 'string' for parameter '_x' but got 'number'",
    );
}

#[tokio::test]
async fn binary_concat_infers_string() {
    type_check(
        r#"
local function f(_x: number) end
f("a" .. "b")
"#,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"a\" .. \"b\")
  |   ^^^^^^^^^^ expected 'number' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn binary_arithmetic_infers_number() {
    type_check(
        "\
local function f(_x: string) end
f(1 + 2)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'number'
 --> test.lua:2:3
  |
2 | f(1 + 2)
  |   ^^^^^ expected 'string' for parameter '_x' but got 'number'",
    );
}

#[tokio::test]
async fn binary_bitwise_infers_integer() {
    type_check(
        "\
local function f(_x: string) end
f(1 & 2)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'integer'
 --> test.lua:2:3
  |
2 | f(1 & 2)
  |   ^^^^^ expected 'string' for parameter '_x' but got 'integer'",
    );
}

#[tokio::test]
async fn binary_comparison_infers_boolean() {
    type_check(
        "\
local function f(_x: number) end
f(1 == 2)",
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'boolean'
 --> test.lua:2:3
  |
2 | f(1 == 2)
  |   ^^^^^^ expected 'number' for parameter '_x' but got 'boolean'",
    );
}

#[tokio::test]
async fn binary_and_or_infers_from_lhs() {
    type_check(
        r#"
local function f(_x: number) end
f("a" or "b")
"#,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"a\" or \"b\")
  |   ^^^^^^^^^^ expected 'number' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn parenthesized_expr() {
    type_check(
        r#"
local function f(_x: number) end
f(("hello"))
"#,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f((\"hello\"))
  |   ^^^^^^^^^ expected 'number' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn function_literal() {
    type_check(
        "\
local function f(_x: number) end
f(function() end)",
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'function'
 --> test.lua:2:3
  |
2 | f(function() end)
  |   ^^^^^^^^^^^^^^ expected 'number' for parameter '_x' but got 'function'",
    );
}

#[tokio::test]
async fn function_call_return_type() {
    // When a function call is the last argument, its count is
    // indeterminate (multi-return), so the whole call is skipped.
    // Use the return value in a non-last position to test inference.
    type_check(
        r#"
local function g(): string return "hi" end
local function f(_a: number, _b: string) end
f(g(), "ok")
"#,
        "\
error[arg_type]: expected 'number' for parameter '_a' but got 'string'
 --> test.lua:4:3
  |
4 | f(g(), \"ok\")
  |   ^^^ expected 'number' for parameter '_a' but got 'string'",
    );
}

#[tokio::test]
async fn named_match() {
    type_check(
        "\
type Foo = { x: number }
local function f(_x: Foo) end
local a: Foo = {}
f(a)",
        "",
    );
}

#[tokio::test]
async fn named_mismatch() {
    // Non-generic type aliases are expanded, so both Foo and Bar
    // become structural Table types. Table-vs-Table is currently
    // compatible (structural), so no diagnostic is emitted.
    // Instead, test that a concrete type (string) fails against
    // a resolved alias that expands to a table.
    type_check(
        r#"
type Foo = { x: number }
local function f(_x: Foo) end
f("wrong")
"#,
        "\
error[arg_type]: expected '{ x: number }' for parameter '_x' but got 'string'
 --> test.lua:4:3
  |
4 | f(\"wrong\")
  |   ^^^^^^^ expected '{ x: number }' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn string_literal_for_string() {
    type_check(
        "\
type Mode = \"read\" | \"write\"
local function f(_m: Mode) end
local s: string = 'hello'
f(s)",
        "",
    );
}

#[tokio::test]
async fn bool_literal_for_boolean() {
    type_check(
        "\
local function f(_x: boolean) end
f(true)",
        "",
    );
}

#[tokio::test]
async fn actual_union_all_compatible() {
    // When the actual type is a union, all variants must be compatible.
    type_check(
        "\
local function f(_x: number) end
local v: number | string = 1
f(v)",
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'number | string'
 --> test.lua:3:3
  |
3 | f(v)
  |   ^ expected 'number' for parameter '_x' but got 'number | string'",
    );
}

#[tokio::test]
async fn actual_optional_for_required() {
    // Passing an optional value to a required param should fail
    // because the value could be nil.
    type_check(
        "\
local function f(_x: number) end
local v: number? = 1
f(v)",
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'number?'
 --> test.lua:3:3
  |
3 | f(v)
  |   ^ expected 'number' for parameter '_x' but got 'number?'",
    );
}

#[tokio::test]
async fn colon_call_skips_self() {
    // Colon-call syntax should skip the self parameter for type checking.
    type_check(
        r#"
type Obj = { greet: (self: Obj, name: string) -> () }
local o: Obj = {}
o:greet(42)
"#,
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:4:9
  |
4 | o:greet(42)
  |         ^^ expected 'string' for parameter 'name' but got 'integer'",
    );
}

#[tokio::test]
async fn unknown_expr_skipped() {
    // Complex expressions the checker can't infer should not produce
    // false positives.
    type_check(
        "\
local function f(_x: number) end
local t = {}
f(t[1])",
        "",
    );
}

#[tokio::test]
async fn diagnostic_location() {
    // The diagnostic should point at the argument expression.
    type_check(
        r#"local function f(_x: number) end
f("hello")"#,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:2:3
  |
2 | f(\"hello\")
  |   ^^^^^^^ expected 'number' for parameter '_x' but got 'string'",
    );
}

#[tokio::test]
async fn optional_actual_for_optional_param() {
    // number? should be accepted by number? param.
    type_check(
        "\
local function f(_x: number?) end
local v: number? = 1
f(v)",
        "",
    );
}

#[tokio::test]
async fn union_actual_all_match() {
    // number | integer should pass for number param since both are
    // compatible with number.
    type_check(
        "\
local function f(_x: number) end
local v: integer | float = 1
f(v)",
        "",
    );
}

#[tokio::test]
async fn float_literal() {
    // Float literals should infer as float.
    type_check(
        "\
local function f(_x: string) end
f(1.5)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'float'
 --> test.lua:2:3
  |
2 | f(1.5)
  |   ^^^ expected 'string' for parameter '_x' but got 'float'",
    );
}

#[tokio::test]
async fn hex_literal_is_integer() {
    type_check(
        "\
local function f(_x: string) end
f(0xFF)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'integer'
 --> test.lua:2:3
  |
2 | f(0xFF)
  |   ^^^^ expected 'string' for parameter '_x' but got 'integer'",
    );
}

#[tokio::test]
async fn exponent_literal_is_float() {
    type_check(
        "\
local function f(_x: string) end
f(1e3)",
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'float'
 --> test.lua:2:3
  |
2 | f(1e3)
  |   ^^^ expected 'string' for parameter '_x' but got 'float'",
    );
}

#[tokio::test]
async fn colon_defined_colon_called_method() {
    // Method defined with function t:m(x: number) and called via
    // t:m("wrong") should check the non-self params.
    type_check(
        r#"
local t = {}
function t:greet(_name: string) end
t:greet(42)
"#,
        "\
error[arg_type]: expected 'string' for parameter '_name' but got 'integer'
 --> test.lua:4:9
  |
4 | t:greet(42)
  |         ^^ expected 'string' for parameter '_name' but got 'integer'",
    );
}

#[tokio::test]
async fn global_function_arg_type() {
    // Calling a global function with wrong arg type.
    use shingetsu_vm::types::{FunctionLuaType, LuaType};
    use shingetsu_vm::GlobalTypeMap;

    let mut gtm = GlobalTypeMap::default();
    gtm.types.insert(
        "greet".into(),
        LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![TypedParam::new(Some("name"), LuaType::String)],
            variadic: None,
            returns: vec![],
            is_method: false,
            inferred_unannotated: false,
            deprecated: None,
            must_use: None,
        })),
    );
    let compiler = Compiler::new(type_check_opts(), gtm);
    let src = "greet(42)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:1:7
  |
1 | greet(42)
  |       ^^ expected 'string' for parameter 'name' but got 'integer'",
    );
}

#[tokio::test]
async fn any_param_with_known_actual_no_diagnostic() {
    // When param is any, no diagnostic even if actual is known.
    type_check(
        "\
local function f(_x: any, _y: any) end
f(42, true)",
        "",
    );
}

#[tokio::test]
async fn arg_type_and_arg_count_on_same_call() {
    // Both arg_count and arg_type can fire on the same call.
    type_check(
        r#"
local function f(_a: number, _b: string) end
f("wrong")
"#,
        "\
error[arg_count]: expected 2 arguments but got 1
 --> test.lua:3:2
  |
3 | f(\"wrong\")
  |  ^^^^^^^^^ expected 2 arguments but got 1
error[arg_type]: expected 'number' for parameter '_a' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"wrong\")
  |   ^^^^^^^ expected 'number' for parameter '_a' but got 'string'",
    );
}

// ===========================================================================
// table.insert — stdlib type checking
// ===========================================================================

fn stdlib_compiler() -> Compiler {
    use shingetsu_vm::GlobalEnv;
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::SANDBOXED).expect("register_libs");
    Compiler::new(type_check_opts(), env.global_type_map())
        .with_module_types(env.preload_module_types())
}

#[tokio::test]
async fn table_insert_two_args_ok() {
    let compiler = stdlib_compiler();
    let src = r#"table.insert({}, "hello")"#;
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(&bc.diagnostics, src, "");
}

#[tokio::test]
async fn table_insert_three_args_ok() {
    let compiler = stdlib_compiler();
    let src = r#"table.insert({}, 1, "hello")"#;
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(&bc.diagnostics, src, "");
}

#[tokio::test]
async fn table_insert_first_arg_not_table() {
    let compiler = stdlib_compiler();
    let src = r#"table.insert(42, 1, 2)"#;
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_type]: expected 'table' for parameter 'list' but got 'integer'
 --> test.lua:1:14
  |
1 | table.insert(42, 1, 2)
  |              ^^ expected 'table' for parameter 'list' but got 'integer'",
    );
}

#[tokio::test]
async fn table_insert_first_arg_string() {
    let compiler = stdlib_compiler();
    let src = r#"table.insert("not_a_table", "value")"#;
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_type]: expected 'table' for parameter 'list' but got 'string'
 --> test.lua:1:14
  |
1 | table.insert(\"not_a_table\", \"value\")
  |              ^^^^^^^^^^^^^ expected 'table' for parameter 'list' but got 'string'",
    );
}

#[tokio::test]
async fn table_insert_too_many_args() {
    let compiler = stdlib_compiler();
    let src = "table.insert({}, 2, 3, 4)";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_count]: expected at most 3 arguments but got 4
 --> test.lua:1:13
  |
1 | table.insert({}, 2, 3, 4)
  |             ^^^^^^^^^^^^^ expected at most 3 arguments but got 4",
    );
}

#[tokio::test]
async fn table_insert_too_few_args() {
    let compiler = stdlib_compiler();
    let src = "table.insert()";
    let bc = compiler.compile(src).await.expect("compile");
    assert_diagnostics(
        &bc.diagnostics,
        src,
        "\
error[arg_count]: expected at least 2 arguments but got 0
 --> test.lua:1:13
  |
1 | table.insert()
  |             ^^ expected at least 2 arguments but got 0",
    );
}

// ===========================================================================
// Optional table fields (`field?`) may be omitted from a table argument.
// Regression: the structural table check treated every declared field as
// required, rejecting calls that left an optional field unset.
// ===========================================================================

#[tokio::test]
async fn omit_optional_table_field_paren_ok() {
    type_check(
        "\
local function f(_t: { name: string, billing: string? }) end
f({ name = 'cc' })",
        "",
    );
}

#[tokio::test]
async fn omit_optional_table_field_sugar_ok() {
    type_check(
        "\
local function f(_t: { name: string, billing: string? }) end
f { name = 'cc' }",
        "",
    );
}

#[tokio::test]
async fn present_optional_table_field_ok() {
    type_check(
        "\
local function f(_t: { name: string, billing: string? }) end
f { name = 'cc', billing = 'card' }",
        "",
    );
}

#[tokio::test]
async fn omit_required_table_field_errors() {
    type_check(
        "\
local function f(_t: { name: string, billing: string? }) end
f { billing = 'x' }",
        "\
error[arg_type]: expected '{ name: string, billing: string? }' for parameter '_t' but got '{ billing: string }'
 --> test.lua:2:3
  |
2 | f { billing = 'x' }
  |   ^^^^^^^^^^^^^^^^^ expected '{ name: string, billing: string? }' for parameter '_t' but got '{ billing: string }'
  |
help: missing field 'name' of type 'string'",
    );
}

#[tokio::test]
async fn wrong_type_optional_table_field_errors() {
    type_check(
        "\
local function f(_t: { name: string, billing: string? }) end
f { name = 'cc', billing = 5 }",
        "\
error[arg_type]: expected '{ name: string, billing: string? }' for parameter '_t' but got '{ name: string, billing: integer }'
 --> test.lua:2:3
  |
2 | f { name = 'cc', billing = 5 }
  |   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected '{ name: string, billing: string? }' for parameter '_t' but got '{ name: string, billing: integer }'
  |
help: field 'billing' expects 'string?' but got 'integer'",
    );
}

#[tokio::test]
async fn omit_union_nil_table_field_ok() {
    // A field typed as a union that includes nil is omittable, just like `field?`.
    type_check(
        "\
local function f(_t: { name: string, tag: string | nil }) end
f { name = 'x' }",
        "",
    );
}

#[tokio::test]
async fn omit_any_typed_table_field_ok() {
    // An `any`-typed field accepts nil, so it too may be omitted.
    type_check(
        "\
local function f(_t: { name: string, extra: any }) end
f { name = 'x' }",
        "",
    );
}

#[tokio::test]
async fn omit_several_optional_table_fields_ok() {
    // Several optional fields may all be omitted while a required one is supplied.
    type_check(
        "\
local function f(_t: { a: string?, b: number?, c: string }) end
f { c = 'x' }",
        "",
    );
}

#[tokio::test]
async fn nested_table_field_type_still_enforced() {
    // Width subtyping applies per level: a present nested field's own
    // fields are still type-checked.
    type_check(
        "\
local function f(_t: { inner: { id: string } }) end
f { inner = { id = 5 } }",
        "\
error[arg_type]: expected '{ inner: { id: string } }' for parameter '_t' but got '{ inner: { id: integer } }'
 --> test.lua:2:3
  |
2 | f { inner = { id = 5 } }
  |   ^^^^^^^^^^^^^^^^^^^^^^ expected '{ inner: { id: string } }' for parameter '_t' but got '{ inner: { id: integer } }'
  |
help: field 'inner' expects '{ id: string }' but got '{ id: integer }'",
    );
}

#[tokio::test]
async fn method_sugar_omit_optional_table_field_ok() {
    // Colon-call sugar (`obj:m { ... }`) checks the table against the
    // param after the implicit `self`; an optional field may be omitted.
    type_check(
        "\
local t = {}
function t:add(_tool: { name: string, available: (() -> ())? }) end
t:add { name = 'w' }",
        "",
    );
}

#[tokio::test]
async fn method_sugar_missing_required_table_field_errors() {
    // The same colon-call sugar still reports a missing required field
    // against the correct post-`self` parameter.
    type_check(
        "\
local t = {}
function t:add(_tool: { name: string, available: (() -> ())? }) end
t:add { available = function() end }",
        "\
error[arg_type]: expected '{ name: string, available: function? }' for parameter '_tool' but got '{ available: function }'
 --> test.lua:3:7
  |
3 | t:add { available = function() end }
  |       ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected '{ name: string, available: function? }' for parameter '_tool' but got '{ available: function }'
  |
help: missing field 'name' of type 'string'",
    );
}

#[tokio::test]
async fn mismatch_help_skips_omitted_optional_fields() {
    // When a required field is present but wrongly typed, the help must
    // name that field even if an optional field declared earlier was
    // omitted.  An omitted optional is not itself a defect and must not
    // be reported as a "missing field" of an optional (`function?`) type.
    type_check(
        "\
local function f(_spec: { name: string, available: (() -> ())?, run: () -> () }) end
f { name = 'weather', run = 5 }",
        "\
error[arg_type]: expected '{ name: string, available: function?, run: function }' for parameter '_spec' but got '{ name: string, run: integer }'
 --> test.lua:2:3
  |
2 | f { name = 'weather', run = 5 }
  |   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected '{ name: string, available: function?, run: function }' for parameter '_spec' but got '{ name: string, run: integer }'
  |
help: field 'run' expects 'function' but got 'integer'",
    );
}

#[tokio::test]
async fn table_keyword_accepts_any_table() {
    // The bare `table` keyword is the generic table type and accepts a
    // table of any shape, whether empty or populated.
    type_check(
        "\
local function f(_t: table) end
f({})
f({ x = 1, y = 'two' })",
        "",
    );
}

#[tokio::test]
async fn table_keyword_field_accepts_any_table() {
    // The same holds when `table` is the type of a nested field.
    type_check(
        "\
local function f(_spec: { name: string, parameters: table }) end
f { name = 'weather', parameters = { type = 'object' } }",
        "",
    );
}

#[tokio::test]
async fn table_keyword_rejects_non_table() {
    // A non-table value is still rejected against a `table` parameter.
    type_check(
        "\
local function f(_t: table) end
f(5)",
        "\
error[arg_type]: expected 'table' for parameter '_t' but got 'integer'
 --> test.lua:2:3
  |
2 | f(5)
  |   ^ expected 'table' for parameter '_t' but got 'integer'",
    );
}
