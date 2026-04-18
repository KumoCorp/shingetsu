mod common;

use std::sync::Arc;

use shingetsu::diagnostic::{
    render_compile_error, render_runtime_error, render_warning, render_warnings, RenderStyle,
};
use shingetsu_compiler::{compile, CompileOptions, Diagnostic, Severity, SourceLocation};
use shingetsu_vm::{Function, Task, Value};

fn compile_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "test.lua".into(),
    }
}

fn run_runtime_error(src: &str) -> shingetsu_vm::error::RuntimeError {
    run_runtime_error_with_env(src, common::new_env())
}

fn run_runtime_error_with_env(
    src: &str,
    env: shingetsu_vm::GlobalEnv,
) -> shingetsu_vm::error::RuntimeError {
    let opts = compile_opts();
    let bc = compile(src, &opts).expect("compile failed");
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
fn stack_overflow_collapses_recursive_frames() {
    let src = "local function f() return f() end\nf()";
    let re = run_runtime_error(src);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    // The 199 recursive f() calls should collapse into one line + repeat count.
    k9::assert_equal!(
        rendered,
        "\
error: stack overflow
 --> test.lua:1:20
  |
1 | local function f() return f() end
  |                    ^^^^^^^^^^ stack overflow
stack traceback:
\ttest.lua:1: in function f()
\t... (repeated 198 times)
\ttest.lua:2: in main chunk"
    );
}

#[test]
fn non_recursive_short_trace_not_truncated() {
    // A short call chain should appear in full without truncation.
    let src = "\
local function a() error('boom') end
local function b() a() end
local function c() b() end
c()";
    let re = run_runtime_error(src);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: test.lua:1: boom
 --> test.lua:1:20
  |
1 | local function a() error('boom') end
  |                    ^^^^^^^^^^^^^ test.lua:1: boom
stack traceback:
\ttest.lua:1: in function a()
\ttest.lua:2: in function b()
\ttest.lua:3: in function c()
\ttest.lua:4: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Dot vs colon call hints
// ---------------------------------------------------------------------------

#[test]
fn hint_dot_call_on_colon_method() {
    // Calling a :-defined method with . passes the wrong self.
    let src = "local obj = {}\nfunction obj:greet(greeting)\n    return greeting .. ' ' .. self.name\nend\nobj.greet('hello')";
    let re = run_runtime_error(src);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: attempt to concatenate a nil value
 --> test.lua:3:5
  |
3 |     return greeting .. ' ' .. self.name
  |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ attempt to concatenate a nil value
help: 'obj:greet' uses ':' syntax \u{2014} call as obj:greet() not obj.greet()
 --> test.lua:5:4
  |
5 | obj.greet('hello')
  |    ^ 'obj:greet' uses ':' syntax \u{2014} call as obj:greet() not obj.greet()
stack traceback:
\ttest.lua:3: in function obj:greet()
\ttest.lua:5: in main chunk"
    );
}

#[test]
fn hint_dot_call_self_is_number() {
    // self becomes a number when dot-called.
    let src =
        "local obj = {}\nfunction obj:set_name(name)\n    self.name = name\nend\nobj.set_name(42)";
    let re = run_runtime_error(src);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: attempt to index local 'self' (a number value) with key 'name'
 --> test.lua:3:5
  |
3 |     self.name = name
  |     ^^^^^^^^^^^^^^^^ attempt to index local 'self' (a number value) with key 'name'
help: 'obj:set_name' uses ':' syntax \u{2014} call as obj:set_name() not obj.set_name()
 --> test.lua:5:4
  |
5 | obj.set_name(42)
  |    ^ 'obj:set_name' uses ':' syntax \u{2014} call as obj:set_name() not obj.set_name()
stack traceback:
\ttest.lua:3: in function obj:set_name()
\ttest.lua:5: in main chunk"
    );
}

#[test]
fn no_hint_when_self_is_table() {
    // Correct colon call — no hint should appear.
    let src =
        "local obj = {}\nfunction obj:broken()\n    return self.missing + 1\nend\nobj:broken()";
    let re = run_runtime_error(src);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: attempt to perform arithmetic on a nil value
 --> test.lua:3:5
  |
3 |     return self.missing + 1
  |     ^^^^^^^^^^^^^^^^^^^^^^^ attempt to perform arithmetic on a nil value
stack traceback:
\ttest.lua:3: in function obj:broken()
\ttest.lua:5: in main chunk"
    );
}

