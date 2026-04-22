//! Line-bound tracking on [`Proto`] — `line_defined` and
//! `last_line_defined` for the main chunk and every nested function.
//!
//! Lua 5.4 convention: main chunk has `linedefined = 0` and
//! `lastlinedefined = <last source line>`.  Nested functions use the
//! source line of the opening `(` (effectively the `function` keyword)
//! and the matching `end`.

use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::Proto;

async fn compile_src(src: &str) -> std::sync::Arc<Proto> {
    let compiler = Compiler::new(
        CompileOptions {
            debug_info: true,
            source_name: "@test.lua".into(),
            type_check: false,
        },
        Default::default(),
    );
    compiler
        .compile(src)
        .await
        .expect("compile failed")
        .top_level
}

#[tokio::test]
async fn main_chunk_bounds_are_zero_and_last_line() {
    // Three lines of source; main chunk `linedefined` is always 0 and
    // `lastlinedefined` is the last source line (line 3 here).
    let src = "local a = 1\nlocal b = 2\nlocal c = 3\n";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.signature.line_defined, 0);
    k9::assert_equal!(proto.signature.last_line_defined, 4);
}

#[tokio::test]
async fn main_chunk_empty_source() {
    // Empty source — both bounds collapse to 0 / 1 (depending on the
    // EOF token position).  This asserts we don't panic or underflow.
    let proto = compile_src("").await;
    k9::assert_equal!(proto.signature.line_defined, 0);
    // EOF on an empty buffer sits at line 1.
    k9::assert_equal!(proto.signature.last_line_defined, 1);
}

#[tokio::test]
async fn nested_named_function_bounds() {
    let src = "\
local x = 1
function foo()
  return 42
end
local y = 2
";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.signature.line_defined, 0);
    // Last line: trailing newline puts eof on line 6.
    k9::assert_equal!(proto.signature.last_line_defined, 6);

    // Exactly one nested proto.
    k9::assert_equal!(proto.protos.len(), 1);
    let foo = &proto.protos[0];
    // `function foo()` is on line 2; `end` is on line 4.
    k9::assert_equal!(foo.signature.line_defined, 2);
    k9::assert_equal!(foo.signature.last_line_defined, 4);
}

#[tokio::test]
async fn nested_local_function_bounds() {
    let src = "\
local function f(x)
  return x + 1
end
";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.protos.len(), 1);
    let f = &proto.protos[0];
    k9::assert_equal!(f.signature.line_defined, 1);
    k9::assert_equal!(f.signature.last_line_defined, 3);
}

#[tokio::test]
async fn anonymous_function_expression_bounds() {
    let src = "\
local f = function(x)
  return x * 2
end
";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.protos.len(), 1);
    let anon = &proto.protos[0];
    // Opening `(` is on line 1; `end` is on line 3.
    k9::assert_equal!(anon.signature.line_defined, 1);
    k9::assert_equal!(anon.signature.last_line_defined, 3);
}

#[tokio::test]
async fn method_form_function_bounds() {
    let src = "\
local t = {}
function t:m(x)
  return x
end
";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.protos.len(), 1);
    let m = &proto.protos[0];
    k9::assert_equal!(m.signature.line_defined, 2);
    k9::assert_equal!(m.signature.last_line_defined, 4);
}

#[tokio::test]
async fn multiple_nested_functions() {
    let src = "\
function a()
  return 1
end

function b()
  return 2
end

function c()
  return 3
end
";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.protos.len(), 3);
    // a: lines 1-3
    k9::assert_equal!(proto.protos[0].signature.line_defined, 1);
    k9::assert_equal!(proto.protos[0].signature.last_line_defined, 3);
    // b: lines 5-7
    k9::assert_equal!(proto.protos[1].signature.line_defined, 5);
    k9::assert_equal!(proto.protos[1].signature.last_line_defined, 7);
    // c: lines 9-11
    k9::assert_equal!(proto.protos[2].signature.line_defined, 9);
    k9::assert_equal!(proto.protos[2].signature.last_line_defined, 11);
}

#[tokio::test]
async fn doubly_nested_function_bounds() {
    let src = "\
function outer()
  local function inner()
    return 42
  end
  return inner
end
";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.protos.len(), 1);
    let outer = &proto.protos[0];
    k9::assert_equal!(outer.signature.line_defined, 1);
    k9::assert_equal!(outer.signature.last_line_defined, 6);

    // `inner` is nested inside `outer`'s proto list.
    k9::assert_equal!(outer.protos.len(), 1);
    let inner = &outer.protos[0];
    k9::assert_equal!(inner.signature.line_defined, 2);
    k9::assert_equal!(inner.signature.last_line_defined, 4);
}

#[tokio::test]
async fn luau_generic_function_bounds() {
    // Luau generic parameters sit between the name and the `(`; the
    // opening-paren position must still land on the correct line.
    let src = "\
local function id<T>(x: T): T
  return x
end
";
    let proto = compile_src(src).await;
    k9::assert_equal!(proto.protos.len(), 1);
    let id = &proto.protos[0];
    k9::assert_equal!(id.signature.line_defined, 1);
    k9::assert_equal!(id.signature.last_line_defined, 3);
}
