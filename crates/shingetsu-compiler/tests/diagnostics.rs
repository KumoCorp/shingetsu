mod common;

use std::sync::Arc;

use shingetsu::diagnostic::{
    render_compile_error, render_runtime_error, render_warning, render_warnings, RenderStyle,
};
use shingetsu_compiler::{CompileOptions, Compiler, Diagnostic, Severity, SourceLocation};
use shingetsu_vm::{Function, Task, Value};

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

async fn run_runtime_error(src: &str) -> shingetsu_vm::error::RuntimeError {
    run_runtime_error_with_env(src, common::new_env()).await
}

async fn run_runtime_error_with_env(
    src: &str,
    env: shingetsu_vm::GlobalEnv,
) -> shingetsu_vm::error::RuntimeError {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, vec![]).await.unwrap_err()
}

// ---------------------------------------------------------------------------
// Compile error diagnostics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compile_error_parse() {
    let src = "local x =\n";
    let compiler = Compiler::new(compile_opts(), Default::default());
    let err = compiler.compile(src).await.unwrap_err();
    let rendered = render_compile_error(&err, src, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: unexpected token `=`, expected an expression
 --> test.lua:1:9
  |
1 | local x =
  |         ^ unexpected token `=`, expected an expression"
    );
}

#[tokio::test]
async fn compile_error_semantic_break_outside_loop() {
    let src = "break\n";
    let compiler = Compiler::new(compile_opts(), Default::default());
    let err = compiler.compile(src).await.unwrap_err();
    let rendered = render_compile_error(&err, src, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: break outside loop
 --> test.lua:1:1
  |
1 | break
  | ^^^^^ break outside loop"
    );
}

// ---------------------------------------------------------------------------
// Runtime error diagnostics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_error_nil_call() {
    let re = run_runtime_error("local x = nil\nx()").await;
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

