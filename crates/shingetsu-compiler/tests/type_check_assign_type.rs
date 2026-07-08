mod common;
use common::type_check;

#[tokio::test]
async fn string_for_number() {
    type_check(
        "local _x: number = \"hello\"",
        "\
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:1:20
  |
1 | local _x: number = \"hello\"
  |                    ^^^^^^^ expected 'number' but got 'string'",
    );
}

#[tokio::test]
async fn correct_type() {
    type_check("local x: number = 42\nreturn x", "");
}

#[tokio::test]
async fn integer_for_number() {
    type_check("local x: number = 42\nreturn x", "");
}

#[tokio::test]
async fn float_for_number() {
    type_check("local x: number = 1.5\nreturn x", "");
}

#[tokio::test]
async fn nil_for_optional() {
    type_check("local x: number? = nil\nreturn x", "");
}

#[tokio::test]
async fn wrong_for_optional() {
    type_check(
        "local _x: number? = \"oops\"",
        "\
error[assign_type]: expected 'number?' but got 'string'
 --> test.lua:1:21
  |
1 | local _x: number? = \"oops\"
  |                     ^^^^^^ expected 'number?' but got 'string'",
    );
}

#[tokio::test]
async fn any_annotation_no_check() {
    type_check("local x: any = \"anything\"\nreturn x", "");
}

#[tokio::test]
async fn no_annotation_no_check() {
    type_check("local x = \"anything\"\nreturn x", "");
}

#[tokio::test]
async fn boolean_for_string() {
    type_check(
        "local _x: string = true",
        "\
error[assign_type]: expected 'string' but got 'boolean'
 --> test.lua:1:20
  |
1 | local _x: string = true
  |                    ^^^^ expected 'string' but got 'boolean'",
    );
}

#[tokio::test]
async fn table_for_table() {
    type_check(
        "type Config = { name: string }\nlocal c: Config = {}\nreturn c",
        "",
    );
}

#[tokio::test]
async fn unknown_rhs_no_check() {
    type_check("local t = {}\nlocal x: number = t[1]\nreturn x", "");
}

#[tokio::test]
async fn union_annotation() {
    type_check(
        "local _x: number | string = true",
        "\
error[assign_type]: expected 'number | string' but got 'boolean'
 --> test.lua:1:29
  |
1 | local _x: number | string = true
  |                             ^^^^ expected 'number | string' but got 'boolean'",
    );
}

#[tokio::test]
async fn union_annotation_match() {
    type_check("local x: number | string = 42\nreturn x", "");
}

#[tokio::test]
async fn expression_rhs() {
    type_check(
        "local _x: string = 1 + 2",
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:1:20
  |
1 | local _x: string = 1 + 2
  |                    ^^^^^ expected 'string' but got 'number'",
    );
}

#[tokio::test]
async fn function_for_number() {
    type_check(
        "local _x: number = function() end",
        "\
error[assign_type]: expected 'number' but got 'function'
 --> test.lua:1:20
  |
1 | local _x: number = function() end
  |                    ^^^^^^^^^^^^^^ expected 'number' but got 'function'",
    );
}

#[tokio::test]
async fn multiple_locals_second_wrong() {
    type_check(
        "local a: number, b: string = 42, true\nreturn a, b",
        "\
error[assign_type]: expected 'string' but got 'boolean'
 --> test.lua:1:34
  |
1 | local a: number, b: string = 42, true
  |                                  ^^^^ expected 'string' but got 'boolean'",
    );
}

#[tokio::test]
async fn constructor_field_with_uninferrable_value_is_present() {
    // A field whose value type the checker cannot statically infer
    // (here the return of a method-call chain on an untyped parameter)
    // is still present in the constructed table, so it must not be
    // reported as a missing field.
    type_check(
        "local function f(p) return p:trim() end\n\
         local t: { name: string, k: string } = { name = 'z', k = f('e') }\n\
         return t",
        "",
    );
}

#[tokio::test]
async fn constructor_omitting_field_still_reports_missing() {
    // Contrast: a field genuinely absent from the constructor is still
    // reported as missing.
    type_check(
        "local _t: { name: string, k: string } = { name = 'z' }",
        "\
error[assign_type]: expected '{ name: string, k: string }' but got '{ name: string }'
 --> test.lua:1:41
  |
1 | local _t: { name: string, k: string } = { name = 'z' }
  |                                         ^^^^^^^^^^^^^^ expected '{ name: string, k: string }' but got '{ name: string }'
  |
help: missing field 'k' of type 'string'",
    );
}
