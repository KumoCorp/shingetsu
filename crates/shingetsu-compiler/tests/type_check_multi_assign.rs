use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu_compiler::{CompileOptions, Compiler};

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "test.lua".into(),
        type_check: true,
    }
}

async fn check(src: &str) -> String {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

async fn check_with_builtins(src: &str) -> String {
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    let compiler = Compiler::new(type_check_opts(), env.global_type_map());
    let bc = compiler.compile(src).await.expect("compile");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

// ---------------------------------------------------------------------------
// Multi-assignment from table fields — types inferred, feeds arg check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_from_table_fields() {
    k9::assert_equal!(
        check(
            "\
type T = { greet: (name: string) -> (), count: (n: number) -> () }
local t: T = {}
local greet, count = t.greet, t.count
greet(42)
count(\"wrong\")"
        )
        .await,
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:4:7
  |
4 | greet(42)
  |       ^^ expected 'string' for parameter 'name' but got 'integer'
error[arg_type]: expected 'number' for parameter 'n' but got 'string'
 --> test.lua:5:7
  |
5 | count(\"wrong\")
  |       ^^^^^^^ expected 'number' for parameter 'n' but got 'string'"
    );
}

// ---------------------------------------------------------------------------
// Multi-assign: one annotated, one inferred from field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_mixed_annotation_inference() {
    k9::assert_equal!(
        check(
            "\
type T = { f: (x: number) -> () }
local t: T = {}
local a: boolean, b = true, t.f
b(\"wrong\")"
        )
        .await,
        "\
error[arg_type]: expected 'number' for parameter 'x' but got 'string'
 --> test.lua:4:3
  |
4 | b(\"wrong\")
  |   ^^^^^^^ expected 'number' for parameter 'x' but got 'string'
warning[unused_variable]: unused variable 'a'
 --> test.lua:3:7
  |
3 | local a: boolean, b = true, t.f
  |       ^ unused variable 'a'
  |
help: prefix the name with '_' to suppress this warning: '_a'"
    );
}

// ---------------------------------------------------------------------------
// Multi-assign from native module fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_from_native_module() {
    k9::assert_equal!(
        check_with_builtins(
            "\
local abs, floor = math.abs, math.floor
abs(1)
floor(2)"
        )
        .await,
        ""
    );
}

// ---------------------------------------------------------------------------
// Local-to-local inference: type propagates through assignment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_to_local_inference() {
    k9::assert_equal!(
        check(
            "\
type T = { f: (x: number) -> () }
local a: T = {}
local b = a
b.f(1, 2)"
        )
        .await,
        "\
error[arg_count]: expected 1 argument but got 2
 --> test.lua:4:4
  |
4 | b.f(1, 2)
  |    ^^^^^^ expected 1 argument but got 2"
    );
}

// ---------------------------------------------------------------------------
// Inference from function call return type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_function_call() {
    k9::assert_equal!(
        check(
            "\
type Point = { x: number, y: number }
local function make_point(): Point
    return {}
end
local p = make_point()
local _ = p.z"
        )
        .await,
        "\
error[field_access]: unknown field 'z' on type 'table'
 --> test.lua:6:11
  |
6 | local _ = p.z
  |           ^^^ unknown field 'z' on type 'table'"
    );
}

// ---------------------------------------------------------------------------
// Inference from literal expressions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_literals() {
    k9::assert_equal!(
        check(
            "\
local a, b, c = 42, \"hello\", true
local x: string = a
local y: number = b
local z: number = c"
        )
        .await,
        "\
error[assign_type]: expected 'string' but got 'integer'
 --> test.lua:2:19
  |
2 | local x: string = a
  |                   ^ expected 'string' but got 'integer'
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:3:19
  |
3 | local y: number = b
  |                   ^ expected 'number' but got 'string'
error[assign_type]: expected 'number' but got 'boolean'
 --> test.lua:4:19
  |
4 | local z: number = c
  |                   ^ expected 'number' but got 'boolean'
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x: string = a
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused variable 'y'
 --> test.lua:3:7
  |
3 | local y: number = b
  |       ^ unused variable 'y'
  |
help: prefix the name with '_' to suppress this warning: '_y'
warning[unused_variable]: unused variable 'z'
 --> test.lua:4:7
  |
4 | local z: number = c
  |       ^ unused variable 'z'
  |
help: prefix the name with '_' to suppress this warning: '_z'"
    );
}

// ---------------------------------------------------------------------------
// No inference when no RHS expression for a variable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_inference_without_rhs() {
    k9::assert_equal!(
        check(
            "\
local a, b = 42
local x: number = b"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'a'
 --> test.lua:1:7
  |
1 | local a, b = 42
  |       ^ unused variable 'a'
  |
help: prefix the name with '_' to suppress this warning: '_a'
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x: number = b
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
    );
}
