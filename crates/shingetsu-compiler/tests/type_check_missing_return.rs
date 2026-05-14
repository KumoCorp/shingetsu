mod common;
use common::{type_check, type_check_filtered, type_check_with_builtins};
// ---------------------------------------------------------------------------
// Basic: falls off the end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn falls_off_end() {
    type_check(
            "\
local function _foo(): number
    local _x = 42
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning 'number'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Basic: has return — no diagnostic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn has_return() {
    type_check(
        "\
local function _foo(): number
    return 42
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// If/else: all branches return
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_else_all_return() {
    type_check(
        "\
local function _classify(x: number): string
    if x > 0 then
        return \"positive\"
    else
        return \"non-positive\"
    end
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// If without else: missing path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_without_else() {
    type_check(
            "\
local function _classify(x: number): string
    if x > 0 then
        return \"positive\"
    end
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'string'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// If/elseif/else: all return
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_elseif_else_all_return() {
    type_check(
        "\
local function _classify(x: number): string
    if x > 0 then
        return \"positive\"
    elseif x < 0 then
        return \"negative\"
    else
        return \"zero\"
    end
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// If/elseif/else: one branch missing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_elseif_else_one_branch_missing() {
    type_check(
            "\
local function _classify(x: number): string
    if x > 0 then
        return \"positive\"
    elseif x < 0 then
        local _ = x
    else
        return \"zero\"
    end
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:9:1
  |
9 | end
  | ^^^ function may fall off the end without returning 'string'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// error() at the end — never returns
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_call_no_diagnostic() {
    type_check_with_builtins(
        "\
local function _foo(): number
    error(\"boom\")
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// No return type annotation — no diagnostic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_return_type() {
    type_check(
        "\
local function _foo()
    local _x = 42
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// Return type `any` — no diagnostic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_any() {
    type_check(
        "\
local function _foo(): any
    local _x = 42
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// Function expression
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_expression() {
    type_check(
            "\
local _foo = function(): number
    local _x = 42
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning 'number'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Function declaration (t.f = function)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_declaration() {
    type_check(
            "\
function test(): number
    local _x = 42
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning 'number'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Do block wrapping a return
// ---------------------------------------------------------------------------

#[tokio::test]
async fn do_block_with_return() {
    type_check(
        "\
local function _foo(): number
    do
        return 42
    end
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// Nested if inside do
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nested_if_in_do() {
    type_check(
        "\
local function _foo(x: number): string
    do
        if x > 0 then
            return \"yes\"
        else
            return \"no\"
        end
    end
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// User-defined never-returning function
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_defined_never_function() {
    type_check(
        "\
local function _crash(): never
    error(\"fatal\")
end
local function _foo(): number
    _crash()
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// Multiple return values
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_return_values() {
    type_check(
            "\
local function _foo(): (number, string)
    local _x = 42
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning ('number', 'string')
 --> test.lua:3:1
  |
3 | end
  | ^^^ function may fall off the end without returning ('number', 'string')
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// error() in one branch of if/else
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_in_else_branch() {
    type_check_with_builtins(
        "\
local function _foo(x: number): string
    if x > 0 then
        return \"positive\"
    else
        error(\"invalid\")
    end
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// If without else followed by return — always terminates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_without_else_then_return() {
    type_check(
        "\
local function _foo(x: number): string
    if x > 0 then
        return \"positive\"
    end
    return \"non-positive\"
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// Empty function body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_body() {
    type_check("local function _foo(): number end",
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:1:31
  |
1 | local function _foo(): number end
  |                               ^^^ function may fall off the end without returning 'number'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Return type `unknown` — skip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_unknown() {
    type_check(
        "\
local function _foo(): unknown
    local _x = 42
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// Return type `never` — skip (function is expected to diverge)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_never() {
    type_check_with_builtins(
        "\
local function _crash(): never
    error(\"boom\")
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// Nested function: inner missing, outer ok
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nested_inner_missing_outer_ok() {
    type_check(
            "\
local function _outer(): number
    local function _inner(): string
        local _ = 1
    end
    return 42
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:4:5
  |
4 |     end
  |     ^^^ function may fall off the end without returning 'string'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// error() via module dot call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_via_module_dot_call() {
    type_check(
        "\
type Mod = { fatal: () -> never }
local mod_: Mod = {}
local function _handler(): string
    mod_.fatal()
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// error() via method call
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_via_method_call() {
    type_check(
        "\
type Obj = { fail: (self) -> never }
local o: Obj = {}
local function _handler(): string
    o:fail()
end",
        "\
error[arg_count]: expected 1 argument but got 0
 --> test.lua:4:11
  |
4 |     o:fail()
  |           ^^ expected 1 argument but got 0",
    );
}

// ---------------------------------------------------------------------------
// If with else but else body is empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_with_empty_else() {
    type_check(
            "\
local function _foo(x: number): string
    if x > 0 then
        return \"positive\"
    else
    end
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:6:1
  |
6 | end
  | ^^^ function may fall off the end without returning 'string'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Deeply nested: if inside do inside if, all returning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deeply_nested_all_return() {
    type_check(
        "\
local function _foo(x: number, y: number): string
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
end",
        "",
    );
}
// ---------------------------------------------------------------------------
// Elseif without else — should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn elseif_without_else() {
    type_check(
            "\
local function _foo(x: number): string
    if x > 0 then
        return \"positive\"
    elseif x < 0 then
        return \"negative\"
    end
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:7:1
  |
7 | end
  | ^^^ function may fall off the end without returning 'string'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Method definition syntax
// ---------------------------------------------------------------------------

#[tokio::test]
async fn method_definition() {
    type_check(
            "\
local t = {}
function t:greet(): string
    local _ = self
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'string'
 --> test.lua:4:1
  |
4 | end
  | ^^^ function may fall off the end without returning 'string'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Return type nil — nil is a real type, should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn return_type_nil() {
    type_check(
            "\
local function _foo(): nil
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'nil'
 --> test.lua:2:1
  |
2 | end
  | ^^^ function may fall off the end without returning 'nil'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Loop as last statement — should not suppress
// ---------------------------------------------------------------------------

#[tokio::test]
async fn loop_as_last_stmt() {
    type_check(
            "\
local function _foo(): number
    for i = 1, 10 do
        local _ = i
    end
end"
        ,
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'number'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Never call not at end of block — should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_call_not_last() {
    type_check_with_builtins(
        "\
local function _foo(): number
    error(\"boom\")
    local _x = 42
end",
        "\
warning[unreachable_code]: unreachable code
 --> test.lua:3:5
  |
3 |     local _x = 42
  |     ^^^^^ unreachable code",
    );
}

// ---------------------------------------------------------------------------
// Lint directive suppression
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lint_directive_suppression() {
    type_check_filtered(
        "\
--# shingetsu: allow(missing_return)
local function _foo(): number
    local _x = 42
end",
        "",
    );
}

// ---------------------------------------------------------------------------
// If/else that always returns, not at end of block
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_terminates_not_last() {
    type_check(
        "\
local function _foo(x: number): string
    if x > 0 then
        return \"positive\"
    else
        return \"non-positive\"
    end
    local _ = 1
end",
        "\
warning[unreachable_code]: unreachable code
 --> test.lua:7:5
  |
7 |     local _ = 1
  |     ^^^^^ unreachable code",
    );
}

// ---------------------------------------------------------------------------
// While loop as last statement — should not suppress
// ---------------------------------------------------------------------------

#[tokio::test]
async fn while_loop_as_last_stmt() {
    type_check(
        "\
local function _foo(): number
    while true do
        local _ = 1
    end
end",
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'number'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Do block without return as last statement — should trigger
// ---------------------------------------------------------------------------

#[tokio::test]
async fn do_block_no_return() {
    type_check(
        "\
local function _foo(): number
    do
        local _ = 1
    end
end",
        "\
error[missing_return]: function may fall off the end without returning 'number'
 --> test.lua:5:1
  |
5 | end
  | ^^^ function may fall off the end without returning 'number'
  |
help: every code path through the function must end in `return <value>` or `error(...)` when the signature declares a return type"
    );
}

// ---------------------------------------------------------------------------
// Never call in middle followed by incomplete if
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_call_in_middle_then_if() {
    type_check_with_builtins(
        "\
local function _foo(x: number): string
    error(\"fatal\")
    if x > 0 then
        return \"positive\"
    end
end",
        "\
warning[unreachable_code]: unreachable code
 --> test.lua:3:5
  |
3 |     if x > 0 then
  |     ^^ unreachable code",
    );
}
