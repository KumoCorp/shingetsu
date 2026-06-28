mod common;
use common::{type_check, type_check_with_builtins};

// ---------------------------------------------------------------------------
// Unknown field on typed table (user-defined type)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_field_dot_access() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p.z",
        "\
error[field_access]: unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`
 --> test.lua:3:11
  |
3 | local _ = p.z
  |           ^^^ unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`",
    );
}

#[tokio::test]
async fn unknown_field_bracket_literal() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p[\"z\"]",
        "\
error[field_access]: unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`
 --> test.lua:3:11
  |
3 | local _ = p[\"z\"]
  |           ^^^^^ unknown field 'z' on type 'Point'. Possible alternatives are `x`, `y`",
    );
}

#[tokio::test]
async fn known_field_no_diagnostic() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p.x",
        "",
    );
}

#[tokio::test]
async fn known_field_bracket_no_diagnostic() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p[\"x\"]",
        "",
    );
}

// ---------------------------------------------------------------------------
// Not callable: field exists but is not a function
// ---------------------------------------------------------------------------

#[tokio::test]
async fn not_callable_dot() {
    type_check(
        "\
type Info = { name: string, count: integer }
local t: Info = {}
t.name()",
        "\
error[field_access]: field 'Info.name' is not callable (type is 'string')
 --> test.lua:3:1
  |
3 | t.name()
  | ^^^^^^^^ field 'Info.name' is not callable (type is 'string')",
    );
}

#[tokio::test]
async fn not_callable_bracket() {
    type_check(
        "\
type Info = { name: string, count: integer }
local t: Info = {}
t[\"name\"]()",
        "\
error[field_access]: field 'Info.name' is not callable (type is 'string')
 --> test.lua:3:1
  |
3 | t[\"name\"]()
  | ^^^^^^^^^^^ field 'Info.name' is not callable (type is 'string')",
    );
}

#[tokio::test]
async fn callable_field_no_diagnostic() {
    type_check(
        "\
type Lib = { add: (a: number, b: number) -> number }
local M: Lib = {}
M.add(1, 2)",
        "",
    );
}

// ---------------------------------------------------------------------------
// Unknown field on call site
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_field_call() {
    type_check(
        "\
type Lib = { add: (a: number, b: number) -> number }
local M: Lib = {}
M.sub(1, 2)",
        "\
error[field_access]: unknown field 'sub' on type 'Lib'. The only valid field is `add`
 --> test.lua:3:1
  |
3 | M.sub(1, 2)
  | ^^^^^^^^^^^ unknown field 'sub' on type 'Lib'. The only valid field is `add`",
    );
}

// ---------------------------------------------------------------------------
// Native module fields (math)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_module_unknown_field() {
    type_check_with_builtins(
        "local _ = math.nonexistent",
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:11
  |
1 | local _ = math.nonexistent
  |           ^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'",
    );
}

#[tokio::test]
async fn native_module_known_field_no_diagnostic() {
    type_check_with_builtins("local _ = math.abs", "");
}

// Fields merged onto an already-installed stdlib global (io.popen via
// register_popen, io.write via register_stdio, os.execute via
// register_exec) must be visible to the checker, not just the base
// fields snapshotted when the global was first set.
#[tokio::test]
async fn io_merged_fields_no_diagnostic() {
    type_check_with_builtins(
        "local _ = io.popen\nlocal _ = io.write\nlocal _ = os.execute",
        "",
    );
}

#[tokio::test]
async fn native_module_not_callable() {
    type_check_with_builtins(
        "math.pi()",
        "\
error[field_access]: field 'math.pi' is not callable (type is 'float')
 --> test.lua:1:1
  |
1 | math.pi()
  | ^^^^^^^^^ field 'math.pi' is not callable (type is 'float')",
    );
}

// ---------------------------------------------------------------------------
// Edge cases: skip checking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generic_table_no_check() {
    type_check(
        "\
local t: Table = {}
local _ = t.anything",
        "\
error[assign_type]: expected 'Table' but got 'table'
 --> test.lua:1:18
  |
1 | local t: Table = {}
  |                  ^^ expected 'Table' but got 'table'",
    );
}

#[tokio::test]
async fn unknown_receiver_no_check() {
    type_check("local _ = unknown_var.foo", "");
}

#[tokio::test]
async fn bracket_non_literal_no_check() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local key = \"z\"
local _ = p[key]",
        "",
    );
}

// ---------------------------------------------------------------------------
// Infer field type for downstream checks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn field_type_feeds_assign_check() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _n: string = p.x",
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:3:20
  |
3 | local _n: string = p.x
  |                    ^^^ expected 'string' but got 'number'",
    );
}

#[tokio::test]
async fn field_type_feeds_arg_check() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local function take_string(_s: string) end
take_string(p.x)",
        "\
error[arg_type]: expected 'string' for parameter '_s' but got 'number'
 --> test.lua:4:13
  |