#[test]
fn hint_colon_call_on_dot_function() {
    // Dot-defined function called with colon — the implicit `self` shifts params.
    let src = "local mod = {}\nfunction mod.add(a, b)\n    return a + b\nend\nmod:add(1, 2)";
    let re = run_runtime_error(src);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: attempt to perform arithmetic on local 'a' (a table value)
 --> test.lua:3:5
  |
3 |     return a + b
  |     ^^^^^^^^^^^^ attempt to perform arithmetic on local 'a' (a table value)
help: 'mod.add' uses '.' syntax — call as mod.add() not mod:add()
 --> test.lua:5:4
  |
5 | mod:add(1, 2)
  |    ^ 'mod.add' uses '.' syntax — call as mod.add() not mod:add()
stack traceback:
\ttest.lua:3: in function mod.add()
\ttest.lua:5: in main chunk"
    );
}

#[test]
fn hint_userdata_method_dot_call() {
    // Userdata method (arg_offset=1) called with `.` instead of `:`.
    use shingetsu::userdata;

    struct Counter(i64);

    #[userdata]
    impl Counter {
        fn type_name(&self) -> &'static str {
            "Counter"
        }

        #[lua_method]
        fn add(&self, n: i64) -> i64 {
            self.0 + n
        }
    }

    let env = common::new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(10))));
    let src = "return c.add(5)";
    let re = run_runtime_error_with_env(src, env);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: bad argument #1 to 'add' (integer expected, got nil)
 --> test.lua:1:1
  |
1 | return c.add(5)
  | ^^^^^^^^^^^^^^^ bad argument #1 to 'add' (integer expected, got nil)
help: 'add' uses ':' syntax — call as c:add() not c.add()
 --> test.lua:1:9
  |
1 | return c.add(5)
  |         ^ 'add' uses ':' syntax — call as c:add() not c.add()
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[test]
fn hint_userdata_method_correct_colon_call() {
    // Userdata method called correctly with `:` — no hint.
    use shingetsu::userdata;

    struct Counter(i64);

    #[userdata]
    impl Counter {
        fn type_name(&self) -> &'static str {
            "Counter"
        }

        #[lua_method]
        fn get_value(&self) -> i64 {
            self.0
        }

        #[lua_method]
        fn bad_add(&self, _n: i64) -> i64 {
            // Deliberately error to test that no hint appears.
            panic!("should not reach")
        }
    }

    let env = common::new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(10))));
    // Call with `:` but pass wrong arg type to trigger BadArgument.
    let src = r#"return c:bad_add("not a number")"#;
    let re = run_runtime_error_with_env(src, env);
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: bad argument #1 to 'bad_add' (integer expected, got string)
 --> test.lua:1:1
  |
