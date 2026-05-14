use std::sync::Arc;
mod common;

use shingetsu::diagnostic::assert_diagnostics;
use shingetsu_compiler::{BuiltInLintId, CompileOptions, Compiler, LintId, Severity};
use std::collections::HashMap;

fn compile_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
        type_check: false,
    }
}

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: Arc::new("@test.lua".to_string()),
        type_check: true,
    }
}

fn type_check_compiler() -> Compiler {
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    Compiler::new(type_check_opts(), env.global_type_map())
}

#[track_caller]
fn filtered_warnings(src: &str, expected: &str) {
    let compiler = Compiler::new(compile_opts(), Default::default());
    // Using block_on here because track_caller doesn't currently
    // work on async functions
    let bc = futures::executor::block_on(compiler.compile(src)).expect("compile failed");
    let filtered = bc.lint_directives.filter(bc.diagnostics);
    assert_diagnostics(&filtered, src, expected);
}

#[track_caller]
fn filtered_warnings_with_types(src: &str, expected: &str) {
    let compiler = type_check_compiler();
    let bc = futures::executor::block_on(compiler.compile(src)).expect("compile failed");
    let filtered = bc.lint_directives.filter(bc.diagnostics);
    assert_diagnostics(&filtered, src, expected);
}

#[track_caller]
fn filtered_warnings_with_project(
    src: &str,
    project_overrides: HashMap<LintId, Severity>,
    expected: &str,
) {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = futures::executor::block_on(compiler.compile(src)).expect("compile failed");
    let mut directives = bc.lint_directives;
    directives.project_overrides = project_overrides;
    let filtered = directives.filter(bc.diagnostics);
    assert_diagnostics(&filtered, src, expected);
}

#[track_caller]
fn filtered_warnings_with_project_typed(
    src: &str,
    project_overrides: HashMap<LintId, Severity>,
    expected: &str,
) {
    let compiler = type_check_compiler();
    let bc = futures::executor::block_on(compiler.compile(src)).expect("compile failed");
    let mut directives = bc.lint_directives;
    directives.project_overrides = project_overrides;
    let filtered = directives.filter(bc.diagnostics);
    assert_diagnostics(&filtered, src, expected);
}

#[tokio::test]
async fn file_level_allow_suppresses_warning() {
    filtered_warnings(
        "\
--# shingetsu: allow(unused_variable)
local x = 1",
        "",
    );
}

#[tokio::test]
async fn file_level_allow_only_suppresses_named_lint() {
    // allow(shadowing) should not suppress unused_variable
    filtered_warnings(
        "\
--# shingetsu: allow(shadowing)
local x = 1",
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'",
    );
}

#[tokio::test]
async fn statement_level_allow_suppresses_warning() {
    filtered_warnings(
        "\
-- shingetsu: allow(unused_variable)
local x = 1",
        "",
    );
}

#[tokio::test]
async fn statement_level_allow_does_not_suppress_other_statements() {
    // allow on the first statement should not affect the second
    filtered_warnings(
        "\
-- shingetsu: allow(unused_variable)
local x = 1
local y = 2",
        "\
warning[unused_variable]: unused variable 'y'
 --> test.lua:3:7
  |
3 | local y = 2
  |       ^ unused variable 'y'
  |
help: prefix the name with '_' to suppress this warning: '_y'",
    );
}

#[tokio::test]
async fn file_level_deny_promotes_warning_to_error() {
    filtered_warnings(
        "\
--# shingetsu: deny(unused_variable)
local x = 1",
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'",
    );
}

#[tokio::test]
async fn file_level_allow_suppresses_arg_count_error() {
    filtered_warnings_with_types(
        "\
--# shingetsu: allow(arg_count)
math.abs()",
        "",
    );
}

#[tokio::test]
async fn unknown_lint_in_directive_produces_warning() {
    filtered_warnings(
        "\
--# shingetsu: allow(bogus_name)
local x = 1",
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unknown_lint]: unknown lint 'bogus_name'
 --> test.lua:1:1
  |
1 | --# shingetsu: allow(bogus_name)
  | ^ unknown lint 'bogus_name'
  |
help: consult the documentation for the full list of built-in lints",
    );
}

#[tokio::test]
async fn file_level_allow_shadowing() {
    filtered_warnings(
        "\
--# shingetsu: allow(shadowing, unused_variable)
local x = 1
local x = 2
return x",
        "",
    );
}

