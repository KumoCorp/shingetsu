mod common;

use common::{compile_err, run_all, run_err, run_one};
use shingetsu::valuevec;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// Numeric literals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn integer_literal() {
    k9::assert_equal!(run_one("return 42").await, Value::Integer(42));
}

#[tokio::test]
async fn float_literal() {
    k9::assert_equal!(run_one("return 3.14").await, Value::Float(3.14));
}

#[tokio::test]
async fn negative_literal() {
    k9::assert_equal!(run_one("return -7").await, Value::Integer(-7));
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_integers() {
    k9::assert_equal!(run_one("return 10 + 20").await, Value::Integer(30));
}

#[tokio::test]
async fn sub_integers() {
    k9::assert_equal!(run_one("return 100 - 37").await, Value::Integer(63));
}

#[tokio::test]
async fn mul_integers() {
    k9::assert_equal!(run_one("return 6 * 7").await, Value::Integer(42));
}

#[tokio::test]
async fn float_div() {
    // `/` always returns float.
    k9::assert_equal!(run_one("return 10 / 4").await, Value::Float(2.5));
}

#[tokio::test]
async fn floor_div() {
    k9::assert_equal!(run_one("return 10 // 3").await, Value::Integer(3));
}

#[tokio::test]
async fn modulo() {
    k9::assert_equal!(run_one("return 10 % 3").await, Value::Integer(1));
}

#[tokio::test]
async fn exponent() {
    k9::assert_equal!(run_one("return 2 ^ 10").await, Value::Float(1024.0));
}

#[tokio::test]
async fn unary_minus() {
    k9::assert_equal!(run_one("local x = 5; return -x").await, Value::Integer(-5));
}

#[tokio::test]
async fn integer_mixed_float() {
    // integer + float → float.
    k9::assert_equal!(run_one("return 1 + 1.5").await, Value::Float(2.5));
}

// ---------------------------------------------------------------------------
// String-to-number coercion in arithmetic (Lua 5.4 §3.4.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arith_coerces_integer_string() {
    // String parses as integer → result is integer.
    let res = run_all(
        r#"
        return "101" - 3,
               5 + "7",
               "5" + "3",
               -"7",
               "6" * "7"
    "#,
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Integer(98),
            Value::Integer(12),
            Value::Integer(8),
            Value::Integer(-7),
            Value::Integer(42),
        ]
    );
}

#[tokio::test]
async fn arith_coerces_float_string_to_float() {
    // String parses as float → result is float (per the "usual rule").
    let res = run_all(
        r#"
        return "2.5" * 2,
               "3.14" + 0,
               1 + "0.5",
               "1e2" - 1
    "#,
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Float(5.0),
            Value::Float(3.14),
            Value::Float(1.5),
            Value::Float(99.0),
        ]
    );
}

#[tokio::test]
async fn arith_coerces_hex_integer_string() {
    let res = run_one(r#"return "0xff" + 1"#).await;
    k9::assert_equal!(res, Value::Integer(0x100));
}

#[tokio::test]
async fn arith_coerces_hex_float_string() {
    // Hex floats (binary exponent or fractional part) parse to
    // `Number::Float` and propagate as the float operand of the
    // operation.  0x1.8p4 = 1.5 * 2^4 = 24.0; 0xA.Bp3 = 85.5.
    let res = run_all(
        r#"
        return "0x1.8p4" + 0,
               "0xA.Bp3" * 2,
               "0xF0.0" - 0
    "#,
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![Value::Float(24.0), Value::Float(171.0), Value::Float(240.0),]
    );
}

#[tokio::test]
async fn arith_idiv_and_mod_preserve_integer_for_string_operands() {
    // `//` and `%` keep integer-typed result when both string operands
    // parse as integers.  Float operands fall through to the float path.
    let res = run_all(
        r#"
        return "10" // "3",
               "10" % "3",
               "10.0" // 3,
               10 % "3.0"
    "#,
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Integer(3),
            Value::Integer(1),
            Value::Float(3.0),
            Value::Float(1.0),
        ]
    );
}

