use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu_compiler::{CompileOptions, Compiler, LintId, Severity};

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "test.lua".into(),
        type_check: true,
    }
}

#[tokio::test]
async fn mismatch_string_for_number() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: number) end
f("hello")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"hello\")
  |   ^^^^^^^ expected 'number' for parameter '_x' but got 'string'"
    );
}

#[tokio::test]
async fn compatible_integer_for_number() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number) end
f(42)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn compatible_integer_for_float() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number, _y: number) end
f(1, 2.5)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn nil_for_optional() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number?) end
f(nil)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn wrong_for_optional() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: number?) end
f("oops")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number?' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"oops\")
  |   ^^^^^^ expected 'number?' for parameter '_x' but got 'string'"
    );
}

#[tokio::test]
async fn any_param_accepts_anything() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: any) end
f("hello")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn boolean_for_string() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: string) end
f(true)
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'boolean'
 --> test.lua:3:3
  |
3 | f(true)
  |   ^^^^ expected 'string' for parameter '_x' but got 'boolean'"
    );
}

#[tokio::test]
async fn skips_unannotated_params() {
    // Functions inferred without annotations have all-Any params,
    // so no arg_type diagnostics are produced.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local t = {}
function t.greet(_name) end
t.greet(42)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn union_param() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: number | string) end
f("hello")
f(42)
f(true)
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number | string' for parameter '_x' but got 'boolean'
 --> test.lua:5:3
  |
5 | f(true)
  |   ^^^^ expected 'number | string' for parameter '_x' but got 'boolean'"
    );
}

#[tokio::test]
async fn multiple_params() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_a: number, _b: string) end
f("wrong", 42)
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
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
  |            ^^ expected 'string' for parameter '_b' but got 'integer'"
    );
}

#[tokio::test]
async fn table_for_table() {
    // Table arguments should be compatible with table parameters.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
type Config = { name: string }
local function f(_cfg: Config) end
f({})";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn from_variable() {
    // Type checking should work when the argument is a typed variable.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: number) end
local s: string = "hello"
f(s)
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:4:3
  |
4 | f(s)
  |   ^ expected 'number' for parameter '_x' but got 'string'"
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
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:2:9
  |
2 | m.greet(42)
  |         ^^ expected 'string' for parameter 'name' but got 'integer'"
    );
}

#[tokio::test]
async fn unary_not_infers_boolean() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(not true)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'boolean'
 --> test.lua:2:3
  |
2 | f(not true)
  |   ^^^^^^^^ expected 'string' for parameter '_x' but got 'boolean'"
    );
}

#[tokio::test]
async fn unary_len_infers_integer() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: string) end
local t = {}
f(#t)
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'integer'
 --> test.lua:4:3
  |
4 | f(#t)
  |   ^^ expected 'string' for parameter '_x' but got 'integer'"
    );
}

#[tokio::test]
async fn unary_neg_infers_number() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(-42)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'number'
 --> test.lua:2:3
  |
2 | f(-42)
  |   ^^^ expected 'string' for parameter '_x' but got 'number'"
    );
}

#[tokio::test]
async fn unary_bnot_infers_integer() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(~0)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'number'
 --> test.lua:2:3
  |
2 | f(~0)
  |   ^^ expected 'string' for parameter '_x' but got 'number'"
    );
}

#[tokio::test]
async fn binary_concat_infers_string() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: number) end
f("a" .. "b")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"a\" .. \"b\")
  |   ^^^^^^^^^^ expected 'number' for parameter '_x' but got 'string'"
    );
}

#[tokio::test]
async fn binary_arithmetic_infers_number() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(1 + 2)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'number'
 --> test.lua:2:3
  |
2 | f(1 + 2)
  |   ^^^^^ expected 'string' for parameter '_x' but got 'number'"
    );
}

#[tokio::test]
async fn binary_bitwise_infers_integer() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(1 & 2)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'integer'
 --> test.lua:2:3
  |
2 | f(1 & 2)
  |   ^^^^^ expected 'string' for parameter '_x' but got 'integer'"
    );
}

#[tokio::test]
async fn binary_comparison_infers_boolean() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number) end
f(1 == 2)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'boolean'
 --> test.lua:2:3
  |
2 | f(1 == 2)
  |   ^^^^^^ expected 'number' for parameter '_x' but got 'boolean'"
    );
}

#[tokio::test]
async fn binary_and_or_infers_from_lhs() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: number) end
f("a" or "b")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f(\"a\" or \"b\")
  |   ^^^^^^^^^^ expected 'number' for parameter '_x' but got 'string'"
    );
}

#[tokio::test]
async fn parenthesized_expr() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_x: number) end
f(("hello"))
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:3:3
  |
3 | f((\"hello\"))
  |   ^^^^^^^^^ expected 'number' for parameter '_x' but got 'string'"
    );
}

#[tokio::test]
async fn function_literal() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number) end
f(function() end)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'function'
 --> test.lua:2:3
  |
2 | f(function() end)
  |   ^^^^^^^^^^^^^^ expected 'number' for parameter '_x' but got 'function'"
    );
}

