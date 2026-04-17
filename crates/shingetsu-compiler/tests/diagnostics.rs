mod common;

use shingetsu::diagnostic::{
    render_compile_error, render_runtime_error, render_warning, RenderStyle,
};
use shingetsu_compiler::{compile, CompileOptions, Diagnostic, Severity, SourceLocation};
use shingetsu_vm::{Function, Task};

fn compile_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "test.lua".into(),
    }
}

fn run_runtime_error(src: &str) -> shingetsu_vm::error::RuntimeError {
    let opts = compile_opts();
    let bc = compile(src, &opts).expect("compile failed");
    let env = common::new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(Task::new(env, func, vec![])).unwrap_err()
}

// ---------------------------------------------------------------------------
// Compile error diagnostics
// ---------------------------------------------------------------------------

#[test]
fn compile_error_parse() {
    let src = "local x =\n";
    let opts = compile_opts();
    let err = compile(src, &opts).unwrap_err();
    let rendered = render_compile_error(&err, src, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: test.lua:1:9: error occurred while creating ast: unexpected token `=`. (starting from line 1, character 9 and ending on line 1, character 10)
       additional information: expected an expression
 --> test.lua:1:9
  |
1 | local x =
  |         ^ test.lua:1:9: error occurred while creating ast: unexpected token `=`. (starting from line 1, character 9 and ending on line 1, character 10)
additional information: expected an expression"
    );
}

#[test]
fn compile_error_semantic_break_outside_loop() {
    let src = "break\n";
    let opts = compile_opts();
    let err = compile(src, &opts).unwrap_err();
    let rendered = render_compile_error(&err, src, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: test.lua:1:1: break outside loop
 --> test.lua:1:1
  |
1 | break
  | ^^^^^ test.lua:1:1: break outside loop"
    );
}

// ---------------------------------------------------------------------------
// Runtime error diagnostics
// ---------------------------------------------------------------------------

#[test]
fn runtime_error_nil_call() {
    let re = run_runtime_error("local x = nil\nx()");
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: attempt to call local 'x' (a nil value)
 --> test.lua:2:1
  |
1 | local x = nil
  | ------------- defined here
2 | x()
  | ^^^ attempt to call local 'x' (a nil value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[test]
fn runtime_error_nil_call_with_reassignment() {
    let re = run_runtime_error("local x = 42\nx = nil\nx()");
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: attempt to call local 'x' (a nil value)
 --> test.lua:3:1
  |
1 | local x = 42
  | ------------ defined here
2 | x = nil
  | ------- last assigned here
3 | x()
  | ^^^ attempt to call local 'x' (a nil value)
stack traceback:
\ttest.lua:3: in main chunk"
    );
}

#[test]
fn runtime_error_in_function() {
    let re = run_runtime_error(
        "\
local function foo()
    error('boom')
end
foo()",
    );
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: test.lua:2: boom
 --> test.lua:2:5
  |
2 |     error('boom')
  |     ^^^^^^^^^^^^^ test.lua:2: boom
stack traceback:
\ttest.lua:2: in function foo()
\ttest.lua:4: in main chunk"
    );
}

