mod common;

use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu_compiler::{CompileOptions, Compiler, LintId, Severity};
use std::collections::HashMap;

fn compile_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "@test.lua".into(),
        type_check: false,
    }
}

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "@test.lua".into(),
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

async fn filtered_warnings_with_project(
    src: &str,
    project_overrides: HashMap<LintId, Severity>,
) -> String {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let mut directives = bc.lint_directives;
    directives.project_overrides = project_overrides;
    let filtered = directives.filter(bc.diagnostics);
    render_warnings(&filtered, src, RenderStyle::Plain)
}

async fn filtered_warnings_with_project_typed(
    src: &str,
    project_overrides: HashMap<LintId, Severity>,
) -> String {
    let compiler = type_check_compiler();
    let bc = compiler.compile(src).await.expect("compile failed");
    let mut directives = bc.lint_directives;
    directives.project_overrides = project_overrides;
    let filtered = directives.filter(bc.diagnostics);
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
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
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
  |       ^ unused variable 'y'
  |
help: prefix the name with '_' to suppress this warning: '_y'"
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
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
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
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unknown_lint]: unknown lint 'bogus_name'
  |
help: available lints: arg_count, arg_type, assign_type, call_convention, empty_loop, field_access, missing_return, return_type, shadowing, unreachable_code, unused_variable"
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

#[tokio::test]
async fn project_level_allow_suppresses_warning() {
    let overrides = HashMap::from([(LintId::UnusedVariable, Severity::Allow)]);
    k9::assert_equal!(
        filtered_warnings_with_project("local x = 1", overrides).await,
        ""
    );
}

#[tokio::test]
async fn project_level_deny_promotes_warning_to_error() {
    let overrides = HashMap::from([(LintId::UnusedVariable, Severity::Error)]);
    k9::assert_equal!(
        filtered_warnings_with_project("local x = 1", overrides).await,
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
    );
}

#[tokio::test]
async fn file_directive_overrides_project_config() {
    // Project says deny, but file-level says allow — file wins.
    let overrides = HashMap::from([(LintId::UnusedVariable, Severity::Error)]);
    k9::assert_equal!(
        filtered_warnings_with_project(
            "\
--# shingetsu: allow(unused_variable)
local x = 1",
            overrides,
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn statement_directive_overrides_file_and_project() {
    // Project says deny, file says deny, but statement says allow — statement wins.
    let overrides = HashMap::from([(LintId::UnusedVariable, Severity::Error)]);
    k9::assert_equal!(
        filtered_warnings_with_project(
            "\
--# shingetsu: deny(unused_variable)
-- shingetsu: allow(unused_variable)
local x = 1",
            overrides,
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn project_allow_does_not_suppress_file_deny() {
    // Project says allow, but file says deny — file wins.
    let overrides = HashMap::from([(LintId::UnusedVariable, Severity::Allow)]);
    k9::assert_equal!(
        filtered_warnings_with_project(
            "\
--# shingetsu: deny(unused_variable)
local x = 1",
            overrides,
        )
        .await,
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
    );
}

#[tokio::test]
async fn programmatic_project_config() {
    // Verify ProjectConfig can be constructed without TOML.
    let config = shingetsu::project_config::ProjectConfig {
        lints: shingetsu::project_config::LintConfig {
            overrides: HashMap::from([
                (LintId::UnusedVariable, Severity::Allow),
                (LintId::Shadowing, Severity::Error),
            ]),
        },
    };
    k9::assert_equal!(
        config.lints.overrides.get(&LintId::UnusedVariable),
        Some(&Severity::Allow)
    );
    k9::assert_equal!(
        config.lints.overrides.get(&LintId::Shadowing),
        Some(&Severity::Error)
    );
}

#[tokio::test]
async fn statement_level_deny_promotes_to_error() {
    k9::assert_equal!(
        filtered_warnings(
            "\
-- shingetsu: deny(unused_variable)
local x = 1"
        )
        .await,
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
    );
}

#[tokio::test]
async fn statement_level_warn_keeps_warning() {
    // unused_variable defaults to warn already, so warn is a no-op here;
    // the diagnostic should still appear as a warning.
    k9::assert_equal!(
        filtered_warnings(
            "\
-- shingetsu: warn(unused_variable)
local x = 1"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
    );
}

#[tokio::test]
async fn project_level_warn_downgrades_error_to_warning() {
    // arg_count defaults to error; project sets it to warn.
    let overrides = HashMap::from([(LintId::ArgCount, Severity::Warning)]);
    k9::assert_equal!(
        filtered_warnings_with_project_typed("math.abs()", overrides).await,
        "\
warning[arg_count]: expected 1 argument but got 0
 --> test.lua:1:9
  |
1 | math.abs()
  |         ^^ expected 1 argument but got 0"
    );
}

#[tokio::test]
async fn multiple_file_directives_different_actions() {
    k9::assert_equal!(
        filtered_warnings(
            "\
--# shingetsu: allow(unused_variable)
--# shingetsu: deny(shadowing)
local x = 1
local x = 2
return x"
        )
        .await,
        "\
error[shadowing]: variable 'x' shadows earlier declaration in same scope
 --> test.lua:4:7
  |
4 | local x = 2
  |       ^ variable 'x' shadows earlier declaration in same scope"
    );
}
