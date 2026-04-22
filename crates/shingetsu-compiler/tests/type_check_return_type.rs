use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu_compiler::{CompileOptions, Compiler};

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "@test.lua".into(),
        type_check: true,
    }
}

async fn check(src: &str) -> String {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

#[tokio::test]
async fn correct_return_type() {
    k9::assert_equal!(
        check("local function f(): number return 42 end\nf()").await,
        ""
    );
}

#[tokio::test]
async fn wrong_return_type() {
    k9::assert_equal!(
        check("local function f(): number return \"hello\" end\nf()").await,
        "\
error[return_type]: expected return type 'number' but got 'string'
 --> test.lua:1:35
  |
1 | local function f(): number return \"hello\" end
  |                                   ^^^^^^^ expected return type 'number' but got 'string'"
    );
}

#[tokio::test]
async fn integer_for_number_return() {
    k9::assert_equal!(
        check("local function f(): number return 42 end\nf()").await,
        ""
    );
}

#[tokio::test]
async fn nil_for_optional_return() {
    k9::assert_equal!(
        check("local function f(): number? return nil end\nf()").await,
        ""
    );
}

#[tokio::test]
async fn wrong_for_optional_return() {
    k9::assert_equal!(
        check("local function f(): number? return \"oops\" end\nf()").await,
        "\
error[return_type]: expected return type 'number?' but got 'string'
 --> test.lua:1:36
  |
1 | local function f(): number? return \"oops\" end
  |                                    ^^^^^^ expected return type 'number?' but got 'string'"
    );
}

#[tokio::test]
async fn no_annotation_no_check() {
    k9::assert_equal!(
        check("local function f() return \"anything\" end\nf()").await,
        ""
    );
}

#[tokio::test]
async fn any_return_type_no_check() {
    k9::assert_equal!(
        check("local function f(): any return \"anything\" end\nf()").await,
        ""
    );
}

#[tokio::test]
async fn multiple_return_values_wrong() {
    k9::assert_equal!(
        check("local function f(): (number, string) return \"wrong\", 42 end\nf()").await,
        "\
error[return_type]: expected return type 'number' at position 1 but got 'string'
 --> test.lua:1:45
  |
1 | local function f(): (number, string) return \"wrong\", 42 end
  |                                             ^^^^^^^ expected return type 'number' at position 1 but got 'string'
error[return_type]: expected return type 'string' at position 2 but got 'integer'
 --> test.lua:1:54
  |
1 | local function f(): (number, string) return \"wrong\", 42 end
  |                                                      ^^ expected return type 'string' at position 2 but got 'integer'"
    );
}

#[tokio::test]
async fn multiple_return_values_correct() {
    k9::assert_equal!(
        check("local function f(): (number, string) return 42, \"hello\" end\nf()").await,
        ""
    );
}

#[tokio::test]
async fn missing_return_value_is_nil() {
    k9::assert_equal!(
        check("local function f(): (number, string) return 42 end\nf()").await,
        "\
error[return_type]: expected return type 'string' at position 2 but got 'nil'
 --> test.lua:1:38
  |
1 | local function f(): (number, string) return 42 end
  |                                      ^^^^^^^^^ expected return type 'string' at position 2 but got 'nil'"
    );
}

#[tokio::test]
async fn function_decl_return_type() {
    k9::assert_equal!(
        check("local t = {}\nfunction t.greet(): number return \"wrong\" end\nt.greet()").await,
        "\
error[return_type]: expected return type 'number' but got 'string'
 --> test.lua:2:35
  |
2 | function t.greet(): number return \"wrong\" end
  |                                   ^^^^^^^ expected return type 'number' but got 'string'"
    );
}

#[tokio::test]
async fn function_expression_return_type() {
    k9::assert_equal!(
        check("local f = function(): number return \"wrong\" end\nf()").await,
        "\
error[return_type]: expected return type 'number' but got 'string'
 --> test.lua:1:37
  |
1 | local f = function(): number return \"wrong\" end
  |                                     ^^^^^^^ expected return type 'number' but got 'string'"
    );
}

#[tokio::test]
async fn nested_function_independent_return() {
    k9::assert_equal!(
        check(
            "\
local function outer(): number
    local function inner(): string return \"hello\" end
    inner()
    return 42
end
outer()"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn nested_function_inner_wrong() {
    k9::assert_equal!(
        check(
            "\
local function outer(): number
    local function inner(): string return 42 end
    inner()
    return 1
end
outer()"
        )
        .await,
        "\
error[return_type]: expected return type 'string' but got 'integer'
 --> test.lua:2:43
  |
2 |     local function inner(): string return 42 end
  |                                           ^^ expected return type 'string' but got 'integer'"
    );
}

#[tokio::test]
async fn union_return_type() {
    k9::assert_equal!(
        check("local function f(): number | string return \"hello\" end\nf()").await,
        ""
    );
}

#[tokio::test]
async fn union_return_type_wrong() {
    k9::assert_equal!(
        check("local function f(): number | string return true end\nf()").await,
        "\
error[return_type]: expected return type 'number | string' but got 'boolean'
 --> test.lua:1:44
  |
1 | local function f(): number | string return true end
  |                                            ^^^^ expected return type 'number | string' but got 'boolean'"
    );
}

#[tokio::test]
async fn return_variable_with_known_type() {
    k9::assert_equal!(
        check(
            "\
local function f(): number
    local s: string = \"hello\"
    return s
end
f()"
        )
        .await,
        "\
error[return_type]: expected return type 'number' but got 'string'
 --> test.lua:3:12
  |
3 |     return s
  |            ^ expected return type 'number' but got 'string'"
    );
}

#[tokio::test]
async fn return_expression_inferred() {
    k9::assert_equal!(
        check("local function f(): string return 1 + 2 end\nf()").await,
        "\
error[return_type]: expected return type 'string' but got 'number'
 --> test.lua:1:35
  |
1 | local function f(): string return 1 + 2 end
  |                                   ^^^^^ expected return type 'string' but got 'number'"
    );
}

#[tokio::test]
async fn return_unknown_expr_no_diagnostic() {
    k9::assert_equal!(
        check(
            "\
local function f(): number
    local t = {}
    return t[1]
end
f()"
        )
        .await,
        ""
    );
}
