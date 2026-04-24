mod common;

use common::{run_all, run_one};
use shingetsu::valuevec;
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// Upvalue / closure tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upvalue_read() {
    // Closure captures a local from the enclosing function and reads it.
    k9::assert_equal!(
        run_one(
            "local x = 42
local function get() return x end
return get()"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn upvalue_write_from_closure() {
    // Closure writes through an upvalue; outer function reads the updated value.
    k9::assert_equal!(
        run_one(
            "local x = 0
local function inc() x = x + 1 end
inc()
inc()
return x"
        )
        .await,
        Value::Integer(2)
    );
}

#[tokio::test]
async fn upvalue_shared_between_closures() {
    // Two closures share the same upvalue cell; mutations are visible to both.
    k9::assert_equal!(
        run_one(
            "local x = 10
local function set(v) x = v end
local function get() return x end
set(99)
return get()"
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn upvalue_counter() {
    // Classic counter closure.
    k9::assert_equal!(
        run_one(
            "local count = 0
local function inc() count = count + 1 return count end
inc()
inc()
return inc()"
        )
        .await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn upvalue_in_loop() {
    // Closure created inside a loop captures the loop variable.
    k9::assert_equal!(
        run_one(
            "local last = nil
for i = 1, 3 do
    local function f() last = i end
    f()
end
return last"
        )
        .await,
        Value::Integer(3)
    );
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Multi-level upvalue capture (3+ nesting depths)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upvalue_grandparent_read() {
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
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn upvalue_grandparent_write() {
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
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn upvalue_four_levels_deep() {
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
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn upvalue_counter_via_closure_chain() {
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
        )
        .await,
        Value::Integer(4)
    );
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// <close> variables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn close_normal_exit() {
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
        )
        .await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn close_pcall_error_unwind() {
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
        )
        .await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn close_lifo_order() {
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
        )
        .await,
        Value::string("cba")
    );
}

#[tokio::test]
async fn close_pcall_error_returns_false() {
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
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn generic_for_close_on_error_unwind() {
    // Verify that error() inside a generic-for triggers __close on
    // the 4th hidden variable.  We use a custom iterator that returns
    // a closeable sentinel as the 4th value.
    k9::assert_equal!(
        run_one(
            r#"local closed = false
local function my_iter()
    local done = false
    local function iter()
        if done then return nil end
        done = true
        return "value"
    end
    local sentinel = setmetatable({}, {
        __close = function() closed = true end
    })
    return iter, nil, nil, sentinel
end
local ok = pcall(function()
    for v in my_iter() do
        error("boom")
    end
end)
return closed"#
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn generic_for_close_nil_is_noop() {
    // pairs/ipairs return 3 values, so the 4th (closing) slot is nil.
    // CloseVar on nil must be a no-op — no crash.
    k9::assert_equal!(
        run_one(
            r#"local sum = 0
for i, v in ipairs({10, 20, 30}) do
    sum = sum + v
end
return sum"#
        )
        .await,
        Value::Integer(60)
    );
}

#[tokio::test]
async fn generic_for_close_table_with_close_metamethod() {
    // Verify that a table with __close returned as the 4th value
    // from a generic-for expression list gets its __close called.
    k9::assert_equal!(
        run_one(
            r#"local closed = false
local function my_iter()
    local done = false
    local function iter()
        if done then return nil end
        done = true
        return "value"
    end
    local sentinel = setmetatable({}, {
        __close = function() closed = true end
    })
    return iter, nil, nil, sentinel
end
for v in my_iter() do
end
return closed"#
        )
        .await,
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// Arithmetic and comparison on upvalue-captured registers.
// These exercise the open_upvalues fallback path in int_fast_binary_op!
// and comparison fast paths (the direct-register fast path is skipped
// when open_upvalues is non-empty).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upvalue_arithmetic_all_ops() {
    k9::assert_equal!(
        run_all(
            "local a = 10
             local function ops()
                 local add = a + 3
                 local sub = a - 3
                 local mul = a * 3
                 local band = a & 6
                 local bor = a | 5
                 local bxor = a ~ 3
                 a = a + 1
                 return add, sub, mul, band, bor, bxor, a
             end
             return ops()"
        )
        .await,
        valuevec![
            Value::Integer(13),
            Value::Integer(7),
            Value::Integer(30),
            Value::Integer(2),
            Value::Integer(15),
            Value::Integer(9),
            Value::Integer(11),
        ]
    );
}

#[tokio::test]
async fn upvalue_comparison_ops() {
    k9::assert_equal!(
        run_all(
            "local x = 5
             local function cmp()
                 return x < 10, x <= 5, x > 3, x >= 5
             end
             return cmp()"
        )
        .await,
        valuevec![
            Value::Boolean(true),
            Value::Boolean(true),
            Value::Boolean(true),
            Value::Boolean(true),
        ]
    );
}

#[tokio::test]
async fn upvalue_move_and_return() {
    k9::assert_equal!(
        run_all(
            "local a = 42
             local b = 'hello'
             local function get()
                 return a, b
             end
             return get()"
        )
        .await,
        valuevec![Value::Integer(42), Value::string("hello")]
    );
}
