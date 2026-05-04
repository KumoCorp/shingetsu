use super::*;
use bstr::ByteSlice;
use shingetsu_vm::types::LuaType;

/// Convert a `Vec<(Bytes, LuaType)>` to a `Vec<(String, String)>` for
/// readable assertions. The string representation of the type uses
/// `simple_type_name` so we don't have to construct nested `LuaType`
/// values in test expectations.
///
/// Names that aren't valid UTF-8 are filtered out, matching the real
/// completion code's behaviour — there's no point asserting on names
/// we wouldn't be able to surface as completion candidates anyway.
fn names_and_types(locals: Vec<(shingetsu_vm::Bytes, LuaType)>) -> Vec<(String, String)> {
    locals
        .into_iter()
        .filter_map(|(name, ty)| {
            name.to_str()
                .ok()
                .map(|n| (n.to_string(), ty.simple_type_name()))
        })
        .collect()
}

#[test]
fn no_locals_returns_empty() {
    let result = locals_at_cursor("", 0);
    k9::assert_equal!(names_and_types(result), Vec::<(String, String)>::new());
}

#[test]
fn top_level_local_visible_after_declaration() {
    // `local x = 1; ` — cursor after the declaration.
    let source = "local x = 1\n";
    let cursor = source.len();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![("x".to_string(), "any".to_string())]
    );
}

#[test]
fn local_declared_after_cursor_is_invisible() {
    // Cursor at byte 0 — `local x` declared on line 1 starts at byte 0,
    // so the rule "start <= cursor" does include it. Move cursor to a
    // position before any local: there's no whitespace before `local` so
    // we can't easily place a cursor BEFORE the declaration in this source.
    // Instead, declare locals and place cursor before the second one.
    let source = "local x = 1\nlocal y = 2\n";
    // Cursor right before `local y`.
    let cursor = source.find("local y").unwrap();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![("x".to_string(), "any".to_string())]
    );
}

#[test]
fn typed_local_carries_type() {
    let source = "local n: number = 0\n";
    let cursor = source.len();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![("n".to_string(), "number".to_string())]
    );
}

#[test]
fn multiple_typed_locals_in_one_assignment() {
    let source = "local a: number, b: string = 0, ''\n";
    let cursor = source.len();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![
            ("a".to_string(), "number".to_string()),
            ("b".to_string(), "string".to_string()),
        ]
    );
}

#[test]
fn function_params_visible_inside_body() {
    // Cursor inside the function body should see typed parameters.
    let source = "function greet(user: string, count: number)\n  return user\nend\n";
    let cursor = source.find("return").unwrap();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![
            ("user".to_string(), "string".to_string()),
            ("count".to_string(), "number".to_string()),
        ]
    );
}

#[test]
fn function_params_not_visible_outside_body() {
    // Cursor after the function ends — params should not leak out.
    let source = "function f(p: number) end\n";
    let cursor = source.len();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(names_and_types(result), Vec::<(String, String)>::new());
}

#[test]
fn ellipsis_parameter_is_skipped() {
    // `...` is a vararg, not a named local.
    let source = "function variadic(first: string, ...)\n  return first\nend\n";
    let cursor = source.find("return").unwrap();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![("first".to_string(), "string".to_string())]
    );
}

#[test]
fn incomplete_input_does_not_panic() {
    // Mid-typed function definition, cursor at end. parse_fallible
    // recovers; we should walk what's there without panicking.
    let source = "function foo(x: number)\n  x.";
    let cursor = source.len();
    let _result = locals_at_cursor(source, cursor);
    // No panic = success. (The exact contents depend on parser recovery.)
}

#[test]
fn cursor_at_byte_zero_returns_empty() {
    let source = "local x = 1";
    let result = locals_at_cursor(source, 0);
    k9::assert_equal!(names_and_types(result), Vec::<(String, String)>::new());
}

#[test]
fn function_params_visible_with_complete_body() {
    // Sanity: complete `function ... end` with cursor inside the body
    // should expose the params.
    let source = "function f(s: string)\n  return s\nend\n";
    let cursor = source.find("return").unwrap() + 6;
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![("s".to_string(), "string".to_string())]
    );
}

#[test]
fn function_params_visible_in_incomplete_body() {
    // The user is in the middle of typing a function body — no `end` yet.
    // parse_fallible recovers; we should still see the params.
    let source = "function f(s: string)\n  s.";
    let cursor = source.len();
    let result = locals_at_cursor(source, cursor);
    k9::assert_equal!(
        names_and_types(result),
        vec![("s".to_string(), "string".to_string())]
    );
}