#[tokio::test]
async fn runtime_error_nil_call_with_reassignment() {
    let re = run_runtime_error("local x = 42\nx = nil\nx()").await;
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

#[tokio::test]
async fn runtime_error_in_function() {
    let re = run_runtime_error(
        "\
local function foo()
    error('boom')
end
foo()",
    )
    .await;
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

#[tokio::test]
async fn runtime_error_string_error() {
    let re = run_runtime_error("error('custom message')").await;
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

#[tokio::test]
async fn runtime_error_type_error_arithmetic() {
    let re = run_runtime_error("local x = 'hello'\nlocal y = x + 1").await;
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

#[tokio::test]
async fn stack_overflow_collapses_recursive_frames() {
    let src = "local function f() return f() end\nf()";
    let re = run_runtime_error(src).await;
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    // The 199 recursive f() calls should collapse into one line + repeat count.
    k9::assert_equal!(
        rendered,
        "\
error: stack overflow
 --> test.lua:1:27
  |
1 | local function f() return f() end
  |                           ^^^ stack overflow
stack traceback:
\ttest.lua:1: in function f()
\t... (repeated 198 times)
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn non_recursive_short_trace_not_truncated() {
    // A short call chain should appear in full without truncation.
    let src = "\
local function a() error('boom') end
local function b() a() end
local function c() b() end
c()";
    let re = run_runtime_error(src).await;
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

#[tokio::test]
async fn hint_dot_call_on_colon_method() {
    // Calling a :-defined method with . passes the wrong self.
    let src = "local obj = {}\nfunction obj:greet(greeting)\n    return greeting .. ' ' .. self.name\nend\nobj.greet('hello')";
    let re = run_runtime_error(src).await;
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

#[tokio::test]
async fn hint_dot_call_self_is_number() {
    // self becomes a number when dot-called.
    let src =
        "local obj = {}\nfunction obj:set_name(name)\n    self.name = name\nend\nobj.set_name(42)";
    let re = run_runtime_error(src).await;
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

#[tokio::test]
async fn no_hint_when_self_is_table() {
    // Correct colon call — no hint should appear.
    let src =
        "local obj = {}\nfunction obj:broken()\n    return self.missing + 1\nend\nobj:broken()";
    let re = run_runtime_error(src).await;
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

#[tokio::test]
async fn hint_colon_call_on_dot_function() {
    // Dot-defined function called with colon — the implicit `self` shifts params.
    let src = "local mod = {}\nfunction mod.add(a, b)\n    return a + b\nend\nmod:add(1, 2)";
    let re = run_runtime_error(src).await;
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

#[tokio::test]
async fn hint_userdata_method_dot_call() {
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
    let re = run_runtime_error_with_env(src, env).await;
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: bad argument #1 to 'add' (integer expected, got nil)
 --> test.lua:1:8
  |
1 | return c.add(5)
  |        ^^^^^^^^ bad argument #1 to 'add' (integer expected, got nil)
help: 'add' uses ':' syntax — call as c:add() not c.add()
 --> test.lua:1:9
  |
1 | return c.add(5)
  |         ^ 'add' uses ':' syntax — call as c:add() not c.add()
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn hint_userdata_method_correct_colon_call() {
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
    let re = run_runtime_error_with_env(src, env).await;
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    k9::assert_equal!(
        rendered,
        "\
error: bad argument #1 to 'bad_add' (integer expected, got string)
 --> test.lua:1:8
  |
1 | return c:bad_add(\"not a number\")
  |        ^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'bad_add' (integer expected, got string)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn compile_error_colored() {
    let src = "local x =\n";
    let compiler = Compiler::new(compile_opts(), Default::default());
    let err = compiler.compile(src).await.unwrap_err();
    let rendered = render_compile_error(&err, src, RenderStyle::Colored);
    k9::assert_equal!(
        rendered,
        "\
\u{1b}[1m\u{1b}[91merror\u{1b}[0m\u{1b}[1m: unexpected token `=`, expected an expression\u{1b}[0m
 \u{1b}[1m\u{1b}[94m--> \u{1b}[0mtest.lua:1:9
  \u{1b}[1m\u{1b}[94m|\u{1b}[0m
\u{1b}[1m\u{1b}[94m1\u{1b}[0m \u{1b}[1m\u{1b}[94m|\u{1b}[0m local x =
  \u{1b}[1m\u{1b}[94m|\u{1b}[0m         \u{1b}[1m\u{1b}[91m^\u{1b}[0m \u{1b}[1m\u{1b}[91munexpected token `=`, expected an expression\u{1b}[0m"
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

async fn warnings(src: &str) -> String {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

#[tokio::test]
async fn unused_variable_simple() {
    k9::assert_equal!(
        warnings("local x = 1").await,
        "\
warning: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[tokio::test]
async fn unused_variable_read_suppresses_warning() {
    k9::assert_equal!(warnings("local x = 1\nreturn x").await, "");
}

#[tokio::test]
async fn unused_variable_underscore_suppressed() {
    k9::assert_equal!(warnings("local _x = 1").await, "");
}

#[tokio::test]
async fn unused_variable_bare_underscore_suppressed() {
    k9::assert_equal!(warnings("local _ = 1").await, "");
}

#[tokio::test]
async fn unused_variable_assigned_but_not_read() {
    k9::assert_equal!(
        warnings("local x = 1\nx = 2").await,
        "\
warning: variable 'x' is assigned to but never read
 --> test.lua:2:1
  |
2 | x = 2
  | ^ variable 'x' is assigned to but never read"
    );
}

#[tokio::test]
async fn unused_variable_close_suppressed() {
    // <close> variables exist for their side effect; no warning expected.
    k9::assert_equal!(warnings("local f <close> = nil").await, "");
}

#[tokio::test]
async fn unused_variable_for_loop() {
    k9::assert_equal!(
        warnings("for i = 1, 10 do end").await,
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

#[tokio::test]
async fn unused_variable_for_loop_underscore() {
    k9::assert_equal!(
        warnings("for _ = 1, 10 do end").await,
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | for _ = 1, 10 do end
  | ^^^ empty loop body"
    );
}

#[tokio::test]
async fn unused_variable_generic_for() {
    k9::assert_equal!(
        warnings("for k, v in pairs({}) do end").await,
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

#[tokio::test]
async fn unused_variable_generic_for_underscore_key() {
    k9::assert_equal!(
        warnings("for _, v in pairs({}) do\nreturn v\nend").await,
        ""
    );
}

#[tokio::test]
async fn unused_variable_in_function() {
    k9::assert_equal!(
        warnings("local function foo()\nlocal x = 1\nend\nfoo()").await,
        "\
warning: unused variable 'x'
 --> test.lua:2:7
  |
2 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[tokio::test]
async fn unused_variable_captured_as_upvalue() {
    // x is captured by the closure — not unused.
    k9::assert_equal!(
        warnings("local x = 1\nlocal function foo()\nreturn x\nend\nreturn foo()").await,
        ""
    );
}

#[tokio::test]
async fn used_in_compound_assignment() {
    // x is read and written by +=, so it's read.
    k9::assert_equal!(warnings("local x = 1\nx += 1\nreturn x").await, "");
}

#[tokio::test]
async fn unused_local_function() {
    k9::assert_equal!(
        warnings("local function foo() end").await,
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

#[tokio::test]
async fn unreachable_after_goto() {
    k9::assert_equal!(
        warnings("do\n::label::\ngoto label\nlocal x = 1\nend").await,
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

#[tokio::test]
async fn no_unreachable_warning_normal_flow() {
    k9::assert_equal!(warnings("local x = 1\nreturn x").await, "");
}

// ---------------------------------------------------------------------------
// Same-scope shadowing warnings (D8d)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shadow_same_scope() {
    k9::assert_equal!(
        warnings("local x = 1\nlocal x = 2").await,
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

#[tokio::test]
async fn shadow_different_scope_no_warning() {
    // Outer-scope shadowing is normal Lua practice; only unused fires.
    k9::assert_equal!(
        warnings("local x = 1\ndo\nlocal x = 2\nreturn x\nend").await,
        "\
warning: unused variable 'x'
 --> test.lua:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'"
    );
}

#[tokio::test]
async fn shadow_underscore_suppressed() {
    k9::assert_equal!(warnings("local _x = 1\nlocal _x = 2").await, "");
}

// ---------------------------------------------------------------------------
// Empty loop body warnings (D8e)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_while_body() {
    k9::assert_equal!(
        warnings("while true do end").await,
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | while true do end
  | ^^^^^ empty loop body"
    );
}

#[tokio::test]
async fn empty_repeat_body() {
    k9::assert_equal!(
        warnings("repeat until true").await,
        "\
warning: empty loop body
 --> test.lua:1:1
  |
1 | repeat until true
  | ^^^^^^ empty loop body"
    );
}

#[tokio::test]
async fn non_empty_while_no_warning() {
    k9::assert_equal!(warnings("while true do\nreturn 1\nend").await, "");
}

// ---------------------------------------------------------------------------
// Dot-vs-colon call syntax mismatch warnings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dot_colon_method_called_with_dot() {
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t:method() return self end\n\
             t.method()"
        )
        .await,
        "\
warning: 'method' was defined with ':' syntax but called as 't.method()'; did you mean 't:method()'?
 --> test.lua:3:2
  |
3 | t.method()
  |  ^ 'method' was defined with ':' syntax but called as 't.method()'; did you mean 't:method()'?"
    );
}

#[tokio::test]
async fn dot_colon_function_called_with_colon() {
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t.func() end\n\
             t:func()"
        )
        .await,
        "\
warning: 'func' was defined with '.' syntax but called as 't:func()'; did you mean 't.func()'?
 --> test.lua:3:2
  |
3 | t:func()
  |  ^ 'func' was defined with '.' syntax but called as 't:func()'; did you mean 't.func()'?"
    );
}

#[tokio::test]
async fn dot_colon_correct_syntax_no_warning() {
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t:method() return self end\n\
             t:method()"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn dot_colon_dot_syntax_correct_no_warning() {
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t.func() end\n\
             t.func()"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn dot_colon_method_called_with_dot_explicit_self_no_warning() {
    // t.method(t) is the manual equivalent of t:method() — no warning.
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t:method() return self end\n\
             t.method(t)"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn dot_colon_no_definition_no_warning() {
    // No function was defined on t, so no warning.
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             t.foo()"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn dot_colon_method_called_with_dot_explicit_self_plus_args_no_warning() {
    // t.method(t, arg) is the manual equivalent of t:method(arg) — no warning.
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t:method() return self end\n\
             t.method(t, 42)"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn dot_colon_method_called_with_dot_wrong_receiver_warns() {
    // t.method(other) — first arg is not the receiver, so this is likely a bug.
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             local other = {}\n\
             function t:method() return self end\n\
             t.method(other)"
        )
        .await,
        "\
warning: 'method' was defined with ':' syntax but called as 't.method()'; did you mean 't:method()'?
 --> test.lua:4:2
  |
4 | t.method(other)
  |  ^ 'method' was defined with ':' syntax but called as 't.method()'; did you mean 't:method()'?"
    );
}

#[tokio::test]
async fn dot_colon_redefinition_overwrites_syntax() {
    // Redefining with the opposite syntax updates the record.
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t:foo() return self end\n\
             function t.foo() end\n\
             t.foo()"
        )
        .await,
        ""
    );
}

#[tokio::test]
async fn dot_colon_multiple_fields_independent() {
    // Each field is tracked independently: meth uses ':', func uses '.'.
    // Calling meth with '.' warns; calling func with '.' is fine.
    k9::assert_equal!(
        warnings(
            "local t = {}\n\
             function t:meth() return self end\n\
             function t.func() end\n\
             t.func()\n\
             t.meth()"
        )
        .await,
        "\
warning: 'meth' was defined with ':' syntax but called as 't.meth()'; did you mean 't:meth()'?
 --> test.lua:5:2
  |
5 | t.meth()
  |  ^ 'meth' was defined with ':' syntax but called as 't.meth()'; did you mean 't:meth()'?"
    );
}

#[tokio::test]
async fn dot_colon_global_table_no_warning() {
    // Global tables don't have field_defs tracked (same-scope locals only).
    k9::assert_equal!(
        warnings(
            "function t.func() end\n\
             t:func()"
        )
        .await,
        ""
    );
}

// ---------------------------------------------------------------------------
// Dot-vs-colon warnings from global type map
// ---------------------------------------------------------------------------

use shingetsu_vm::types::{FunctionLuaType, TableLuaType};
use shingetsu_vm::{GlobalTypeMap, LuaType};

/// Build a `Compiler` with a global type map that includes a module `modname`
/// with the given fields.
fn compiler_with_module(modname: &str, fields: Vec<(bytes::Bytes, LuaType)>) -> Compiler {
    let mut map = GlobalTypeMap::default();
    map.types.insert(
        bytes::Bytes::copy_from_slice(modname.as_bytes()),
        LuaType::Table(Box::new(TableLuaType {
            fields,
            indexer: None,
        })),
    );
    Compiler::new(compile_opts(), map)
}

async fn warnings_with_compiler(compiler: &Compiler, src: &str) -> String {
    let bc = compiler.compile(src).await.expect("compile failed");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

#[tokio::test]
async fn global_method_called_with_dot_warns() {
    let compiler = compiler_with_module(
        "mymod",
        vec![(
            bytes::Bytes::from_static(b"greet"),
            LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![(Some(bytes::Bytes::from_static(b"name")), LuaType::String)],
                variadic: None,
                returns: vec![LuaType::String],
                is_method: true,
            })),
        )],
    );
    k9::assert_equal!(
        warnings_with_compiler(&compiler, "mymod.greet('world')").await,
        "\
warning: 'greet' was defined with ':' syntax but called as 'mymod.greet()'; did you mean 'mymod:greet()'?
 --> test.lua:1:6
  |
1 | mymod.greet('world')
  |      ^ 'greet' was defined with ':' syntax but called as 'mymod.greet()'; did you mean 'mymod:greet()'?"
    );
}

#[tokio::test]
async fn global_function_called_with_colon_warns() {
    let compiler = compiler_with_module(
        "mymod",
        vec![(
            bytes::Bytes::from_static(b"run"),
            LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![],
                variadic: None,
                returns: vec![],
                is_method: false,
            })),
        )],
    );
    k9::assert_equal!(
        warnings_with_compiler(&compiler, "mymod:run()").await,
        "\
warning: 'run' was defined with '.' syntax but called as 'mymod:run()'; did you mean 'mymod.run()'?
 --> test.lua:1:6
  |
1 | mymod:run()
  |      ^ 'run' was defined with '.' syntax but called as 'mymod:run()'; did you mean 'mymod.run()'?"
    );
}

#[tokio::test]
async fn global_correct_syntax_no_warning() {
    let compiler = compiler_with_module(
        "mymod",
        vec![
            (
                bytes::Bytes::from_static(b"greet"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: None,
                    returns: vec![],
                    is_method: true,
                })),
            ),
            (
                bytes::Bytes::from_static(b"run"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: None,
                    returns: vec![],
                    is_method: false,
                })),
            ),
        ],
    );
    k9::assert_equal!(
        warnings_with_compiler(&compiler, "mymod:greet()\nmymod.run()").await,
        ""
    );
}

#[tokio::test]
async fn global_unknown_field_no_warning() {
    let compiler = compiler_with_module(
        "mymod",
        vec![(
            bytes::Bytes::from_static(b"greet"),
            LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![],
                variadic: None,
                returns: vec![],
                is_method: true,
            })),
        )],
    );
    // Calling a field not in the type map should not warn.
    k9::assert_equal!(
        warnings_with_compiler(&compiler, "mymod.unknown()").await,
        ""
    );
}

#[tokio::test]
async fn global_method_called_with_dot_explicit_self_no_warning() {
    let compiler = compiler_with_module(
        "mymod",
        vec![(
            bytes::Bytes::from_static(b"greet"),
            LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![],
                variadic: None,
                returns: vec![],
                is_method: true,
            })),
        )],
    );
    // Explicit self-passing: mymod.greet(mymod) should not warn.
    k9::assert_equal!(
        warnings_with_compiler(&compiler, "mymod.greet(mymod)").await,
        ""
    );
}

// ---------------------------------------------------------------------------
// require error diagnostics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_error_require_not_found() {
    use shingetsu_vm::GlobalEnv;

    let dir = tempfile::tempdir().expect("tempdir");

    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::PACKAGE,
    )
    .expect("register");
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    env.set_package_path(Some(search));

    let compiler = Compiler::new(compile_opts(), env.global_type_map());
    let bc = compiler
        .compile("local m = require('noexist')\nreturn m")
        .await
        .expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let re = Task::new(env, func, vec![]).await.unwrap_err();
    let rendered = render_runtime_error(&re, RenderStyle::Plain);
    let stable = rendered.replace(&format!("{}", dir.path().display()), "TMPDIR");
    k9::assert_equal!(
        stable,
        concat!(
            "error: error in 'require': module 'noexist' not found:\n",
            "           no field package.preload['noexist']\n",
            "           TMPDIR/noexist.lua: No such file or directory\n",
            " --> test.lua:1:11\n",
            "  |\n",
            "1 | local m = require('noexist')\n",
            "  |           ^^^^^^^^^^^^^^^^^^ error in 'require': module 'noexist' not found: ...\n",
            "stack traceback:\n",
            "\ttest.lua:1: in main chunk",
        )
    );
}

// ---------------------------------------------------------------------------
// Typed locals: dot-vs-colon checking via inferred_type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn typed_local_method_called_with_dot_warns() {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let src = "\
type MyMod = { greet: (self: MyMod, name: string) -> string }
local m: MyMod = {}
m.greet('world')";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'greet' was defined with ':' syntax but called as 'm.greet()'; did you mean 'm:greet()'?\n",
            " --> test.lua:3:2\n",
            "  |\n",
            "3 | m.greet('world')\n",
            "  |  ^ 'greet' was defined with ':' syntax but called as 'm.greet()'; did you mean 'm:greet()'?",
        )
    );
}

#[tokio::test]
async fn typed_local_function_called_with_colon_warns() {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let src = "\
type Utils = { add: (a: number, b: number) -> number }
local u: Utils = {}
u:add(1, 2)";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'add' was defined with '.' syntax but called as 'u:add()'; did you mean 'u.add()'?\n",
            " --> test.lua:3:2\n",
            "  |\n",
            "3 | u:add(1, 2)\n",
            "  |  ^ 'add' was defined with '.' syntax but called as 'u:add()'; did you mean 'u.add()'?",
        )
    );
}

#[tokio::test]
async fn typed_local_correct_call_no_warning() {
    let compiler = Compiler::new(compile_opts(), Default::default());
    let src = "\
type MyMod = { greet: (self: MyMod, name: string) -> string }
local m: MyMod = {}
m:greet('world')";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(warnings, "");
}

#[tokio::test]
async fn typed_local_from_global_method_called_with_dot_warns() {
    // `local m = mymod` should propagate the global's type to the local,
    // enabling dot-vs-colon checking on the local.
    let compiler = compiler_with_module(
        "mymod",
        vec![(
            bytes::Bytes::from_static(b"greet"),
            LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![(Some(bytes::Bytes::from_static(b"name")), LuaType::String)],
                variadic: None,
                returns: vec![LuaType::String],
                is_method: true,
            })),
        )],
    );
    let src = "local m = mymod\nm.greet('world')";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'greet' was defined with ':' syntax but called as 'm.greet()'; did you mean 'm:greet()'?\n",
            " --> test.lua:2:2\n",
            "  |\n",
            "2 | m.greet('world')\n",
            "  |  ^ 'greet' was defined with ':' syntax but called as 'm.greet()'; did you mean 'm:greet()'?",
        )
    );
}

#[tokio::test]
async fn require_imports_exported_types() {
    use shingetsu_vm::types::{
        FunctionLuaType, ModuleTypeInfo, ModuleTypeRegistry, TableLuaType, TypeAlias,
    };

    // Build a module type registry with a module "mylib" that exports a type.
    let mut registry = ModuleTypeRegistry::default();
    registry.insert(
        "mylib",
        ModuleTypeInfo {
            exported_types: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    bytes::Bytes::from("MyObj"),
                    TypeAlias {
                        params: vec![],
                        body: LuaType::Table(Box::new(TableLuaType {
                            fields: vec![(
                                bytes::Bytes::from("run"),
                                LuaType::Function(Box::new(FunctionLuaType {
                                    type_params: vec![],
                                    params: vec![(Some(bytes::Bytes::from("self")), LuaType::Any)],
                                    variadic: None,
                                    returns: vec![],
                                    is_method: true,
                                })),
                            )],
                            indexer: None,
                        })),
                        exported: true,
                    },
                );
                m
            },
            return_type: None,
        },
    );

    let compiler = Compiler::new(compile_opts(), Default::default()).with_module_types(registry);
    // After require, the exported type "MyObj" should be available as a type alias.
    let src = "\
local _M = require('mylib')
local obj: MyObj = {}
obj.run()";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    // obj.run() should warn: 'run' is a method (has self), called with dot.
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'run' was defined with ':' syntax but called as 'obj.run()'; did you mean 'obj:run()'?\n",
            " --> test.lua:3:4\n",
            "  |\n",
            "3 | obj.run()\n",
            "  |    ^ 'run' was defined with ':' syntax but called as 'obj.run()'; did you mean 'obj:run()'?",
        )
    );
}

// ---------------------------------------------------------------------------
// Demand-driven require resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn require_demand_driven_resolves_types() {
    // Write a module file that exports a type and a return type.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("greeter.lua"),
        r#"
export type Greeter = { greet: (self: Greeter) -> string }
local M: Greeter = {}
function M:greet() return "hello" end
return M
"#,
    )
    .expect("write");

    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    // The compiler should resolve the require on-demand and import
    // the type info so that the dot-vs-colon warning fires.
    let src = "\
local G = require('greeter')
G.greet()";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'greet' was defined with ':' syntax but called as 'G.greet()'; did you mean 'G:greet()'?\n",
            " --> <string>:2:2\n",
            "  |\n",
            "2 | G.greet()\n",
            "  |  ^ 'greet' was defined with ':' syntax but called as 'G.greet()'; did you mean 'G:greet()'?",
        )
    );
}

#[tokio::test]
async fn require_demand_driven_missing_module_silent() {
    // When the module file doesn't exist, demand-driven resolution
    // silently skips — no compile error, just no type info.
    let dir = tempfile::tempdir().expect("tempdir");
    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    let src = "local M = require('noexist')\nreturn M";
    let bc = compiler.compile(src).await.expect("compile");
    // No warnings or errors — the missing module is silently skipped.
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(warnings, "");
}

#[tokio::test]
async fn require_demand_driven_circular_silent() {
    // Circular requires should not infinite-loop; the second
    // require returns no type info.
    let dir = tempfile::tempdir().expect("tempdir");
    // Module A requires module B, module B requires module A.
    std::fs::write(
        dir.path().join("mod_a.lua"),
        "local B = require('mod_b')\nreturn {}",
    )
    .expect("write");
    std::fs::write(
        dir.path().join("mod_b.lua"),
        "local A = require('mod_a')\nreturn {}",
    )
    .expect("write");

    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    // Should compile without panic or infinite loop.
    let src = "local A = require('mod_a')\nreturn A";
    let _bc = compiler.compile(src).await.expect("compile");
}

#[tokio::test]
async fn require_demand_driven_caches_across_calls() {
    // Two require calls for the same module in one file should only
    // compile the module once (cache hit on second call).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("counter.lua"),
        r#"
export type Counter = { inc: (self: Counter) -> () }
return {}
"#,
    )
    .expect("write");

    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    // Both lines import the same module; the exported type should
    // be available after both.
    let src = "\
local _A = require('counter')
local _B = require('counter')
local c: Counter = {}
c.inc()";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'inc' was defined with ':' syntax but called as 'c.inc()'; did you mean 'c:inc()'?\n",
            " --> <string>:4:2\n",
            "  |\n",
            "4 | c.inc()\n",
            "  |  ^ 'inc' was defined with ':' syntax but called as 'c.inc()'; did you mean 'c:inc()'?",
        )
    );
}

#[tokio::test]
async fn require_demand_driven_no_loader_silent() {
    // type_check enabled but no module loader — require should
    // silently produce no type info.
    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    );

    let src = "local M = require('anything')\nreturn M";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(warnings, "");
}

#[tokio::test]
async fn require_demand_driven_no_package_path_silent() {
    // Loader set but no package_path — require should silently skip.
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader);

    let src = "local M = require('anything')\nreturn M";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(warnings, "");
}

#[tokio::test]
async fn require_demand_driven_dotted_name() {
    // require("foo.bar") should resolve to foo/bar.lua via candidate_paths.
    let dir = tempfile::tempdir().expect("tempdir");
    let sub = dir.path().join("foo");
    std::fs::create_dir(&sub).expect("mkdir");
    std::fs::write(
        sub.join("bar.lua"),
        r#"
export type Widget = { click: (self: Widget) -> () }
return {}
"#,
    )
    .expect("write");

    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    let src = "\
local _W = require('foo.bar')
local w: Widget = {}
w.click()";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'click' was defined with ':' syntax but called as 'w.click()'; did you mean 'w:click()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | w.click()\n",
            "  |  ^ 'click' was defined with ':' syntax but called as 'w.click()'; did you mean 'w:click()'?",
        )
    );
}