#[test]
fn runtime_error_string_error() {
    let re = run_runtime_error("error('custom message')");
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: test.lua:1: custom message
 --> test.lua:1:1
  |
1 | error('custom message')
  | ^^^^^^^^^^^^^^^^^^^^^^^ test.lua:1: custom message
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[test]
fn runtime_error_type_error_arithmetic() {
    let re = run_runtime_error("local x = 'hello'\nlocal y = x + 1");
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: attempt to perform arithmetic on local 'x' (a string value)
 --> test.lua:2:1
  |
1 | local x = 'hello'
  | ----------------- defined here
2 | local y = x + 1
  | ^^^^^^^^^^^^^^^ attempt to perform arithmetic on local 'x' (a string value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[test]
fn compile_error_colored() {
    let src = "local x =\n";
    let opts = compile_opts();
    let err = compile(src, &opts).unwrap_err();
    let rendered = render_compile_error(&err, src, RenderStyle::Colored);
    k9::assert_equal!(
        rendered,
        "\
\u{1b}[1m\u{1b}[91merror\u{1b}[0m\u{1b}[1m: test.lua:1:9: error occurred while creating ast: unexpected token `=`. (starting from line 1, character 9 and ending on line 1, character 10)\u{1b}[0m
       \u{1b}[1madditional information: expected an expression\u{1b}[0m
 \u{1b}[1m\u{1b}[94m--> \u{1b}[0mtest.lua:1:9
  \u{1b}[1m\u{1b}[94m|\u{1b}[0m
\u{1b}[1m\u{1b}[94m1\u{1b}[0m \u{1b}[1m\u{1b}[94m|\u{1b}[0m local x =
  \u{1b}[1m\u{1b}[94m|\u{1b}[0m         \u{1b}[1m\u{1b}[91m^\u{1b}[0m \u{1b}[1m\u{1b}[91mtest.lua:1:9: error occurred while creating ast: unexpected token `=`. (starting from line 1, character 9 and ending on line 1, character 10)
additional information: expected an expression\u{1b}[0m"
    );
}

#[test]
fn render_warning_plain() {
    let src = "local x = 42\nprint(x)\n";
    let diag = Diagnostic {
        severity: Severity::Warning,
        location: SourceLocation {
            source_name: "test.lua".into(),
            line: 1,
            column: 7,
            byte_offset: 6,
            byte_len: 1,
        },
        message: "unused variable 'x'".into(),
    };
    let rendered = render_warning(&diag, src, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
warning: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 42
  |       ^ unused variable 'x'"
    );
}

#[test]
fn render_warning_colored() {
    let src = "local x = 42\nprint(x)\n";
    let diag = Diagnostic {
        severity: Severity::Warning,
        location: SourceLocation {
            source_name: "test.lua".into(),
            line: 1,
            column: 7,
            byte_offset: 6,
            byte_len: 1,
        },
        message: "unused variable 'x'".into(),
    };
    let rendered = render_warning(&diag, src, RenderStyle::Colored);
    k9::assert_equal!(
        rendered,
        "\u{1b}[1m\u{1b}[33mwarning\u{1b}[0m\u{1b}[1m: unused variable 'x'\u{1b}[0m\n \u{1b}[1m\u{1b}[94m--> \u{1b}[0mtest.lua:1:7\n  \u{1b}[1m\u{1b}[94m|\u{1b}[0m\n\u{1b}[1m\u{1b}[94m1\u{1b}[0m \u{1b}[1m\u{1b}[94m|\u{1b}[0m local x = 42\n  \u{1b}[1m\u{1b}[94m|\u{1b}[0m       \u{1b}[1m\u{1b}[33m^\u{1b}[0m \u{1b}[1m\u{1b}[33munused variable 'x'\u{1b}[0m"
    );
}

// ---------------------------------------------------------------------------
// Unused variable warnings (D8b)
// ---------------------------------------------------------------------------

fn warnings(src: &str) -> Vec<String> {
    let opts = compile_opts();
    let bc = compile(src, &opts).expect("compile failed");
    bc.diagnostics
        .iter()
        .map(|d| render_warning(d, src, RenderStyle::Plain))
        .collect()
}

#[test]
fn unused_variable_simple() {
    k9::assert_equal!(
        warnings("local x = 1"),
        vec!["\
warning: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'"]
    );
}

#[test]
fn unused_variable_read_suppresses_warning() {
    k9::assert_equal!(warnings("local x = 1\nreturn x"), Vec::<String>::new());
}

#[test]
fn unused_variable_underscore_suppressed() {
    k9::assert_equal!(warnings("local _x = 1"), Vec::<String>::new());
}

#[test]
fn unused_variable_bare_underscore_suppressed() {
    k9::assert_equal!(warnings("local _ = 1"), Vec::<String>::new());
}

#[test]
fn unused_variable_assigned_but_not_read() {
    k9::assert_equal!(
        warnings("local x = 1\nx = 2"),
        vec!["\
warning: variable 'x' is assigned to but never read
 --> test.lua:2:1
  |
2 | x = 2
  | ^ variable 'x' is assigned to but never read"]
    );
}

#[test]
fn unused_variable_close_suppressed() {
    // <close> variables exist for their side effect; no warning expected.
    k9::assert_equal!(warnings("local f <close> = nil"), Vec::<String>::new());
}

#[test]
fn unused_variable_for_loop() {
    k9::assert_equal!(
        warnings("for i = 1, 10 do end"),
        vec!["\
warning: unused variable 'i'
 --> test.lua:1:5
  |
1 | for i = 1, 10 do end
  |     ^ unused variable 'i'"]
    );
}

#[test]
fn unused_variable_for_loop_underscore() {
    k9::assert_equal!(warnings("for _ = 1, 10 do end"), Vec::<String>::new());
}

#[test]
fn unused_variable_generic_for() {
    k9::assert_equal!(
        warnings("for k, v in pairs({}) do end"),
        vec![
            "\
warning: unused variable 'k'
 --> test.lua:1:5
  |
1 | for k, v in pairs({}) do end
  |     ^ unused variable 'k'",
            "\
warning: unused variable 'v'
 --> test.lua:1:8
  |
1 | for k, v in pairs({}) do end
  |        ^ unused variable 'v'"
        ]
    );
}

#[test]
fn unused_variable_generic_for_underscore_key() {
    k9::assert_equal!(
        warnings("for _, v in pairs({}) do\nreturn v\nend"),
        Vec::<String>::new()
    );
}

#[test]
fn unused_variable_in_function() {
    k9::assert_equal!(
        warnings("local function foo()\nlocal x = 1\nend\nfoo()"),
        vec!["\
warning: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'"]
    );
}

#[test]
fn unused_variable_captured_as_upvalue() {
    // x is captured by the closure — not unused.
    k9::assert_equal!(
        warnings("local x = 1\nlocal function foo()\nreturn x\nend\nreturn foo()"),
        Vec::<String>::new()
    );
}

#[test]
fn used_in_compound_assignment() {
    // x is read and written by +=, so it's read.
    k9::assert_equal!(
        warnings("local x = 1\nx += 1\nreturn x"),
        Vec::<String>::new()
    );
}

#[test]
fn unused_local_function() {
    k9::assert_equal!(
        warnings("local function foo() end"),
        vec!["\
warning: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo() end
  |                ^^^ unused function 'foo'"]
    );
}