4 | take_string(p.x)
  |             ^^^ expected 'string' for parameter '_s' but got 'number'",
    );
}

// ---------------------------------------------------------------------------
// Table with indexer — skip field checking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_with_indexer_no_check() {
    type_check(
        "\
type Dict = { [string]: number }
local d: Dict = {}
local _ = d.anything",
        "",
    );
}

// ---------------------------------------------------------------------------
// Method-call on unknown field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_call_unknown_field_diagnostic() {
    type_check(
        "\
type Obj = { greet: (self) -> string }
local o: Obj = {}
o:unknown()",
        "\
error[field_access]: unknown field 'unknown' on type 'Obj'. The only valid field is `greet`
 --> test.lua:3:1
  |
3 | o:unknown()
  | ^^^^^^^^^^^ unknown field 'unknown' on type 'Obj'. The only valid field is `greet`",
    );
}

// ---------------------------------------------------------------------------
// Non-Name prefix — silently skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parenthesized_prefix_no_check() {
    type_check(
        "\
type Point = { x: number, y: number }
local function get_point(): Point end
local _ = (get_point()).z",
        "\
error[missing_return]: function may fall off the end without returning '{ x: number, y: number }'
 --> test.lua:2:35
  |
2 | local function get_point(): Point end
  |                                   ^^^ function may fall off the end without returning '{ x: number, y: number }'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type",
    );
}

// ---------------------------------------------------------------------------
// Multi-level access — skipped (only single-level checked)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_level_access_no_check() {
    type_check(
        "\
type Inner = { val: number }
type Outer = { inner: Inner }
local o: Outer = {}
local _ = o.inner.nonexistent",
        "",
    );
}

// ---------------------------------------------------------------------------
// Infer field type through bracket literal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn field_type_bracket_feeds_assign_check() {
    type_check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _n: string = p[\"x\"]",
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:3:20
  |
3 | local _n: string = p[\"x\"]
  |                    ^^^^^ expected 'string' but got 'number'",
    );
}

// ---------------------------------------------------------------------------
// Unknown field on native module via bracket
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_module_unknown_field_bracket() {
    type_check_with_builtins(
        "local _ = math[\"nonexistent\"]",
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:11
  |
1 | local _ = math[\"nonexistent\"]
  |           ^^^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'",
    );
}

// ---------------------------------------------------------------------------
// Display name fallback — inline table type (no alias)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inline_table_type_fallback_display() {
    type_check(
        "\
local t: { x: number, y: number } = {}
local _ = t.z",
        "\
error[field_access]: unknown field 'z' on type '{ x: number, y: number }'. Possible alternatives are `x`, `y`
 --> test.lua:2:11
  |
2 | local _ = t.z
  |           ^^^ unknown field 'z' on type '{ x: number, y: number }'. Possible alternatives are `x`, `y`",
    );
}

// ---------------------------------------------------------------------------
// Native module unknown field in call position
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_module_unknown_field_call() {
    type_check_with_builtins(
        "math.nonexistent(1)",
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:1
  |
1 | math.nonexistent(1)
  | ^^^^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'",
    );
}

// ---------------------------------------------------------------------------
// Method-call not-callable — colon syntax on non-function field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_call_not_callable() {
    type_check(
        "\
type Info = { name: string, count: integer }
local t: Info = {}
t:name()",
        "\
error[field_access]: field 'Info.name' is not callable (type is 'string')
 --> test.lua:3:1
  |
3 | t:name()
  | ^^^^^^^^ field 'Info.name' is not callable (type is 'string')",
    );
}

// ---------------------------------------------------------------------------
// Non-table receiver — silently skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_table_receiver_no_check() {
    type_check(
        "\
local x: string = \"hi\"
local _ = x.sub",
        "",
    );
}

// ---------------------------------------------------------------------------
// Field access on require'd module with typed exports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn require_module_field_access() {
    type_check_with_builtins(
        "local _ = math.nonexistent",
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:11
  |
1 | local _ = math.nonexistent
  |           ^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'",
    );
}

// ---------------------------------------------------------------------------
// Global with no display name — variable name used as fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_no_alias_uses_variable_name() {
    type_check_with_builtins(
        "math.nonexistent()",
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:1
  |
1 | math.nonexistent()
  | ^^^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'",
    );
}

// ---------------------------------------------------------------------------
// Local assigned from global — qualified field name uses variable name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_from_global_not_callable() {
    type_check_with_builtins(
        "\
local m = math
m.pi()",
        "\
error[field_access]: field 'm.pi' is not callable (type is 'float')
 --> test.lua:2:1
  |
2 | m.pi()
  | ^^^^^^ field 'm.pi' is not callable (type is 'float')",
    );
}