#[tokio::test]
async fn require_demand_driven_compile_error_silent() {
    // If the required module has a syntax error, the loader returns
    // an error and demand-driven resolution silently skips.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("broken.lua"),
        "local x =\n", // parse error
    )
    .expect("write");

    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    let src = "local M = require('broken')\nreturn M";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(warnings, "");
}

#[tokio::test]
async fn require_demand_driven_second_path_candidate() {
    // First template doesn't match, second does.
    let dir = tempfile::tempdir().expect("tempdir");
    let libdir = dir.path().join("libs");
    std::fs::create_dir(&libdir).expect("mkdir");
    std::fs::write(
        libdir.join("util.lua"),
        r#"
export type Util = { run: (self: Util) -> () }
return {}
"#,
    )
    .expect("write");

    // First template points at dir root (no file there), second at libs/.
    let search = format!(
        "{}{}?.lua;{}{}?.lua",
        dir.path().display(),
        std::path::MAIN_SEPARATOR,
        libdir.display(),
        std::path::MAIN_SEPARATOR,
    );
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> = std::sync::Arc::new(
        shingetsu::module_loader::LuaModuleLoader::new(Default::default()),
    );

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    let src = "\
local _U = require('util')
local u: Util = {}
u.run()";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'run' was defined with ':' syntax but called as 'u.run()'; did you mean 'u:run()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | u.run()\n",
            "  |  ^ 'run' was defined with ':' syntax but called as 'u.run()'; did you mean 'u:run()'?",
        )
    );
}

