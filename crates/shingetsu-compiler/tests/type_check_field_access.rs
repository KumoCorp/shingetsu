use shingetsu::diagnostic::{render_warnings, RenderStyle};
use shingetsu_compiler::{CompileOptions, Compiler};

fn type_check_opts() -> CompileOptions {
    CompileOptions {
        debug_info: true,
        source_name: "test.lua".into(),
        type_check: true,
    }
}

async fn check(src: &str) -> String {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

async fn check_with_builtins(src: &str) -> String {
    let env = shingetsu_vm::GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register");
    let compiler = Compiler::new(type_check_opts(), env.global_type_map());
    let bc = compiler.compile(src).await.expect("compile");
    render_warnings(&bc.diagnostics, src, RenderStyle::Plain)
}

// ---------------------------------------------------------------------------
// Unknown field on typed table (user-defined type)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_field_dot_access() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p.z",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'z' on type 'Point'
 --> test.lua:3:11
  |
3 | local _ = p.z
  |           ^^^ unknown field 'z' on type 'Point'"
    );
}

#[tokio::test]
async fn unknown_field_bracket_literal() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p[\"z\"]",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'z' on type 'Point'
 --> test.lua:3:11
  |
3 | local _ = p[\"z\"]
  |           ^^^^^ unknown field 'z' on type 'Point'"
    );
}

#[tokio::test]
async fn known_field_no_diagnostic() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p.x",
    )
    .await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn known_field_bracket_no_diagnostic() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local _ = p[\"x\"]",
    )
    .await;
    k9::assert_equal!(diags, "");
}

// ---------------------------------------------------------------------------
// Not callable: field exists but is not a function
// ---------------------------------------------------------------------------

#[tokio::test]
async fn not_callable_dot() {
    let diags = check(
        "\
type Info = { name: string, count: integer }
local t: Info = {}
t.name()",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: field 'Info.name' is not callable (type is 'string')
 --> test.lua:3:1
  |
3 | t.name()
  | ^^^^^^^^ field 'Info.name' is not callable (type is 'string')"
    );
}

#[tokio::test]
async fn not_callable_bracket() {
    let diags = check(
        "\
type Info = { name: string, count: integer }
local t: Info = {}
t[\"name\"]()",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: field 'Info.name' is not callable (type is 'string')
 --> test.lua:3:1
  |
3 | t[\"name\"]()
  | ^^^^^^^^^^^ field 'Info.name' is not callable (type is 'string')"
    );
}

#[tokio::test]
async fn callable_field_no_diagnostic() {
    let diags = check(
        "\
type Lib = { add: (a: number, b: number) -> number }
local M: Lib = {}
M.add(1, 2)",
    )
    .await;
    k9::assert_equal!(diags, "");
}

// ---------------------------------------------------------------------------
// Unknown field on call site
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_field_call() {
    let diags = check(
        "\
type Lib = { add: (a: number, b: number) -> number }
local M: Lib = {}
M.sub(1, 2)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'sub' on type 'Lib'
 --> test.lua:3:1
  |
3 | M.sub(1, 2)
  | ^^^^^^^^^^^ unknown field 'sub' on type 'Lib'"
    );
}

// ---------------------------------------------------------------------------
// Native module fields (math)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_module_unknown_field() {
    let diags = check_with_builtins("local _ = math.nonexistent").await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:11
  |
1 | local _ = math.nonexistent
  |           ^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'"
    );
}

#[tokio::test]
async fn native_module_known_field_no_diagnostic() {
    let diags = check_with_builtins("local _ = math.abs").await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn native_module_not_callable() {
    let diags = check_with_builtins("math.pi()").await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: field 'math.pi' is not callable (type is 'float')
 --> test.lua:1:1
  |
1 | math.pi()
  | ^^^^^^^^^ field 'math.pi' is not callable (type is 'float')"
    );
}

// ---------------------------------------------------------------------------
// Edge cases: skip checking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generic_table_no_check() {
    let diags = check(
        "\
local t: Table = {}
local _ = t.anything",
    )
    .await;
    let field_diags: Vec<&str> = diags
        .lines()
        .filter(|l| l.contains("field_access"))
        .collect();
    k9::assert_equal!(field_diags.len(), 0);
}

#[tokio::test]
async fn unknown_receiver_no_check() {
    let diags = check("local _ = unknown_var.foo").await;
    let field_diags: Vec<&str> = diags
        .lines()
        .filter(|l| l.contains("field_access"))
        .collect();
    k9::assert_equal!(field_diags.len(), 0);
}

#[tokio::test]
async fn bracket_non_literal_no_check() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local key = \"z\"
local _ = p[key]",
    )
    .await;
    let field_diags: Vec<&str> = diags
        .lines()
        .filter(|l| l.contains("field_access"))
        .collect();
    k9::assert_equal!(field_diags.len(), 0);
}

// ---------------------------------------------------------------------------
// Infer field type for downstream checks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn field_type_feeds_assign_check() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local n: string = p.x",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:3:19
  |
3 | local n: string = p.x
  |                   ^^^ expected 'string' but got 'number'
warning[unused_variable]: unused variable 'n'
 --> test.lua:3:7
  |
3 | local n: string = p.x
  |       ^ unused variable 'n'
  |
help: prefix the name with '_' to suppress this warning: '_n'"
    );
}

#[tokio::test]
async fn field_type_feeds_arg_check() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local function take_string(s: string) end
take_string(p.x)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[arg_type]: expected 'string' for parameter 's' but got 'number'
 --> test.lua:4:13
  |
4 | take_string(p.x)
  |             ^^^ expected 'string' for parameter 's' but got 'number'