1 | return c:bad_add(\"not a number\")
  | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'bad_add' (integer expected, got string)
stack traceback:
\ttest.lua:1: in main chunk"
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

fn warnings(src: &str) -> String {
    let opts = compile_opts();
    let bc = compile(src, &opts).expect("compile failed");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

#[test]
fn unused_variable_simple() {
    k9::assert_equal!(
        warnings("local x = 1"),
        "\
warning: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[test]
fn unused_variable_read_suppresses_warning() {
    k9::assert_equal!(warnings("local x = 1\nreturn x"), "");
}

#[test]
fn unused_variable_underscore_suppressed() {
    k9::assert_equal!(warnings("local _x = 1"), "");
}

#[test]
fn unused_variable_bare_underscore_suppressed() {
    k9::assert_equal!(warnings("local _ = 1"), "");
}

#[test]
fn unused_variable_assigned_but_not_read() {
    k9::assert_equal!(
        warnings("local x = 1\nx = 2"),
        "\
warning: variable 'x' is assigned to but never read
 --> test.lua:2:1
  |
2 | x = 2
  | ^ variable 'x' is assigned to but never read"
    );
}

#[test]
fn unused_variable_close_suppressed() {
    // <close> variables exist for their side effect; no warning expected.
    k9::assert_equal!(warnings("local f <close> = nil"), "");
}

#[test]
fn unused_variable_for_loop() {
    k9::assert_equal!(
        warnings("for i = 1, 10 do end"),
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | for i = 1, 10 do end
  | ^^^ - unused variable 'i'
  | |
  | empty loop body"
    );
}

#[test]
fn unused_variable_for_loop_underscore() {
    k9::assert_equal!(
        warnings("for _ = 1, 10 do end"),
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | for _ = 1, 10 do end
  | ^^^ empty loop body"
    );
}

#[test]
fn unused_variable_generic_for() {
    k9::assert_equal!(
        warnings("for k, v in pairs({}) do end"),
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | for k, v in pairs({}) do end
  | ^^^ -  - unused variable 'v'
  | |   |
  | |   unused variable 'k'
  | empty loop body"
    );
}

#[test]
fn unused_variable_generic_for_underscore_key() {
    k9::assert_equal!(warnings("for _, v in pairs({}) do\nreturn v\nend"), "");
}

#[test]
fn unused_variable_in_function() {
    k9::assert_equal!(
        warnings("local function foo()\nlocal x = 1\nend\nfoo()"),
        "\
warning: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[test]
fn unused_variable_captured_as_upvalue() {
    // x is captured by the closure — not unused.
    k9::assert_equal!(
        warnings("local x = 1\nlocal function foo()\nreturn x\nend\nreturn foo()"),
        ""
    );
}

#[test]
fn used_in_compound_assignment() {
    // x is read and written by +=, so it's read.
    k9::assert_equal!(warnings("local x = 1\nx += 1\nreturn x"), "");
}

#[test]
fn unused_local_function() {
    k9::assert_equal!(
        warnings("local function foo() end"),
        "\
warning: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo() end
  |                ^^^ unused function 'foo'"
    );
}

// ---------------------------------------------------------------------------
// Unreachable code warnings (D8c)
// ---------------------------------------------------------------------------

#[test]
fn unreachable_after_goto() {
    k9::assert_equal!(
        warnings("do\n::label::\ngoto label\nlocal x = 1\nend"),
        "\
warning: unreachable code
 --> test.lua:4:1
  |
4 | local x = 1
  | ^^^^^ - unused variable 'x'
  | |
  | unreachable code"
    );
}

#[test]
fn no_unreachable_warning_normal_flow() {
    k9::assert_equal!(warnings("local x = 1\nreturn x"), "");
}

// ---------------------------------------------------------------------------
// Same-scope shadowing warnings (D8d)
// ---------------------------------------------------------------------------

#[test]
fn shadow_same_scope() {
    k9::assert_equal!(
        warnings("local x = 1\nlocal x = 2"),
        "\
warning: variable 'x' shadows earlier declaration in same scope
 --> test.lua:2:7
  |
1 | local x = 1
  |       - unused variable 'x'
2 | local x = 2
  |       ^
  |       |
  |       variable 'x' shadows earlier declaration in same scope
  |       unused variable 'x'"
    );
}

#[test]
fn shadow_different_scope_no_warning() {
    // Outer-scope shadowing is normal Lua practice; only unused fires.
    k9::assert_equal!(
        warnings("local x = 1\ndo\nlocal x = 2\nreturn x\nend"),
        "\
warning: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[test]
fn shadow_underscore_suppressed() {
    k9::assert_equal!(warnings("local _x = 1\nlocal _x = 2"), "");
}

// ---------------------------------------------------------------------------
// Empty loop body warnings (D8e)
// ---------------------------------------------------------------------------

#[test]
fn empty_while_body() {
    k9::assert_equal!(
        warnings("while true do end"),
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | while true do end
  | ^^^^^ empty loop body"
    );
}

#[test]
fn empty_repeat_body() {
    k9::assert_equal!(
        warnings("repeat until true"),
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | repeat until true
  | ^^^^^^ empty loop body"
    );
}

#[test]
fn non_empty_while_no_warning() {
    k9::assert_equal!(warnings("while true do\nreturn 1\nend"), "");
}