#[tokio::test]
async fn require_demand_driven_return_type_propagation() {
    // The module's return type should flow into the local receiving
    // the require result, enabling dot-vs-colon checking via the
    // inferred type — even without same-scope field_defs.
    let dir = tempfile::tempdir().expect("tempdir");
    // This module defines `greet` as a method (colon syntax) and
    // the return type is inferred from the type annotation on M.
    std::fs::write(
        dir.path().join("person.lua"),
        "\
export type Person = { greet: (self: Person, name: string) -> string }
local M: Person = {}
function M:greet(name) return 'hi ' .. name end
return M",
    )
    .expect("write");

    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    // Calling with dot when it was defined with colon should fire
    // a dot-vs-colon warning.  This relies on the return_type being
    // propagated (not exported_types / field_defs).
    let src = "\
local P = require('person')
P.greet('Alice')";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'greet' was defined with ':' syntax but called as 'P.greet()'; did you mean 'P:greet()'?\n",
            " --> <string>:2:2\n",
            "  |\n",
            "2 | P.greet('Alice')\n",
            "  |  ^ 'greet' was defined with ':' syntax but called as 'P.greet()'; did you mean 'P:greet()'?",
        )
    );
}

#[tokio::test]
async fn require_demand_driven_chained_modules() {
    // A requires B, B requires C. C's exported type should be
    // available in A after both modules are demand-resolved.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("leaf.lua"),
        "\
export type Leaf = { value: (self: Leaf) -> number }
return {}",
    )
    .expect("write");
    std::fs::write(
        dir.path().join("middle.lua"),
        "\
local _L = require('leaf')
export type Middle = { process: (self: Middle) -> () }
return {}",
    )
    .expect("write");

    let search = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(search);

    // Requiring middle should also resolve leaf; middle's exported
    // type should be available in the root script.
    let src = "\
local _M = require('middle')
local m: Middle = {}
m.process()";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'process' was defined with ':' syntax but called as 'm.process()'; did you mean 'm:process()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | m.process()\n",
            "  |  ^ 'process' was defined with ':' syntax but called as 'm.process()'; did you mean 'm:process()'?",
        )
    );
}

