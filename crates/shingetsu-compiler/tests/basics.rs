mod common;

use common::{run_all, run_one};
use shingetsu_compiler::{compile, CompileOptions};
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// Numeric literals
// ---------------------------------------------------------------------------

#[test]
fn integer_literal() {
    k9::assert_equal!(run_one("return 42"), Value::Integer(42));
}

#[test]
fn float_literal() {
    k9::assert_equal!(run_one("return 3.14"), Value::Float(3.14));
}

#[test]
fn negative_literal() {
    k9::assert_equal!(run_one("return -7"), Value::Integer(-7));
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

#[test]
fn add_integers() {
    k9::assert_equal!(run_one("return 10 + 20"), Value::Integer(30));
}

#[test]
fn sub_integers() {
    k9::assert_equal!(run_one("return 100 - 37"), Value::Integer(63));
}

#[test]
fn mul_integers() {
    k9::assert_equal!(run_one("return 6 * 7"), Value::Integer(42));
}

#[test]
fn float_div() {
    // `/` always returns float.
    k9::assert_equal!(run_one("return 10 / 4"), Value::Float(2.5));
}

#[test]
fn floor_div() {
    k9::assert_equal!(run_one("return 10 // 3"), Value::Integer(3));
}

#[test]
fn modulo() {
    k9::assert_equal!(run_one("return 10 % 3"), Value::Integer(1));
}

#[test]
fn exponent() {
    k9::assert_equal!(run_one("return 2 ^ 10"), Value::Float(1024.0));
}

#[test]
fn unary_minus() {
    k9::assert_equal!(run_one("local x = 5; return -x"), Value::Integer(-5));
}

#[test]
fn integer_mixed_float() {
    // integer + float → float.
    k9::assert_equal!(run_one("return 1 + 1.5"), Value::Float(2.5));
}

// ---------------------------------------------------------------------------
// Bitwise
// ---------------------------------------------------------------------------

#[test]
fn band() {
    k9::assert_equal!(run_one("return 0xFF & 0x0F"), Value::Integer(0x0F));
}

#[test]
fn bor() {
    k9::assert_equal!(run_one("return 0xF0 | 0x0F"), Value::Integer(0xFF));
}

#[test]
fn bxor() {
    k9::assert_equal!(run_one("return 0xFF ~ 0x0F"), Value::Integer(0xF0));
}

#[test]
fn bnot() {
    k9::assert_equal!(run_one("return ~0"), Value::Integer(-1));
}

#[test]
fn shl() {
    k9::assert_equal!(run_one("return 1 << 4"), Value::Integer(16));
}

#[test]
fn shr() {
    k9::assert_equal!(run_one("return 16 >> 2"), Value::Integer(4));
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

#[test]
fn eq_true() {
    k9::assert_equal!(run_one("return 1 == 1"), Value::Boolean(true));
}

#[test]
fn eq_false() {
    k9::assert_equal!(run_one("return 1 == 2"), Value::Boolean(false));
}

#[test]
fn ne() {
    k9::assert_equal!(run_one("return 1 ~= 2"), Value::Boolean(true));
}

#[test]
fn lt() {
    k9::assert_equal!(run_one("return 1 < 2"), Value::Boolean(true));
}

#[test]
fn le() {
    k9::assert_equal!(run_one("return 2 <= 2"), Value::Boolean(true));
}

#[test]
fn gt() {
    k9::assert_equal!(run_one("return 3 > 2"), Value::Boolean(true));
}

#[test]
fn ge() {
    k9::assert_equal!(run_one("return 3 >= 3"), Value::Boolean(true));
}

// ---------------------------------------------------------------------------
// Logical operators
// ---------------------------------------------------------------------------

#[test]
fn logical_not_true() {
    k9::assert_equal!(run_one("return not true"), Value::Boolean(false));
}

#[test]
fn logical_not_false() {
    k9::assert_equal!(run_one("return not false"), Value::Boolean(true));
}

#[test]
fn logical_and_short_circuit() {
    // `false and anything` returns false without evaluating rhs.
    k9::assert_equal!(run_one("return false and 42"), Value::Boolean(false));
}

#[test]
fn logical_and_truthy() {
    k9::assert_equal!(run_one("return 1 and 2"), Value::Integer(2));
}

#[test]
fn logical_or_short_circuit() {
    k9::assert_equal!(run_one("return 1 or 2"), Value::Integer(1));
}

#[test]
fn logical_or_fallback() {
    k9::assert_equal!(run_one("return false or 42"), Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Local variables
// ---------------------------------------------------------------------------

#[test]
fn local_variable() {
    k9::assert_equal!(
        run_one("local x = 10; local y = 20; return x + y"),
        Value::Integer(30)
    );
}

#[test]
fn local_const_write_error() {
    let opts = CompileOptions::default();
    let err = compile("local x <const> = 5; x = 10", &opts).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("const"),
        "expected 'const' in error, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Control flow
// ---------------------------------------------------------------------------

#[test]
fn if_true_branch() {
    k9::assert_equal!(
        run_one("if true then return 1 else return 2 end"),
        Value::Integer(1)
    );
}

#[test]
fn if_false_branch() {
    k9::assert_equal!(
        run_one("if false then return 1 else return 2 end"),
        Value::Integer(2)
    );
}

#[test]
fn if_elseif() {
    k9::assert_equal!(
        run_one(
            "local x = 2
if x == 1 then return 10
elseif x == 2 then return 20
else return 30
end"
        ),
        Value::Integer(20)
    );
}

#[test]
fn while_loop() {
    k9::assert_equal!(
        run_one(
            "local x = 0
local i = 1
while i <= 5 do
  x = x + i
  i = i + 1
end
return x"
        ),
        Value::Integer(15)
    );
}

#[test]
fn repeat_loop() {
    k9::assert_equal!(
        run_one(
            "local x = 0
local i = 1
repeat
  x = x + i
  i = i + 1
until i > 5
return x"
        ),
        Value::Integer(15)
    );
}

#[test]
fn numeric_for() {
    k9::assert_equal!(
        run_one(
            "local sum = 0
for i = 1, 10 do
  sum = sum + i
end
return sum"
        ),
        Value::Integer(55)
    );
}

#[test]
fn numeric_for_with_step() {
    k9::assert_equal!(
        run_one(
            "local sum = 0
for i = 0, 10, 2 do
  sum = sum + i
end
return sum"
        ),
        Value::Integer(30)
    );
}

#[test]
fn do_end_scope() {
    // Variable declared inside `do` is not visible outside.
    k9::assert_equal!(
        run_one(
            "local x = 1
do
  local x = 99
end
return x"
        ),
        Value::Integer(1)
    );
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

#[test]
fn function_call() {
    k9::assert_equal!(
        run_one(
            "local function add(a, b) return a + b end
return add(3, 4)"
        ),
        Value::Integer(7)
    );
}

#[test]
fn multiple_return_values() {
    let vals = run_all(
        "local function two() return 1, 2 end
return two()",
    );
    k9::assert_equal!(vals, vec![Value::Integer(1), Value::Integer(2)]);
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

#[test]
fn string_literal_escapes() {
    k9::assert_equal!(
        run_one(r#"return "hello\nworld""#),
        Value::string("hello\nworld")
    );
}

#[test]
fn string_hex_escape() {
    k9::assert_equal!(run_one(r#"return "\x41\x42\x43""#), Value::string("ABC"));
}

#[test]
fn string_decimal_escape() {
    k9::assert_equal!(run_one("return \"\\65\\66\\67\""), Value::string("ABC"));
}

#[test]
fn string_len() {
    k9::assert_equal!(run_one(r#"return #"hello""#), Value::Integer(5));
}

#[test]
fn string_concat_non_trivial() {
    k9::assert_equal!(
        run_one(r#"local a = "foo" local b = "bar" return a .. b"#),
        Value::string("foobar")
    );
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

#[test]
fn table_new_and_len() {
    k9::assert_equal!(run_one("local t = {} return #t"), Value::Integer(0));
}

#[test]
fn table_positional_fields() {
    k9::assert_equal!(
        run_one("local t = {10, 20, 30} return t[2]"),
        Value::Integer(20)
    );
}

#[test]
fn table_named_fields() {
    k9::assert_equal!(run_one("local t = {x = 42} return t.x"), Value::Integer(42));
}

#[test]
fn table_expr_key() {
    k9::assert_equal!(
        run_one("local k = \"z\" local t = {[k] = 99} return t.z"),
        Value::Integer(99)
    );
}

#[test]
fn table_set_field() {
    k9::assert_equal!(
        run_one("local t = {} t.x = 7 return t.x"),
        Value::Integer(7)
    );
}

#[test]
fn table_set_index() {
    k9::assert_equal!(
        run_one("local t = {} t[3] = 99 return t[3]"),
        Value::Integer(99)
    );
}

#[test]
fn table_length_sequence() {
    k9::assert_equal!(
        run_one("local t = {10, 20, 30} return #t"),
        Value::Integer(3)
    );
}

#[test]
fn table_missing_key_is_nil() {
    k9::assert_equal!(run_one("local t = {} return t.missing"), Value::Nil);
}

#[test]
fn table_integer_float_key_same() {
    // t[1] and t[1.0] must be the same entry.
    k9::assert_equal!(
        run_one("local t = {} t[1] = 42 return t[1.0]"),
        Value::Integer(42)
    );
}

#[test]
fn table_dotted_function_decl() {
    k9::assert_equal!(
        run_one(
            "local mod = {}
function mod.add(a, b) return a + b end
return mod.add(3, 4)"
        ),
        Value::Integer(7)
    );
}

#[test]
fn table_method_call() {
    k9::assert_equal!(
        run_one(
            "local obj = {value = 10}
function obj:get() return self.value end
return obj:get()"
        ),
        Value::Integer(10)
    );
}

#[test]
fn table_chained_index() {
    k9::assert_equal!(
        run_one(
            "local a = {b = {c = 99}}
return a.b.c"
        ),
        Value::Integer(99)
    );
}

#[test]
fn table_chained_call() {
    k9::assert_equal!(
        run_one(
            "local lib = {}
function lib.add(a, b) return a + b end
local mod = {lib = lib}
return mod.lib.add(5, 6)"
        ),
        Value::Integer(11)
    );
}

// ---------------------------------------------------------------------------
// Suffix chains — call followed by any suffix (call, method, index)
// ---------------------------------------------------------------------------

#[test]
fn call_then_dot_index() {
    // `f().x` — call, then dot access
    k9::assert_equal!(
        run_one(
            "local function f() return {x = 42} end
return f().x"
        ),
        Value::Integer(42)
    );
}

#[test]
fn call_then_bracket_index() {
    // `f()[k]` — call, then bracket access
    k9::assert_equal!(
        run_one(
            "local function f() return {hello = 7} end
return f()['hello']"
        ),
        Value::Integer(7)
    );
}

#[test]
fn call_then_call() {
    // `f()()` — call returns a function, call that
    k9::assert_equal!(
        run_one(
            "local function outer() return function(n) return n * 10 end end
return outer()(5)"
        ),
        Value::Integer(50)
    );
}

#[test]
fn call_then_method_call() {
    // `f():m()` — call, then method call
    k9::assert_equal!(
        run_one(
            "local function f()
    return {v = 10, get = function(self) return self.v end}
end
return f():get()"
        ),
        Value::Integer(10)
    );
}

#[test]
fn call_chain_truncates_to_one_value() {
    // A call returning multiple values is truncated to the first when
    // another suffix follows it.
    k9::assert_equal!(
        run_one(
            "local function two() return {a = 1}, {a = 2} end
return two().a"
        ),
        Value::Integer(1)
    );
}

#[test]
fn call_dot_call_chain() {
    // `mod().fn(args)` — call, dot, call.
    k9::assert_equal!(
        run_one(
            "local function mod() return {add = function(a, b) return a + b end} end
return mod().add(2, 3)"
        ),
        Value::Integer(5)
    );
}

#[test]
fn method_chain_with_args() {
    // `f():m(x):n(y)` — method, then method again.
    k9::assert_equal!(
        run_one(
            "local function start()
    local o = {n = 0}
    function o:add(x) self.n = self.n + x; return self end
    return o
end
return start():add(2):add(3).n"
        ),
        Value::Integer(5)
    );
}

#[test]
fn nested_mid_chain_call_as_arg() {
    // A mid-chain call whose argument is itself a mid-chain call.
    k9::assert_equal!(
        run_one(
            "local function make()
    return {dbl = function(_, n) return n * 2 end}
end
return make():dbl(make():dbl(3))"
        ),
        Value::Integer(12)
    );
}

#[test]
fn assign_to_call_dot() {
    // `f().x = v` — assignment target threads a call in the chain.
    k9::assert_equal!(
        run_one(
            "local t = {x = 0}
local function get() return t end
get().x = 99
return t.x"
        ),
        Value::Integer(99)
    );
}

#[test]
fn assign_to_call_bracket() {
    k9::assert_equal!(
        run_one(
            "local t = {}
local function get() return t end
get()['k'] = 'v'
return t['k']"
        ),
        Value::string("v")
    );
}

#[test]
fn compound_assign_to_call_dot() {
    // LuaU compound assignment through a call-in-chain target.
    k9::assert_equal!(
        run_one(
            "local t = {x = 5}
local function get() return t end
get().x += 10
return t.x"
        ),
        Value::Integer(15)
    );
}

#[test]
fn method_call_then_index() {
    // `obj:m().field` — method, then index.
    k9::assert_equal!(
        run_one(
            "local o = {getself = function(self) return self end, val = 7}
return o:getself().val"
        ),
        Value::Integer(7)
    );
}

#[test]
fn call_with_string_arg_then_index() {
    // `f'str'.x` — the string-arg shorthand as a mid-chain call.
    k9::assert_equal!(
        run_one(
            "local function wrap(s) return {val = s} end
return wrap'hi'.val"
        ),
        Value::string("hi")
    );
}

// ---------------------------------------------------------------------------
// Break
// ---------------------------------------------------------------------------

#[test]
fn break_while() {
    k9::assert_equal!(
        run_one(
            "local i = 0
while true do
    i = i + 1
    if i >= 5 then break end
end
return i"
        ),
        Value::Integer(5)
    );
}

#[test]
fn break_for() {
    k9::assert_equal!(
        run_one(
            "local last = 0
for i = 1, 100 do
    last = i
    if i == 7 then break end
end
return last"
        ),
        Value::Integer(7)
    );
}

#[test]
fn break_repeat() {
    k9::assert_equal!(
        run_one(
            "local i = 0
repeat
    i = i + 1
    if i == 4 then break end
until i >= 10
return i"
        ),
        Value::Integer(4)
    );
}
