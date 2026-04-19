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
// Basic: falls off the end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn falls_off_end() {
    k9::assert_equal!(
        check(
            "\
local function foo(): number
    local x = 42
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning 'number'
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:11
  |
2 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Basic: has return — no diagnostic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn has_return() {
    k9::assert_equal!(
        check(
            "\
local function foo(): number
    return 42
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// If/else: all branches return
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_else_all_return() {
    k9::assert_equal!(
        check(
            "\
local function classify(x: number): string
    if x > 0 then
        return \"positive\"
    else
        return \"non-positive\"
    end
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'classify'
 --> test.lua:1:16
  |
1 | local function classify(x: number): string
  |                ^^^^^^^^ unused function 'classify'
  |
help: prefix the name with '_' to suppress this warning: '_classify'"
    );
}

// ---------------------------------------------------------------------------
// If without else: missing path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_without_else() {
    k9::assert_equal!(
        check(
            "\
local function classify(x: number): string
    if x > 0 then
        return \"positive\"
    end
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'string'
warning[unused_variable]: unused function 'classify'
 --> test.lua:1:16
  |
1 | local function classify(x: number): string
  |                ^^^^^^^^ unused function 'classify'
  |
help: prefix the name with '_' to suppress this warning: '_classify'"
    );
}

// ---------------------------------------------------------------------------
// If/elseif/else: all return
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_elseif_else_all_return() {
    k9::assert_equal!(
        check(
            "\
local function classify(x: number): string
    if x > 0 then
        return \"positive\"
    elseif x < 0 then
        return \"negative\"
    else
        return \"zero\"
    end
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'classify'
 --> test.lua:1:16
  |
1 | local function classify(x: number): string
  |                ^^^^^^^^ unused function 'classify'
  |
help: prefix the name with '_' to suppress this warning: '_classify'"
    );
}

// ---------------------------------------------------------------------------
// If/elseif/else: one branch missing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_elseif_else_one_branch_missing() {
    k9::assert_equal!(
        check(
            "\
local function classify(x: number): string
    if x > 0 then
        return \"positive\"
    elseif x < 0 then
        local _ = x
    else
        return \"zero\"
    end
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:9:1
  |
9 | end
  | ^^^ function may fall off the end without returning 'string'
warning[unused_variable]: unused function 'classify'
 --> test.lua:1:16
  |
1 | local function classify(x: number): string
  |                ^^^^^^^^ unused function 'classify'
  |
help: prefix the name with '_' to suppress this warning: '_classify'"
    );
}

// ---------------------------------------------------------------------------
// error() at the end — never returns
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_call_no_diagnostic() {
    k9::assert_equal!(
        check_with_builtins(
            "\
local function foo(): number
    error(\"boom\")
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// No return type annotation — no diagnostic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_return_type() {
    k9::assert_equal!(
        check(
            "\
local function foo()
    local x = 42
end"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:11
  |
2 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo()
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Return type `any` — no diagnostic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_any() {
    k9::assert_equal!(
        check(
            "\
local function foo(): any
    local x = 42
end"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:11
  |
2 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): any
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Function expression
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_expression() {
    k9::assert_equal!(
        check(
            "\
local foo = function(): number
    local x = 42
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning 'number'
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:11
  |
2 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused variable 'foo'
 --> test.lua:1:7
  |
1 | local foo = function(): number
  |       ^^^ unused variable 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Function declaration (t.f = function)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_declaration() {
    k9::assert_equal!(
        check(
            "\
function test(): number
    local x = 42
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning 'number'
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:11
  |
2 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'"
    );
}

// ---------------------------------------------------------------------------
// Do block wrapping a return
// ---------------------------------------------------------------------------

#[tokio::test]
async fn do_block_with_return() {
    k9::assert_equal!(
        check(
            "\
local function foo(): number
    do
        return 42
    end
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Nested if inside do
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nested_if_in_do() {
    k9::assert_equal!(
        check(
            "\
local function foo(x: number): string
    do
        if x > 0 then
            return \"yes\"
        else
            return \"no\"
        end
    end
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// User-defined never-returning function
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_defined_never_function() {
    k9::assert_equal!(
        check(
            "\
local function crash(): never
    error(\"fatal\")
end
local function foo(): number
    crash()
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:4:16
  |
4 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Multiple return values
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_return_values() {
    k9::assert_equal!(
        check(
            "\
local function foo(): (number, string)
    local x = 42
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning ('number', 'string')
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning ('number', 'string')
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:11
  |
2 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): (number, string)
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// error() in one branch of if/else
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_in_else_branch() {
    k9::assert_equal!(
        check_with_builtins(
            "\
local function foo(x: number): string
    if x > 0 then
        return \"positive\"
    else
        error(\"invalid\")
    end
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// If without else followed by return — always terminates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_without_else_then_return() {
    k9::assert_equal!(
        check(
            "\
local function foo(x: number): string
    if x > 0 then
        return \"positive\"
    end
    return \"non-positive\"
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Empty function body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_body() {
    k9::assert_equal!(
        check("local function foo(): number end").await,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:1:30
  |
1 | local function foo(): number end
  |                              ^^^ function may fall off the end without returning 'number'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number end
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Return type `unknown` — skip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_unknown() {
    k9::assert_equal!(
        check(
            "\
local function foo(): unknown
    local x = 42
end"
        )
        .await,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:2:11
  |
2 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): unknown
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Return type `never` — skip (function is expected to diverge)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_never() {
    k9::assert_equal!(
        check_with_builtins(
            "\
local function crash(): never
    error(\"boom\")
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'crash'
 --> test.lua:1:16
  |
1 | local function crash(): never
  |                ^^^^^ unused function 'crash'
  |
help: prefix the name with '_' to suppress this warning: '_crash'"
    );
}

// ---------------------------------------------------------------------------
// Nested function: inner missing, outer ok
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nested_inner_missing_outer_ok() {
    k9::assert_equal!(
        check(
            "\
local function outer(): number
    local function inner(): string
        local _ = 1
    end
    return 42
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:4:5
  |
4 |     end
  |     ^^^ function may fall off the end without returning 'string'
warning[unused_variable]: unused function 'inner'
 --> test.lua:2:20
  |
2 |     local function inner(): string
  |                    ^^^^^ unused function 'inner'
  |
help: prefix the name with '_' to suppress this warning: '_inner'
warning[unused_variable]: unused function 'outer'
 --> test.lua:1:16
  |
1 | local function outer(): number
  |                ^^^^^ unused function 'outer'
  |
help: prefix the name with '_' to suppress this warning: '_outer'"
    );
}

// ---------------------------------------------------------------------------
// error() via module dot call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_via_module_dot_call() {
    k9::assert_equal!(
        check(
            "\
type Mod = { fatal: () -> never }
local mod_: Mod = {}
local function handler(): string
    mod_.fatal()
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'handler'
 --> test.lua:3:16
  |
3 | local function handler(): string
  |                ^^^^^^^ unused function 'handler'
  |
help: prefix the name with '_' to suppress this warning: '_handler'"
    );
}

// ---------------------------------------------------------------------------
// error() via method call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_via_method_call() {
    k9::assert_equal!(
        check(
            "\
type Obj = { fail: (self) -> never }
local o: Obj = {}
local function handler(): string
    o:fail()
end"
        )
        .await,
        "\
error[arg_count]: expected 1 argument but got 0
 --> test.lua:4:11
  |
4 |     o:fail()
  |           ^^ expected 1 argument but got 0
warning[unused_variable]: unused function 'handler'
 --> test.lua:3:16
  |
3 | local function handler(): string
  |                ^^^^^^^ unused function 'handler'
  |
help: prefix the name with '_' to suppress this warning: '_handler'"
    );
}

// ---------------------------------------------------------------------------
// If with else but else body is empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_with_empty_else() {
    k9::assert_equal!(
        check(
            "\
local function foo(x: number): string
    if x > 0 then
        return \"positive\"
    else
    end
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:6:1
  |
6 | end
  | ^^^ function may fall off the end without returning 'string'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Deeply nested: if inside do inside if, all returning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deeply_nested_all_return() {
    k9::assert_equal!(
        check(
            "\
local function foo(x: number, y: number): string
    if x > 0 then
        do
            if y > 0 then
                return \"both positive\"
            else
                return \"x positive\"
            end
        end
    else
        return \"x non-positive\"
    end
end"
        )
        .await,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number, y: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

async fn check_filtered(src: &str) -> String {
    let compiler = Compiler::new(type_check_opts(), Default::default());
    let bc = compiler.compile(src).await.expect("compile");
    let filtered = bc.lint_directives.filter(bc.diagnostics);
    render_warnings(&filtered, src, RenderStyle::Plain)
}

// ---------------------------------------------------------------------------
// Elseif without else — should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn elseif_without_else() {
    k9::assert_equal!(
        check(
            "\
local function foo(x: number): string
    if x > 0 then
        return \"positive\"
    elseif x < 0 then
        return \"negative\"
    end
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:7:1
  |
7 | end
  | ^^^ function may fall off the end without returning 'string'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Method definition syntax
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_definition() {
    k9::assert_equal!(
        check(
            "\
local t = {}
function t:greet(): string
    local _ = self
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:4:1
  |
4 | end
  | ^^^ function may fall off the end without returning 'string'"
    );
}

// ---------------------------------------------------------------------------
// Return type nil — nil is a real type, should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_nil() {
    k9::assert_equal!(
        check(
            "\
local function foo(): nil
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'nil'
 --> test.lua:2:1
  |
2 | end
  | ^^^ function may fall off the end without returning 'nil'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): nil
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Loop as last statement — should not suppress
// ---------------------------------------------------------------------------

#[tokio::test]
async fn loop_as_last_stmt() {
    k9::assert_equal!(
        check(
            "\
local function foo(): number
    for i = 1, 10 do
        local _ = i
    end
end"
        )
        .await,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'number'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Never call not at end of block — should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_call_not_last() {
    let d = check_with_builtins(
        "\
local function foo(): number
    error(\"boom\")
    local x = 42
end",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:3:11
  |
3 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'
warning[unreachable_code]: unreachable code
 --> test.lua:3:5
  |
3 |     local x = 42
  |     ^^^^^ unreachable code"
    );
}

// ---------------------------------------------------------------------------
// Lint directive suppression
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lint_directive_suppression() {
    let d = check_filtered(
        "\
--# shingetsu: allow(missing_return)
local function foo(): number
    local x = 42
end",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
warning[unused_variable]: unused variable 'x'
 --> test.lua:3:11
  |
3 |     local x = 42
  |           ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
warning[unused_variable]: unused function 'foo'
 --> test.lua:2:16
  |
2 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// If/else that always returns, not at end of block
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_terminates_not_last() {
    let d = check(
        "\
local function foo(x: number): string
    if x > 0 then
        return \"positive\"
    else
        return \"non-positive\"
    end
    local _ = 1
end",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
warning[unreachable_code]: unreachable code
 --> test.lua:7:5
  |
7 |     local _ = 1
  |     ^^^^^ unreachable code
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// While loop as last statement — should not suppress
// ---------------------------------------------------------------------------

#[tokio::test]
async fn while_loop_as_last_stmt() {
    let d = check(
        "\
local function foo(): number
    while true do
        local _ = 1
    end
end",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'number'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Do block without return as last statement — should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn do_block_no_return() {
    let d = check(
        "\
local function foo(): number
    do
        local _ = 1
    end
end",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'number'
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(): number
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'"
    );
}

// ---------------------------------------------------------------------------
// Never call in middle followed by incomplete if
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_call_in_middle_then_if() {
    let d = check_with_builtins(
        "\
local function foo(x: number): string
    error(\"fatal\")
    if x > 0 then
        return \"positive\"
    end
end",
    )
    .await;
    k9::assert_equal!(
        d,
        "\
warning[unused_variable]: unused function 'foo'
 --> test.lua:1:16
  |
1 | local function foo(x: number): string
  |                ^^^ unused function 'foo'
  |
help: prefix the name with '_' to suppress this warning: '_foo'
warning[unreachable_code]: unreachable code
 --> test.lua:3:5
  |
3 |     if x > 0 then
  |     ^^ unreachable code"
    );
}