#[tokio::test]
async fn require_demand_driven_non_literal_skipped() {
    // require(variable) should be silently skipped — no crash,
    // no type info.
    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    );

    let src = "\
local name = 'foo'
local _M = require(name)
return _M";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(warnings, "");
}

// ---------------------------------------------------------------------------
// package.path mutation tracking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn package_path_simple_replacement() {
    // `package.path = "new/path/?.lua"` should update the search path
    // used by subsequent require calls.
    let dir = tempfile::tempdir().expect("tempdir");
    let subdir = dir.path().join("libs");
    std::fs::create_dir(&subdir).expect("mkdir");
    std::fs::write(
        subdir.join("helper.lua"),
        "\
export type Helper = { run: (self: Helper) -> () }
local M: Helper = {}
function M:run() end
return M",
    )
    .expect("write");

    // Initial package.path does NOT include the libs subdir.
    let initial_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let libs_path = format!("{}{}?.lua", subdir.display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(initial_path);

    // The script replaces package.path to include the libs dir,
    // then requires a module from it.
    let src = format!(
        "\
package.path = '{libs_path}'
local H = require('helper')
H.run()",
    );
    let bc = compiler.compile(&src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, &src, RenderStyle::Plain);
    // H.run() is a method called with dot syntax — should produce a warning.
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'run' was defined with ':' syntax but called as 'H.run()'; did you mean 'H:run()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | H.run()\n",
            "  |  ^ 'run' was defined with ':' syntax but called as 'H.run()'; did you mean 'H:run()'?",
        )
    );
}

#[tokio::test]
async fn package_path_append_with_concat() {
    // `package.path = package.path .. ";new/path"` should append to
    // the search path.
    let dir = tempfile::tempdir().expect("tempdir");
    let subdir = dir.path().join("extra");
    std::fs::create_dir(&subdir).expect("mkdir");
    std::fs::write(
        subdir.join("addon.lua"),
        "\
export type Addon = { init: (self: Addon) -> () }
local M: Addon = {}
function M:init() end
return M",
    )
    .expect("write");

    let initial_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let extra_path = format!("{}{}?.lua", subdir.display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(initial_path);

    // Append the extra path with `..` concatenation.
    let src = format!(
        "\
package.path = package.path .. ';{extra_path}'
local A = require('addon')
A.init()",
    );
    let bc = compiler.compile(&src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, &src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'init' was defined with ':' syntax but called as 'A.init()'; did you mean 'A:init()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | A.init()\n",
            "  |  ^ 'init' was defined with ':' syntax but called as 'A.init()'; did you mean 'A:init()'?",
        )
    );
}

