mod common;
use common::{type_check, type_check_with_builtins};

// ---------------------------------------------------------------------------
// Multi-assignment from table fields — types inferred, feeds arg check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_from_table_fields() {
    type_check(
        "\
type T = { greet: (name: string) -> (), count: (n: number) -> () }
local t: T = {}
local greet, count = t.greet, t.count
greet(42)
count(\"wrong\")",
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:4:7
  |
4 | greet(42)
  |       ^^ expected 'string' for parameter 'name' but got 'integer'
error[arg_type]: expected 'number' for parameter 'n' but got 'string'
 --> test.lua:5:7
  |
5 | count(\"wrong\")
  |       ^^^^^^^ expected 'number' for parameter 'n' but got 'string'",
    );
}

// ---------------------------------------------------------------------------
// Multi-assign: one annotated, one inferred from field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_mixed_annotation_inference() {
    type_check(
        "\
type T = { f: (x: number) -> () }
local t: T = {}
local _a: boolean, b = true, t.f
b(\"wrong\")",
        "\
error[arg_type]: expected 'number' for parameter 'x' but got 'string'
 --> test.lua:4:3
  |
4 | b(\"wrong\")
  |   ^^^^^^^ expected 'number' for parameter 'x' but got 'string'",
    );
}

// ---------------------------------------------------------------------------
// Multi-assign from native module fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_from_native_module() {
    type_check_with_builtins(
        "\
local abs, floor = math.abs, math.floor
abs(1)
floor(2)",
        "",
    );
}

// ---------------------------------------------------------------------------
// Local-to-local inference: type propagates through assignment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_to_local_inference() {
    type_check(
        "\
type T = { f: (x: number) -> () }
local a: T = {}
local b = a
b.f(1, 2)",
        "\
error[arg_count]: expected 1 argument but got 2
 --> test.lua:4:4
  |
4 | b.f(1, 2)
  |    ^^^^^^ expected 1 argument but got 2",
    );
}

