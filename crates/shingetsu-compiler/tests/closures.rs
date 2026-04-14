mod common;

use common::run_one;
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// Upvalue / closure tests
// ---------------------------------------------------------------------------

#[test]
fn upvalue_read() {
    // Closure captures a local from the enclosing function and reads it.
    k9::assert_equal!(
        run_one(
            "local x = 42
local function get() return x end
return get()"
        ),
        Value::Integer(42)
    );
}

#[test]
fn upvalue_write_from_closure() {
    // Closure writes through an upvalue; outer function reads the updated value.
    k9::assert_equal!(
        run_one(
            "local x = 0
local function inc() x = x + 1 end
inc()
inc()
return x"
        ),
        Value::Integer(2)
    );
}

#[test]
fn upvalue_shared_between_closures() {
    // Two closures share the same upvalue cell; mutations are visible to both.
    k9::assert_equal!(
        run_one(
            "local x = 10
local function set(v) x = v end
local function get() return x end
set(99)
return get()"
        ),
        Value::Integer(99)
    );
}

#[test]
fn upvalue_counter() {
    // Classic counter closure.
    k9::assert_equal!(
        run_one(
            "local count = 0
local function inc() count = count + 1 return count end
inc()
inc()
return inc()"
        ),
        Value::Integer(3)
    );
}

#[test]
fn upvalue_in_loop() {
    // Closure created inside a loop captures the loop variable.
    k9::assert_equal!(
        run_one(
            "local last = nil
for i = 1, 3 do
    local function f() last = i end
    f()
end
return last"
        ),
        Value::Integer(3)
    );
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Multi-level upvalue capture (3+ nesting depths)
// ---------------------------------------------------------------------------

#[test]
fn upvalue_grandparent_read() {
    // C captures x from A (skipping over B which doesn't use x).
    k9::assert_equal!(
        run_one(
            "local x = 42
local function B()
    local function C()
        return x
    end
    return C()
end
return B()"
        ),
        Value::Integer(42)
    );
}

#[test]
fn upvalue_grandparent_write() {
    // C mutates x (owned by A); B sees the mutation too.
    k9::assert_equal!(
        run_one(
            "local x = 1
local function B()
    local function C()
        x = 99
    end
    C()
    return x
end
return B()"
        ),
        Value::Integer(99)
    );
}

#[test]
fn upvalue_four_levels_deep() {
    // D captures x from A (4 levels deep).
    k9::assert_equal!(
        run_one(
            "local x = 7
local function A2()
    local function B2()
        local function C2()
            return x
        end
        return C2()
    end
    return B2()
end
return A2()"
        ),
        Value::Integer(7)
    );
}

#[test]
fn upvalue_counter_via_closure_chain() {
    // Classic counter pattern: inner closure mutates counter owned by outer.
    k9::assert_equal!(
        run_one(
            "local function make_counter()
    local n = 0
    local function get()
        return n
    end
    local function inc()
        n = n + 1
    end
    local function make_double_inc()
        -- This function is 3 levels deep from n.
        local function do_it()
            inc()
            inc()
        end
        return do_it
    end
    return get, make_double_inc()
end
local get, double_inc = make_counter()
double_inc()
double_inc()
return get()"
        ),
        Value::Integer(4)
    );
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// <close> variables
// ---------------------------------------------------------------------------

#[test]
fn close_normal_exit() {
    // __close is called when the scope exits normally.
    // Uses an upvalue counter since table.insert is not yet in stdlib.
    k9::assert_equal!(
        run_one(
            r#"local closed = 0
local mt = { __close = function(self) closed = closed + 1 end }
do
    local x <close> = setmetatable({}, mt)
end
return closed"#
        ),
        Value::Integer(1)
    );
}

#[test]
fn close_pcall_error_unwind() {
    // __close is called when the scope is exited via pcall-caught error.
    k9::assert_equal!(
        run_one(
            r#"local unwound = 0
local mt = { __close = function(self) unwound = unwound + 1 end }
local ok = pcall(function()
    local x <close> = setmetatable({}, mt)
    error("oops")
end)
return unwound"#
        ),
        Value::Integer(1)
    );
}

#[test]
fn close_lifo_order() {
    // Multiple <close> vars are closed in reverse declaration order.
    // Each closer appends its name to a string upvalue.
    k9::assert_equal!(
        run_one(
            r#"local order = ""
local function make(name)
    return setmetatable({}, { __close = function() order = order .. name end })
end
do
    local a <close> = make("a")
    local b <close> = make("b")
    local c <close> = make("c")
end
return order"#
        ),
        Value::String(bytes::Bytes::from_static(b"cba"))
    );
}

#[test]
fn close_pcall_error_returns_false() {
    // pcall still returns false, err even when __close is invoked.
    k9::assert_equal!(
        run_one(
            r#"local closed = 0
local mt = { __close = function() closed = closed + 1 end }
local ok, err = pcall(function()
    local x <close> = setmetatable({}, mt)
    error("boom")
end)
return ok"#
        ),
        Value::Boolean(false)
    );
}