#[tokio::test]
async fn package_path_prepend_with_concat() {
    // `package.path = "new/path" .. package.path` should prepend to
    // the search path.
    let dir = tempfile::tempdir().expect("tempdir");
    let subdir = dir.path().join("first");
    std::fs::create_dir(&subdir).expect("mkdir");
    std::fs::write(
        subdir.join("priority.lua"),
        "\
export type Priority = { exec: (self: Priority) -> () }
local M: Priority = {}
function M:exec() end
return M",
    )
    .expect("write");

    let initial_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let first_path = format!("{}{}?.lua", subdir.display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(initial_path);

    // Prepend the new path.
    let src = format!(
        "\
package.path = '{first_path};' .. package.path
local P = require('priority')
P.exec()",
    );
    let bc = compiler.compile(&src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, &src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'exec' was defined with ':' syntax but called as 'P.exec()'; did you mean 'P:exec()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | P.exec()\n",
            "  |  ^ 'exec' was defined with ':' syntax but called as 'P.exec()'; did you mean 'P:exec()'?",
        )
    );
}

#[tokio::test]
async fn package_path_compound_concat() {
    // `package.path ..= ";new/path"` should append to the search path.
    let dir = tempfile::tempdir().expect("tempdir");
    let subdir = dir.path().join("plugins");
    std::fs::create_dir(&subdir).expect("mkdir");
    std::fs::write(
        subdir.join("plugin.lua"),
        "\
export type Plugin = { start: (self: Plugin) -> () }
local M: Plugin = {}
function M:start() end
return M",
    )
    .expect("write");

    let initial_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let plugin_path = format!("{}{}?.lua", subdir.display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(initial_path);

    // Append via compound assignment.
    let src = format!(
        "\
package.path ..= ';{plugin_path}'
local P = require('plugin')
P.start()",
    );
    let bc = compiler.compile(&src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, &src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'start' was defined with ':' syntax but called as 'P.start()'; did you mean 'P:start()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | P.start()\n",
            "  |  ^ 'start' was defined with ':' syntax but called as 'P.start()'; did you mean 'P:start()'?",
        )
    );
}

#[tokio::test]
async fn package_path_multiple_mutations() {
    // Sequential mutations should accumulate correctly.
    let dir = tempfile::tempdir().expect("tempdir");
    let subdir = dir.path().join("final");
    std::fs::create_dir(&subdir).expect("mkdir");
    std::fs::write(
        subdir.join("target.lua"),
        "\
export type Target = { fire: (self: Target) -> () }
local M: Target = {}
function M:fire() end
return M",
    )
    .expect("write");

    let initial_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let final_path = format!("{}{}?.lua", subdir.display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(initial_path);

    // First replace, then append — the final path should include both.
    let src = format!(
        "\
package.path = '{final_path}'
package.path = package.path .. ';/bogus/?.lua'
local T = require('target')
T.fire()",
    );
    let bc = compiler.compile(&src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, &src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'fire' was defined with ':' syntax but called as 'T.fire()'; did you mean 'T:fire()'?\n",
            " --> <string>:4:2\n",
            "  |\n",
            "4 | T.fire()\n",
            "  |  ^ 'fire' was defined with ':' syntax but called as 'T.fire()'; did you mean 'T:fire()'?",
        )
    );
}

#[tokio::test]
async fn package_path_non_static_rhs_ignored() {
    // `package.path = get_path()` should silently leave the
    // effective path unchanged (no crash, no type resolution).
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("widget.lua"),
        "\
export type Widget = { draw: (self: Widget) -> () }
local M: Widget = {}
function M:draw() end
return M",
    )
    .expect("write");

    let initial_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(initial_path.clone());

    // Overwrite with a dynamic value — original path should still work.
    let src = "\
package.path = get_path()
local W = require('widget')
W.draw()";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    // Non-static RHS means effective_package_path is unchanged;
    // widget.lua is on the initial path so it should still resolve.
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'draw' was defined with ':' syntax but called as 'W.draw()'; did you mean 'W:draw()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | W.draw()\n",
            "  |  ^ 'draw' was defined with ':' syntax but called as 'W.draw()'; did you mean 'W:draw()'?",
        )
    );
}

#[tokio::test]
async fn package_path_no_initial_path_with_replacement() {
    // No `with_package_path` configured, but the script sets one.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("fresh.lua"),
        "\
export type Fresh = { go: (self: Fresh) -> () }
local M: Fresh = {}
function M:go() end
return M",
    )
    .expect("write");

    let fresh_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    // No with_package_path — effective_package_path starts as None.
    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader);

    // Script sets package.path from scratch.
    let src = format!(
        "\
package.path = '{fresh_path}'
local F = require('fresh')
F.go()",
    );
    let bc = compiler.compile(&src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, &src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'go' was defined with ':' syntax but called as 'F.go()'; did you mean 'F:go()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | F.go()\n",
            "  |  ^ 'go' was defined with ':' syntax but called as 'F.go()'; did you mean 'F:go()'?",
        )
    );
}

#[tokio::test]
async fn package_path_multi_part_concat() {
    // `package.path = "prefix" .. ";" .. package.path` with multiple
    // concat operands chained together.
    let dir = tempfile::tempdir().expect("tempdir");
    let subdir = dir.path().join("chain");
    std::fs::create_dir(&subdir).expect("mkdir");
    std::fs::write(
        subdir.join("link.lua"),
        "\
export type Link = { click: (self: Link) -> () }
local M: Link = {}
function M:click() end
return M",
    )
    .expect("write");

    let initial_path = format!("{}{}?.lua", dir.path().display(), std::path::MAIN_SEPARATOR);
    let chain_path = format!("{}{}?.lua", subdir.display(), std::path::MAIN_SEPARATOR);
    let loader: std::sync::Arc<dyn shingetsu_vm::ModuleLoader> =
        std::sync::Arc::new(shingetsu::module_loader::LuaModuleLoader::new(
            Default::default(),
        ));

    let compiler = Compiler::new(
        CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        },
        Default::default(),
    )
    .with_module_loader(loader)
    .with_package_path(initial_path);

    // Three-part concat: "new;" .. package.path .. ";extra"
    // (Lua `..` is right-associative, so this parses as
    //  "new;" .. (package.path .. ";extra"))
    let src = format!(
        "\
package.path = '{chain_path};' .. package.path .. ';/dummy/?.lua'
local L = require('link')
L.click()",
    );
    let bc = compiler.compile(&src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, &src, RenderStyle::Plain);
    k9::assert_equal!(
        warnings,
        concat!(
            "warning: 'click' was defined with ':' syntax but called as 'L.click()'; did you mean 'L:click()'?\n",
            " --> <string>:3:2\n",
            "  |\n",
            "3 | L.click()\n",
            "  |  ^ 'click' was defined with ':' syntax but called as 'L.click()'; did you mean 'L:click()'?",
        )
    );
}