#[tokio::test]
async fn statement_level_allow_shadowing() {
    filtered_warnings(
        "\
-- shingetsu: allow(unused_variable)
local x = 1
-- shingetsu: allow(shadowing)
local x = 2
return x",
        "",
    );
}

#[tokio::test]
async fn statement_level_allow_shadowing_does_not_affect_later_statement() {
    // The allow(shadowing) on the first local should not suppress
    // the shadowing warning on the second local.
    filtered_warnings(
        "\
-- shingetsu: allow(shadowing, unused_variable)
local x = 1
local x = 2
return x",
        "\
warning[shadowing]: variable 'x' shadows earlier declaration in same scope
 --> test.lua:3:7
  |
3 | local x = 2
  |       ^ variable 'x' shadows earlier declaration in same scope",
    );
}

#[tokio::test]
async fn project_level_allow_suppresses_warning() {
    let overrides = HashMap::from([(
        LintId::BuiltIn(BuiltInLintId::UnusedVariable),
        Severity::Allow,
    )]);
    filtered_warnings_with_project("local x = 1", overrides, "");
}

#[tokio::test]
async fn project_level_deny_promotes_warning_to_error() {
    let overrides = HashMap::from([(
        LintId::BuiltIn(BuiltInLintId::UnusedVariable),
        Severity::Error,
    )]);
    filtered_warnings_with_project(
        "local x = 1",
        overrides,
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'",
    );
}

#[tokio::test]
async fn file_directive_overrides_project_config() {
    // Project says deny, but file-level says allow — file wins.
    let overrides = HashMap::from([(
        LintId::BuiltIn(BuiltInLintId::UnusedVariable),
        Severity::Error,
    )]);
    filtered_warnings_with_project(
        "\
--# shingetsu: allow(unused_variable)
local x = 1",
        overrides,
        "",
    );
}

#[tokio::test]
async fn statement_directive_overrides_file_and_project() {
    // Project says deny, file says deny, but statement says allow — statement wins.
    let overrides = HashMap::from([(
        LintId::BuiltIn(BuiltInLintId::UnusedVariable),
        Severity::Error,
    )]);
    filtered_warnings_with_project(
        "\
--# shingetsu: deny(unused_variable)
-- shingetsu: allow(unused_variable)
local x = 1",
        overrides,
        "",
    );
}

#[tokio::test]
async fn project_allow_does_not_suppress_file_deny() {
    // Project says allow, but file says deny — file wins.
    let overrides = HashMap::from([(
        LintId::BuiltIn(BuiltInLintId::UnusedVariable),
        Severity::Allow,
    )]);
    filtered_warnings_with_project(
        "\
--# shingetsu: deny(unused_variable)
local x = 1",
        overrides,
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'",
    );
}

#[tokio::test]
async fn programmatic_project_config() {
    // Verify ProjectConfig can be constructed without TOML.
    let config = shingetsu::project_config::ProjectConfig {
        lints: shingetsu::project_config::LintConfig {
            overrides: HashMap::from([
                (
                    LintId::BuiltIn(BuiltInLintId::UnusedVariable),
                    Severity::Allow,
                ),
                (LintId::BuiltIn(BuiltInLintId::Shadowing), Severity::Error),
            ]),
        },
        check: Default::default(),
        config_dir: None,
    };
    k9::assert_equal!(
        config
            .lints
            .overrides
            .get(&LintId::BuiltIn(BuiltInLintId::UnusedVariable)),
        Some(&Severity::Allow)
    );
    k9::assert_equal!(
        config
            .lints
            .overrides
            .get(&LintId::BuiltIn(BuiltInLintId::Shadowing)),
        Some(&Severity::Error)
    );
}

#[tokio::test]
async fn statement_level_deny_promotes_to_error() {
    filtered_warnings(
        "\
-- shingetsu: deny(unused_variable)
local x = 1",
        "\
error[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'",
    );
}

#[tokio::test]
async fn statement_level_warn_keeps_warning() {
    // unused_variable defaults to warn already, so warn is a no-op here;
    // the diagnostic should still appear as a warning.
    filtered_warnings(
        "\
-- shingetsu: warn(unused_variable)
local x = 1",
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'",
    );
}

