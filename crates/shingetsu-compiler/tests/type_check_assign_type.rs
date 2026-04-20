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

#[tokio::test]
async fn string_for_number() {
    k9::assert_equal!(
        check("local _x: number = \"hello\"").await,
        "\
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:1:20
  |
1 | local _x: number = \"hello\"
  |                    ^^^^^^^ expected 'number' but got 'string'"
    );
}

#[tokio::test]
async fn correct_type() {
    k9::assert_equal!(check("local x: number = 42\nreturn x").await, "");
}

#[tokio::test]
async fn integer_for_number() {
    k9::assert_equal!(check("local x: number = 42\nreturn x").await, "");
}

#[tokio::test]
async fn float_for_number() {
    k9::assert_equal!(check("local x: number = 1.5\nreturn x").await, "");
}

#[tokio::test]
async fn nil_for_optional() {
    k9::assert_equal!(check("local x: number? = nil\nreturn x").await, "");
}

#[tokio::test]
async fn wrong_for_optional() {
    k9::assert_equal!(
        check("local _x: number? = \"oops\"").await,
        "\
error[assign_type]: expected 'number?' but got 'string'
 --> test.lua:1:21
  |
1 | local _x: number? = \"oops\"
  |                     ^^^^^^ expected 'number?' but got 'string'"
    );
}

#[tokio::test]
async fn any_annotation_no_check() {
    k9::assert_equal!(check("local x: any = \"anything\"\nreturn x").await, "");
}

#[tokio::test]
async fn no_annotation_no_check() {
    k9::assert_equal!(check("local x = \"anything\"\nreturn x").await, "");
}

#[tokio::test]
async fn boolean_for_string() {
    k9::assert_equal!(
        check("local _x: string = true").await,
        "\
error[assign_type]: expected 'string' but got 'boolean'
 --> test.lua:1:20
  |
1 | local _x: string = true
  |                    ^^^^ expected 'string' but got 'boolean'"
    );
}

#[tokio::test]
async fn table_for_table() {
    k9::assert_equal!(
        check("type Config = { name: string }\nlocal c: Config = {}\nreturn c").await,
        ""
    );
}

#[tokio::test]
async fn unknown_rhs_no_check() {
    k9::assert_equal!(
        check("local t = {}\nlocal x: number = t[1]\nreturn x").await,
        ""
    );
}

#[tokio::test]
async fn union_annotation() {
    k9::assert_equal!(
        check("local _x: number | string = true").await,
        "\
error[assign_type]: expected 'number | string' but got 'boolean'
 --> test.lua:1:29
  |
1 | local _x: number | string = true
  |                             ^^^^ expected 'number | string' but got 'boolean'"
    );
}

#[tokio::test]
async fn union_annotation_match() {
    k9::assert_equal!(check("local x: number | string = 42\nreturn x").await, "");
}

#[tokio::test]
async fn expression_rhs() {
    k9::assert_equal!(
        check("local _x: string = 1 + 2").await,
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:1:20
  |
1 | local _x: string = 1 + 2
  |                    ^^^^^ expected 'string' but got 'number'"
    );
}

#[tokio::test]
async fn function_for_number() {
    k9::assert_equal!(
        check("local _x: number = function() end").await,
        "\
error[assign_type]: expected 'number' but got 'function'
 --> test.lua:1:20
  |
1 | local _x: number = function() end
  |                    ^^^^^^^^^^^^^^ expected 'number' but got 'function'"
    );
}

#[tokio::test]
async fn multiple_locals_second_wrong() {
    k9::assert_equal!(
        check("local a: number, b: string = 42, true\nreturn a, b").await,
        "\
error[assign_type]: expected 'string' but got 'boolean'
 --> test.lua:1:34
  |
1 | local a: number, b: string = 42, true
  |                                  ^^^^ expected 'string' but got 'boolean'"
    );
}