#[tokio::test]
async fn bitwise_does_not_coerce_strings() {
    // Per Lua 5.4 §3.4.3, bitwise ops accept integers and
    // integer-valued floats but NOT strings.
    let rendered = run_err(r#"return "0xff" | 0"#).await;
    k9::assert_equal!(
        rendered,
        "\
error: attempt to perform arithmetic on a string value
 --> test.lua:1:8
  |
1 | return \"0xff\" | 0
  |        ^^^^^^^^^^ attempt to perform arithmetic on a string value
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn bitwise_accepts_integer_valued_float() {
    // `2.0` has an exact integer value, so `2.0 | 1` works.
    k9::assert_equal!(run_one("return 2.0 | 1").await, Value::Integer(3));
}

#[tokio::test]
async fn bitwise_rejects_non_integer_float() {
    let rendered = run_err(r#"return 2.5 | 1"#).await;
    k9::assert_equal!(
        rendered,
        "\
error: attempt to perform arithmetic on a number value
 --> test.lua:1:8
  |
1 | return 2.5 | 1
  |        ^^^^^^^ attempt to perform arithmetic on a number value
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Bitwise
// ---------------------------------------------------------------------------

#[tokio::test]
async fn band() {
    k9::assert_equal!(run_one("return 0xFF & 0x0F").await, Value::Integer(0x0F));
}

#[tokio::test]
async fn bor() {
    k9::assert_equal!(run_one("return 0xF0 | 0x0F").await, Value::Integer(0xFF));
}

#[tokio::test]
async fn bxor() {
    k9::assert_equal!(run_one("return 0xFF ~ 0x0F").await, Value::Integer(0xF0));
}

#[tokio::test]
async fn bnot() {
    k9::assert_equal!(run_one("return ~0").await, Value::Integer(-1));
}

#[tokio::test]
async fn shl() {
    k9::assert_equal!(run_one("return 1 << 4").await, Value::Integer(16));
}

#[tokio::test]
async fn shr() {
    k9::assert_equal!(run_one("return 16 >> 2").await, Value::Integer(4));
}

#[tokio::test]
async fn shift_zero_when_count_at_or_above_bitwidth() {
    // Per Lua 5.4 §3.4.3: "displacements with absolute values equal
    // to or higher than the number of bits in an integer result in
    // zero (as all bits are shifted out)".
    let res = run_all(
        "return -1 >> 64,
                -1 >> math.maxinteger,
                -1 << 64,
                -1 << math.mininteger,
                -1 >> 63,
                1 << 63",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![
            Value::Integer(0),
            Value::Integer(0),
            Value::Integer(0),
            Value::Integer(0),
            Value::Integer(1),        // 63 < 64, one bit remains
            Value::Integer(i64::MIN), // top-bit set
        ]
    );
}

#[tokio::test]
async fn negative_shift_inverts_direction() {
    // Lua: shifting by a negative amount shifts in the opposite
    // direction.  `8 >> -2` is `8 << 2` and vice versa.
    let res = run_all("return 8 >> -2, 8 << -3").await;
    k9::assert_equal!(res, valuevec![Value::Integer(32), Value::Integer(1)]);
}

#[tokio::test]
async fn getmetatable_string_returns_shared_metatable() {
    // Lua 5.4 §6.4: `getmetatable("")` returns the shared string
    // metatable, which carries `__index` (the `string` library) so
    // method-call syntax like `("hi"):upper()` works.  User code
    // can install custom operator metamethods (e.g. `__band`) on it.
    let res = run_all(
        "local mt = getmetatable('')
         return type(mt), mt.__index == string",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("table"), Value::Boolean(true)]);
}

#[tokio::test]
async fn bitwise_op_on_strings_dispatches_via_string_metatable() {
    // `__band` on the string metatable lets user code define how
    // bitwise ops on string operands are coerced to integers.
    // Mirrors the `bwcoercion.lua` reference test.
    let res = run_one(
        r#"
        local mt = getmetatable("")
        local saved = mt.__band
        mt.__band = function(a, b)
            return tonumber(a) & tonumber(b)
        end
        local result = "0xF0" & "0x0F"
        mt.__band = saved
        return result
    "#,
    )
    .await;
    k9::assert_equal!(res, Value::Integer(0));
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eq_true() {
    k9::assert_equal!(run_one("return 1 == 1").await, Value::Boolean(true));
}

#[tokio::test]
async fn eq_false() {
    k9::assert_equal!(run_one("return 1 == 2").await, Value::Boolean(false));
}

#[tokio::test]
async fn ne() {
    k9::assert_equal!(run_one("return 1 ~= 2").await, Value::Boolean(true));
}

#[tokio::test]
async fn lt() {
    k9::assert_equal!(run_one("return 1 < 2").await, Value::Boolean(true));
}

#[tokio::test]
async fn le() {
    k9::assert_equal!(run_one("return 2 <= 2").await, Value::Boolean(true));
}

#[tokio::test]
async fn gt() {
    k9::assert_equal!(run_one("return 3 > 2").await, Value::Boolean(true));
}

#[tokio::test]
async fn ge() {
    k9::assert_equal!(run_one("return 3 >= 3").await, Value::Boolean(true));
}

// ---------------------------------------------------------------------------
// Logical operators
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logical_not_true() {
    k9::assert_equal!(run_one("return not true").await, Value::Boolean(false));
}

#[tokio::test]
async fn logical_not_false() {
    k9::assert_equal!(run_one("return not false").await, Value::Boolean(true));
}

#[tokio::test]
async fn logical_and_short_circuit() {
    // `false and anything` returns false without evaluating rhs.
    k9::assert_equal!(run_one("return false and 42").await, Value::Boolean(false));
}

#[tokio::test]
async fn logical_and_truthy() {
    k9::assert_equal!(run_one("return 1 and 2").await, Value::Integer(2));
}

#[tokio::test]
async fn logical_or_short_circuit() {
    k9::assert_equal!(run_one("return 1 or 2").await, Value::Integer(1));
}

#[tokio::test]
async fn logical_or_fallback() {
    k9::assert_equal!(run_one("return false or 42").await, Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Local variables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_variable() {
    k9::assert_equal!(
        run_one("local x = 10; local y = 20; return x + y").await,
        Value::Integer(30)
    );
}

#[tokio::test]
async fn local_const_write_error() {
    k9::assert_equal!(
        compile_err("local x <const> = 5; x = 10").await,
        "\
error: attempt to assign to const variable 'x'
 --> test.lua:1:22
  |
1 | local x <const> = 5; x = 10
  |                      ^ attempt to assign to const variable 'x'"
    );
}

#[tokio::test]
async fn local_const_compound_assign_error() {
    k9::assert_equal!(
        compile_err("local x <const> = 5; x += 1").await,
        "\
error: attempt to assign to const variable 'x'
 --> test.lua:1:22
  |
1 | local x <const> = 5; x += 1
  |                      ^ attempt to assign to const variable 'x'"
    );
}

#[tokio::test]
async fn local_const_function_decl_rebind_error() {
    k9::assert_equal!(
        compile_err("local f <const> = 1; function f() end").await,
        "\
error: attempt to assign to const variable 'f'
 --> test.lua:1:31
  |
1 | local f <const> = 1; function f() end
  |                               ^ attempt to assign to const variable 'f'"
    );
}

#[tokio::test]
async fn local_const_upvalue_write_error() {
    k9::assert_equal!(
        compile_err("local x <const> = 5\nlocal function f() x = 10 end\nf()").await,
        "\
error: attempt to assign to const variable 'x'
 --> test.lua:2:20
  |
2 | local function f() x = 10 end
  |                    ^ attempt to assign to const variable 'x'"
    );
}

#[tokio::test]
async fn local_const_upvalue_compound_assign_error() {
    k9::assert_equal!(
        compile_err("local x <const> = 5\nlocal function f() x += 1 end\nf()").await,
        "\
error: attempt to assign to const variable 'x'
 --> test.lua:2:20
  |
2 | local function f() x += 1 end
  |                    ^ attempt to assign to const variable 'x'"
    );
}

#[tokio::test]
async fn local_const_unknown_attribute_error() {
    k9::assert_equal!(
        compile_err("local x <foo> = 5").await,
        "\
error: unknown attribute 'foo'
 --> test.lua:1:10
  |
1 | local x <foo> = 5
  |          ^^^ unknown attribute 'foo'"
    );
}

#[tokio::test]
async fn local_const_table_index_assignment_ok() {
    // Const binds the *binding*, not the value.  Mutating contents is fine.
    k9::assert_equal!(
        run_one("local t <const> = {}; t.x = 1; return t.x").await,
        Value::Integer(1)
    );
}

// ---------------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn if_true_branch() {
    k9::assert_equal!(
        run_one("if true then return 1 else return 2 end").await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn if_false_branch() {
    k9::assert_equal!(
        run_one("if false then return 1 else return 2 end").await,
        Value::Integer(2)
    );
}

#[tokio::test]
async fn if_elseif() {
    k9::assert_equal!(
        run_one(
            "local x = 2
if x == 1 then return 10
elseif x == 2 then return 20
else return 30
end"
        )
        .await,
        Value::Integer(20)
    );
}

#[tokio::test]
async fn while_loop() {
    k9::assert_equal!(
        run_one(
            "local x = 0
local i = 1
while i <= 5 do
  x = x + i
  i = i + 1
end
return x"
        )
        .await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn repeat_loop() {
    k9::assert_equal!(
        run_one(
            "local x = 0
local i = 1
repeat
  x = x + i
  i = i + 1
until i > 5
return x"
        )
        .await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn numeric_for() {
    k9::assert_equal!(
        run_one(
            "local sum = 0
for i = 1, 10 do
  sum = sum + i
end
return sum"
        )
        .await,
        Value::Integer(55)
    );
}

#[tokio::test]
async fn numeric_for_with_step() {
    k9::assert_equal!(
        run_one(
            "local sum = 0
for i = 0, 10, 2 do
  sum = sum + i
end
return sum"
        )
        .await,
        Value::Integer(30)
    );
}

#[tokio::test]
async fn do_end_scope() {
    // Variable declared inside `do` is not visible outside.
    k9::assert_equal!(
        run_one(
            "local x = 1
do
  local x = 99
end
return x"
        )
        .await,
        Value::Integer(1)
    );
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_call() {
    k9::assert_equal!(
        run_one(
            "local function add(a, b) return a + b end
return add(3, 4)"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn multiple_return_values() {
    let vals = run_all(
        "local function two() return 1, 2 end
return two()",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(1), Value::Integer(2)]);
}

#[tokio::test]
async fn multi_assign_indexed_call_pads_with_nil() {
    // Regression: `local a, b, c = t.f()` where `t.f()` returns a single
    // value must pad `b` and `c` with nil.  An earlier bug left the
    // receiver table and the index key stashed in those slots, leaking
    // them out as "return values".  The bug only reproduced for indexed
    // calls (`t.f()`, `t:f()`, `lib.f()`) because the register layout for
    // a bare `f()` call doesn't use neighbouring slots during dispatch.
    let vals = run_all(
        "local t = { f = function() return 99 end }
local a, b, c = t.f()
return a, b, c",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(99), Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn multi_assign_method_call_pads_with_nil() {
    // Same as above but via method-call syntax.
    let vals = run_all(
        "local t = { f = function(self) return 7 end }
local a, b = t:f()
return a, b",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(7), Value::Nil]);
}

#[tokio::test]
async fn multi_assign_bracket_indexed_call_pads_with_nil() {
    // Bracket-indexed call exercises `apply_index_suffix(Brackets)`, where
    // the key is computed by `compile_expr` rather than `LoadK`.  Same
    // register-leak potential as the dot form.
    let vals = run_all(
        "local t = { f = function() return 99 end }
local key = \"f\"
local a, b, c = t[key]()
return a, b, c",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(99), Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn multi_assign_chained_dot_call_pads_with_nil() {
    // Chained `a.b.c()` exercises the non-last-index loop in
    // `compile_function_call`, which threads the receiver through multiple
    // index suffixes before dispatching the call.
    let vals = run_all(
        "local outer = { inner = { fn = function() return 88 end } }
local a, b, c = outer.inner.fn()
return a, b, c",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(88), Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn multi_assign_indexed_call_zero_returns_all_nil() {
    // An indexed call that returns no values at all must fill every
    // requested slot with nil — without the padding fix the first slot
    // would leak the callee function itself and the second the index key.
    let vals = run_all(
        "local t = { f = function() end }
local a, b = t.f()
return a, b",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn multi_assign_native_indexed_call_pads_with_nil() {
    // Exercises `write_return_values` via the native-future resolution
    // path in task.rs (rather than the Lua-Return path) with three pad
    // slots.  `tostring` is a registered native that returns exactly one
    // value; parking it under a table forces the indexed-call dispatch.
    let vals = run_all(
        "local t = { s = tostring }
local a, b, c, d = t.s(42)
return a, b, c, d",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![Value::string("42"), Value::Nil, Value::Nil, Value::Nil]
    );
}

#[tokio::test]
async fn multi_assign_indexed_call_extra_returns_truncated() {
    // When the callee returns more values than requested, the excess must
    // be silently dropped — the `.take(n)` in `write_return_values`.
    // Guards against over-eager padding that would wipe real values.
    let vals = run_all(
        "local t = { f = function() return 1, 2, 3, 4, 5 end }
local a, b = t.f()
return a, b",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(1), Value::Integer(2)]);
}

#[tokio::test]
async fn return_forwards_indexed_call_without_leaking_setup_regs() {
    // `return t.f()` compiles to a `Call { nresults: -1 }` followed by
    // `Return { nresults: -1 }` which reads from `base` to the top of the
    // register file.  Without the `truncate` branch in
    // `write_return_values` (kept alive by this test), any stale table/key
    // from the indexed-call setup would be returned as extra values.
    let vals = run_all(
        "local t = { f = function() return 1 end }
return t.f()",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(1)]);
}

#[tokio::test]
async fn multi_assign_global_from_indexed_call_expands() {
    // Non-local (global) multi-assign also expands the last RHS call.
    // `compile_assignment` previously adjusted every RHS to exactly one
    // value; this pins the corrected behaviour so `a, b, c = t.f()`
    // behaves the same as `local a, b, c = t.f()`.
    let vals = run_all(
        "local t = { f = function() return 10, 20, 30 end }
a, b, c = t.f()
return a, b, c",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[tokio::test]
async fn multi_assign_global_from_indexed_call_pads_with_nil() {
    // Same path as above but with the callee returning fewer values than
    // requested — covers the padding case for `compile_assignment`.
    let vals = run_all(
        "local t = { f = function() return 99 end }
a, b, c = t.f()
return a, b, c",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(99), Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn multi_assign_call_not_last_adjusts_to_one() {
    // When the call is not the last RHS expression, its returns are
    // adjusted to exactly one value regardless of how many it produces.
    // Pins the adjustment rule against someone "fixing" it the wrong way.
    let vals = run_all(
        "local t = { f = function() return 1, 2, 3 end }
local a, b = t.f(), 99
return a, b",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(1), Value::Integer(99)]);
}

#[tokio::test]
async fn multi_assign_method_chain_pads_with_nil() {
    // Chained method call: `obj:first():second()` stacks receiver tracking
    // across the intermediate call.  The final call still returns fewer
    // values than the multi-assign requests, so padding must be applied.
    let vals = run_all(
        "local obj = {}
function obj:first()
  local inner = {}
  function inner:second() return 123 end
  return inner
end
local a, b, c = obj:first():second()
return a, b, c",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(123), Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn multi_assign_mixed_lhs_name_and_indexed() {
    // Regression: `a, t[1] = f()` where `f()` returns 2 values.  The
    // Var::Expression LHS branch in `compile_assignment` alloc_temp's
    // three slots (obj, key, val) per iteration.  Without reserving the
    // call's extra result registers as live temps, the LHS's alloc_temp
    // would overwrite them before they were consumed, and `t[1]` would
    // end up with the table value itself instead of the second return.
    let vals = run_all(
        "local t = {}
local a
local f = function() return 10, 20 end
a, t[1] = f()
return a, t[1]",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(10), Value::Integer(20)]);
}

#[tokio::test]
async fn multi_assign_all_indexed_lhs() {
    // All-Var::Expression LHS with multi-return expansion.  Each LHS
    // target consumes three temps during its SetTable emission; none
    // may clobber the call's result registers.
    let vals = run_all(
        "local t = {}
local f = function() return 100, 200, 300 end
t[1], t[2], t[3] = f()
return t[1], t[2], t[3]",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![
            Value::Integer(100),
            Value::Integer(200),
            Value::Integer(300)
        ]
    );
}

#[tokio::test]
async fn multi_assign_upvalue_lhs_with_expansion() {
    // Upvalue LHS branch in `compile_assignment` with multi-return
    // expansion.  The upvalue branch only alloc_temp's a slot for the
    // LoadNil-when-src-is-None case, which doesn't fire here, so this
    // test is really pinning that SetUpval is emitted for each slot.
    let vals = run_all(
        "local upv_a, upv_b = 0, 0
local function setter()
    local t = { f = function() return 11, 22 end }
    upv_a, upv_b = t.f()
end
setter()
return upv_a, upv_b",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(11), Value::Integer(22)]);
}

#[tokio::test]
async fn multi_assign_global_from_vararg_expands() {
    // Vararg branch in `compile_assignment`: `a, b, c = ...` should
    // expand the varargs to fill all three LHS slots.  Exercises the
    // new is_vararg_expr branch I added alongside the FunctionCall branch.
    let vals = run_all(
        "local function caller(...)
    a, b, c = ...
    return a, b, c
end
return caller(1, 2, 3)",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[tokio::test]
async fn generic_for_with_indexed_call_iterator() {
    // `for k, v in ipairs(t) do ... end` evaluates `ipairs(t)` to three
    // values (iterator, state, initial control) via multi-return
    // expansion.  This uses a completely different lowering path than
    // the multi-assign ones above, so worth pinning while we're here.
    let v = run_one(
        "local t = {10, 20, 30}
local sum = 0
for i, v in ipairs(t) do
    sum = sum + i + v
end
return sum",
    )
    .await;
    // (1+10) + (2+20) + (3+30) = 66
    k9::assert_equal!(v, Value::Integer(66));
}

#[tokio::test]
async fn generic_for_with_method_call_iterator() {
    // Regression: method-call syntax in for-in expression list placed
    // the receiver in a temp register instead of the self-arg slot
    // (dst+1) when scope-allocated control variables separated dst
    // from temp_top.
    let res = run_all(
        "\
        local t = {}
        for w in ('hello world'):gmatch('%a+') do
            t[#t+1] = w
        end
        return t[1], t[2]",
    )
    .await;
    k9::assert_equal!(
        res,
        valuevec![Value::string("hello"), Value::string("world")]
    );
}

#[tokio::test]
async fn generic_for_method_call_with_args() {
    // Method call in for-in with extra arguments beyond the self.
    let res = run_all(
        "\
        local t = {}
        for w in ('one two three'):gmatch('%a+', 5) do
            t[#t+1] = w
        end
        return t[1], t[2]",
    )
    .await;
    k9::assert_equal!(res, valuevec![Value::string("two"), Value::string("three")]);
}

#[tokio::test]
async fn table_constructor_with_trailing_indexed_call_expands() {
    // `{ 10, 20, t.f() }` expands the trailing call's returns into the
    // array part of the table via `Call { nresults: -1 }` + `SetList`.
    // This is the call-as-final-field lowering path in
    // `compile_table_constructor`.
    let vals = run_all(
        "local t = { f = function() return 1, 2, 3 end }
local arr = { 10, 20, t.f() }
return arr[1], arr[2], arr[3], arr[4], arr[5]",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3)
        ]
    );
}

#[tokio::test]
async fn varargs_from_indexed_call_as_argument() {
    // `select("#", t.f())` forwards `t.f()`'s returns as varargs to
    // `select` via the `nresults = -1` expansion in
    // `compile_args_and_call`.  Returns the count of forwarded values.
    let v = run_one(
        "local t = { f = function() return 10, 20, 30 end }
return select(\"#\", t.f())",
    )
    .await;
    k9::assert_equal!(v, Value::Integer(3));
}

#[tokio::test]
async fn call_metamethod_with_multi_return_expansion() {
    // Table with `__call` invoked as a function.  The dispatch goes
    // through the metamethod branch in the VM (task.rs:Call → Value::Table
    // → __call lookup → dispatch_metamethod) rather than the direct
    // function branch.  Return values must still be expanded to fill the
    // multi-assign slots.
    let vals = run_all(
        "local callable = setmetatable({}, {
    __call = function(self, x, y) return x + y, x - y, x * y end
})
local a, b, c = callable(10, 3)
return a, b, c",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![Value::Integer(13), Value::Integer(7), Value::Integer(30)]
    );
}

#[tokio::test]
async fn call_metamethod_with_padding() {
    // Same dispatch path as above, but the __call returns fewer values
    // than the multi-assign requests — the padding fix in
    // `write_return_values` applies here too.
    let vals = run_all(
        "local callable = setmetatable({}, {
    __call = function(self) return 42 end
})
local a, b, c = callable()
return a, b, c",
    )
    .await;
    k9::assert_equal!(vals, valuevec![Value::Integer(42), Value::Nil, Value::Nil]);
}

#[tokio::test]
async fn pcall_forwards_multiple_returns() {
    // `pcall` is a native builtin that prepends `true` to the callee's
    // returns on success, giving `(true, r1, r2, ...)`.  Multi-assign
    // must expand all four values.
    let vals = run_all(
        "local ok, a, b, c = pcall(function() return 10, 20, 30 end)
return ok, a, b, c",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![
            Value::Boolean(true),
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(30)
        ]
    );
}

#[tokio::test]
async fn pcall_returns_false_and_error_on_failure() {
    // The other pcall return shape: `(false, err)`.  Using `error(msg, 0)`
    // suppresses the location prefix so the error value is exactly the
    // string passed in.
    let vals = run_all(
        "local ok, err = pcall(function() error(\"boom\", 0) end)
return ok, err",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![Value::Boolean(false), Value::string("boom")]
    );
}

#[tokio::test]
async fn numeric_for_bounds_from_indexed_call_adjusts_to_one() {
    // `for i = t.start(), 10 do ... end` — numeric for evaluates each
    // bound with `compile_expr`, which adjusts any call to exactly one
    // value.  Even though `t.start()` returns three values, only the
    // first (3) becomes the loop start.
    let v = run_one(
        "local t = { start = function() return 3, 99, 77 end }
local count = 0
for i = t.start(), 5 do
    count = count + 1
end
return count",
    )
    .await;
    // Iterations: i = 3, 4, 5 — three iterations.
    k9::assert_equal!(v, Value::Integer(3));
}

#[tokio::test]
async fn table_constructor_with_trailing_vararg_expands() {
    // `{ 10, 20, ... }` inside a varargic function — the trailing `...`
    // gets `Vararg { nresults: -1 }` + `SetList`, parallel to the
    // call-as-final-field path but via Vararg instead of Call.
    let vals = run_all(
        "local function caller(...)
    local arr = { 10, 20, ... }
    return arr[1], arr[2], arr[3], arr[4], arr[5]
end
return caller(1, 2, 3)",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3)
        ]
    );
}

#[tokio::test]
async fn index_metamethod_returning_function_multi_return() {
    // `__index` metamethod returns a function; calling that function
    // produces multiple values.  Layered scenario: the metamethod
    // dispatches on the table access, then the resulting function is
    // called through the normal indexed-call path with multi-return
    // expansion.
    let vals = run_all(
        "local t = setmetatable({}, {
    __index = function(tab, key)
        return function() return key, \"found\" end
    end
})
local a, b, c = t.missing()
return a, b, c",
    )
    .await;
    k9::assert_equal!(
        vals,
        valuevec![Value::string("missing"), Value::string("found"), Value::Nil]
    );
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn string_literal_escapes() {
    k9::assert_equal!(
        run_one(r#"return "hello\nworld""#).await,
        Value::string("hello\nworld")
    );
}

#[tokio::test]
async fn string_hex_escape() {
    k9::assert_equal!(
        run_one(r#"return "\x41\x42\x43""#).await,
        Value::string("ABC")
    );
}

#[tokio::test]
async fn string_decimal_escape() {
    k9::assert_equal!(
        run_one("return \"\\65\\66\\67\"").await,
        Value::string("ABC")
    );
}

#[tokio::test]
async fn string_len() {
    k9::assert_equal!(run_one(r#"return #"hello""#).await, Value::Integer(5));
}

#[tokio::test]
async fn string_concat_non_trivial() {
    k9::assert_equal!(
        run_one(r#"local a = "foo" local b = "bar" return a .. b"#).await,
        Value::string("foobar")
    );
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn table_new_and_len() {
    k9::assert_equal!(run_one("local t = {} return #t").await, Value::Integer(0));
}

#[tokio::test]
async fn table_positional_fields() {
    k9::assert_equal!(
        run_one("local t = {10, 20, 30} return t[2]").await,
        Value::Integer(20)
    );
}

#[tokio::test]
async fn table_named_fields() {
    k9::assert_equal!(
        run_one("local t = {x = 42} return t.x").await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn table_expr_key() {
    k9::assert_equal!(
        run_one("local k = \"z\" local t = {[k] = 99} return t.z").await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn table_set_field() {
    k9::assert_equal!(
        run_one("local t = {} t.x = 7 return t.x").await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn table_set_index() {
    k9::assert_equal!(
        run_one("local t = {} t[3] = 99 return t[3]").await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn table_length_sequence() {
    k9::assert_equal!(
        run_one("local t = {10, 20, 30} return #t").await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn table_missing_key_is_nil() {
    k9::assert_equal!(run_one("local t = {} return t.missing").await, Value::Nil);
}

#[tokio::test]
async fn table_integer_float_key_same() {
    // t[1] and t[1.0] must be the same entry.
    k9::assert_equal!(
        run_one("local t = {} t[1] = 42 return t[1.0]").await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn table_dotted_function_decl() {
    k9::assert_equal!(
        run_one(
            "local mod = {}
function mod.add(a, b) return a + b end
return mod.add(3, 4)"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn table_method_call() {
    k9::assert_equal!(
        run_one(
            "local obj = {value = 10}
function obj:get() return self.value end
return obj:get()"
        )
        .await,
        Value::Integer(10)
    );
}

#[tokio::test]
async fn table_chained_index() {
    k9::assert_equal!(
        run_one(
            "local a = {b = {c = 99}}
return a.b.c"
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn table_chained_call() {
    k9::assert_equal!(
        run_one(
            "local lib = {}
function lib.add(a, b) return a + b end
local mod = {lib = lib}
return mod.lib.add(5, 6)"
        )
        .await,
        Value::Integer(11)
    );
}

// ---------------------------------------------------------------------------
// Suffix chains — call followed by any suffix (call, method, index)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn call_then_dot_index() {
    // `f().x` — call, then dot access
    k9::assert_equal!(
        run_one(
            "local function f() return {x = 42} end
return f().x"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn call_then_bracket_index() {
    // `f()[k]` — call, then bracket access
    k9::assert_equal!(
        run_one(
            "local function f() return {hello = 7} end
return f()['hello']"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn call_then_call() {
    // `f()()` — call returns a function, call that
    k9::assert_equal!(
        run_one(
            "local function outer() return function(n) return n * 10 end end
return outer()(5)"
        )
        .await,
        Value::Integer(50)
    );
}

#[tokio::test]
async fn call_then_method_call() {
    // `f():m()` — call, then method call
    k9::assert_equal!(
        run_one(
            "local function f()
    return {v = 10, get = function(self) return self.v end}
end
return f():get()"
        )
        .await,
        Value::Integer(10)
    );
}

#[tokio::test]
async fn call_chain_truncates_to_one_value() {
    // A call returning multiple values is truncated to the first when
    // another suffix follows it.
    k9::assert_equal!(
        run_one(
            "local function two() return {a = 1}, {a = 2} end
return two().a"
        )
        .await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn call_dot_call_chain() {
    // `mod().fn(args)` — call, dot, call.
    k9::assert_equal!(
        run_one(
            "local function mod() return {add = function(a, b) return a + b end} end
return mod().add(2, 3)"
        )
        .await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn method_chain_with_args() {
    // `f():m(x):n(y)` — method, then method again.
    k9::assert_equal!(
        run_one(
            "local function start()
    local o = {n = 0}
    function o:add(x) self.n = self.n + x; return self end
    return o
end
return start():add(2):add(3).n"
        )
        .await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn nested_mid_chain_call_as_arg() {
    // A mid-chain call whose argument is itself a mid-chain call.
    k9::assert_equal!(
        run_one(
            "local function make()
    return {dbl = function(_, n) return n * 2 end}
end
return make():dbl(make():dbl(3))"
        )
        .await,
        Value::Integer(12)
    );
}

#[tokio::test]
async fn assign_to_call_dot() {
    // `f().x = v` — assignment target threads a call in the chain.
    k9::assert_equal!(
        run_one(
            "local t = {x = 0}
local function get() return t end
get().x = 99
return t.x"
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn assign_to_call_bracket() {
    k9::assert_equal!(
        run_one(
            "local t = {}
local function get() return t end
get()['k'] = 'v'
return t['k']"
        )
        .await,
        Value::string("v")
    );
}

#[tokio::test]
async fn compound_assign_to_call_dot() {
    // LuaU compound assignment through a call-in-chain target.
    k9::assert_equal!(
        run_one(
            "local t = {x = 5}
local function get() return t end
get().x += 10
return t.x"
        )
        .await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn method_call_then_index() {
    // `obj:m().field` — method, then index.
    k9::assert_equal!(
        run_one(
            "local o = {getself = function(self) return self end, val = 7}
return o:getself().val"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn call_with_string_arg_then_index() {
    // `f'str'.x` — the string-arg shorthand as a mid-chain call.
    k9::assert_equal!(
        run_one(
            "local function wrap(s) return {val = s} end
return wrap'hi'.val"
        )
        .await,
        Value::string("hi")
    );
}

// ---------------------------------------------------------------------------
// Break
// ---------------------------------------------------------------------------

#[tokio::test]
async fn break_while() {
    k9::assert_equal!(
        run_one(
            "local i = 0
while true do
    i = i + 1
    if i >= 5 then break end
end
return i"
        )
        .await,
        Value::Integer(5)
    );
}

#[tokio::test]
async fn break_for() {
    k9::assert_equal!(
        run_one(
            "local last = 0
for i = 1, 100 do
    last = i
    if i == 7 then break end
end
return last"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn break_repeat() {
    k9::assert_equal!(
        run_one(
            "local i = 0
repeat
    i = i + 1
    if i == 4 then break end
until i >= 10
return i"
        )
        .await,
        Value::Integer(4)
    );
}

// ---------------------------------------------------------------------------
// Lua→Lua call arg passing (make_lua_frame_from_slice coverage)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn call_more_args_than_params() {
    k9::assert_equal!(
        run_one(
            "local function f(a, b) return a + b end
             return f(1, 2, 3, 4)"
        )
        .await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn call_fewer_args_than_params() {
    k9::assert_equal!(
        run_all(
            "local function f(a, b, c) return a, b, c end
             return f(42)"
        )
        .await,
        valuevec![Value::Integer(42), Value::Nil, Value::Nil]
    );
}

#[tokio::test]
async fn call_varargs_via_slice() {
    k9::assert_equal!(
        run_all(
            "local function f(a, ...)
                 return a, ...
             end
             return f(10, 20, 30)"
        )
        .await,
        valuevec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[tokio::test]
async fn return_fewer_than_expected() {
    k9::assert_equal!(
        run_all(
            "local function f() return 1 end
             local a, b, c = f()
             return a, b, c"
        )
        .await,
        valuevec![Value::Integer(1), Value::Nil, Value::Nil]
    );
}

#[tokio::test]
async fn return_more_than_expected() {
    k9::assert_equal!(
        run_one(
            "local function f() return 1, 2, 3 end
             local a = f()
             return a"
        )
        .await,
        Value::Integer(1)
    );
}

#[tokio::test]
async fn return_variable_nresults() {
    k9::assert_equal!(
        run_all(
            "local function f() return 10, 20, 30 end
             return f()"
        )
        .await,
        valuevec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[tokio::test]
async fn nested_calls_return_correctly() {
    k9::assert_equal!(
        run_one(
            "local function add(a, b) return a + b end
             local function mul(a, b) return a * b end
             return add(mul(3, 4), mul(5, 6))"
        )
        .await,
        Value::Integer(42)
    );
}
