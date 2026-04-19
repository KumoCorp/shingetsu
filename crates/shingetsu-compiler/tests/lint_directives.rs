mod common;

use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu_compiler::{CompileOptions, Compiler};

fn compile_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "test.lua".into(),
        type_check: false,
    }
}

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "test.lua".into(),
        type_check: true,
    }
}

fn type_check_compiler() -> Compiler {
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    Compiler::new(type_check_opts(), env.global_type_map())
}

async fn filtered_warnings(src: &str) -> String {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let filtered = bc.lint_directives.filter(bc.diagnostics);
    render_warnings(&filtered, src, RenderStyle::Plain)
}

async fn filtered_warnings_with_types(src: &str) -> String {
    let compiler = type_check_compiler();
    let bc = compiler.compile(src).await.expect("compile failed");
    let filtered = bc.lint_directives.filter(bc.diagnostics);
    render_warnings(&filtered, src, RenderStyle::Plain)
}

#[tokio::test]
async fn file_level_allow_suppresses_warning() {
    k9::assert_equal!(
        filtered_warnings(
            "\
--# shingetsu: allow(unused_variable)
local x = 1"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn file_level_allow_only_suppresses_named_lint() {
    // allow(shadowing) should not suppress unused_variable
    k9::assert_equal!(
        filtered_warnings(
            "\
--# shingetsu: allow(shadowing)
local x = 1"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[tokio::test]
async fn statement_level_allow_suppresses_warning() {
    k9::assert_equal!(
        filtered_warnings(
            "\
-- shingetsu: allow(unused_variable)
local x = 1"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn statement_level_allow_does_not_suppress_other_statements() {
    // allow on the first statement should not affect the second
    k9::assert_equal!(
        filtered_warnings(
            "\
-- shingetsu: allow(unused_variable)
local x = 1
local y = 2"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'y'
 --> test.lua:3:7
  |
3 | local y = 2
  |       ^ unused variable 'y'"
    );
}

#[tokio::test]
async fn file_level_deny_promotes_warning_to_error() {
    k9::assert_equal!(
        filtered_warnings(
            "\
--# shingetsu: deny(unused_variable)
local x = 1"
        )
        .await,
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[tokio::test]
async fn file_level_allow_suppresses_arg_count_error() {
    k9::assert_equal!(
        filtered_warnings_with_types(
            "\
--# shingetsu: allow(arg_count)
math.abs()"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn unknown_lint_in_directive_produces_warning() {
    k9::assert_equal!(
        filtered_warnings(
            "\
--# shingetsu: allow(bogus_name)
local x = 1"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
warning[unknown_lint]: unknown lint 'bogus_name'"
    );
}

#[tokio::test]
async fn file_level_allow_shadowing() {
    k9::assert_equal!(
        filtered_warnings(
            "\
--# shingetsu: allow(shadowing, unused_variable)
local x = 1
local x = 2
return x"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn statement_level_allow_shadowing() {
    k9::assert_equal!(
        filtered_warnings(
            "\
-- shingetsu: allow(unused_variable)
local x = 1
-- shingetsu: allow(shadowing)
local x = 2
return x"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn statement_level_allow_shadowing_does_not_affect_later_statement() {
    // The allow(shadowing) on the first local should not suppress
    // the shadowing warning on the second local.
    k9::assert_equal!(
        filtered_warnings(
            "\
-- shingetsu: allow(shadowing, unused_variable)
local x = 1
local x = 2
return x"
        )
        .await,
        "\
warning[shadowing]: variable 'x' shadows earlier declaration in same scope
 --> test.lua:3:7
  |
3 | local x = 2
  |       ^ variable 'x' shadows earlier declaration in same scope"
    );
}