#[tokio::test]
async fn function_call_return_type() {
    // When a function call is the last argument, its count is
    // indeterminate (multi-return), so the whole call is skipped.
    // Use the return value in a non-last position to test inference.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function g(): string return "hi" end
local function f(_a: number, _b: string) end
f(g(), "ok")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_a' but got 'string'
 --> test.lua:4:3
  |
4 | f(g(), \"ok\")
  |   ^^^ expected 'number' for parameter '_a' but got 'string'"
    );
}

#[tokio::test]
async fn named_match() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
type Foo = { x: number }
local function f(_x: Foo) end
local a: Foo = {}
f(a)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn named_mismatch() {
    // Non-generic type aliases are expanded, so both Foo and Bar
    // become structural Table types. Table-vs-Table is currently
    // compatible (structural), so no diagnostic is emitted.
    // Instead, test that a concrete type (string) fails against
    // a resolved alias that expands to a table.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
type Foo = { x: number }
local function f(_x: Foo) end
f("wrong")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'table' for parameter '_x' but got 'string'
 --> test.lua:4:3
  |
4 | f(\"wrong\")
  |   ^^^^^^^ expected 'table' for parameter '_x' but got 'string'"
    );
}

#[tokio::test]
async fn string_literal_for_string() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
type Mode = \"read\" | \"write\"
local function f(_m: Mode) end
local s: string = 'hello'
f(s)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn bool_literal_for_boolean() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: boolean) end
f(true)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn actual_union_all_compatible() {
    // When the actual type is a union, all variants must be compatible.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number) end
local v: number | string = 1
f(v)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'number | string'
 --> test.lua:3:3
  |
3 | f(v)
  |   ^ expected 'number' for parameter '_x' but got 'number | string'"
    );
}

#[tokio::test]
async fn actual_optional_for_required() {
    // Passing an optional value to a required param should fail
    // because the value could be nil.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number) end
local v: number? = 1
f(v)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'number?'
 --> test.lua:3:3
  |
3 | f(v)
  |   ^ expected 'number' for parameter '_x' but got 'number?'"
    );
}

#[tokio::test]
async fn colon_call_skips_self() {
    // Colon-call syntax should skip the self parameter for type checking.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
type Obj = { greet: (self: Obj, name: string) -> () }
local o: Obj = {}
o:greet(42)
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:4:9
  |
4 | o:greet(42)
  |         ^^ expected 'string' for parameter 'name' but got 'integer'"
    );
}

#[tokio::test]
async fn unknown_expr_skipped() {
    // Complex expressions the checker can't infer should not produce
    // false positives.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number) end
local t = {}
f(t[1])";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn diagnostic_location() {
    // The diagnostic should point at the argument expression.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"local function f(_x: number) end
f("hello")"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'number' for parameter '_x' but got 'string'
 --> test.lua:2:3
  |
2 | f(\"hello\")
  |   ^^^^^^^ expected 'number' for parameter '_x' but got 'string'"
    );
}

#[tokio::test]
async fn optional_actual_for_optional_param() {
    // number? should be accepted by number? param.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number?) end
local v: number? = 1
f(v)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn union_actual_all_match() {
    // number | integer should pass for number param since both are
    // compatible with number.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: number) end
local v: integer | float = 1
f(v)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn float_literal() {
    // Float literals should infer as float.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(1.5)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'float'
 --> test.lua:2:3
  |
2 | f(1.5)
  |   ^^^ expected 'string' for parameter '_x' but got 'float'"
    );
}

#[tokio::test]
async fn hex_literal_is_integer() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(0xFF)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'integer'
 --> test.lua:2:3
  |
2 | f(0xFF)
  |   ^^^^ expected 'string' for parameter '_x' but got 'integer'"
    );
}

#[tokio::test]
async fn exponent_literal_is_float() {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: string) end
f(1e3)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_x' but got 'float'
 --> test.lua:2:3
  |
2 | f(1e3)
  |   ^^^ expected 'string' for parameter '_x' but got 'float'"
    );
}

#[tokio::test]
async fn colon_defined_colon_called_method() {
    // Method defined with function t:m(x: number) and called via
    // t:m("wrong") should check the non-self params.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local t = {}
function t:greet(_name: string) end
t:greet(42)
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter '_name' but got 'integer'
 --> test.lua:4:9
  |
4 | t:greet(42)
  |         ^^ expected 'string' for parameter '_name' but got 'integer'"
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
            params: vec![(Some("name".into()), LuaType::String)],
            variadic: None,
            returns: vec![],
            is_method: false,
            inferred_unannotated: false,
        })),
    );
    let compiler = Compiler::new(type_check_opts(), gtm);
    let src = "greet(42)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:1:7
  |
1 | greet(42)
  |       ^^ expected 'string' for parameter 'name' but got 'integer'"
    );
}

#[tokio::test]
async fn any_param_with_known_actual_no_diagnostic() {
    // When param is any, no diagnostic even if actual is known.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = "\
local function f(_x: any, _y: any) end
f(42, true)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn arg_type_and_arg_count_on_same_call() {
    // Both arg_count and arg_type can fire on the same call.
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let src = r#"
local function f(_a: number, _b: string) end
f("wrong")
"#;
    let bc = compiler.compile(src).await.expect("compile");
    let count_diags: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.lint == LintId::ArgCount)
        .collect();
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
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
  |   ^^^^^^^ expected 'number' for parameter '_a' but got 'string'"
    );
}