#[tokio::test]
async fn project_level_warn_downgrades_error_to_warning() {
    // arg_count defaults to error; project sets it to warn.
    let overrides = HashMap::from([(LintId::BuiltIn(BuiltInLintId::ArgCount), Severity::Warning)]);
    filtered_warnings_with_project_typed(
        "math.abs()",
        overrides,
        "\
warning[arg_count]: expected 1 argument but got 0
 --> test.lua:1:9
  |
1 | math.abs()
  |         ^^ expected 1 argument but got 0",
    );
}

#[tokio::test]
async fn multiple_file_directives_different_actions() {
    filtered_warnings(
        "\
--# shingetsu: allow(unused_variable)
--# shingetsu: deny(shadowing)
local x = 1
local x = 2
return x",
        "\
error[shadowing]: variable 'x' shadows earlier declaration in same scope
 --> test.lua:4:7
  |
4 | local x = 2
  |       ^ variable 'x' shadows earlier declaration in same scope",
    );
}

#[tokio::test]
async fn project_prefix_lint_parsed_as_override() {
    // `project:`-prefixed names parse into overrides; unknown
    // plugin validation happens later via validate_against_plugins.
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler
        .compile("--# shingetsu: allow(project:my_plugin)\nlocal x = 1")
        .await
        .expect("compile failed");
    k9::assert_equal!(
        bc.lint_directives
            .file_overrides
            .get(&LintId::Plugin(Arc::from("my_plugin"))),
        Some(&Severity::Allow)
    );
    k9::assert_equal!(bc.lint_directives.plugin_refs.len(), 1);
    k9::assert_equal!(bc.lint_directives.plugin_refs[0].name, "my_plugin");
}

#[tokio::test]
async fn typoed_builtin_lint_suggests_correction() {
    // "argcount" is close to the real "arg_count" -- render_suggestion
    // produces a did-you-mean hint.  The full alternative list is
    // too long to enumerate (19 other built-ins), so the message
    // truncates with a documentation pointer.
    filtered_warnings(
            "\
--# shingetsu: deny(argcount)
local x = 1"
        ,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unknown_lint]: unknown lint 'argcount'
 --> test.lua:1:1
  |
1 | --# shingetsu: deny(argcount)
  | ^ unknown lint 'argcount'
  |
help: Did you mean `arg_count`? There are too many alternatives to list here; consult the documentation!"
    );
}

#[tokio::test]
async fn unknown_plugin_lint_warns_without_plugins() {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler
        .compile("--# shingetsu: allow(project:missing)\nlocal x = 1")
        .await
        .expect("compile failed");
    let plugin_diags = bc.lint_directives.validate_against_plugins(&[]);
    let source = "--# shingetsu: allow(project:missing)\nlocal x = 1";
    assert_diagnostics(
        &plugin_diags,
        source,
        "\
warning[unknown_lint]: unknown plugin lint 'project:missing'
 --> test.lua:1:1
  |
1 | --# shingetsu: allow(project:missing)
  | ^ unknown plugin lint 'project:missing'
  |
help: no lint plugins are loaded in this run",
    );
}

#[tokio::test]
async fn unknown_plugin_lint_warns_and_suggests_known_plugins() {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler
        .compile("--# shingetsu: allow(project:misspelled)\nlocal x = 1")
        .await
        .expect("compile failed");
    let plugin_diags = bc
        .lint_directives
        .validate_against_plugins(&["demo", "other"]);
    let source = "--# shingetsu: allow(project:misspelled)\nlocal x = 1";
    assert_diagnostics(
        &plugin_diags,
        source,
        "\
warning[unknown_lint]: unknown plugin lint 'project:misspelled'
 --> test.lua:1:1
  |
1 | --# shingetsu: allow(project:misspelled)
  | ^ unknown plugin lint 'project:misspelled'
  |
help: Did you mean one of `project:demo`, `project:other`?",
    );
}

#[tokio::test]
async fn plugin_ref_validation_known_succeeds() {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler
        .compile("--# shingetsu: allow(project:demo)\nlocal x = 1")
        .await
        .expect("compile failed");
    let plugin_diags = bc.lint_directives.validate_against_plugins(&["demo"]);
    let source = "--# shingetsu: allow(project:demo)\nlocal x = 1";
    assert_diagnostics(&plugin_diags, source, "");
}