// ---------------------------------------------------------------------------
// Inference from function call return type
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_function_call() {
    type_check(
        "\
type Point = { x: number, y: number }
local function make_point(): Point
    return {}
end
local p = make_point()
local _ = p.z",
        "\
error[field_access]: unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`
 --> test.lua:6:11
  |
6 | local _ = p.z
  |           ^^^ unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Inference from literal expressions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_literals() {
    type_check(
        "\
local a, b, c = 42, \"hello\", true
local _x: string = a
local _y: number = b
local _z: number = c",
        "\
error[assign_type]: expected 'string' but got 'integer'
 --> test.lua:2:20
  |
2 | local _x: string = a
  |                    ^ expected 'string' but got 'integer'
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:3:20
  |
3 | local _y: number = b
  |                    ^ expected 'number' but got 'string'
error[assign_type]: expected 'number' but got 'boolean'
 --> test.lua:4:20
  |
4 | local _z: number = c
  |                    ^ expected 'number' but got 'boolean'",
    );
}

// ---------------------------------------------------------------------------
// No inference when no RHS expression for a variable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_inference_without_rhs() {
    type_check(
        "\
local _a, b = 42
local _x: number = b",
        "",
    );
}

// ---------------------------------------------------------------------------
// Local-to-local display name propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_to_local_display_name() {
    type_check(
        "\
type Point = { x: number, y: number }
local a: Point = {}
local b = a
local _ = b.z",
        "\
error[field_access]: unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`
 --> test.lua:4:11
  |
4 | local _ = b.z
  |           ^^^ unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Chained local inference — display name propagates through chain
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chained_local_inference() {
    type_check(
        "\
type Point = { x: number, y: number }
local a: Point = {}
local b = a
local c = b
local _ = c.z",
        "\
error[field_access]: unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`
 --> test.lua:5:11
  |
5 | local _ = c.z
  |           ^^^ unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Function call with no return annotation — no alias display name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_call_no_return_annotation() {
    type_check(
        "\
local function make()
    return {}
end
local p = make()
local _ = p.z",
        "",
    );
}

// ---------------------------------------------------------------------------
// Infer from binary operator
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_binary_op() {
    type_check(
        "\
local a: number = 1
local b: number = 2
local c = a + b
local _x: string = c",
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:4:20
  |
4 | local _x: string = c
  |                    ^ expected 'string' but got 'number'",
    );
}

// ---------------------------------------------------------------------------
// Multi-assign with fewer RHS than LHS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_assign_fewer_rhs() {
    type_check(
        "\
type T = { x: number, y: string }
local t: T = {}
local a, b, c = t.x, t.y
local _: string = a
local _: number = b
local _: number = c",
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:4:19
  |
4 | local _: string = a
  |                   ^ expected 'string' but got 'number'
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:5:19
  |
5 | local _: number = b
  |                   ^ expected 'number' but got 'string'",
    );
}

// ---------------------------------------------------------------------------
// Infer return type from method call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_method_call() {
    type_check(
        "\
type Result = { value: number, ok: boolean }
type Service = { process: (x: number) -> Result }
local svc: Service = {}
local result = svc.process(42)
local _ = result.z",
        "\
error[field_access]: unknown field 'z' on type 'Result'. Possible alternatives are `ok`, `value`
 --> test.lua:5:11
  |
5 | local _ = result.z
  |           ^^^^^^^^ unknown field 'z' on type 'Result'. Possible alternatives are `ok`, `value`",
    );
}

// ---------------------------------------------------------------------------
// Infer return type from dot-call on typed table
// ---------------------------------------------------------------------------

#[tokio::test]
async fn infer_from_dot_call() {
    type_check(
        "\
type Point = { x: number, y: number }
type Factory = { make: (n: number) -> Point }
local factory: Factory = {}
local p = factory.make(1)
local _ = p.z",
        "\
error[field_access]: unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`
 --> test.lua:5:11
  |
5 | local _ = p.z
  |           ^^^ unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Infer from chained: method return feeds field access check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_return_feeds_arg_check() {
    type_check(
        "\
type Handler = { run: () -> (x: number) -> () }
local h: Handler = {}
local callback = h.run()
callback(\"wrong\")",
        "\
error[arg_type]: expected 'number' for parameter 'x' but got 'string'
 --> test.lua:4:10
  |
4 | callback(\"wrong\")
  |          ^^^^^^^ expected 'number' for parameter 'x' but got 'string'",
    );
}

// ---------------------------------------------------------------------------
// Builtin function return inference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn builtin_function_return_inference() {
    type_check_with_builtins(
        "\
local s = tostring(42)
local _: number = s",
        "\
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:2:19
  |
2 | local _: number = s
  |                   ^ expected 'number' but got 'string'
  |
help: local 's' was assigned from 'tostring(...)' at line 1",
    );
}

// ---------------------------------------------------------------------------
// Dot-call return with no matching alias — shows 'table'
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dot_call_return_no_alias() {
    type_check(
        "\
type Factory = { make: () -> { x: number, y: number } }
local f: Factory = {}
local p = f.make()
local _ = p.z",
        "\
error[field_access]: unknown field 'z' on type '{ x: number, y: number }'. Possible alternatives are `x`, `y`
 --> test.lua:4:11
  |
4 | local _ = p.z
  |           ^^^ unknown field 'z' on type '{ x: number, y: number }'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Chained: dot-call return feeds field assign check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chained_dot_call_field_inference() {
    type_check(
        "\
type Point = { x: number, y: number }
type Factory = { make: () -> Point }
local f: Factory = {}
local p = f.make()
local _: string = p.x",
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:5:19
  |
5 | local _: string = p.x
  |                   ^^^ expected 'string' but got 'number'",
    );
}

// ---------------------------------------------------------------------------
// Local function return_display_name path (not find_alias_name)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_function_return_display_name_priority() {
    type_check(
        "\
type Point = { x: number, y: number }
local function make(): Point
    return {}
end
local p = make()
local _ = p.z",
        "\
error[field_access]: unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`
 --> test.lua:6:11
  |
6 | local _ = p.z
  |           ^^^ unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Table constructor field inference — basic named fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_named_fields() {
    type_check(
        "\
local t = { x = 1, y = \"hello\", z = true }
local _: string = t.x
local _: number = t.y
local _: number = t.z",
        "\
error[assign_type]: expected 'string' but got 'integer'
 --> test.lua:2:19
  |
2 | local _: string = t.x
  |                   ^^^ expected 'string' but got 'integer'
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:3:19
  |
3 | local _: number = t.y
  |                   ^^^ expected 'number' but got 'string'
error[assign_type]: expected 'number' but got 'boolean'
 --> test.lua:4:19
  |
4 | local _: number = t.z
  |                   ^^^ expected 'number' but got 'boolean'",
    );
}

// ---------------------------------------------------------------------------
// Table constructor — unknown field access
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_unknown_field() {
    type_check(
        "\
local t = { x = 1, y = 2 }
local _ = t.z",
        "\
error[field_access]: unknown field 'z' on type '{ x: integer, y: integer }'. Possible alternatives are `x`, `y`
 --> test.lua:2:11
  |
2 | local _ = t.z
  |           ^^^ unknown field 'z' on type '{ x: integer, y: integer }'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Table constructor with function values
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_function_fields() {
    type_check(
        "\
local t = {
    greet = function(name: string): string
        return \"hello \" .. name
    end,
}
t.greet(42)",
        "\
error[arg_type]: expected 'string' for parameter 'name' but got 'integer'
 --> test.lua:6:9
  |
6 | t.greet(42)
  |         ^^ expected 'string' for parameter 'name' but got 'integer'",
    );
}

// ---------------------------------------------------------------------------
// Table constructor in return position — feeds caller
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_return_position() {
    type_check(
        "\
local function make_api()
    return {
        run = function(x: number): number
            return x + 1
        end,
    }
end
local api = make_api()
api.run(\"wrong\")",
        "\
error[arg_type]: expected 'number' for parameter 'x' but got 'string'
 --> test.lua:9:9
  |
9 | api.run(\"wrong\")
  |         ^^^^^^^ expected 'number' for parameter 'x' but got 'string'",
    );
}

// ---------------------------------------------------------------------------
// Table constructor — empty constructor remains empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_empty() {
    type_check(
        "\
local t = {}
local _ = t.anything",
        "",
    );
}

// ---------------------------------------------------------------------------
// Table constructor — positional (NoKey) fields are not named fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_positional_no_named_fields() {
    type_check(
        "\
local t = { 1, 2, 3 }
local _ = t.x",
        "",
    );
}

// ---------------------------------------------------------------------------
// Nested table constructor — inner table fields inferred
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nested_table_constructor() {
    type_check(
        "\
local t = { inner = { x = 1, y = \"hello\" } }
local inner = t.inner
local _ = inner.z",
        "\
error[field_access]: unknown field 'z' on type '{ x: integer, y: string }'. Possible alternatives are `x`, `y`
 --> test.lua:3:11
  |
3 | local _ = inner.z
  |           ^^^^^^^ unknown field 'z' on type '{ x: integer, y: string }'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Mixed named and positional — named fields still inferred
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mixed_named_and_positional() {
    type_check(
        "\
local t = { 1, 2, x = \"hello\" }
local _: number = t.x",
        "\
error[assign_type]: expected 'number' but got 'string'
 --> test.lua:2:19
  |
2 | local _: number = t.x
  |                   ^^^ expected 'number' but got 'string'",
    );
}

// ---------------------------------------------------------------------------
// Return type inferred from literal return
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_inferred_from_literal() {
    type_check(
        "\
local function f()
    return 42
end
local x = f()
local _: string = x",
        "\
error[assign_type]: expected 'string' but got 'integer'
 --> test.lua:5:19
  |
5 | local _: string = x
  |                   ^ expected 'string' but got 'integer'
  |
help: local 'x' was assigned from 'f(...)' at line 4",
    );
}

// ---------------------------------------------------------------------------
// Annotated params, return inferred from body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn annotated_params_inferred_return() {
    type_check(
        "\
local function f(x: number)
    return x + 1
end
local r = f(1)
local _: string = r",
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:5:19
  |
5 | local _: string = r
  |                   ^ expected 'string' but got 'number'
  |
help: local 'r' was assigned from 'f(...)' at line 4",
    );
}

// ---------------------------------------------------------------------------
// Table constructor with unannotated function — no false positives
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_constructor_unannotated_function() {
    type_check(
        "\
local t = { handler = function(x) return x end }
t.handler(42)
t.handler(\"hello\")",
        "",
    );
}