// ---------------------------------------------------------------------------
// Native module type info verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_module_math_type_info() {
    use shingetsu_vm::types::LuaType;

    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    let type_map = env.global_type_map();

    // math should be a Table type with function fields.
    let math_type = type_map.get(b"math" as &[u8]).expect("math in type map");
    let table = match math_type {
        LuaType::Table(t) => t,
        other => panic!("expected Table for math, got {:?}", other),
    };

    // math.abs should be a function (not a method).
    let abs_field = table
        .fields
        .iter()
        .find(|(name, _)| name == &b"abs"[..])
        .expect("math.abs");
    match &abs_field.1 {
        LuaType::Function(f) => {
            k9::assert_equal!(f.is_method, false);
        }
        other => panic!("expected Function for math.abs, got {:?}", other),
    }

    // Verify that calling math.abs with : syntax warns.
    let compiler = Compiler::new(compile_opts(), type_map);
    let src = "math:abs(-1)";
    let bc = compiler.compile(src).await.expect("compile");
    let warnings = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    // Should contain a warning about . vs : syntax.
    assert!(
        !warnings.is_empty(),
        "expected a dot-vs-colon warning for math:abs"
    );
}

#[test]
fn native_module_string_type_info() {
    use shingetsu_vm::types::LuaType;

    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    let type_map = env.global_type_map();

    // string should be a Table type.
    let string_type = type_map
        .get(b"string" as &[u8])
        .expect("string in type map");
    let table = match string_type {
        LuaType::Table(t) => t,
        other => panic!("expected Table for string, got {:?}", other),
    };

    // string.len should be a function (takes a string argument, not self).
    let len_field = table
        .fields
        .iter()
        .find(|(name, _)| name == &b"len"[..])
        .expect("string.len");
    match &len_field.1 {
        LuaType::Function(f) => {
            k9::assert_equal!(f.is_method, false);
        }
        other => panic!("expected Function for string.len, got {:?}", other),
    }
}

#[test]
fn native_modules_present_in_type_map() {
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    let type_map = env.global_type_map();

    // All stdlib modules should be present.
    for name in &["math", "string", "table", "io", "os"] {
        assert!(
            type_map.get(name.as_bytes()).is_some(),
            "{name} missing from type map"
        );
    }
}

// ---------------------------------------------------------------------------
// Type checker: argument count checking
// ---------------------------------------------------------------------------

/// Build a compiler with builtins and type checking enabled.
fn type_check_compiler() -> Compiler {
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    Compiler::new(type_check_opts(), env.global_type_map())
}

#[tokio::test]
async fn type_check_too_few_args() {
    let compiler = type_check_compiler();
    let src = "math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        concat!(
            "error: expected 1 argument but got 0\n",
            " --> test.lua:1:9\n",
            "  |\n",
            "1 | math.abs()\n",
            "  |         ^^ expected 1 argument but got 0",
        )
    );
}

#[tokio::test]
async fn type_check_too_many_args() {
    let compiler = type_check_compiler();
    let src = "math.abs(1, 2, 3)";
    let bc = compiler.compile(src).await.expect("compile");
    let diags = render_warnings(&bc.diagnostics, src, RenderStyle::Plain);
    k9::assert_equal!(
        diags,
        concat!(
            "error: expected 1 argument but got 3\n",
            " --> test.lua:1:9\n",
            "  |\n",
            "1 | math.abs(1, 2, 3)\n",
            "  |         ^^^^^^^^^ expected 1 argument but got 3",
        )
    );
}

#[tokio::test]
async fn type_check_correct_args_no_error() {
    let compiler = type_check_compiler();
    let src = "math.abs(-5)";
    let bc = compiler.compile(src).await.expect("compile");
    // Only warnings from the lowering pass; no type-check errors.
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_variadic_too_few() {
    // string.format requires at least 1 argument (the format string).
    let compiler = type_check_compiler();
    let src = "string.format()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected at least 1 argument but got 0");
}

