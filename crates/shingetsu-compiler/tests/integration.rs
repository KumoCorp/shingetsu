use shingetsu_compiler::{compile, CompileOptions};
use shingetsu_vm::{Function, GlobalEnv, Task, Value};

/// Compile and run a Lua snippet, returning the first return value.
fn run_one(src: &str) -> Value {
    run_all(src).into_iter().next().unwrap_or(Value::Nil)
}

/// Compile and run a Lua snippet, returning all return values.
fn run_all(src: &str) -> Vec<Value> {
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let env = GlobalEnv::new();
    let func = crate::Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed")
}

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
    k9::assert_equal!(run_one("local x = 10; local y = 20; return x + y"), Value::Integer(30));
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
return two()"
    );
    k9::assert_equal!(vals, vec![Value::Integer(1), Value::Integer(2)]);
}

// ---------------------------------------------------------------------------
// Goto / label
// ---------------------------------------------------------------------------

#[test]
fn goto_forward() {
    k9::assert_equal!(
        run_one(
            "local x = 0
goto done
x = 99
::done::
return x"
        ),
        Value::Integer(0)
    );
}

#[test]
fn goto_backward() {
    k9::assert_equal!(
        run_one(
            "local i = 0
::loop::
if i >= 3 then goto done end
i = i + 1
goto loop
::done::
return i"
        ),
        Value::Integer(3)
    );
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

#[test]
fn string_literal_escapes() {
    k9::assert_equal!(
        run_one(r#"return "hello\nworld""#),
        Value::String(bytes::Bytes::from("hello\nworld"))
    );
}

#[test]
fn string_hex_escape() {
    k9::assert_equal!(
        run_one(r#"return "\x41\x42\x43""#),
        Value::String(bytes::Bytes::from("ABC"))
    );
}

#[test]
fn string_decimal_escape() {
    k9::assert_equal!(
        run_one("return \"\\65\\66\\67\""),
        Value::String(bytes::Bytes::from("ABC"))
    );
}

#[test]
fn string_len() {
    k9::assert_equal!(run_one(r#"return #"hello""#), Value::Integer(5));
}

#[test]
fn string_concat_non_trivial() {
    k9::assert_equal!(
        run_one(r#"local a = "foo" local b = "bar" return a .. b"#),
        Value::String(bytes::Bytes::from("foobar"))
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
    k9::assert_equal!(
        run_one("local t = {x = 42} return t.x"),
        Value::Integer(42)
    );
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
    k9::assert_equal!(
        run_one("local t = {} return t.missing"),
        Value::Nil
    );
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
// error / assert / pcall / xpcall
// ---------------------------------------------------------------------------

#[test]
fn pcall_success() {
    k9::assert_equal!(
        run_one("local ok, v = pcall(function() return 42 end) return ok"),
        Value::Boolean(true)
    );
}

#[test]
fn pcall_success_result() {
    k9::assert_equal!(
        run_one("local ok, v = pcall(function() return 42 end) return v"),
        Value::Integer(42)
    );
}

#[test]
fn pcall_error_caught() {
    k9::assert_equal!(
        run_one(
            "local ok, msg = pcall(function() error('boom') end)
return ok"
        ),
        Value::Boolean(false)
    );
}

#[test]
fn pcall_error_message() {
    k9::assert_equal!(
        run_one(
            "local ok, msg = pcall(function() error('boom') end)
return msg"
        ),
        Value::String(bytes::Bytes::from_static(b"boom"))
    );
}

#[test]
fn pcall_error_value() {
    // error() can throw any value; pcall preserves it.
    k9::assert_equal!(
        run_one(
            "local ok, v = pcall(function() error(99) end)
return v"
        ),
        Value::Integer(99)
    );
}

#[test]
fn pcall_nested() {
    // Inner pcall catches its error; outer pcall succeeds.
    k9::assert_equal!(
        run_one(
            "local function inner()
    local ok, msg = pcall(function() error('inner') end)
    return ok
end
local ok, v = pcall(inner)
return v"
        ),
        Value::Boolean(false)
    );
}

#[test]
fn assert_pass() {
    k9::assert_equal!(run_one("return assert(42)"), Value::Integer(42));
}

#[test]
fn assert_fail() {
    k9::assert_equal!(
        run_one(
            "local ok, msg = pcall(function() assert(false, 'bad') end)
return msg"
        ),
        Value::String(bytes::Bytes::from_static(b"bad"))
    );
}

#[test]
fn xpcall_success() {
    k9::assert_equal!(
        run_one(
            "local ok, v = xpcall(function() return 7 end, function(e) return 'handled' end)
return ok"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn xpcall_handler_called() {
    k9::assert_equal!(
        run_one(
            "local ok, v = xpcall(
    function() error('oops') end,
    function(e) return 'caught: ' .. e end
)
return v"
        ),
        Value::String(bytes::Bytes::from("caught: oops"))
    );
}

// ---------------------------------------------------------------------------
// Vararg / select / collectgarbage / string length
// ---------------------------------------------------------------------------

#[test]
fn string_length() {
    k9::assert_equal!(run_one("return #'hello'"), Value::Integer(5));
}

#[test]
fn string_length_empty() {
    k9::assert_equal!(run_one("return #''"), Value::Integer(0));
}

#[test]
fn vararg_single_value() {
    // `...` in single-value context takes only the first vararg.
    k9::assert_equal!(
        run_one(
            "local function f(...)
    local x = ...
    return x
end
return f(42)"
        ),
        Value::Integer(42)
    );
}

#[test]
fn vararg_return_all() {
    k9::assert_equal!(
        run_all(
            "local function f(...)
    return ...
end
return f(1, 2, 3)"
        ),
        vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[test]
fn vararg_local_multi() {
    // `local a, b = ...` expands varargs into both slots.
    k9::assert_equal!(
        run_all(
            "local function f(...)
    local a, b = ...
    return a, b
end
return f(10, 20)"
        ),
        vec![Value::Integer(10), Value::Integer(20)]
    );
}

#[test]
fn vararg_pass_to_call() {
    // Passing `...` as the last argument to another function.
    k9::assert_equal!(
        run_all(
            "local function sum(a, b) return a + b end
local function proxy(...)
    return sum(...)
end
return proxy(3, 4)"
        ),
        vec![Value::Integer(7)]
    );
}

#[test]
fn vararg_count_via_select() {
    k9::assert_equal!(
        run_one(
            "local function count(...)
    return select('#', ...)
end
return count(1, 2, 3)"
        ),
        Value::Integer(3)
    );
}

#[test]
fn select_hash() {
    k9::assert_equal!(
        run_one("return select('#', 10, 20, 30)"),
        Value::Integer(3)
    );
}

#[test]
fn select_index() {
    k9::assert_equal!(
        run_all("return select(2, 'a', 'b', 'c')"),
        vec![
            Value::String(bytes::Bytes::from_static(b"b")),
            Value::String(bytes::Bytes::from_static(b"c")),
        ]
    );
}

#[test]
fn select_negative_index() {
    k9::assert_equal!(
        run_one("return select(-1, 'a', 'b', 'c')"),
        Value::String(bytes::Bytes::from_static(b"c"))
    );
}

#[test]
fn collectgarbage_collect() {
    k9::assert_equal!(
        run_one("return collectgarbage('collect')"),
        Value::Integer(0)
    );
}

#[test]
fn collectgarbage_count() {
    k9::assert_equal!(
        run_one("return collectgarbage('count')"),
        Value::Float(0.0)
    );
}

// ---------------------------------------------------------------------------
// Metatables
// ---------------------------------------------------------------------------

#[test]
fn setmetatable_getmetatable() {
    k9::assert_equal!(
        run_one(
            "local t = {}
local mt = {}
setmetatable(t, mt)
return getmetatable(t) == mt"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn metatable_index_table() {
    // __index as a table: prototype-based inheritance.
    k9::assert_equal!(
        run_one(
            "local proto = { x = 42 }
local obj = setmetatable({}, { __index = proto })
return obj.x"
        ),
        Value::Integer(42)
    );
}

#[test]
fn metatable_index_table_own_field_wins() {
    k9::assert_equal!(
        run_one(
            "local proto = { x = 1 }
local obj = setmetatable({ x = 99 }, { __index = proto })
return obj.x"
        ),
        Value::Integer(99)
    );
}

#[test]
fn metatable_index_chain() {
    // Two-level prototype chain.
    k9::assert_equal!(
        run_one(
            "local base = { z = 7 }
local mid  = setmetatable({}, { __index = base })
local obj  = setmetatable({}, { __index = mid })
return obj.z"
        ),
        Value::Integer(7)
    );
}

#[test]
fn metatable_index_function() {
    // __index as a function: called with (table, key).
    k9::assert_equal!(
        run_one(
            "local obj = setmetatable({}, {
    __index = function(t, k) return k .. '!' end
})
return obj.hello"
        ),
        Value::String(bytes::Bytes::from_static(b"hello!"))
    );
}

#[test]
fn metatable_newindex_function() {
    // __newindex is called when assigning a new key.
    k9::assert_equal!(
        run_one(
            "local log = nil
local obj = setmetatable({}, {
    __newindex = function(t, k, v)
        log = k
        rawset(t, k, v)
    end
})
obj.foo = 42
return log"
        ),
        Value::String(bytes::Bytes::from_static(b"foo"))
    );
}

#[test]
fn metatable_newindex_existing_skips_mm() {
    // __newindex is NOT called when the key already exists.
    k9::assert_equal!(
        run_one(
            "local called = false
local obj = setmetatable({ x = 1 }, {
    __newindex = function(t, k, v) called = true end
})
obj.x = 2  -- key exists, no __newindex
return called"
        ),
        Value::Boolean(false)
    );
}

#[test]
fn metatable_call() {
    // __call makes a table callable.
    k9::assert_equal!(
        run_one(
            "local callable = setmetatable({}, {
    __call = function(self, a, b) return a + b end
})
return callable(3, 4)"
        ),
        Value::Integer(7)
    );
}

#[test]
fn metatable_len() {
    // __len overrides #.
    k9::assert_equal!(
        run_one(
            "local obj = setmetatable({}, {
    __len = function(t) return 42 end
})
return #obj"
        ),
        Value::Integer(42)
    );
}

#[test]
fn oop_class_pattern() {
    // Full OOP class pattern.
    k9::assert_equal!(
        run_one(
            "local Animal = {}
Animal.__index = Animal

function Animal.new(name)
    return setmetatable({ name = name }, Animal)
end

function Animal:speak()
    return self.name .. ' says hello'
end

local a = Animal.new('Cat')
return a:speak()"
        ),
        Value::String(bytes::Bytes::from("Cat says hello"))
    );
}

#[test]
fn rawget_bypasses_index() {
    k9::assert_equal!(
        run_one(
            "local proto = { x = 99 }
local obj = setmetatable({}, { __index = proto })
return rawget(obj, 'x')"
        ),
        Value::Nil
    );
}

#[test]
fn rawset_bypasses_newindex() {
    k9::assert_equal!(
        run_one(
            "local called = false
local obj = setmetatable({}, {
    __newindex = function() called = true end
})
rawset(obj, 'k', 1)
return called"
        ),
        Value::Boolean(false)
    );
}
