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

// ---------------------------------------------------------------------------
// Closed upvalues: closure outlives the enclosing frame
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upvalue_survives_frame_exit() {
    // Closure is returned from the enclosing function and called after
    // the enclosing frame has been popped.  The upvalue must be "closed"
    // (value copied into owned storage) before the frame is dropped.
    k9::assert_equal!(
        run_one(
            "local function make()
                 local x = 42
                 return function() return x end
             end
             local f = make()
             return f()"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn upvalue_mutation_after_frame_exit() {
    // Closure mutates a closed upvalue — the cell must be writable
    // even after the enclosing frame is gone.
    k9::assert_equal!(
        run_one(
            "local function make()
                 local n = 0
                 local function inc() n = n + 1 return n end
                 return inc
             end
             local inc = make()
             inc()
             inc()
             return inc()"
        )
        .await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn upvalue_shared_after_frame_exit() {
    // Two closures share an upvalue; both outlive the enclosing frame.
    // Mutations through one must be visible through the other.
    k9::assert_equal!(
        run_one(
            "local function make()
                 local x = 0
                 local function set(v) x = v end
                 local function get() return x end
                 return set, get
             end
             local set, get = make()
             set(77)
             return get()"
        )
        .await,
        Value::Integer(77)
    );
}

#[tokio::test]
async fn upvalue_nested_escape() {
    // Inner closure escapes through an outer closure, both capturing
    // different levels.  Both must close correctly.
    k9::assert_equal!(
        run_one(
            "local function outer()
                 local a = 10
                 local function middle()
                     local b = 20
                     local function inner() return a + b end
                     return inner
                 end
                 return middle()
             end
             local f = outer()
             return f()"
        )
        .await,
        Value::Integer(30)
    );
}

// ---------------------------------------------------------------------------
// Upvalue + vararg interaction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upvalue_in_variadic_function() {
    // Closure inside a variadic function captures a local (not varargs).
    k9::assert_equal!(
        run_one(
            "local function f(...)
                 local x = 100
                 local function get() return x end
                 return get()
             end
             return f(1, 2, 3)"
        )
        .await,
        Value::Integer(100)
    );
}

#[tokio::test]
async fn upvalue_mutation_in_variadic_function() {
    // Upvalue is mutated inside a variadic function, then read back.
    k9::assert_equal!(
        run_one(
            "local function f(...)
                 local sum = 0
                 local function add(v) sum = sum + v end
                 for _, v in ipairs({...}) do
                     add(v)
                 end
                 return sum
             end
             return f(10, 20, 30)"
        )
        .await,
        Value::Integer(60)
    );
}

// ---------------------------------------------------------------------------
// Upvalue + error unwind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upvalue_survives_pcall_error() {
    // Closure escapes via upvalue before an error; must remain usable
    // after pcall catches the error.
    k9::assert_equal!(
        run_one(
            "local saved
             pcall(function()
                 local x = 42
                 saved = function() return x end
                 error('boom')
             end)
             return saved()"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn upvalue_mutation_visible_after_pcall_error() {
    // Upvalue is mutated before error; closed value reflects the mutation.
    k9::assert_equal!(
        run_one(
            "local saved
             pcall(function()
                 local x = 0
                 saved = function() return x end
                 x = 99
                 error('boom')
             end)
             return saved()"
        )
        .await,
        Value::Integer(99)
    );
}

// ---------------------------------------------------------------------------
// Upvalue + loop iteration scoping
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upvalue_numeric_for_per_iteration_scoping() {
    // Lua 5.4 §3.3.5: each iteration of a numeric for gets its own
    // loop variable, so closures capture independent copies.
    k9::assert_equal!(
        run_one(
            "local f
             for i = 1, 3 do
                 if i == 2 then
                     f = function() return i end
                 end
             end
             return f()"
        )
        .await,
        Value::Integer(2)
    );
}

#[tokio::test]
async fn upvalue_numeric_for_each_iteration_separate() {
    // Each iteration of a numeric for captures its own i.
    k9::assert_equal!(
        run_all(
            "local t = {}
             for i = 1, 5 do
                 t[i] = function() return i end
             end
             return t[1](), t[3](), t[5]()"
        )
        .await,
        valuevec![Value::Integer(1), Value::Integer(3), Value::Integer(5)]
    );
}

#[tokio::test]
async fn upvalue_generic_for_per_iteration_scoping() {
    // Each iteration of a generic for also gets its own loop variables.
    // Test each closure individually to avoid multi-return truncation.
    k9::assert_equal!(
        run_all(
            "local t = {'a', 'b', 'c'}
             local fns = {}
             for i, v in ipairs(t) do
                 fns[i] = function() return i, v end
             end
             local i1, v1 = fns[1]()
             local i2, v2 = fns[2]()
             local i3, v3 = fns[3]()
             return i1, v1, i2, v2, i3, v3"
        )
        .await,
        valuevec![
            Value::Integer(1), Value::string("a"),
            Value::Integer(2), Value::string("b"),
            Value::Integer(3), Value::string("c"),
        ]
    );
}

#[tokio::test]
async fn upvalue_while_loop_shared() {
    // A while loop does NOT create a new scope per iteration, so
    // closures capture the same variable.
    k9::assert_equal!(
        run_one(
            "local fns = {}
             local i = 1
             while i <= 3 do
                 fns[i] = function() return i end
                 i = i + 1
             end
             -- All closures see the final value of i (4).
             return fns[1]()"
        )
        .await,
        Value::Integer(4)
    );
}
