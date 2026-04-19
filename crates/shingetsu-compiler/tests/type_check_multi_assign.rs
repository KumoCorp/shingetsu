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
error[field_access]: unknown field 'z' on type 'Point'
 --> test.lua:6:11
  |
6 | local _ = p.z
  |           ^^^ unknown field 'z' on type 'Point'"
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

// ---------------------------------------------------------------------------
// Local-to-local display name propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_to_local_display_name() {
    let d = check(
        "\
type Point = { x: number, y: number }
local a: Point = {}
local b = a
local _ = b.z",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
error[field_access]: unknown field 'z' on type 'Point'
 --> test.lua:4:11
  |
4 | local _ = b.z
  |           ^^^ unknown field 'z' on type 'Point'"
    );
}

// ---------------------------------------------------------------------------
// Chained local inference — display name propagates through chain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chained_local_inference() {
    let d = check(
        "\
type Point = { x: number, y: number }
local a: Point = {}
local b = a
local c = b
local _ = c.z",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
error[field_access]: unknown field 'z' on type 'Point'
 --> test.lua:5:11
  |
5 | local _ = c.z
  |           ^^^ unknown field 'z' on type 'Point'"
    );
}

// ---------------------------------------------------------------------------
// Function call with no return annotation — no alias display name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_call_no_return_annotation() {
    let d = check(
        "\
local function make()
    return {}
end
local p = make()
local _ = p.z",
    )
    .await;
    k9::assert_equal!(d, "");
}

// ---------------------------------------------------------------------------
// Infer from binary operator
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_binary_op() {
    let d = check(
        "\
local a: number = 1
local b: number = 2
local c = a + b
local x: string = c",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:4:19
  |
4 | local x: string = c
  |                   ^ expected 'string' but got 'number'
warning[unused_variable]: unused variable 'x'
 --> test.lua:4:7
  |
4 | local x: string = c
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
    );
}

// ---------------------------------------------------------------------------
// Multi-assign with fewer RHS than LHS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_fewer_rhs() {
    let d = check(
        "\
type T = { x: number, y: string }
local t: T = {}
local a, b, c = t.x, t.y
local _: string = a
local _: number = b
local _: number = c",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:4:19
  |
4 | local _: string = a
  |                   ^ expected 'string' but got 'number'
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:5:19
  |
5 | local _: number = b
  |                   ^ expected 'number' but got 'string'"
    );
}