#[tokio::test]
async fn type_check_variadic_enough_args() {
    // string.format with enough args should not error.
    let compiler = type_check_compiler();
    let src = r#"string.format("%d %d", 1, 2)"#;
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_method_call_arg_count() {
    // Calling a method with `:` — self is implicit, so explicit arg
    // count is checked against params minus self.
    let mut map = GlobalTypeMap::default();
    map.types.insert(
        bytes::Bytes::from_static(b"obj"),
        LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                bytes::Bytes::from_static(b"foo"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![
                        (Some(bytes::Bytes::from_static(b"self")), LuaType::Any),
                        (Some(bytes::Bytes::from_static(b"x")), LuaType::Integer),
                    ],
                    variadic: None,
                    returns: vec![],
                    is_method: true,
                })),
            )],
            indexer: None,
        })),
    );
    let compiler = Compiler::new(type_check_opts(), map);
    // `:foo()` passes self implicitly — 0 explicit args but foo needs 1
    // (x: integer).
    let src = "obj:foo()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_method_call_correct_args() {
    let mut map = GlobalTypeMap::default();
    map.types.insert(
        bytes::Bytes::from_static(b"obj"),
        LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                bytes::Bytes::from_static(b"foo"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![
                        (Some(bytes::Bytes::from_static(b"self")), LuaType::Any),
                        (Some(bytes::Bytes::from_static(b"x")), LuaType::Integer),
                    ],
                    variadic: None,
                    returns: vec![],
                    is_method: true,
                })),
            )],
            indexer: None,
        })),
    );
    let compiler = Compiler::new(type_check_opts(), map);
    let src = "obj:foo(42)";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_disabled_by_default() {
    // With type_check: false (the default), no type errors should be
    // emitted even for incorrect argument counts.
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    let compiler = Compiler::new(compile_opts(), env.global_type_map());
    let src = "math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_vararg_last_arg_skips_check() {
    // When the last argument is `...`, the count is indeterminate.
    let compiler = type_check_compiler();
    let src = "math.abs(...)";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_nested_call_checked() {
    // A function call inside another call's arguments should also be checked.
    let compiler = type_check_compiler();
    let src = "print(math.abs())";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_direct_global_function() {
    // Calling a direct global function (not a table field) with wrong
    // argument count.
    let compiler = type_check_compiler();
    let src = "tostring()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_string_arg_syntax() {
    // `math.abs "hello"` is a call with 1 string argument.
    let compiler = type_check_compiler();
    let src = "math.abs \"hello\"";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_table_arg_syntax() {
    // `tostring {}` is a call with 1 table constructor argument.
    let compiler = type_check_compiler();
    let src = "tostring {}";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_dot_call_on_method_needs_explicit_self() {
    // Calling a method with `.` means the caller must pass self explicitly.
    // `obj.foo(42)` — foo has params (self, x), so dot-call needs 2 explicit
    // args. Passing only 1 is an arg count error.
    let mut map = GlobalTypeMap::default();
    map.types.insert(
        bytes::Bytes::from_static(b"obj"),
        LuaType::Table(Box::new(TableLuaType {
            fields: vec![(
                bytes::Bytes::from_static(b"foo"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![
                        (Some(bytes::Bytes::from_static(b"self")), LuaType::Any),
                        (Some(bytes::Bytes::from_static(b"x")), LuaType::Integer),
                    ],
                    variadic: None,
                    returns: vec![],
                    is_method: true,
                })),
            )],
            indexer: None,
        })),
    );
    let compiler = Compiler::new(type_check_opts(), map);
    // Dot call doesn't pass self implicitly — all params count.
    let src = "obj.foo(42)";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 2 arguments but got 1");
}

#[tokio::test]
async fn type_check_multiple_errors_in_one_file() {
    let compiler = type_check_compiler();
    let src = "\
math.abs()
math.floor()
math.ceil()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 3);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
    k9::assert_equal!(errors[1].message, "expected 1 argument but got 0");
    k9::assert_equal!(errors[2].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_if_block() {
    let compiler = type_check_compiler();
    let src = "\
if true then
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_while_loop() {
    let compiler = type_check_compiler();
    let src = "\
while true do
    math.abs()
    break
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_for_loop() {
    let compiler = type_check_compiler();
    let src = "\
for i = 1, 10 do
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_function_body() {
    let compiler = type_check_compiler();
    let src = "\
local function f()
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_untyped_globals_no_false_positives() {
    // `print` is variadic/untyped — any number of args should be fine.
    let compiler = type_check_compiler();
    let src = "print(1, 2, 3, 'hello', true, nil)";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_inside_elseif_block() {
    let compiler = type_check_compiler();
    let src = "\
if false then
    print('ok')
elseif true then
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_else_block() {
    let compiler = type_check_compiler();
    let src = "\
if false then
    print('ok')
else
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_repeat_until() {
    let compiler = type_check_compiler();
    let src = "\
repeat
    math.abs()
until true";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_do_block() {
    let compiler = type_check_compiler();
    let src = "\
do
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_inside_generic_for() {
    let compiler = type_check_compiler();
    let src = "\
for k, v in pairs({}) do
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_binary_expression() {
    let compiler = type_check_compiler();
    let src = "local x = 1 + math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_local_assignment_rhs() {
    let compiler = type_check_compiler();
    let src = "local x = math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_assignment_rhs() {
    let compiler = type_check_compiler();
    let src = "\
local x = 0
x = math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_call_expansion_last_arg_skips() {
    // When the last arg is a function call, arg count is indeterminate.
    let compiler = type_check_compiler();
    let src = "tostring(math.abs(1))";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_chained_access_silently_skipped() {
    // `a.b.c()` has 2 index suffixes — the type checker can't resolve
    // this and should silently skip, not crash or false-positive.
    let compiler = type_check_compiler();
    let src = "math.huge.foo()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_non_name_prefix_silently_skipped() {
    // `(expr).foo()` — prefix is a Parentheses expression, not a Name.
    let compiler = type_check_compiler();
    let src = "({}).foo()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_method_on_chained_access_skipped() {
    // `a.b:foo()` has index suffixes before the method call — skipped.
    let compiler = type_check_compiler();
    let src = "math.pi:foo()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_in_compound_assignment_rhs() {
    let compiler = type_check_compiler();
    let src = "\
local x = 0
x += math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_return_statement() {
    let compiler = type_check_compiler();
    let src = "return math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_return_multiple_values() {
    let compiler = type_check_compiler();
    let src = "return 1, math.abs(), 3";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_table_constructor_positional() {
    let compiler = type_check_compiler();
    let src = "local t = { math.abs() }";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_table_constructor_named() {
    let compiler = type_check_compiler();
    let src = "local t = { x = math.abs() }";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_table_constructor_expression_key() {
    let compiler = type_check_compiler();
    let src = "local t = { [math.abs()] = 1 }";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_unary_expression() {
    let compiler = type_check_compiler();
    let src = "local x = -math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_parenthesized_expression() {
    let compiler = type_check_compiler();
    let src = "local x = (math.abs())";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_if_expression() {
    let compiler = type_check_compiler();
    let src = "local x = if true then math.abs() else 0";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_anonymous_function_body() {
    let compiler = type_check_compiler();
    let src = "local f = function() return math.abs() end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_while_condition() {
    let compiler = type_check_compiler();
    let src = "\
while math.abs() do
    break
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_if_condition() {
    let compiler = type_check_compiler();
    let src = "\
if math.abs() then
    print('ok')
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_repeat_until_condition() {
    let compiler = type_check_compiler();
    let src = "\
repeat
    print('ok')
until math.abs()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_numeric_for_start() {
    let compiler = type_check_compiler();
    let src = "\
for i = math.abs(), 10 do
    print(i)
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_numeric_for_end() {
    let compiler = type_check_compiler();
    let src = "\
for i = 1, math.abs() do
    print(i)
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_numeric_for_step() {
    let compiler = type_check_compiler();
    let src = "\
for i = 1, 10, math.abs() do
    print(i)
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_generic_for_iterator_expr() {
    let compiler = type_check_compiler();
    let src = "\
for k, v in math.abs() do
    print(k, v)
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_in_function_declaration_body() {
    // Non-local function declaration: `function f() end`
    let compiler = type_check_compiler();
    let src = "\
function f()
    math.abs()
end";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 1);
    k9::assert_equal!(errors[0].message, "expected 1 argument but got 0");
}

#[tokio::test]
async fn type_check_bracket_index_silently_skipped() {
    // `t["foo"]()` uses bracket index, not dot — should be skipped.
    let compiler = type_check_compiler();
    let src = r#"math["abs"]()"#;
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}

#[tokio::test]
async fn type_check_non_function_field_no_false_positive() {
    // `math.pi()` — pi is a number, not a function. The type checker
    // should not produce an arg-count error (it's not a callable type).
    let compiler = type_check_compiler();
    let src = "math.pi()";
    let bc = compiler.compile(src).await.expect("compile");
    let errors: Vec<_> = bc
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    k9::assert_equal!(errors.len(), 0);
}