warning[unused_variable]: unused variable 's'
 --> test.lua:3:28
  |
3 | local function take_string(s: string) end
  |                            ^ unused variable 's'
  |
help: prefix the name with '_' to suppress this warning: '_s'"
    );
}

// ---------------------------------------------------------------------------
// Table with indexer — skip field checking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_with_indexer_no_check() {
    let diags = check(
        "\
type Dict = { [string]: number }
local d: Dict = {}
local _ = d.anything",
    )
    .await;
    let field_diags: Vec<&str> = diags
        .lines()
        .filter(|l| l.contains("field_access"))
        .collect();
    k9::assert_equal!(field_diags.len(), 0);
}

// ---------------------------------------------------------------------------
// Method-call on unknown field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_call_unknown_field_diagnostic() {
    let diags = check(
        "\
type Obj = { greet: (self) -> string }
local o: Obj = {}
o:unknown()",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'unknown' on type 'Obj'
 --> test.lua:3:1
  |
3 | o:unknown()
  | ^^^^^^^^^^^ unknown field 'unknown' on type 'Obj'"
    );
}

// ---------------------------------------------------------------------------
// Non-Name prefix — silently skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parenthesized_prefix_no_check() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local function get_point(): Point end
local _ = (get_point()).z",
    )
    .await;
    let field_diags: Vec<&str> = diags
        .lines()
        .filter(|l| l.contains("field_access"))
        .collect();
    k9::assert_equal!(field_diags.len(), 0);
}

// ---------------------------------------------------------------------------
// Multi-level access — skipped (only single-level checked)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multi_level_access_no_check() {
    let diags = check(
        "\
type Inner = { val: number }
type Outer = { inner: Inner }
local o: Outer = {}
local _ = o.inner.nonexistent",
    )
    .await;
    let field_diags: Vec<&str> = diags
        .lines()
        .filter(|l| l.contains("field_access"))
        .collect();
    k9::assert_equal!(field_diags.len(), 0);
}

// ---------------------------------------------------------------------------
// Infer field type through bracket literal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn field_type_bracket_feeds_assign_check() {
    let diags = check(
        "\
type Point = { x: number, y: number }
local p: Point = {}
local n: string = p[\"x\"]",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[assign_type]: expected 'string' but got 'number'
 --> test.lua:3:19
  |
3 | local n: string = p[\"x\"]
  |                   ^^^^^ expected 'string' but got 'number'
warning[unused_variable]: unused variable 'n'
 --> test.lua:3:7
  |
3 | local n: string = p[\"x\"]
  |       ^ unused variable 'n'
  |
help: prefix the name with '_' to suppress this warning: '_n'"
    );
}

// ---------------------------------------------------------------------------
// Unknown field on native module via bracket
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_module_unknown_field_bracket() {
    let diags = check_with_builtins("local _ = math[\"nonexistent\"]").await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:11
  |
1 | local _ = math[\"nonexistent\"]
  |           ^^^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'"
    );
}

// ---------------------------------------------------------------------------
// Display name fallback — inline table type (no alias)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inline_table_type_fallback_display() {
    let diags = check(
        "\
local t: { x: number, y: number } = {}
local _ = t.z",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'z' on type 'table'
 --> test.lua:2:11
  |
2 | local _ = t.z
  |           ^^^ unknown field 'z' on type 'table'"
    );
}

// ---------------------------------------------------------------------------
// Native module unknown field in call position
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_module_unknown_field_call() {
    let diags = check_with_builtins("math.nonexistent(1)").await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:1
  |
1 | math.nonexistent(1)
  | ^^^^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'"
    );
}

// ---------------------------------------------------------------------------
// Method-call not-callable — colon syntax on non-function field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_call_not_callable() {
    let diags = check(
        "\
type Info = { name: string, count: integer }
local t: Info = {}
t:name()",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: field 'Info.name' is not callable (type is 'string')
 --> test.lua:3:1
  |
3 | t:name()
  | ^^^^^^^^ field 'Info.name' is not callable (type is 'string')"
    );
}

// ---------------------------------------------------------------------------
// Non-table receiver — silently skipped
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_table_receiver_no_check() {
    let diags = check(
        "\
local x: string = \"hi\"
local _ = x.sub",
    )
    .await;
    let field_diags: Vec<&str> = diags
        .lines()
        .filter(|l| l.contains("field_access"))
        .collect();
    k9::assert_equal!(field_diags.len(), 0);
}

// ---------------------------------------------------------------------------
// Field access on require'd module with typed exports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn require_module_field_access() {
    let diags = check_with_builtins("local _ = math.nonexistent").await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:11
  |
1 | local _ = math.nonexistent
  |           ^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'"
    );
}

// ---------------------------------------------------------------------------
// Global with no display name — variable name used as fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_no_alias_uses_variable_name() {
    let diags = check_with_builtins("math.nonexistent()").await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: unknown field 'nonexistent' on type 'math'
 --> test.lua:1:1
  |
1 | math.nonexistent()
  | ^^^^^^^^^^^^^^^^^^ unknown field 'nonexistent' on type 'math'"
    );
}

// ---------------------------------------------------------------------------
// Local assigned from global — qualified field name uses variable name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_from_global_not_callable() {
    let diags = check_with_builtins(
        "\
local m = math
m.pi()",
    )
    .await;
    k9::assert_equal!(
        diags,
        "\
error[field_access]: field 'm.pi' is not callable (type is 'float')
 --> test.lua:2:1
  |
2 | m.pi()
  | ^^^^^^ field 'm.pi' is not callable (type is 'float')"
    );
}
