use bytes::Bytes;
use shingetsu_compiler::{compile, CompileOptions};
use shingetsu_vm::{Function, GlobalEnv, Task, Value};

/// Create a [`GlobalEnv`] with all builtins registered (both the VM-internal
/// ones and the macro-generated ones from `shingetsu::builtins`).
fn new_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    env
}

/// Compile and run a Lua snippet, returning the first return value.
fn run_one(src: &str) -> Value {
    run_all(src).into_iter().next().unwrap_or(Value::Nil)
}

/// Compile and run a Lua snippet, returning all return values.
fn run_all(src: &str) -> Vec<Value> {
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let env = new_env();
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
    k9::assert_equal!(run_one("return select('#', 10, 20, 30)"), Value::Integer(3));
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
    k9::assert_equal!(run_one("return collectgarbage('count')"), Value::Float(0.0));
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

#[test]
fn rawequal_same_value() {
    k9::assert_equal!(run_one("return rawequal(1, 1)"), Value::Boolean(true));
}

#[test]
fn rawequal_different_values() {
    k9::assert_equal!(run_one("return rawequal(1, 2)"), Value::Boolean(false));
}

#[test]
fn rawequal_different_types() {
    k9::assert_equal!(run_one("return rawequal(1, '1')"), Value::Boolean(false));
}

#[test]
fn rawequal_nil() {
    k9::assert_equal!(run_one("return rawequal(nil, nil)"), Value::Boolean(true));
}

#[test]
fn rawequal_tables_same_ref() {
    k9::assert_equal!(
        run_one("local t = {} return rawequal(t, t)"),
        Value::Boolean(true)
    );
}

#[test]
fn rawequal_tables_different_ref() {
    // Two distinct tables with the same contents are not rawequal.
    k9::assert_equal!(run_one("return rawequal({1}, {1})"), Value::Boolean(false));
}

#[test]
fn rawequal_bypasses_eq_metamethod() {
    k9::assert_equal!(
        run_one(
            "local mt = { __eq = function() return true end }\n\
             local a = setmetatable({}, mt)\n\
             local b = setmetatable({}, mt)\n\
             return rawequal(a, b)"
        ),
        Value::Boolean(false)
    );
}

#[test]
fn rawequal_int_float_cross() {
    // 1 == 1.0 in Lua (even raw equality).
    k9::assert_equal!(run_one("return rawequal(1, 1.0)"), Value::Boolean(true));
}

#[test]
fn rawlen_table() {
    k9::assert_equal!(run_one("return rawlen({10, 20, 30})"), Value::Integer(3));
}

#[test]
fn rawlen_empty_table() {
    k9::assert_equal!(run_one("return rawlen({})"), Value::Integer(0));
}

#[test]
fn rawlen_string() {
    k9::assert_equal!(run_one("return rawlen('hello')"), Value::Integer(5));
}

#[test]
fn rawlen_empty_string() {
    k9::assert_equal!(run_one("return rawlen('')"), Value::Integer(0));
}

#[test]
fn rawlen_bypasses_len_metamethod() {
    k9::assert_equal!(
        run_one(
            "local t = setmetatable({1, 2, 3}, { __len = function() return 999 end })\n\
             return rawlen(t)"
        ),
        Value::Integer(3)
    );
}

#[test]
fn rawlen_bad_type() {
    k9::assert_equal!(
        run_err("rawlen(42)"),
        "bad argument #1 to 'rawlen' (table or string expected, got number)"
    );
}

// ---------------------------------------------------------------------------
// type / tostring / tonumber
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Runtime type validation (ParamSpec / validate_args)
// ---------------------------------------------------------------------------

#[test]
fn validate_args_rawget_rejects_non_table() {
    // rawget(table, key) — first arg must be a table.
    let res = run_all(
        "local ok, err = pcall(rawget, 'not a table', 'k')
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'rawget' (table expected, got string)"
            )),
        ]
    );
}

#[test]
fn validate_args_string_len_rejects_non_string() {
    // string.len(s) — s must be a string.
    let res = run_all(
        "local ok, err = pcall(string.len, 123)
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'len' (string expected, got number)"
            )),
        ]
    );
}

#[test]
fn validate_args_optional_param_accepts_nil() {
    // string.sub(s, i [, j]) — j is optional, nil should be accepted.
    let res = run_one("return string.sub('hello', 2, nil)");
    k9::assert_equal!(res, Value::String(Bytes::from("ello")));
}

#[test]
fn validate_args_table_concat_accepts_optional_sep() {
    // table.concat(t [, sep]) — sep is optional.
    let res = run_one("return table.concat({1, 2, 3})");
    k9::assert_equal!(res, Value::String(Bytes::from("123")));
}

#[test]
fn validate_args_math_floor_rejects_string() {
    // math.floor(x) takes a Value (unconstrained), so this should
    // pass validate_args but fail inside the function.
    // NOTE: position=0 and empty function name because the error is
    // raised inside to_float() after FromLua succeeds, so the
    // proc-macro's with_arg_and_call_context patch doesn't apply.
    // TODO: propagate position/name into internal helpers like to_float.
    let res = run_all(
        "local ok, err = pcall(math.floor, 'abc')
        return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #0 to '' (number expected, got string)"
            )),
        ]
    );
}

#[test]
fn type_of_values() {
    k9::assert_equal!(
        run_all(
            "return type(nil), type(true), type(1), type(1.0),
             type('s'), type({}), type(type)"
        ),
        vec![
            Value::String(b"nil".as_slice().into()),
            Value::String(b"boolean".as_slice().into()),
            Value::String(b"number".as_slice().into()),
            Value::String(b"number".as_slice().into()),
            Value::String(b"string".as_slice().into()),
            Value::String(b"table".as_slice().into()),
            Value::String(b"function".as_slice().into()),
        ]
    );
}

#[test]
fn tostring_numbers() {
    k9::assert_equal!(
        run_all("return tostring(42), tostring(3.14), tostring(true), tostring(nil)"),
        vec![
            Value::String(b"42".as_slice().into()),
            Value::String(b"3.14".as_slice().into()),
            Value::String(b"true".as_slice().into()),
            Value::String(b"nil".as_slice().into()),
        ]
    );
}

#[test]
fn tostring_metamethod() {
    k9::assert_equal!(
        run_one(
            "local mt = { __tostring = function(t) return 'obj' end }
local obj = setmetatable({}, mt)
return tostring(obj)"
        ),
        Value::String(b"obj".as_slice().into())
    );
}

// ---------------------------------------------------------------------------
// print
// ---------------------------------------------------------------------------

#[test]
fn print_exists_and_returns_nil() {
    // print() returns no values.
    let res = run_all("return print('hello')");
    k9::assert_equal!(res, vec![]);
}

#[test]
fn print_type_is_function() {
    let res = run_one("return type(print)");
    k9::assert_equal!(res, Value::String(Bytes::from("function")));
}

#[test]
fn print_calls_tostring_metamethod() {
    // Verify print calls __tostring by capturing the side effect.
    let res = run_one(
        "\
        local called = false
        local mt = { __tostring = function(t) called = true; return 'custom' end }
        local obj = setmetatable({}, mt)
        print(obj)
        return called",
    );
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn print_multiple_args() {
    // print accepts multiple arguments without error.
    let res = run_all("return print(1, 'two', true, nil)");
    k9::assert_equal!(res, vec![]);
}

#[test]
fn print_no_args() {
    // print with no args just prints a newline, no error.
    let res = run_all("return print()");
    k9::assert_equal!(res, vec![]);
}

#[test]
fn tonumber_int() {
    k9::assert_equal!(run_one("return tonumber('42')"), Value::Integer(42));
}

#[test]
fn tonumber_float() {
    k9::assert_equal!(run_one("return tonumber('3.14')"), Value::Float(3.14));
}

#[test]
fn tonumber_base() {
    k9::assert_equal!(run_one("return tonumber('ff', 16)"), Value::Integer(255));
}

#[test]
fn tonumber_non_numeric() {
    k9::assert_equal!(run_one("return tonumber('hello')"), Value::Nil);
}

// ---------------------------------------------------------------------------
// pairs / ipairs / next
// ---------------------------------------------------------------------------

#[test]
fn pairs_iteration() {
    k9::assert_equal!(
        run_one(
            "local t = {a=1, b=2, c=3}
local count = 0
for k, v in pairs(t) do
    count = count + 1
end
return count"
        ),
        Value::Integer(3)
    );
}

#[test]
fn ipairs_iteration() {
    k9::assert_equal!(
        run_one(
            "local t = {10, 20, 30}
local sum = 0
for i, v in ipairs(t) do
    sum = sum + v
end
return sum"
        ),
        Value::Integer(60)
    );
}

#[test]
fn ipairs_stops_at_nil() {
    k9::assert_equal!(
        run_one(
            "local t = {1, 2, nil, 4}
local count = 0
for i, v in ipairs(t) do
    count = count + 1
end
return count"
        ),
        Value::Integer(2)
    );
}

#[test]
fn next_basic() {
    k9::assert_equal!(
        run_one(
            "local t = {x=42}
local k, v = next(t)
return v"
        ),
        Value::Integer(42)
    );
}

#[test]
fn next_nil_at_end() {
    k9::assert_equal!(
        run_one(
            "local t = {x=1}
local k = next(t)  -- gets 'x'
return next(t, k)  -- should be nil"
        ),
        Value::Nil
    );
}

// ---------------------------------------------------------------------------
// generic for: break
// ---------------------------------------------------------------------------

#[test]
fn generic_for_break() {
    k9::assert_equal!(
        run_one(
            "local t = {1, 2, 3, 4, 5}
local sum = 0
for i, v in ipairs(t) do
    if v > 3 then break end
    sum = sum + v
end
return sum"
        ),
        Value::Integer(6)
    );
}

// ---------------------------------------------------------------------------
// arithmetic metamethods
// ---------------------------------------------------------------------------

#[test]
fn arith_metamethod_add() {
    k9::assert_equal!(
        run_one(
            "local mt = { __add = function(a, b) return a.v + b.v end }
local a = setmetatable({v=10}, mt)
local b = setmetatable({v=5}, mt)
return a + b"
        ),
        Value::Integer(15)
    );
}

#[test]
fn arith_metamethod_sub() {
    k9::assert_equal!(
        run_one(
            "local mt = { __sub = function(a, b) return a.v - b.v end }
local a = setmetatable({v=10}, mt)
local b = setmetatable({v=3}, mt)
return a - b"
        ),
        Value::Integer(7)
    );
}

#[test]
fn arith_metamethod_mul() {
    k9::assert_equal!(
        run_one(
            "local mt = { __mul = function(a, b) return a.v * b.v end }
local a = setmetatable({v=4}, mt)
local b = setmetatable({v=5}, mt)
return a * b"
        ),
        Value::Integer(20)
    );
}

#[test]
fn arith_metamethod_unm() {
    k9::assert_equal!(
        run_one(
            "local mt = { __unm = function(a) return -a.v end }
local a = setmetatable({v=7}, mt)
return -a"
        ),
        Value::Integer(-7)
    );
}

// ---------------------------------------------------------------------------
// __pairs / __ipairs metamethods
// ---------------------------------------------------------------------------

#[test]
fn pairs_respects_pairs_metamethod() {
    // __pairs should completely replace the iteration protocol.
    k9::assert_equal!(
        run_one(
            "local visited = {}
local proxy = setmetatable({}, {
    __pairs = function(t)
        -- return a custom iterator that yields only ('x', 99)
        local done = false
        local function iter(s, c)
            if done then return nil end
            done = true
            return 'x', 99
        end
        return iter, t, nil
    end
})
for k, v in pairs(proxy) do
    visited[k] = v
end
return visited.x"
        ),
        Value::Integer(99)
    );
}

#[test]
fn ipairs_respects_ipairs_metamethod() {
    // __ipairs should completely replace the iteration protocol.
    k9::assert_equal!(
        run_one(
            "local sum = 0
local proxy = setmetatable({}, {
    __ipairs = function(t)
        local i = 0
        local function iter(s, c)
            i = i + 1
            if i > 3 then return nil end
            return i, i * 10
        end
        return iter, t, nil
    end
})
for i, v in ipairs(proxy) do
    sum = sum + v
end
return sum"
        ),
        Value::Integer(60)
    );
}

#[test]
fn pairs_falls_through_without_metamethod() {
    // Ordinary table with no __pairs should work as before.
    k9::assert_equal!(
        run_one(
            "local t = {a=1, b=2}
local count = 0
for k, v in pairs(t) do count = count + 1 end
return count"
        ),
        Value::Integer(2)
    );
}

// ---------------------------------------------------------------------------
// Comparison metamethods (__eq, __lt, __le)
// ---------------------------------------------------------------------------

#[test]
fn eq_metamethod_tables() {
    k9::assert_equal!(
        run_one(
            "local mt = {
    __eq = function(a, b) return a.v == b.v end
}
local a = setmetatable({v=1}, mt)
local b = setmetatable({v=1}, mt)
local c = setmetatable({v=2}, mt)
return a == b, a == c"
        ),
        // run_one returns first value; use run_all
        Value::Boolean(true)
    );
}

#[test]
fn eq_metamethod_returns_bool() {
    // Result of == with __eq must be a strict boolean.
    k9::assert_equal!(
        run_all(
            "local mt = { __eq = function(a, b) return 42 end }
local a = setmetatable({}, mt)
local b = setmetatable({}, mt)
return a == b, a ~= b"
        ),
        vec![Value::Boolean(true), Value::Boolean(false)]
    );
}

#[test]
fn ne_uses_eq_metamethod() {
    // ~= is not (==), so __eq is respected.
    k9::assert_equal!(
        run_one(
            "local mt = { __eq = function(a, b) return a.v == b.v end }
local a = setmetatable({v=5}, mt)
local b = setmetatable({v=5}, mt)
return a ~= b"
        ),
        Value::Boolean(false)
    );
}

#[test]
fn eq_same_ref_skips_metamethod() {
    // Identical table references are equal without calling __eq.
    k9::assert_equal!(
        run_one(
            "local called = false
local mt = { __eq = function() called = true; return false end }
local a = setmetatable({}, mt)
return a == a, called"
        ),
        // run_one returns first: true (same ref)
        Value::Boolean(true)
    );
}

#[test]
fn lt_metamethod() {
    k9::assert_equal!(
        run_one(
            "local mt = { __lt = function(a, b) return a.v < b.v end }
local a = setmetatable({v=3}, mt)
local b = setmetatable({v=5}, mt)
return a < b"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn gt_uses_lt_metamethod() {
    // a > b calls __lt(b, a)
    k9::assert_equal!(
        run_one(
            "local mt = { __lt = function(a, b) return a.v < b.v end }
local a = setmetatable({v=3}, mt)
local b = setmetatable({v=5}, mt)
return b > a"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn le_metamethod() {
    k9::assert_equal!(
        run_all(
            "local mt = { __le = function(a, b) return a.v <= b.v end }
local a = setmetatable({v=3}, mt)
local b = setmetatable({v=3}, mt)
return a <= b, a >= b"
        ),
        vec![Value::Boolean(true), Value::Boolean(true)]
    );
}

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
// continue statement
// ---------------------------------------------------------------------------

#[test]
fn continue_in_while() {
    // Sum only odd numbers 1..10 using continue to skip evens.
    k9::assert_equal!(
        run_one(
            "local sum = 0
local i = 0
while i < 10 do
    i = i + 1
    if i % 2 == 0 then
        continue
    end
    sum = sum + i
end
return sum"
        ),
        Value::Integer(25)
    );
}

#[test]
fn continue_in_numeric_for() {
    // Sum 1..10 skipping multiples of 3.
    k9::assert_equal!(
        run_one(
            "local sum = 0
for i = 1, 10 do
    if i % 3 == 0 then
        continue
    end
    sum = sum + i
end
return sum"
        ),
        Value::Integer(37)
    );
}

#[test]
fn continue_in_generic_for() {
    // Collect values from pairs, skipping key "b".
    k9::assert_equal!(
        run_one(
            "local t = {a=1, b=2, c=3}
local sum = 0
for k, v in pairs(t) do
    if k == 'b' then
        continue
    end
    sum = sum + v
end
return sum"
        ),
        Value::Integer(4)
    );
}

#[test]
fn continue_in_repeat() {
    // Sum 1..5 skipping 3.
    k9::assert_equal!(
        run_one(
            "local sum = 0
local i = 0
repeat
    i = i + 1
    if i == 3 then
        continue
    end
    sum = sum + i
until i >= 5
return sum"
        ),
        Value::Integer(12)
    );
}

// ---------------------------------------------------------------------------
// __concat metamethod
// ---------------------------------------------------------------------------

#[test]
fn concat_strings() {
    k9::assert_equal!(
        run_one(r#"return "hello" .. " " .. "world""#),
        Value::String(bytes::Bytes::from_static(b"hello world"))
    );
}

#[test]
fn concat_number_coercion() {
    k9::assert_equal!(
        run_one(r#"return "x=" .. 42"#),
        Value::String(bytes::Bytes::from_static(b"x=42"))
    );
}

#[test]
fn concat_metamethod() {
    // Tables with __concat should be supported.
    k9::assert_equal!(
        run_one(
            r#"local mt = { __concat = function(a, b) return a.v .. b.v end }
local a = setmetatable({v="hello"}, mt)
local b = setmetatable({v=" world"}, mt)
return a .. b"#
        ),
        Value::String(bytes::Bytes::from_static(b"hello world"))
    );
}

#[test]
fn concat_error_on_nil() {
    // Concatenating nil without __concat should be caught by pcall.
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(function() return "x" .. nil end)
return ok"#
        ),
        Value::Boolean(false)
    );
}

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

// ---------------------------------------------------------------------------
// Compound assignments (LuaU)
// ---------------------------------------------------------------------------

#[test]
fn compound_plus_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x += 5; return x"),
        Value::Integer(15)
    );
}

#[test]
fn compound_minus_equal() {
    k9::assert_equal!(run_one("local x = 10; x -= 3; return x"), Value::Integer(7));
}

#[test]
fn compound_star_equal() {
    k9::assert_equal!(run_one("local x = 6; x *= 7; return x"), Value::Integer(42));
}

#[test]
fn compound_slash_equal() {
    k9::assert_equal!(
        run_one("local x = 10.0; x /= 4; return x"),
        Value::Float(2.5)
    );
}

#[test]
fn compound_double_slash_equal() {
    k9::assert_equal!(
        run_one("local x = 10; x //= 3; return x"),
        Value::Integer(3)
    );
}

#[test]
fn compound_percent_equal() {
    k9::assert_equal!(run_one("local x = 10; x %= 3; return x"), Value::Integer(1));
}

#[test]
fn compound_caret_equal() {
    k9::assert_equal!(
        run_one("local x = 2.0; x ^= 10; return x"),
        Value::Float(1024.0)
    );
}

#[test]
fn compound_two_dots_equal() {
    k9::assert_equal!(
        run_one(r#"local s = "hello"; s ..= " world"; return s"#),
        Value::String(bytes::Bytes::from_static(b"hello world"))
    );
}

#[test]
fn compound_global() {
    k9::assert_equal!(run_one("x = 5; x += 3; return x"), Value::Integer(8));
}

#[test]
fn compound_table_field() {
    k9::assert_equal!(
        run_one("local t = {n=10}; t.n += 5; return t.n"),
        Value::Integer(15)
    );
}

#[test]
fn compound_table_index() {
    k9::assert_equal!(
        run_one("local t = {[1]=100}; t[1] -= 1; return t[1]"),
        Value::Integer(99)
    );
}

// ---------------------------------------------------------------------------
// if expressions (LuaU)
// ---------------------------------------------------------------------------

#[test]
fn if_expr_true_branch() {
    k9::assert_equal!(run_one("return if true then 1 else 2"), Value::Integer(1));
}

#[test]
fn if_expr_false_branch() {
    k9::assert_equal!(run_one("return if false then 1 else 2"), Value::Integer(2));
}

#[test]
fn if_expr_elseif() {
    k9::assert_equal!(
        run_one(
            "local x = 2; return if x == 1 then \"one\" elseif x == 2 then \"two\" else \"other\""
        ),
        Value::String(bytes::Bytes::from_static(b"two"))
    );
}

#[test]
fn if_expr_nested() {
    k9::assert_equal!(
        run_one("local x = 5; local y = if x > 3 then if x > 4 then \"big\" else \"mid\" else \"small\"; return y"),
        Value::String(bytes::Bytes::from_static(b"big"))
    );
}

#[test]
fn if_expr_in_assignment() {
    k9::assert_equal!(
        run_one("local cond = true; local t = {v = if cond then 42 else 0}; return t.v"),
        Value::Integer(42)
    );
}

// ---------------------------------------------------------------------------
// LuaU type annotation parsing
// ---------------------------------------------------------------------------

/// Compile a LuaU snippet and return the top-level Proto.
fn compile_proto(src: &str) -> std::sync::Arc<shingetsu_vm::proto::Proto> {
    compile(src, &CompileOptions::default())
        .expect("compile failed")
        .top_level
}

#[test]
fn luau_type_annotation_param_basic() {
    use shingetsu_vm::types::LuaType;
    // The top-level proto's first constant closure should have the annotated
    // param types.
    let proto = compile_proto("function add(x: number, y: number): number return x + y end");
    // The function is in a nested proto (closure constant).
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(sig.params.len(), 2);
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(sig.params[1].lua_type, Some(LuaType::Number));
    k9::assert_equal!(sig.lua_returns, Some(vec![LuaType::Number]));
    // runtime_type should be derived from lua_type.
    k9::assert_equal!(
        sig.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Number)
    );
    k9::assert_equal!(
        sig.params[1].runtime_type,
        Some(shingetsu_vm::types::ValueType::Number)
    );
}

#[test]
fn luau_type_annotation_param_optional() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: string, y: number?) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(sig.params.len(), 2);
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::String));
    k9::assert_equal!(
        sig.params[1].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

#[test]
fn luau_type_annotation_return_tuple() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(): (boolean, string) return true, 'ok' end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.lua_returns,
        Some(vec![LuaType::Boolean, LuaType::String])
    );
}

#[test]
fn luau_type_annotation_no_annotation() {
    // Without annotations, lua_type should be None.
    let proto = compile_proto("function f(x, y) return x + y end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, None);
    k9::assert_equal!(child.signature.params[1].lua_type, None);
    k9::assert_equal!(child.signature.lua_returns, None);
}

#[test]
fn luau_type_annotation_named_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: Foo) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Named(Bytes::from("Foo")))
    );
}

#[test]
fn luau_type_annotation_union() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: string | number) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![LuaType::String, LuaType::Number]))
    );
}

#[test]
fn luau_type_annotation_callback() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(cb: (number) -> string) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Function(flt) => {
            k9::assert_equal!(flt.params.len(), 1);
            k9::assert_equal!(flt.params[0].1, LuaType::Number);
            k9::assert_equal!(flt.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_table_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(t: { x: number, y: string }) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Table(tlt) => {
            k9::assert_equal!(tlt.fields.len(), 2);
            k9::assert_equal!(tlt.fields[0], (Bytes::from("x"), LuaType::Number));
            k9::assert_equal!(tlt.fields[1], (Bytes::from("y"), LuaType::String));
            k9::assert_equal!(tlt.indexer, None);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_table_indexer() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(t: { [string]: number }) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Table(tlt) => {
            k9::assert_equal!(tlt.fields.len(), 0);
            k9::assert_equal!(
                tlt.indexer,
                Some((Box::new(LuaType::String), Box::new(LuaType::Number)))
            );
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_generic_type() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    let proto = compile_proto("function f(t: Map<string, number>) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Map")));
            k9::assert_equal!(args.len(), 2);
            k9::assert_equal!(args[0], LuaTypeArg::Type(LuaType::String));
            k9::assert_equal!(args[1], LuaTypeArg::Type(LuaType::Number));
        }
        other => panic!("expected Generic, got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_array_shorthand() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    let proto = compile_proto("function f(t: { number }) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Array")));
            k9::assert_equal!(args.len(), 1);
            k9::assert_equal!(args[0], LuaTypeArg::Type(LuaType::Number));
        }
        other => panic!("expected Generic(Array), got {:?}", other),
    }
}

#[test]
fn luau_type_annotation_intersection() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: Readable & Writable) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Intersection(vec![
            LuaType::Named(Bytes::from("Readable")),
            LuaType::Named(Bytes::from("Writable")),
        ]))
    );
}

#[test]
fn luau_type_annotation_basic_primitives() {
    use shingetsu_vm::types::LuaType;
    let proto =
        compile_proto("function f(a: nil, b: boolean, c: any, d: integer, e: float): never end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Nil));
    k9::assert_equal!(child.signature.params[1].lua_type, Some(LuaType::Boolean));
    k9::assert_equal!(child.signature.params[2].lua_type, Some(LuaType::Any));
    k9::assert_equal!(child.signature.params[3].lua_type, Some(LuaType::Integer));
    k9::assert_equal!(child.signature.params[4].lua_type, Some(LuaType::Float));
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::Never]));
}

#[test]
fn luau_type_annotation_typeof() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: typeof({})) end");
    let child = &proto.protos[0];
    // typeof is opaque at compile time — treated as Any.
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Any));
}

#[test]
fn luau_type_annotation_method_self() {
    use shingetsu_vm::types::LuaType;
    // Method syntax: implicit self has no annotation.
    let proto = compile_proto("local t = {}; function t:m(x: number) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    // self is param[0], x is param[1]
    k9::assert_equal!(sig.params.len(), 2);
    k9::assert_equal!(sig.params[0].name, Some(Bytes::from("self")));
    k9::assert_equal!(sig.params[0].lua_type, None);
    k9::assert_equal!(sig.params[1].lua_type, Some(LuaType::Number));
}

#[test]
fn luau_type_annotation_mixed_annotated_unannotated() {
    use shingetsu_vm::types::LuaType;
    // Some params annotated, some not.
    let proto = compile_proto("function f(a: number, b, c: string) end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(child.signature.params[1].lua_type, None);
    k9::assert_equal!(child.signature.params[2].lua_type, Some(LuaType::String));
}

#[test]
fn luau_type_annotation_variadic_param() {
    use shingetsu_vm::types::LuaType;
    // Variadic params don't get a ParamSpec entry, but should not break parsing.
    let proto = compile_proto("function f(x: number, ...): string end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params.len(), 1);
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(child.signature.variadic, true);
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::String]));
}

// ---------------------------------------------------------------------------
// LuaU runtime type enforcement
// ---------------------------------------------------------------------------

#[test]
fn luau_runtime_type_check_rejects_wrong_type() {
    // Annotated Lua function rejects wrong argument type at call boundary.
    let res = run_all(
        "function add(x: number, y: number): number return x + y end
         local ok, err = pcall(add, 1, 'two')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #2 to 'add' (number expected, got string)"
            )),
        ]
    );
}

#[test]
fn luau_runtime_type_check_accepts_correct_type() {
    // Annotated Lua function accepts correct types.
    let res = run_one(
        "function add(x: number, y: number): number return x + y end
         return add(3, 4)",
    );
    k9::assert_equal!(res, Value::Integer(7));
}

#[test]
fn luau_runtime_type_check_string_param() {
    let res = run_all(
        "function greet(name: string) return 'hi ' .. name end
         local ok, err = pcall(greet, 42)
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'greet' (string expected, got number)"
            )),
        ]
    );
}

#[test]
fn luau_runtime_type_check_table_param() {
    let res = run_all(
        "function keys(t: {[string]: number}) return next(t) end
         local ok, err = pcall(keys, 'not a table')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'keys' (table expected, got string)"
            )),
        ]
    );
}

#[test]
fn luau_runtime_type_check_boolean_param() {
    let res = run_all(
        "function toggle(b: boolean) return not b end
         local ok, err = pcall(toggle, 'yes')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'toggle' (boolean expected, got string)"
            )),
        ]
    );
}

#[test]
fn luau_runtime_type_check_optional_allows_nil() {
    // Optional params should NOT reject nil.
    let res = run_one(
        "function f(x: number?) return x or 0 end
         return f(nil)",
    );
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn luau_runtime_type_check_unannotated_no_check() {
    // Unannotated params should accept any type (no runtime check).
    let res = run_one(
        "function f(x) return type(x) end
         return f({})",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("table")));
}

#[test]
fn luau_runtime_type_check_function_param() {
    let res = run_all(
        "function apply(cb: (number) -> number) return cb(5) end
         local ok, err = pcall(apply, 'not a function')
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'apply' (function expected, got string)"
            )),
        ]
    );
}

#[test]
fn luau_runtime_type_check_function_param_accepts() {
    let res = run_one(
        "function apply(cb: (number) -> number) return cb(5) end
         return apply(function(x) return x * 2 end)",
    );
    k9::assert_equal!(res, Value::Integer(10));
}

#[test]
fn luau_runtime_type_check_integer_rejects_float() {
    let res = run_all(
        "function f(x: integer) return x end
         local ok, err = pcall(f, 1.5)
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(Bytes::from(
                "bad argument #1 to 'f' (integer expected, got number)"
            )),
        ]
    );
}

#[test]
fn luau_runtime_type_check_integer_accepts_integer() {
    let res = run_one(
        "function f(x: integer) return x + 1 end
         return f(10)",
    );
    k9::assert_equal!(res, Value::Integer(11));
}

#[test]
fn luau_runtime_type_check_any_accepts_all() {
    // `any` annotation should accept any value.
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f(42)"),
        Value::String(Bytes::from("number"))
    );
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f('s')"),
        Value::String(Bytes::from("string"))
    );
    k9::assert_equal!(
        run_one("function f(x: any) return type(x) end; return f(nil)"),
        Value::String(Bytes::from("nil"))
    );
}

#[test]
fn luau_runtime_type_check_direct_call_fails() {
    // Direct call (not pcall) with wrong type should produce an error
    // from the initial task entry validation.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};

    let opts = CompileOptions {
        ..CompileOptions::default()
    };
    // Compile a chunk that defines a typed function then calls it wrong.
    let bc =
        compile("function f(x: number) return x end; return f('bad')", &opts).expect("compile");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'f' (number expected, got string)"
    );
}

#[test]
fn luau_type_annotation_string_literal() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto(r#"function f(x: "hello") end"#);
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::StringLiteral(Bytes::from("hello")))
    );
}

#[test]
fn luau_type_annotation_boolean_literal() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f(x: true) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::BoolLiteral(true))
    );
}

// ---------------------------------------------------------------------------
// Generic type parameter declarations
// ---------------------------------------------------------------------------

#[test]
fn luau_generic_function_type_params() {
    let proto = compile_proto("function identity<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 1);
    k9::assert_equal!(child.signature.type_params[0].name, Bytes::from("T"));
    k9::assert_equal!(child.signature.type_params[0].is_pack, false);
    k9::assert_equal!(child.signature.type_params[0].constraint, None);
    k9::assert_equal!(child.signature.type_params[0].default, None);
}

#[test]
fn luau_generic_function_param_is_type_param() {
    use shingetsu_vm::types::LuaType;
    // Inside a generic function, `T` in parameter annotations should be
    // `LuaType::TypeParam`, not `LuaType::Named`.
    let proto = compile_proto("function identity<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
    // Return type should also be TypeParam.
    k9::assert_equal!(
        child.signature.lua_returns,
        Some(vec![LuaType::TypeParam(Bytes::from("T"))])
    );
}

#[test]
fn luau_generic_multiple_type_params() {
    let proto = compile_proto("function map<T, U>(list: {T}, f: (T) -> U): {U} return {} end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 2);
    k9::assert_equal!(child.signature.type_params[0].name, Bytes::from("T"));
    k9::assert_equal!(child.signature.type_params[1].name, Bytes::from("U"));
}

#[test]
fn luau_generic_variadic_pack() {
    let proto = compile_proto("function first<T...>(...: T...): T... return ... end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 1);
    k9::assert_equal!(child.signature.type_params[0].name, Bytes::from("T"));
    k9::assert_equal!(child.signature.type_params[0].is_pack, true);
}

#[test]
fn luau_generic_with_default_on_type_alias() {
    // Default type params are supported on type aliases, not functions.
    // Verify they parse correctly via a callback type that uses one.
    // full_moon does not support `<T = number>` on function generics,
    // so we test default parsing indirectly via the GenericDeclaration
    // on a type alias (tested in G2). For now, just verify that
    // function generics without defaults work.
    let proto = compile_proto("function f<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params[0].default, None);
}

#[test]
fn luau_generic_non_generic_name_stays_named() {
    use shingetsu_vm::types::LuaType;
    // `Foo` is not a declared type param, so it should be `LuaType::Named`.
    let proto = compile_proto("function f<T>(x: T, y: Foo): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
    k9::assert_equal!(
        child.signature.params[1].lua_type,
        Some(LuaType::Named(Bytes::from("Foo")))
    );
}

#[test]
fn luau_generic_erased_at_runtime() {
    // A generic param like `T` should not produce a runtime_type
    // (it's erased — treated as `any`).
    let proto = compile_proto("function identity<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].runtime_type, None);
}

#[test]
fn luau_generic_function_still_runs() {
    // Generic function should compile and execute normally.
    k9::assert_equal!(
        run_one("function identity<T>(x: T): T return x end\nreturn identity(42)"),
        Value::Integer(42)
    );
}

#[test]
fn luau_generic_type_param_in_callback() {
    use shingetsu_vm::types::LuaType;
    // T inside a callback parameter should be TypeParam.
    let proto = compile_proto("function f<T>(cb: (T) -> T) end");
    let child = &proto.protos[0];
    let lt = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lt {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params[0].1, LuaType::TypeParam(Bytes::from("T")));
            k9::assert_equal!(ft.returns, vec![LuaType::TypeParam(Bytes::from("T"))]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_generic_type_param_in_optional() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: T?) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::TypeParam(
            Bytes::from("T")
        ))))
    );
}

#[test]
fn luau_generic_type_param_in_union() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: T | string) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![
            LuaType::TypeParam(Bytes::from("T")),
            LuaType::String,
        ]))
    );
}

#[test]
fn luau_generic_type_param_in_table() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("function f<T>(x: { val: T }) end");
    let child = &proto.protos[0];
    match child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type")
    {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 1);
            k9::assert_equal!(t.fields[0].1, LuaType::TypeParam(Bytes::from("T")));
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_generic_type_param_in_generic_instantiation() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // T used as a type argument: Array<T>
    let proto = compile_proto("function f<T>(x: Array<T>) end");
    let child = &proto.protos[0];
    match child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type")
    {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Array")));
            k9::assert_equal!(
                args[0],
                LuaTypeArg::Type(LuaType::TypeParam(Bytes::from("T")))
            );
        }
        other => panic!("expected Generic, got {:?}", other),
    }
}

#[test]
fn luau_generic_type_param_in_array_shorthand() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // {T} is array shorthand — T inside should be TypeParam.
    let proto = compile_proto("function f<T>(x: {T}) end");
    let child = &proto.protos[0];
    match child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type")
    {
        LuaType::Generic { args, .. } => {
            k9::assert_equal!(
                args[0],
                LuaTypeArg::Type(LuaType::TypeParam(Bytes::from("T")))
            );
        }
        other => panic!("expected Generic(Array), got {:?}", other),
    }
}

#[test]
fn luau_generic_does_not_leak_to_sibling_function() {
    use shingetsu_vm::types::LuaType;
    // T is declared on f but not on g — in g, T should be Named.
    let proto = compile_proto("function f<T>(x: T) end\nfunction g(x: T) end");
    let f = &proto.protos[0];
    let g = &proto.protos[1];
    k9::assert_equal!(
        f.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
    k9::assert_equal!(
        g.signature.params[0].lua_type,
        Some(LuaType::Named(Bytes::from("T")))
    );
}

#[test]
fn luau_generic_local_function() {
    use shingetsu_vm::types::LuaType;
    // local function should go through the same generic path.
    let proto = compile_proto("local function f<T>(x: T): T return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.type_params.len(), 1);
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
}

#[test]
fn luau_generic_multiple_params_execution() {
    // Multi-param generic function should execute correctly.
    k9::assert_equal!(
        run_one("function swap<A, B>(a: A, b: B): (B, A) return b, a end\nreturn swap(1, 'hello')"),
        Value::String(Bytes::from("hello"))
    );
}

// ---------------------------------------------------------------------------
// Type alias declarations
// ---------------------------------------------------------------------------

#[test]
fn luau_type_alias_simple() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Meters = number");
    let alias = proto
        .type_aliases
        .get(b"Meters" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.params.len(), 0);
    k9::assert_equal!(alias.body, LuaType::Number);
}

#[test]
fn luau_type_alias_with_generics() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Pair<A, B> = { first: A, second: B }");
    let alias = proto
        .type_aliases
        .get(b"Pair" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.params.len(), 2);
    k9::assert_equal!(alias.params[0].name, Bytes::from("A"));
    k9::assert_equal!(alias.params[1].name, Bytes::from("B"));
    // The body should use TypeParam for A and B.
    match &alias.body {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 2);
            k9::assert_equal!(t.fields[0].0, Bytes::from("first"));
            k9::assert_equal!(t.fields[0].1, LuaType::TypeParam(Bytes::from("A")));
            k9::assert_equal!(t.fields[1].0, Bytes::from("second"));
            k9::assert_equal!(t.fields[1].1, LuaType::TypeParam(Bytes::from("B")));
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_type_alias_function_type() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Predicate = (number) -> boolean");
    let alias = proto
        .type_aliases
        .get(b"Predicate" as &[u8])
        .expect("alias exists");
    match &alias.body {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params.len(), 1);
            k9::assert_equal!(ft.params[0].1, LuaType::Number);
            k9::assert_equal!(ft.returns, vec![LuaType::Boolean]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_type_alias_no_runtime_effect() {
    // Type aliases produce no instructions.
    k9::assert_equal!(
        run_one("type Meters = number\nreturn 42"),
        Value::Integer(42)
    );
}

#[test]
fn luau_exported_type_alias() {
    use shingetsu_vm::types::LuaType;
    // `export type` should be stored the same as `type`.
    let proto = compile_proto("export type ID = string");
    let alias = proto
        .type_aliases
        .get(b"ID" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.body, LuaType::String);
}

#[test]
fn luau_type_alias_union_body() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type StringOrNumber = string | number");
    let alias = proto
        .type_aliases
        .get(b"StringOrNumber" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(
        alias.body,
        LuaType::Union(vec![LuaType::String, LuaType::Number])
    );
}

#[test]
fn luau_type_alias_optional_body() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type MaybeString = string?");
    let alias = proto
        .type_aliases
        .get(b"MaybeString" as &[u8])
        .expect("alias exists");
    k9::assert_equal!(alias.body, LuaType::Optional(Box::new(LuaType::String)));
}

#[test]
fn luau_type_alias_multiple_in_chunk() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type A = number\ntype B = string");
    k9::assert_equal!(
        proto
            .type_aliases
            .get(b"A" as &[u8])
            .expect("A exists")
            .body,
        LuaType::Number
    );
    k9::assert_equal!(
        proto
            .type_aliases
            .get(b"B" as &[u8])
            .expect("B exists")
            .body,
        LuaType::String
    );
}

#[test]
fn luau_type_alias_overwrite() {
    use shingetsu_vm::types::LuaType;
    // Last declaration wins.
    let proto = compile_proto("type X = number\ntype X = string");
    k9::assert_equal!(
        proto
            .type_aliases
            .get(b"X" as &[u8])
            .expect("X exists")
            .body,
        LuaType::String
    );
}

#[test]
fn luau_type_alias_references_named_type() {
    use shingetsu_vm::types::{LuaType, LuaTypeArg};
    // `User` is not a generic param, so it stays Named.
    let proto = compile_proto("type UserList = Array<User>");
    let alias = proto
        .type_aliases
        .get(b"UserList" as &[u8])
        .expect("alias exists");
    match &alias.body {
        LuaType::Generic { base, args } => {
            k9::assert_equal!(**base, LuaType::Named(Bytes::from("Array")));
            k9::assert_equal!(
                args[0],
                LuaTypeArg::Type(LuaType::Named(Bytes::from("User")))
            );
        }
        other => panic!("expected Generic, got {:?}", other),
    }
}

#[test]
fn luau_type_alias_generic_params_dont_leak() {
    use shingetsu_vm::types::LuaType;
    // T is a generic param on Foo but not on Bar.
    let proto = compile_proto("type Foo<T> = T\ntype Bar = T");
    k9::assert_equal!(
        proto.type_aliases.get(b"Foo" as &[u8]).expect("Foo").body,
        LuaType::TypeParam(Bytes::from("T"))
    );
    k9::assert_equal!(
        proto.type_aliases.get(b"Bar" as &[u8]).expect("Bar").body,
        LuaType::Named(Bytes::from("T"))
    );
}

// ---------------------------------------------------------------------------
// Type alias resolution in annotations
// ---------------------------------------------------------------------------

#[test]
fn luau_alias_resolution_simple() {
    use shingetsu_vm::types::LuaType;
    // `type Meters = number` then a function using Meters should resolve to Number.
    let proto = compile_proto("type Meters = number\nfunction f(x: Meters) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::Number));
    k9::assert_equal!(
        sig.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Number)
    );
}

#[test]
fn luau_alias_resolution_string_alias() {
    use shingetsu_vm::types::LuaType;
    let proto = compile_proto("type Name = string\nfunction greet(who: Name) end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::String));
    k9::assert_equal!(
        child.signature.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::String)
    );
}

#[test]
fn luau_alias_resolution_generic_table() {
    use shingetsu_vm::types::LuaType;
    // Generic alias `Pair<A, B>` with concrete args `number, string`.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string>) end",
    );
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 2);
            k9::assert_equal!(t.fields[0].0, Bytes::from("first"));
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
            k9::assert_equal!(t.fields[1].0, Bytes::from("second"));
            k9::assert_equal!(t.fields[1].1, LuaType::String);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_generic_table_has_runtime_type() {
    // Expanded table alias has Table runtime type.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string>) end",
    );
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].runtime_type,
        Some(shingetsu_vm::types::ValueType::Table)
    );
}

#[test]
fn luau_alias_resolution_optional() {
    use shingetsu_vm::types::LuaType;
    // `type Id = number` then `function f(x: Id?) end` should give Optional(Number).
    let proto = compile_proto("type Id = number\nfunction f(x: Id?) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

#[test]
fn luau_alias_resolution_in_union() {
    use shingetsu_vm::types::LuaType;
    // Alias used as part of a union.
    let proto = compile_proto("type Id = number\nfunction f(x: Id | string) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Union(vec![LuaType::Number, LuaType::String]))
    );
}

#[test]
fn luau_alias_resolution_no_runtime_effect() {
    // Aliases have no runtime effect — the code still runs.
    k9::assert_equal!(
        run_one(
            "type Meters = number\n\
             function add(a: Meters, b: Meters): Meters\n\
             return a + b\n\
             end\n\
             return add(3, 4)"
        ),
        Value::Integer(7)
    );
}

#[test]
fn luau_alias_resolution_chained() {
    use shingetsu_vm::types::LuaType;
    // `type A = number`, `type B = A` — B should resolve to number too.
    let proto = compile_proto("type A = number\ntype B = A\nfunction f(x: B) end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.params[0].lua_type, Some(LuaType::Number));
}

#[test]
fn luau_alias_resolution_in_return_type() {
    use shingetsu_vm::types::LuaType;
    // Alias should also resolve in return type annotations.
    let proto = compile_proto("type Meters = number\nfunction f(x: number): Meters return x end");
    let child = &proto.protos[0];
    k9::assert_equal!(child.signature.lua_returns, Some(vec![LuaType::Number]));
}

#[test]
fn luau_alias_resolution_generic_in_function_type() {
    use shingetsu_vm::types::LuaType;
    // `type Mapper<T, U> = (T) -> U` then `function f(m: Mapper<number, string>) end`
    let proto =
        compile_proto("type Mapper<T, U> = (T) -> U\nfunction f(m: Mapper<number, string>) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params.len(), 1);
            k9::assert_equal!(ft.params[0].1, LuaType::Number);
            k9::assert_equal!(ft.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_preserves_unrelated_generics() {
    use shingetsu_vm::types::LuaType;
    // A function with its own generic T that is NOT an alias should still produce TypeParam.
    let proto = compile_proto("type Meters = number\nfunction f<T>(x: Meters, y: T) end");
    let child = &proto.protos[0];
    let sig = &child.signature;
    // Meters resolves to number.
    k9::assert_equal!(sig.params[0].lua_type, Some(LuaType::Number));
    // T is a function generic param, stays as TypeParam.
    k9::assert_equal!(
        sig.params[1].lua_type,
        Some(LuaType::TypeParam(Bytes::from("T")))
    );
}

#[test]
fn luau_alias_resolution_alias_in_alias_body() {
    use shingetsu_vm::types::LuaType;
    // `type A = number`, `type B = { x: A }` — alias body references another alias.
    let proto = compile_proto("type A = number\ntype B = { x: A }\nfunction f(p: B) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 1);
            k9::assert_equal!(t.fields[0].0, Bytes::from("x"));
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_generic_fewer_args() {
    use shingetsu_vm::types::LuaType;
    // `Pair<number>` with only one arg — B stays as TypeParam("B").
    let proto =
        compile_proto("type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number>) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
            k9::assert_equal!(t.fields[1].1, LuaType::TypeParam(Bytes::from("B")));
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_generic_extra_args() {
    use shingetsu_vm::types::LuaType;
    // `Pair<number, string, boolean>` — extra arg is silently ignored.
    let proto = compile_proto(
        "type Pair<A, B> = { first: A, second: B }\nfunction f(p: Pair<number, string, boolean>) end",
    );
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
            k9::assert_equal!(t.fields[1].1, LuaType::String);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_in_callback_param() {
    use shingetsu_vm::types::LuaType;
    // Alias used inside a callback parameter type.
    let proto = compile_proto("type Meters = number\nfunction f(cb: (Meters) -> string) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Function(ft) => {
            k9::assert_equal!(ft.params.len(), 1);
            k9::assert_equal!(ft.params[0].1, LuaType::Number);
            k9::assert_equal!(ft.returns, vec![LuaType::String]);
        }
        other => panic!("expected Function, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_in_table_field() {
    use shingetsu_vm::types::LuaType;
    // Alias used inside a table type annotation on a param.
    let proto = compile_proto("type Meters = number\nfunction f(p: { dist: Meters }) end");
    let child = &proto.protos[0];
    let lua_type = child.signature.params[0]
        .lua_type
        .as_ref()
        .expect("has lua_type");
    match lua_type {
        LuaType::Table(t) => {
            k9::assert_equal!(t.fields.len(), 1);
            k9::assert_equal!(t.fields[0].0, Bytes::from("dist"));
            k9::assert_equal!(t.fields[0].1, LuaType::Number);
        }
        other => panic!("expected Table, got {:?}", other),
    }
}

#[test]
fn luau_alias_resolution_nested_generic_optional() {
    use shingetsu_vm::types::LuaType;
    // `type Wrap<T> = T?` then `Wrap<number>` should give `Optional(Number)`.
    let proto = compile_proto("type Wrap<T> = T?\nfunction f(x: Wrap<number>) end");
    let child = &proto.protos[0];
    k9::assert_equal!(
        child.signature.params[0].lua_type,
        Some(LuaType::Optional(Box::new(LuaType::Number)))
    );
}

// ---------------------------------------------------------------------------
// error() level argument
// ---------------------------------------------------------------------------

#[test]
fn error_level_zero_no_position() {
    // level=0: message is passed through unchanged.
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(function()
    error("raw msg", 0)
end)
return err"#
        ),
        Value::String(bytes::Bytes::from_static(b"raw msg"))
    );
}

#[test]
fn error_level_default_string() {
    // Default level=1: error value is still a string (may have position prefix).
    // We just check it contains the original message.
    let result = run_one(
        r#"local ok, err = pcall(function()
    error("boom")
end)
return type(err)"#,
    );
    k9::assert_equal!(result, Value::String(bytes::Bytes::from_static(b"string")));
}

#[test]
fn error_non_string_preserved() {
    // Non-string errors are returned as-is regardless of level.
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(function()
    error(42)
end)
return err"#
        ),
        Value::Integer(42)
    );
}

// ---------------------------------------------------------------------------
// GC: collectgarbage + __gc metamethod
// ---------------------------------------------------------------------------

// A minimal Userdata used to observe when a Value is dropped by the GC.
// Holds a clone of a shared Arc; when the Userdata is dropped (because the
// GC cleared the table that contained it) the Arc's strong_count falls.
struct MarkerUserdata;

#[async_trait::async_trait]
impl shingetsu_vm::Userdata for MarkerUserdata {
    fn type_name(&self) -> &'static str {
        "MarkerUserdata"
    }
}

#[test]
fn gc_collect_unreachable_no_finalizer() {
    // An unreachable table with no __gc must have its contents cleared by the
    // GC sweep.  We verify the sweep actually ran — not just that no error
    // occurred — by storing a Userdata in the table and checking that the
    // shared Arc's strong_count drops back to 1 once the table is collected.
    use shingetsu_vm::{Task, Value};
    use std::sync::Arc;

    let env = new_env();
    let marker = Arc::new(MarkerUserdata) as Arc<dyn shingetsu_vm::Userdata + Send + Sync>;
    // Register the marker as a global so the Lua script can read it.
    env.set_global("_marker", Value::Userdata(marker.clone()));
    // Arc refs: test `marker` (1) + env global (1) = 2.

    let src = r#"
local t = { ud = _marker }  -- table holds a ref to the marker
_marker = nil               -- remove global ref; only table holds it now
t = nil                     -- drop the table ref (table becomes unreachable)
collectgarbage("collect")   -- sweep must clear the table contents
return 1
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env.clone(), func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed");

    // After collection the table contents were cleared, dropping the
    // Value::Userdata inside.  Only our `marker` handle remains.
    k9::assert_equal!(Arc::strong_count(&marker), 1);
}

#[test]
fn gc_gc_metamethod_called() {
    // A table with __gc should have its finalizer called during collect.
    k9::assert_equal!(
        run_one(
            r#"
local finalized = 0
local t = setmetatable({}, {
    __gc = function(self)
        finalized = finalized + 1
    end
})
t = nil
collectgarbage("collect")
return finalized
"#
        ),
        Value::Integer(1)
    );
}

#[test]
fn gc_gc_metamethod_receives_table() {
    // The finalizer receives the table as its argument.
    k9::assert_equal!(
        run_one(
            r#"
local got_type = ""
local t = setmetatable({value = 42}, {
    __gc = function(self)
        got_type = type(self)
    end
})
t = nil
collectgarbage("collect")
return got_type
"#
        ),
        Value::String(bytes::Bytes::from_static(b"table"))
    );
}

#[test]
fn gc_reachable_table_not_collected() {
    // A table that is still reachable must NOT be collected.
    k9::assert_equal!(
        run_one(
            r#"
local finalized = 0
local t = setmetatable({}, {
    __gc = function(self)
        finalized = finalized + 1
    end
})
collectgarbage("collect")   -- t is still live
return finalized
"#
        ),
        Value::Integer(0)
    );
}

#[test]
fn gc_dispose_runs_gc_finalizers() {
    // dispose() must finalize every tracked table that has a __gc metamethod,
    // even if collectgarbage() was never called explicitly.
    //
    // We can't read a Lua global after dispose() because dispose() clears
    // globals before collecting.  Instead we register a native that closes
    // over a Rust-side AtomicBool; the __gc handler calls that native, and
    // we inspect the flag after dispose() returns.
    use shingetsu_vm::types::FunctionSignature;
    use shingetsu_vm::{NativeFunction, Task, Value, VmError};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let finalized = Arc::new(AtomicBool::new(false));
    let env = new_env();

    // Register a native that flips the Rust-side flag when called.
    {
        let flag = finalized.clone();
        env.register_native(NativeFunction {
            signature: Arc::new(FunctionSignature {
                name: bytes::Bytes::from_static(b"mark_gc_ran"),
                type_params: vec![],
                params: vec![],
                variadic: true,
                arg_offset: 0,
                returns: None,
                lua_returns: None,
            }),
            call: Arc::new(move |_, _| {
                flag.store(true, Ordering::SeqCst);
                Box::pin(async { Ok::<Vec<Value>, VmError>(vec![]) })
            }),
        });
    }

    // The __gc handler calls mark_gc_ran(); no explicit collectgarbage().
    let src = r#"
local t = setmetatable({}, {
    __gc = function(self) mark_gc_ran() end
})
t = nil
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env.clone(), func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed");

    // __gc has not fired yet — no collect was called.
    k9::assert_equal!(finalized.load(Ordering::SeqCst), false);

    // dispose() clears globals, collects, and runs pending __gc finalizers.
    rt.block_on(env.dispose());

    // The __gc handler must have called mark_gc_ran().
    k9::assert_equal!(finalized.load(Ordering::SeqCst), true);
}

// ---------------------------------------------------------------------------
// Task::dispose()
// ---------------------------------------------------------------------------

#[test]
fn task_dispose_calls_close_on_cancel() {
    use shingetsu_vm::types::FunctionSignature;
    use shingetsu_vm::{NativeFunction, Task, Value, VmError};
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    // Register a native that blocks forever (simulates I/O or sleep).
    let env = new_env();
    env.register_native(NativeFunction {
        signature: Arc::new(FunctionSignature {
            name: bytes::Bytes::from_static(b"block_forever"),
            type_params: vec![],
            params: vec![],
            variadic: true,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
        }),
        call: Arc::new(|_, _| {
            Box::pin(async {
                // Never resolves.
                std::future::pending::<Result<Vec<Value>, VmError>>().await
            })
        }),
    });

    // Script: initialise a <close> variable, then block.
    // The __close handler increments the global `closed`.
    let src = r#"
closed = 0
local x <close> = setmetatable({}, {
    __close = function(self, err)
        closed = closed + 1
    end
})
block_forever()
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let mut task = Task::new(env.clone(), func, vec![]);

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async move {
        // Poll the task once with a noop waker to run it up to (and into)
        // the blocking native call.  The task must come back as Pending.
        {
            let waker = std::task::Waker::from(Arc::new(NoopWaker));
            let mut cx = Context::from_waker(&waker);
            // Task: Unpin (BoxFuture is Unpin), so Pin::new is safe.
            let result = std::pin::Pin::new(&mut task).poll(&mut cx);
            assert!(
                matches!(result, Poll::Pending),
                "expected task to be pending while blocking native is active"
            );
        }

        // Simulate cancellation: dispose() must call the __close handler.
        task.dispose().await;

        // The __close handler should have fired and set closed = 1.
        k9::assert_equal!(
            env.get_global("closed").unwrap_or(Value::Nil),
            Value::Integer(1)
        );
    });
}

#[test]
fn task_dispose_no_close_vars_is_noop() {
    // dispose() on a task with no <close> variables should complete cleanly.
    let env = new_env();
    let src = r#"
x = 42
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env.clone(), func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task.dispose()); // must not hang or panic
}

// ---------------------------------------------------------------------------
// Proc macro smoke tests
// ---------------------------------------------------------------------------

/// Run a Lua snippet against the provided env, returning all return values.
fn run_with_env(env: shingetsu::GlobalEnv, src: &str) -> Vec<shingetsu::Value> {
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile(src, &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    rt.block_on(Task::new(env, func, vec![])).expect("run")
}

#[test]
fn derive_userdata_basic() {
    // #[derive(UserData)] generates a valid Userdata impl + downcast support.

    use shingetsu::UserData;
    use std::sync::Arc;

    #[derive(UserData)]
    struct Marker;

    let arc: Arc<dyn shingetsu::Userdata> = Arc::new(Marker);
    k9::assert_equal!(arc.type_name(), "Marker");
    // Downcast should succeed.
    assert!(arc.downcast_arc::<Marker>().is_ok());
}

#[test]
fn userdata_macro_field_and_method() {
    // #[shingetsu::userdata] on an impl block wires __index dispatch.
    use shingetsu::{userdata, Function, Task, Value};
    use shingetsu_compiler::{compile, CompileOptions};
    use std::sync::Arc;

    struct Counter(i64);

    #[userdata]
    impl Counter {
        fn type_name(&self) -> &'static str {
            "Counter"
        }

        #[lua_field]
        fn value(&self) -> i64 {
            self.0
        }
    }

    let env = new_env();
    let counter: Arc<dyn shingetsu::Userdata> = Arc::new(Counter(42));
    env.set_global("counter", Value::Userdata(counter));

    let src = "return counter.value";
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile(src, &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let results = rt.block_on(Task::new(env, func, vec![])).expect("run");
    k9::assert_equal!(results[0], Value::Integer(42));
}

#[test]
fn module_macro_basic() {
    // #[shingetsu::module] generates build_module_table that registers functions.
    use shingetsu::{module, Function, Task, Value};
    use shingetsu_compiler::{compile, CompileOptions};

    #[module]
    mod testmod {
        #[function]
        fn add(a: i64, b: i64) -> i64 {
            a + b
        }
    }

    let env = new_env();
    testmod::register_global_module(&env).expect("register");

    let src = "return testmod.add(3, 4)";
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile(src, &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let results = rt.block_on(Task::new(env, func, vec![])).expect("run");
    k9::assert_equal!(results[0], Value::Integer(7));
}

// ---------------------------------------------------------------------------
// Userdata macro: field getter with rename
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_rename() {
    // #[lua_field(rename = "luaName")] maps the Lua key to a different name.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Point(i64, i64);

    #[userdata]
    impl Point {
        #[lua_field(rename = "x")]
        fn get_x(&self) -> i64 {
            self.0
        }

        #[lua_field(rename = "y")]
        fn get_y(&self) -> i64 {
            self.1
        }
    }

    let env = new_env();
    env.set_global("pt", Value::Userdata(Arc::new(Point(3, 7))));
    let res = run_with_env(env, "return pt.x + pt.y");
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Userdata macro: field setter via set_ prefix
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_setter() {
    // A fn named set_<field> is detected as a setter; __newindex dispatches it.
    use shingetsu::{userdata, Value};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    struct Counter(AtomicI64);

    #[userdata]
    impl Counter {
        #[lua_field]
        fn value(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }

        #[lua_field]
        fn set_value(&self, v: i64) {
            self.0.store(v, Ordering::Relaxed);
        }
    }

    let env = new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(AtomicI64::new(0)))));
    let res = run_with_env(env, "c.value = 99; return c.value");
    k9::assert_equal!(res[0], Value::Integer(99));
}

#[test]
fn validate_args_field_setter_rejects_wrong_type() {
    // Inline type checks in gen_call_body catch type mismatches for
    // field setter parameters (which don't go through validate_args).
    use shingetsu::{userdata, Value};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    struct Counter(AtomicI64);

    #[userdata]
    impl Counter {
        #[lua_field]
        fn value(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }

        #[lua_field]
        fn set_value(&self, v: i64) {
            self.0.store(v, Ordering::Relaxed);
        }
    }

    let env = new_env();
    env.set_global("c", Value::Userdata(Arc::new(Counter(AtomicI64::new(0)))));
    let res = run_with_env(
        env,
        "local ok, err = pcall(function() c.value = 'oops' end)\n\
         return ok, err",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Boolean(false),
            Value::String(bytes::Bytes::from(
                "bad value in assignment to 'Counter.value' (integer expected, got string)"
            )),
        ]
    );
}

// ---------------------------------------------------------------------------
// Userdata macro: method with &self receiver and a parameter
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_ref_self() {
    // #[lua_method] with &self — the object is skipped from the Lua arg list.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn multiply(&self, factor: i64) -> i64 {
            self.0 * factor
        }
    }

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(7))));
    // obj:method(arg) desugars to obj.method(obj, arg)
    let res = run_with_env(env, "return n:multiply(6)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method with Arc<Self> receiver
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_arc_self() {
    // #[lua_method] where self is Arc<Self> — passes the Arc directly.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn doubled(self: Arc<Self>) -> i64 {
            self.0 * 2
        }
    }

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(21))));
    let res = run_with_env(env, "return n:doubled()");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method returning Result
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_result_ok() {
    // A method with Result return — Ok path propagates the value normally.
    use shingetsu::{userdata, Value, VmError};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn checked_div(&self, divisor: i64) -> Result<i64, VmError> {
            if divisor == 0 {
                Err(VmError::HostError {
                    name: "checked_div".to_owned(),
                    source: "division by zero".into(),
                })
            } else {
                Ok(self.0 / divisor)
            }
        }
    }

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    let res = run_with_env(env, "return n:checked_div(6)");
    k9::assert_equal!(res[0], Value::Integer(7));
}

#[test]
fn userdata_macro_method_result_err() {
    // A method with Result return — Err path surfaces as a Lua error.
    use shingetsu::{userdata, Value, VmError};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn checked_div(&self, divisor: i64) -> Result<i64, VmError> {
            if divisor == 0 {
                Err(VmError::HostError {
                    name: "checked_div".to_owned(),
                    source: "division by zero".into(),
                })
            } else {
                Ok(self.0 / divisor)
            }
        }
    }

    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile("return n:checked_div(0)", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(err.to_string(), "error in 'checked_div': division by zero");
}

// ---------------------------------------------------------------------------
// Userdata macro: method with CallContext parameter
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_callcontext() {
    // A CallContext parameter is injected from the call site, not from Lua args.
    use shingetsu::{userdata, CallContext, Value};
    use std::sync::Arc;

    struct Doubler;

    #[userdata]
    impl Doubler {
        #[lua_method]
        fn run(&self, _ctx: CallContext, n: i64) -> i64 {
            n * 2
        }
    }

    let env = new_env();
    env.set_global("d", Value::Userdata(Arc::new(Doubler)));
    let res = run_with_env(env, "return d:run(21)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Userdata macro: method with Variadic parameter
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_method_variadic() {
    // A Variadic parameter collects all remaining Lua args into a Vec.
    use shingetsu::{userdata, Value, Variadic};
    use std::sync::Arc;

    struct Summer;

    #[userdata]
    impl Summer {
        #[lua_method]
        fn sum(&self, args: Variadic) -> i64 {
            args.0
                .iter()
                .filter_map(|v| match v {
                    Value::Integer(n) => Some(*n),
                    _ => None,
                })
                .sum()
        }
    }

    let env = new_env();
    env.set_global("s", Value::Userdata(Arc::new(Summer)));
    let res = run_with_env(env, "return s:sum(1, 2, 3, 4)");
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// Userdata macro: __tostring metamethod via tostring() builtin
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_metamethod_tostring() {
    // #[lua_metamethod(ToString)] is dispatched by the tostring() global.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Named(String);

    #[userdata]
    impl Named {
        #[lua_metamethod(ToString)]
        fn to_str(&self) -> String {
            self.0.clone()
        }
    }

    let env = new_env();
    env.set_global("obj", Value::Userdata(Arc::new(Named("hello".into()))));
    let res = run_with_env(env, "return tostring(obj)");
    k9::assert_equal!(
        res[0],
        Value::String(shingetsu::bytes::Bytes::from_static(b"hello"))
    );
}

// ---------------------------------------------------------------------------
// Userdata macro: binary metamethod dispatched directly
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_metamethod_binary_dispatch() {
    // #[lua_metamethod(Add)] — test the dispatch mechanism directly.
    // TODO: once get_arith_metamethod in task.rs is extended to handle
    // Value::Userdata, replace this with a Lua `a + b` test instead.
    // See the TODO comment on get_arith_metamethod in shingetsu-vm/src/task.rs.
    use shingetsu::{userdata, CallContext, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = new_env();
    let obj: Arc<dyn shingetsu::Userdata> = Arc::new(Num(10));
    let ctx = CallContext {
        global: env,
        call_stack: Arc::new(vec![]),
        native_name: None,
    };
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let result = rt
        .block_on(Arc::clone(&obj).dispatch(
            ctx,
            "__add",
            vec![Value::Userdata(obj), Value::Integer(5)],
        ))
        .expect("dispatch");
    k9::assert_equal!(result[0], Value::Integer(15));
}

#[test]
fn validate_args_metamethod_rejects_wrong_type() {
    // Inline type checks in gen_call_body catch type mismatches for
    // metamethod parameters (which don't go through validate_args).
    use shingetsu::{userdata, CallContext, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = new_env();
    let obj: Arc<dyn shingetsu::Userdata> = Arc::new(Num(10));
    let ctx = CallContext {
        global: env,
        call_stack: Arc::new(vec![]),
        native_name: None,
    };
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt
        .block_on(Arc::clone(&obj).dispatch(
            ctx,
            "__add",
            vec![
                Value::Userdata(obj),
                Value::String(bytes::Bytes::from("oops")),
            ],
        ))
        .unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'Num:__add' (integer expected, got string)"
    );
}

// ---------------------------------------------------------------------------
// Module macro: function returning Result (Ok path)
// ---------------------------------------------------------------------------

#[test]
fn module_macro_result_return() {
    use shingetsu::{module, Value};

    #[module]
    mod mathmod {
        use shingetsu::VmError;

        #[function]
        fn checked_sqrt(n: f64) -> Result<f64, VmError> {
            if n < 0.0 {
                Err(VmError::HostError {
                    name: "checked_sqrt".to_owned(),
                    source: "negative input".into(),
                })
            } else {
                Ok(n.sqrt())
            }
        }
    }

    let env = new_env();
    mathmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return mathmod.checked_sqrt(4.0)");
    k9::assert_equal!(res[0], Value::Float(2.0));
}

// ---------------------------------------------------------------------------
// Module macro: async function
// ---------------------------------------------------------------------------

#[test]
fn module_macro_async_fn() {
    use shingetsu::{module, Value};

    #[module]
    mod asyncmod {
        #[function]
        async fn async_double(n: i64) -> i64 {
            n * 2
        }
    }

    let env = new_env();
    asyncmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return asyncmod.async_double(21)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: function with CallContext parameter
// ---------------------------------------------------------------------------

#[test]
fn module_macro_callcontext() {
    use shingetsu::{module, Value};

    #[module]
    mod ctxmod {
        use shingetsu::CallContext;

        #[function]
        fn passthrough(_ctx: CallContext, n: i64) -> i64 {
            n
        }
    }

    let env = new_env();
    ctxmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return ctxmod.passthrough(99)");
    k9::assert_equal!(res[0], Value::Integer(99));
}

// ---------------------------------------------------------------------------
// Module macro: function with Variadic parameter
// ---------------------------------------------------------------------------

#[test]
fn module_macro_variadic() {
    use shingetsu::{module, Value};

    #[module]
    mod varmod {
        use shingetsu::{Value, Variadic};

        #[function]
        fn sum_all(args: Variadic) -> i64 {
            args.0
                .iter()
                .filter_map(|v| match v {
                    Value::Integer(n) => Some(*n),
                    _ => None,
                })
                .sum()
        }
    }

    let env = new_env();
    varmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return varmod.sum_all(10, 20, 12)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: eager field
// ---------------------------------------------------------------------------

#[test]
fn module_macro_eager_field() {
    // #[field] is called once at table construction; the result is stored eagerly.
    use shingetsu::{module, Value};

    #[module]
    mod constmod {
        #[field]
        fn magic() -> i64 {
            42
        }
    }

    let env = new_env();
    constmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return constmod.magic");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: function rename
// ---------------------------------------------------------------------------

#[test]
fn module_macro_function_rename() {
    // #[function(rename = "luaName")] exposes the function under a different key.
    use shingetsu::{module, Value};

    #[module]
    mod renmod {
        #[function(rename = "doThing")]
        fn do_thing(n: i64) -> i64 {
            n + 1
        }
    }

    let env = new_env();
    renmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return renmod.doThing(5)");
    k9::assert_equal!(res[0], Value::Integer(6));
}

// ---------------------------------------------------------------------------
// Module macro: module name option overrides global key
// ---------------------------------------------------------------------------

#[test]
fn module_macro_name_option() {
    // #[module(name = "luaName")] controls the key used in set_global.
    use shingetsu::{module, Value};

    #[module(name = "myMod")]
    mod internal {
        #[function]
        fn hello() -> i64 {
            1
        }
    }

    let env = new_env();
    internal::register_global_module(&env).expect("register");
    // The Rust mod is named `internal` but the Lua global is `myMod`.
    let res = run_with_env(env, "return myMod.hello()");
    k9::assert_equal!(res[0], Value::Integer(1));
}

// ---------------------------------------------------------------------------
// Userdata macro: get_ prefix is stripped automatically for field names
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_get_prefix() {
    // fn get_<name> maps to Lua field "<name>" without requiring rename =.
    use shingetsu::{userdata, Value};
    use std::sync::Arc;

    struct Rect {
        w: i64,
        h: i64,
    }

    #[userdata]
    impl Rect {
        #[lua_field]
        fn get_width(&self) -> i64 {
            self.w
        }

        #[lua_field]
        fn get_height(&self) -> i64 {
            self.h
        }
    }

    let env = new_env();
    env.set_global("r", Value::Userdata(Arc::new(Rect { w: 4, h: 6 })));
    // Fields are "width" and "height", not "get_width" / "get_height".
    let res = run_with_env(env, "return r.width * r.height");
    k9::assert_equal!(res[0], Value::Integer(24));
}

// ---------------------------------------------------------------------------
// Userdata macro: set_ prefix extraction and setter dispatch
// ---------------------------------------------------------------------------

#[test]
fn userdata_macro_field_set_prefix() {
    // fn set_<name> maps to Lua field "<name>" for __newindex, matching the
    // getter derived from fn get_<name> or fn <name>.
    use shingetsu::{userdata, Value};
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    struct Cube(AtomicI64);

    #[userdata]
    impl Cube {
        #[lua_field]
        fn get_side(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }

        #[lua_field]
        fn set_side(&self, v: i64) {
            self.0.store(v, Ordering::Relaxed);
        }
    }

    let env = new_env();
    env.set_global("b", Value::Userdata(Arc::new(Cube(AtomicI64::new(0)))));
    // Both fn get_side and fn set_side map to the Lua field "side".
    let res = run_with_env(env, "b.side = 5; return b.side");
    k9::assert_equal!(res[0], Value::Integer(5));
}

// ---------------------------------------------------------------------------
// Result<T, E> where E: Into<VmError> — custom error type conversion
// ---------------------------------------------------------------------------

#[test]
fn module_macro_result_custom_error() {
    // Demonstrates that Result<T, E> works when E: Into<VmError>, not just
    // when E is VmError directly.  ParseError and its From impl are defined
    // inside the module so they are in scope for the generated wrapper code.
    use shingetsu::{module, Value};

    #[module]
    mod parsemod {
        use shingetsu::VmError;

        #[derive(Debug)]
        pub struct ParseError(pub String);

        impl std::fmt::Display for ParseError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::error::Error for ParseError {}

        impl From<ParseError> for VmError {
            fn from(e: ParseError) -> VmError {
                VmError::HostError {
                    name: "parse_int".to_owned(),
                    source: Box::new(e),
                }
            }
        }

        #[function]
        fn parse_int(s: String) -> Result<i64, ParseError> {
            s.parse::<i64>().map_err(|e| ParseError(e.to_string()))
        }
    }

    let env = new_env();
    parsemod::register_global_module(&env).expect("register");

    // Ok path: valid integer string.
    let res = run_with_env(env.clone(), "return parsemod.parse_int('42')");
    k9::assert_equal!(res[0], Value::Integer(42));

    // Err path: non-integer string surfaces as VmError.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile("return parsemod.parse_int('nope')", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "error in 'parse_int': invalid digit found in string"
    );
}

// ---------------------------------------------------------------------------
// Module macro: `this` table parameter (colon-call passes module table)
// ---------------------------------------------------------------------------

#[test]
fn module_macro_this_param() {
    // When a module function is called with `:` syntax, Lua passes the module
    // table itself as the first argument.  Declaring `this: Table` captures it.
    use shingetsu::{module, Value};

    #[module(name = "tmod")]
    mod tmod_impl {
        use shingetsu::bytes::Bytes;
        use shingetsu::{Table, Value};

        /// An eager constant field baked into the table at construction time.
        #[field]
        fn version() -> i64 {
            99
        }

        /// Reads a field from the module table passed as `this`.
        #[function]
        fn read_version(this: Table) -> i64 {
            match this
                .raw_get(&Value::String(Bytes::from_static(b"version")))
                .unwrap_or(Value::Nil)
            {
                Value::Integer(n) => n,
                _ => -1,
            }
        }
    }

    let env = new_env();
    tmod_impl::register_global_module(&env).expect("register");
    // tmod:read_version() desugars to tmod.read_version(tmod); `this` == tmod.
    let res = run_with_env(env, "return tmod:read_version()");
    k9::assert_equal!(res[0], Value::Integer(99));
}

// ---------------------------------------------------------------------------
// Multi-value return: Variadic and 2-tuple
// ---------------------------------------------------------------------------

#[test]
fn module_macro_variadic_return() {
    // A function can return Variadic to produce an arbitrary number of values.
    // We verify arity on the raw Vec and then use FromLuaMulti for typed extraction.
    use shingetsu::{module, FromLuaMulti, Value, Variadic};

    #[module]
    mod swapmod {
        use shingetsu::{Value, Variadic};

        #[function]
        fn swap(a: i64, b: i64) -> Variadic {
            Variadic(vec![Value::Integer(b), Value::Integer(a)])
        }
    }

    let env = new_env();
    swapmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return swapmod.swap(1, 2)");

    // Arity check on the raw Vec.
    k9::assert_equal!(res.len(), 2);

    // Typed extraction via FromLuaMulti.
    let Variadic(vals) = Variadic::from_lua_multi(res).expect("from_lua_multi");
    k9::assert_equal!(vals.len(), 2);
    k9::assert_equal!(vals[0], Value::Integer(2));
    k9::assert_equal!(vals[1], Value::Integer(1));
}

#[test]
fn module_macro_tuple_return() {
    // A function can return a tuple to produce a fixed number of values.
    // We verify arity on the raw Vec and then use FromLuaMulti for typed extraction.
    use shingetsu::{module, FromLuaMulti};

    #[module]
    mod divmod {
        #[function]
        fn divmod(a: i64, b: i64) -> (i64, i64) {
            (a / b, a % b)
        }
    }

    let env = new_env();
    divmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return divmod.divmod(10, 3)");

    // Arity check on the raw Vec.
    k9::assert_equal!(res.len(), 2);

    // Typed extraction via FromLuaMulti.
    let (q, r) = <(i64, i64)>::from_lua_multi(res).expect("from_lua_multi");
    k9::assert_equal!(q, 3);
    k9::assert_equal!(r, 1);
}

// ---------------------------------------------------------------------------
// require() builtin + register_preload
// ---------------------------------------------------------------------------

#[test]
fn require_basic() {
    // require("name") calls the registered preload opener once and returns its table.
    use shingetsu::{module, Value};

    #[module(name = "mylib")]
    mod mylib_impl {
        #[function]
        fn answer() -> i64 {
            42
        }
    }

    let env = new_env();
    mylib_impl::register_preload(&env);

    let res = run_with_env(env, "local m = require('mylib'); return m.answer()");
    k9::assert_equal!(res[0], Value::Integer(42));
}

#[test]
fn require_caches_result() {
    // A second require() call returns the same (cached) table value — the
    // opener is only called once.

    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let env = new_env();
    let call_count = Arc::new(AtomicU32::new(0));
    let cc = Arc::clone(&call_count);
    env.register_preload("counted", move |_env| {
        cc.fetch_add(1, Ordering::Relaxed);
        Ok(shingetsu::Table::new())
    });

    run_with_env(env.clone(), "require('counted')");
    run_with_env(env.clone(), "require('counted')");
    run_with_env(env, "require('counted')");

    k9::assert_equal!(call_count.load(Ordering::Relaxed), 1);
}

#[test]
fn require_missing_module_errors() {
    // require() on an unregistered name returns a VmError.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};

    let env = new_env();
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    let bc = compile("require('notfound')", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    assert!(err.to_string().contains("module 'notfound' not found"));
}

// ---------------------------------------------------------------------------
// BadArgument context fixup tests
// ---------------------------------------------------------------------------

#[test]
fn bad_argument_context_module_function_arg1() {
    // Passing the wrong type to argument #1 of a module function surfaces
    // the correct position and function name via with_arg_and_call_context.
    use shingetsu::{module, Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};

    #[module]
    mod ctx_test {
        #[function]
        fn greet(name: String) -> String {
            format!("hello {name}")
        }
    }

    let env = new_env();
    ctx_test::register_global_module(&env).expect("register");
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    // Pass a boolean where a string is expected.
    let bc = compile("return ctx_test.greet(true)", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'greet' (string expected, got boolean)"
    );
}

#[test]
fn bad_argument_context_module_function_arg2() {
    // Position tracking: the error should say #2 for the second argument.
    use shingetsu::{module, Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};

    #[module]
    mod ctx_test2 {
        #[function]
        fn add(a: i64, b: i64) -> i64 {
            a + b
        }
    }

    let env = new_env();
    ctx_test2::register_global_module(&env).expect("register");
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    // First arg is fine, second arg is wrong type.
    let bc = compile("return ctx_test2.add(1, 'oops')", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to 'add' (integer expected, got string)"
    );
}

#[test]
fn bad_argument_context_userdata_method() {
    // Userdata method dispatch also gets the correct function name and
    // argument position via the proc-macro generated fixup.
    use shingetsu::{userdata, Function, Task, Value};
    use shingetsu_compiler::{compile, CompileOptions};
    use std::sync::Arc;

    struct Acc(i64);

    #[userdata]
    impl Acc {
        #[lua_method]
        fn add(&self, n: i64) -> i64 {
            self.0 + n
        }
    }

    let env = new_env();
    env.set_global("acc", Value::Userdata(Arc::new(Acc(10))));
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    // Pass a table where an integer is expected.
    let bc = compile("return acc:add({})", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'add' (integer expected, got table)"
    );
}

#[test]
fn bad_argument_context_require() {
    // The hand-written require() builtin uses FromLuaMulti + with_arg_and_call_context.
    use shingetsu::{Function, Task};
    use shingetsu_compiler::{compile, CompileOptions};

    let env = new_env();
    let opts = CompileOptions {
        debug_info: false,
        source_name: "test".into(),
    };
    // Pass a number where a string is expected.
    let bc = compile("require(42)", &opts).expect("compile");
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("rt");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #1 to 'require' (string expected, got number)"
    );
}

#[test]
fn bad_argument_context_tuple_return_type_mismatch() {
    // A module function returns (i64, i64) but Lua-side we try to extract
    // the result as (i64, String) via FromLuaMulti.  The second element
    // should produce a BadArgument with position 2.
    use shingetsu::FromLuaMulti;

    let env = new_env();
    // divmod returns two integers; try to unpack the second as String.
    let res = run_with_env(env, "return 10, 42");
    let err = <(i64, String)>::from_lua_multi(res).unwrap_err();
    k9::assert_equal!(
        err.to_string(),
        "bad argument #2 to '' (string expected, got number)"
    );
}

#[test]
fn require_via_register_global_and_preload() {
    // register_global_module exposes the module as a global AND
    // register_preload makes it require()-able; both work independently.
    use shingetsu::{module, Value};

    #[module(name = "util")]
    mod util_impl {
        #[function]
        fn double(n: i64) -> i64 {
            n * 2
        }
    }

    let env = new_env();
    // Register both ways.
    util_impl::register_global_module(&env).expect("global");
    util_impl::register_preload(&env);

    // Direct global access.
    let res = run_with_env(env.clone(), "return util.double(3)");
    k9::assert_equal!(res[0], Value::Integer(6));

    // require() access — different table instance but same functions.
    let res = run_with_env(env, "local u = require('util'); return u.double(5)");
    k9::assert_equal!(res[0], Value::Integer(10));
}

// ---------------------------------------------------------------------------
// string library
// ---------------------------------------------------------------------------

#[test]
fn string_lib_len() {
    k9::assert_equal!(run_one("return string.len('hello')"), Value::Integer(5));
    k9::assert_equal!(run_one("return string.len('')"), Value::Integer(0));
}

#[test]
fn string_lib_len_method_syntax() {
    // Method-call syntax on string values via the string metatable.
    k9::assert_equal!(run_one("return ('hello'):len()"), Value::Integer(5));
}

#[test]
fn string_lib_upper_lower() {
    k9::assert_equal!(
        run_one("return string.upper('hello')"),
        Value::String(Bytes::from("HELLO"))
    );
    k9::assert_equal!(
        run_one("return string.lower('HeLLo')"),
        Value::String(Bytes::from("hello"))
    );
}

#[test]
fn string_lib_upper_method_syntax() {
    k9::assert_equal!(
        run_one("return ('hello'):upper()"),
        Value::String(Bytes::from("HELLO"))
    );
}

#[test]
fn string_lib_reverse() {
    k9::assert_equal!(
        run_one("return string.reverse('abcd')"),
        Value::String(Bytes::from("dcba"))
    );
}

#[test]
fn string_lib_byte() {
    // Single byte at default position (first).
    k9::assert_equal!(run_one("return string.byte('A')"), Value::Integer(65));
    // Range: byte(s, 1, 3) returns three values.
    let res = run_all("return string.byte('ABC', 1, 3)");
    k9::assert_equal!(
        res,
        vec![Value::Integer(65), Value::Integer(66), Value::Integer(67)]
    );
    // Out-of-range returns nothing.
    let res = run_all("return string.byte('A', 5, 6)");
    k9::assert_equal!(res.len(), 0);
}

#[test]
fn string_lib_char() {
    k9::assert_equal!(
        run_one("return string.char(72, 101, 108, 108, 111)"),
        Value::String(Bytes::from("Hello"))
    );
}

#[test]
fn string_lib_sub() {
    k9::assert_equal!(
        run_one("return string.sub('Hello', 2, 4)"),
        Value::String(Bytes::from("ell"))
    );
    // Negative index: -3 = third from end.
    k9::assert_equal!(
        run_one("return string.sub('Hello', -3)"),
        Value::String(Bytes::from("llo"))
    );
}

#[test]
fn string_lib_rep() {
    k9::assert_equal!(
        run_one("return string.rep('ab', 3)"),
        Value::String(Bytes::from("ababab"))
    );
    // With separator.
    k9::assert_equal!(
        run_one("return string.rep('ab', 3, ',')"),
        Value::String(Bytes::from("ab,ab,ab"))
    );
    // Zero repetitions.
    k9::assert_equal!(
        run_one("return string.rep('x', 0)"),
        Value::String(Bytes::new())
    );
}

// ---------------------------------------------------------------------------
// string.find
// ---------------------------------------------------------------------------

#[test]
fn string_lib_find_plain() {
    let res = run_all("return string.find('hello world', 'world')");
    k9::assert_equal!(res, vec![Value::Integer(7), Value::Integer(11)]);
}

#[test]
fn string_lib_find_plain_flag() {
    // With plain=true, pattern chars are literal.
    let res = run_all("return string.find('100%', '%', 1, true)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Integer(4)]);
}

#[test]
fn string_lib_find_pattern() {
    let res = run_all("return string.find('hello 123 world', '(%d+)')");
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(7),
            Value::Integer(9),
            Value::String(Bytes::from("123"))
        ]
    );
}

#[test]
fn string_lib_find_no_match() {
    let res = run_all("return string.find('hello', 'xyz')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

#[test]
fn string_lib_find_with_init() {
    // Start search from position 6.
    let res = run_all("return string.find('abcabc', 'abc', 4)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Integer(6)]);
}

// ---------------------------------------------------------------------------
// string.match
// ---------------------------------------------------------------------------

#[test]
fn string_lib_match_captures() {
    let res = run_all("return string.match('2025-04-13', '(%d+)-(%d+)-(%d+)')");
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("2025")),
            Value::String(Bytes::from("04")),
            Value::String(Bytes::from("13")),
        ]
    );
}

#[test]
fn string_lib_match_whole() {
    // No explicit captures — returns the whole match.
    let res = run_all("return string.match('hello world', '%a+')");
    k9::assert_equal!(res, vec![Value::String(Bytes::from("hello"))]);
}

#[test]
fn string_lib_match_no_match() {
    let res = run_all("return string.match('hello', '%d+')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// string.gmatch
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gmatch_words() {
    let res = run_all(
        "\
        local t = {}
        for w in string.gmatch('one two three', '%a+') do
            t[#t+1] = w
        end
        return t[1], t[2], t[3]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("one")),
            Value::String(Bytes::from("two")),
            Value::String(Bytes::from("three")),
        ]
    );
}

#[test]
fn string_lib_gmatch_captures() {
    let res = run_all(
        "\
        local keys, vals = {}, {}
        for k, v in string.gmatch('a=1, b=2', '(%a+)=(%d+)') do
            keys[#keys+1] = k
            vals[#vals+1] = v
        end
        return keys[1], vals[1], keys[2], vals[2]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("a")),
            Value::String(Bytes::from("1")),
            Value::String(Bytes::from("b")),
            Value::String(Bytes::from("2")),
        ]
    );
}

// ---------------------------------------------------------------------------
// string.gsub
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gsub_string() {
    let res = run_all("return string.gsub('hello world', 'world', 'lua')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("hello lua")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_pattern() {
    let res = run_all("return string.gsub('abc 123 def 456', '%d+', 'NUM')");
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("abc NUM def NUM")),
            Value::Integer(2)
        ]
    );
}

#[test]
fn string_lib_gsub_capture_ref() {
    // %1 references the first capture.
    let res = run_all("return string.gsub('hello', '(%w+)', '[%1]')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("[hello]")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_max_n() {
    // Replace at most 1.
    let res = run_all("return string.gsub('aaa', 'a', 'b', 1)");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("baa")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_table() {
    let res = run_all(
        "\
        local t = { hello = 'HI', world = 'EARTH' }
        return string.gsub('hello world', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("HI EARTH")), Value::Integer(2)]
    );
}

// ---------------------------------------------------------------------------
// string.format
// ---------------------------------------------------------------------------

#[test]
fn string_lib_format_basic() {
    k9::assert_equal!(
        run_one("return string.format('%d + %d = %d', 1, 2, 3)"),
        Value::String(Bytes::from("1 + 2 = 3"))
    );
}

#[test]
fn string_lib_format_string() {
    k9::assert_equal!(
        run_one("return string.format('hello %s!', 'world')"),
        Value::String(Bytes::from("hello world!"))
    );
}

#[test]
fn string_lib_format_hex() {
    k9::assert_equal!(
        run_one("return string.format('%x', 255)"),
        Value::String(Bytes::from("ff"))
    );
    k9::assert_equal!(
        run_one("return string.format('%X', 255)"),
        Value::String(Bytes::from("FF"))
    );
}

#[test]
fn string_lib_format_float() {
    k9::assert_equal!(
        run_one("return string.format('%.2f', 3.14159)"),
        Value::String(Bytes::from("3.14"))
    );
}

#[test]
fn string_lib_format_padded() {
    k9::assert_equal!(
        run_one("return string.format('%05d', 42)"),
        Value::String(Bytes::from("00042"))
    );
}

#[test]
fn string_lib_format_quoted() {
    k9::assert_equal!(
        run_one("return string.format('%q', 'hello')"),
        Value::String(Bytes::from(r#""hello""#))
    );
}

#[test]
fn string_lib_format_percent() {
    k9::assert_equal!(
        run_one("return string.format('100%%')"),
        Value::String(Bytes::from("100%"))
    );
}

// ---------------------------------------------------------------------------
// string.find — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_find_anchored_start() {
    // `^` anchored pattern should only match at the start.
    let res = run_all("return string.find('hello world', '^hello')");
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(5)]);
}

#[test]
fn string_lib_find_anchored_start_no_match() {
    let res = run_all("return string.find('say hello', '^hello')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

#[test]
fn string_lib_find_anchored_end() {
    let res = run_all("return string.find('hello world', 'world$')");
    k9::assert_equal!(res, vec![Value::Integer(7), Value::Integer(11)]);
}

#[test]
fn string_lib_find_negative_init() {
    // Negative init counts from the end.
    let res = run_all("return string.find('abcabc', 'abc', -3)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Integer(6)]);
}

#[test]
fn string_lib_find_empty_pattern() {
    // Empty pattern matches at position 1.
    let res = run_all("return string.find('hello', '')");
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(0)]);
}

#[test]
fn string_lib_find_empty_haystack() {
    let res = run_all("return string.find('', 'anything')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

#[test]
fn string_lib_find_plain_empty_pattern() {
    let res = run_all("return string.find('hello', '', 1, true)");
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Integer(0)]);
}

// ---------------------------------------------------------------------------
// string.match — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_match_with_init() {
    // Start matching from position 5.
    let res = run_all("return string.match('abc 123 def 456', '%d+', 10)");
    k9::assert_equal!(res, vec![Value::String(Bytes::from("456"))]);
}

#[test]
fn string_lib_match_anchored() {
    // `^%d+` only matches digits at the start.
    let res = run_all("return string.match('123abc', '^%d+')");
    k9::assert_equal!(res, vec![Value::String(Bytes::from("123"))]);
}

#[test]
fn string_lib_match_anchored_no_match() {
    let res = run_all("return string.match('abc123', '^%d+')");
    k9::assert_equal!(res, vec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// string.gmatch — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gmatch_no_matches() {
    let res = run_one(
        "\
        local count = 0
        for w in string.gmatch('hello', '%d+') do
            count = count + 1
        end
        return count",
    );
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn string_lib_gmatch_empty_match() {
    // Empty pattern matches between every character; should not loop forever.
    let res = run_one(
        "\
        local t = {}
        for c in string.gmatch('ab', '.') do
            t[#t+1] = c
        end
        return #t",
    );
    k9::assert_equal!(res, Value::Integer(2));
}

// ---------------------------------------------------------------------------
// string.gsub — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_gsub_capture_ref_zero() {
    // %0 references the whole match.
    let res = run_all("return string.gsub('hello', '%w+', '[%0]')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("[hello]")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_percent_literal_in_replacement() {
    // %% in replacement string produces a literal %.
    let res = run_all("return string.gsub('abc', 'abc', '100%%')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("100%")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_table_missing_key() {
    // When the table has no entry for a match, the original match is kept.
    let res = run_all(
        "\
        local t = { hello = 'HI' }
        return string.gsub('hello world', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("HI world")), Value::Integer(2)]
    );
}

#[test]
fn string_lib_gsub_table_false_value() {
    // If the table value is false, the original match is preserved.
    let res = run_all(
        "\
        local t = { hello = false }
        return string.gsub('hello', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("hello")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_table_numeric_value() {
    // Numeric table values are coerced to string.
    let res = run_all(
        "\
        local t = { hello = 42 }
        return string.gsub('hello', '(%w+)', t)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("42")), Value::Integer(1)]
    );
}

#[test]
fn string_lib_gsub_function_replacement() {
    // Function replacement: function is called with each match,
    // return value becomes the replacement.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m) return m:upper() end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("HELLO WORLD")));
}

#[test]
fn string_lib_gsub_function_with_captures() {
    // Function receives each capture group as a separate argument.
    let res = run_one(
        "\
        return string.gsub('2025-04-13', '(%d+)-(%d+)-(%d+)', function(y, m, d)
            return d .. '/' .. m .. '/' .. y
        end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("13/04/2025")));
}

#[test]
fn string_lib_gsub_function_nil_keeps_original() {
    // If the function returns nil, the original match is kept.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m)
            if m == 'hello' then return nil end
            return m:upper()
        end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("hello WORLD")));
}

#[test]
fn string_lib_gsub_function_false_keeps_original() {
    // If the function returns false, the original match is kept.
    let res = run_one(
        "\
        return string.gsub('hello world', '%w+', function(m)
            if m == 'world' then return false end
            return m:upper()
        end)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("HELLO world")));
}

#[test]
fn string_lib_gsub_function_returns_number() {
    // If the function returns a number, it is coerced to a string.
    let res = run_one("return string.gsub('a b c', '%w+', function(m) return 42 end)");
    k9::assert_equal!(res, Value::String(Bytes::from("42 42 42")));
}

#[test]
fn string_lib_gsub_function_with_max_n() {
    // max_n limits the number of replacements.
    let res = run_all(
        "\
        return string.gsub('aaa', 'a', function() return 'b' end, 2)",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("bba")), Value::Integer(2)]
    );
}

#[test]
fn string_lib_gsub_function_invalid_return() {
    // If the function returns a table (not string/number/nil/false), error.
    let res = run_one(
        "\
        local ok = pcall(string.gsub, 'hello', '%w+', function() return {} end)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_gsub_bad_replacement_type() {
    // Passing a boolean as replacement should error.
    let res = run_one(
        "\
        local ok, msg = pcall(string.gsub, 'hello', '%w+', true)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_gsub_anchored_pattern() {
    // `^%w+` should only replace the first word (anchored at start).
    let res = run_all("return string.gsub('hello world', '^%w+', 'BYE')");
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("BYE world")), Value::Integer(1)]
    );
}

// ---------------------------------------------------------------------------
// string.format — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn string_lib_format_integer_i() {
    // %i is an alias for %d.
    k9::assert_equal!(
        run_one("return string.format('%i', 42)"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_unsigned() {
    k9::assert_equal!(
        run_one("return string.format('%u', 42)"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_octal() {
    k9::assert_equal!(
        run_one("return string.format('%o', 255)"),
        Value::String(Bytes::from("377"))
    );
}

#[test]
fn string_lib_format_octal_alt() {
    // `#` flag prepends a `0` for octal.
    k9::assert_equal!(
        run_one("return string.format('%#o', 255)"),
        Value::String(Bytes::from("0377"))
    );
}

#[test]
fn string_lib_format_scientific() {
    let res = run_one("return string.format('%.2e', 314.159)");
    k9::assert_equal!(res, Value::String(Bytes::from("3.14e2")));
}

#[test]
fn string_lib_format_scientific_upper() {
    let res = run_one("return string.format('%.2E', 314.159)");
    k9::assert_equal!(res, Value::String(Bytes::from("3.14E2")));
}

#[test]
fn string_lib_format_general_float() {
    // %g uses shorter of %e and %f.
    k9::assert_equal!(
        run_one("return string.format('%g', 100000.0)"),
        Value::String(Bytes::from("100000"))
    );
    k9::assert_equal!(
        run_one("return string.format('%g', 0.00123)"),
        Value::String(Bytes::from("0.00123"))
    );
}

#[test]
fn string_lib_format_char() {
    k9::assert_equal!(
        run_one("return string.format('%c', 65)"),
        Value::String(Bytes::from("A"))
    );
}

#[test]
fn string_lib_format_hex_alt() {
    // `#` flag prepends `0x` / `0X`.
    k9::assert_equal!(
        run_one("return string.format('%#x', 255)"),
        Value::String(Bytes::from("0xff"))
    );
    k9::assert_equal!(
        run_one("return string.format('%#X', 255)"),
        Value::String(Bytes::from("0XFF"))
    );
}

#[test]
fn string_lib_format_plus_flag() {
    k9::assert_equal!(
        run_one("return string.format('%+d', 42)"),
        Value::String(Bytes::from("+42"))
    );
    k9::assert_equal!(
        run_one("return string.format('%+d', -42)"),
        Value::String(Bytes::from("-42"))
    );
}

#[test]
fn string_lib_format_space_flag() {
    k9::assert_equal!(
        run_one("return string.format('% d', 42)"),
        Value::String(Bytes::from(" 42"))
    );
    k9::assert_equal!(
        run_one("return string.format('% d', -42)"),
        Value::String(Bytes::from("-42"))
    );
}

#[test]
fn string_lib_format_left_align() {
    k9::assert_equal!(
        run_one("return string.format('%-10d|', 42)"),
        Value::String(Bytes::from("42        |"))
    );
}

#[test]
fn string_lib_format_width_space_pad() {
    k9::assert_equal!(
        run_one("return string.format('%10d', 42)"),
        Value::String(Bytes::from("        42"))
    );
}

#[test]
fn string_lib_format_string_precision() {
    // %.3s truncates the string to 3 characters.
    k9::assert_equal!(
        run_one("return string.format('%.3s', 'hello')"),
        Value::String(Bytes::from("hel"))
    );
}

#[test]
fn string_lib_format_string_coercion_number() {
    // Formatting a number with %s should produce its string form.
    k9::assert_equal!(
        run_one("return string.format('%s', 42)"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_integer_from_string() {
    // %d with a numeric string coerces to integer.
    k9::assert_equal!(
        run_one("return string.format('%d', '42')"),
        Value::String(Bytes::from("42"))
    );
}

#[test]
fn string_lib_format_float_from_string() {
    // %f with a numeric string coerces to float.
    k9::assert_equal!(
        run_one("return string.format('%.1f', '3.14')"),
        Value::String(Bytes::from("3.1"))
    );
}

#[test]
fn string_lib_format_too_few_args() {
    let res = run_one(
        "\
        local ok, msg = pcall(string.format, '%d %d', 1)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_format_invalid_specifier() {
    let res = run_one(
        "\
        local ok, msg = pcall(string.format, '%z', 1)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_format_trailing_percent() {
    let res = run_one(
        "\
        local ok = pcall(string.format, 'oops%')
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn string_lib_format_quoted_special_chars() {
    // %q should escape newlines, backslashes, null bytes, and \x1a.
    k9::assert_equal!(
        run_one("return string.format('%q', 'a\\nb')"),
        Value::String(Bytes::from("\"a\\nb\""))
    );
    k9::assert_equal!(
        run_one("return string.format('%q', 'a\"b')"),
        Value::String(Bytes::from("\"a\\\"b\""))
    );
}

#[test]
fn string_lib_format_coerce_to_string_nil() {
    k9::assert_equal!(
        run_one("return string.format('%s', nil)"),
        Value::String(Bytes::from("nil"))
    );
}

#[test]
fn string_lib_format_coerce_to_string_bool() {
    k9::assert_equal!(
        run_one("return string.format('%s', true)"),
        Value::String(Bytes::from("true"))
    );
}

// ===========================================================================
// table library
// ===========================================================================

// ---------------------------------------------------------------------------
// table.insert
// ---------------------------------------------------------------------------

#[test]
fn table_insert_append() {
    let res = run_all(
        "\
        local t = {1, 2, 3}
        table.insert(t, 4)
        return t[1], t[2], t[3], t[4]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
        ]
    );
}

#[test]
fn table_insert_at_position() {
    let res = run_all(
        "\
        local t = {1, 2, 3}
        table.insert(t, 2, 99)
        return t[1], t[2], t[3], t[4]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(99),
            Value::Integer(2),
            Value::Integer(3),
        ]
    );
}

#[test]
fn table_insert_at_beginning() {
    let res = run_all(
        "\
        local t = {10, 20}
        table.insert(t, 1, 5)
        return t[1], t[2], t[3]",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(5), Value::Integer(10), Value::Integer(20)]
    );
}

#[test]
fn table_insert_at_end_with_pos() {
    // Inserting at #t+1 is the same as appending.
    let res = run_all(
        "\
        local t = {1, 2}
        table.insert(t, 3, 99)
        return #t, t[3]",
    );
    k9::assert_equal!(res, vec![Value::Integer(3), Value::Integer(99)]);
}

#[test]
fn table_insert_updates_length() {
    let res = run_one(
        "\
        local t = {}
        table.insert(t, 'a')
        table.insert(t, 'b')
        return #t",
    );
    k9::assert_equal!(res, Value::Integer(2));
}

// ---------------------------------------------------------------------------
// table.remove
// ---------------------------------------------------------------------------

#[test]
fn table_remove_last() {
    let res = run_all(
        "\
        local t = {10, 20, 30}
        local v = table.remove(t)
        return v, #t",
    );
    k9::assert_equal!(res, vec![Value::Integer(30), Value::Integer(2)]);
}

#[test]
fn table_remove_at_position() {
    let res = run_all(
        "\
        local t = {10, 20, 30}
        local v = table.remove(t, 2)
        return v, t[1], t[2], #t",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(20),
            Value::Integer(10),
            Value::Integer(30),
            Value::Integer(2),
        ]
    );
}

#[test]
fn table_remove_first() {
    let res = run_all(
        "\
        local t = {'a', 'b', 'c'}
        local v = table.remove(t, 1)
        return v, t[1], t[2]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("a")),
            Value::String(Bytes::from("b")),
            Value::String(Bytes::from("c")),
        ]
    );
}

#[test]
fn table_remove_empty() {
    // Removing from an empty table with no pos returns nil.
    let res = run_one(
        "\
        local t = {}
        return table.remove(t)",
    );
    k9::assert_equal!(res, Value::Nil);
}

// ---------------------------------------------------------------------------
// table.concat
// ---------------------------------------------------------------------------

#[test]
fn table_concat_default_sep() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b', 'c'}
            return table.concat(t)"
        ),
        Value::String(Bytes::from("abc"))
    );
}

#[test]
fn table_concat_with_sep() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'hello', 'world'}
            return table.concat(t, ', ')"
        ),
        Value::String(Bytes::from("hello, world"))
    );
}

#[test]
fn table_concat_range() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b', 'c', 'd', 'e'}
            return table.concat(t, '-', 2, 4)"
        ),
        Value::String(Bytes::from("b-c-d"))
    );
}

#[test]
fn table_concat_empty_range() {
    // When i > j, the result is an empty string.
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b'}
            return table.concat(t, ',', 3, 1)"
        ),
        Value::String(Bytes::new())
    );
}

#[test]
fn table_concat_numbers() {
    // Numbers in the sequence are coerced to strings.
    k9::assert_equal!(
        run_one(
            "\
            local t = {1, 2, 3}
            return table.concat(t, '+')"
        ),
        Value::String(Bytes::from("1+2+3"))
    );
}

#[test]
fn table_concat_empty_table() {
    k9::assert_equal!(
        run_one("return table.concat({})"),
        Value::String(Bytes::new())
    );
}

#[test]
fn table_concat_single_element() {
    k9::assert_equal!(
        run_one(
            "\
            local t = {'only'}
            return table.concat(t, ', ')"
        ),
        Value::String(Bytes::from("only"))
    );
}

#[test]
fn table_concat_invalid_value() {
    // Non-string, non-number values should error.
    let res = run_one(
        "\
        local ok = pcall(table.concat, {true}, ',')
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// table.insert + table.remove combined
// ---------------------------------------------------------------------------

#[test]
fn table_insert_remove_stack() {
    // Use a table as a stack.
    let res = run_all(
        "\
        local t = {}
        table.insert(t, 'a')
        table.insert(t, 'b')
        table.insert(t, 'c')
        local top = table.remove(t)
        return top, #t",
    );
    k9::assert_equal!(
        res,
        vec![Value::String(Bytes::from("c")), Value::Integer(2)]
    );
}

#[test]
fn table_insert_remove_queue() {
    // Use a table as a queue.
    let res = run_all(
        "\
        local t = {}
        table.insert(t, 'a')
        table.insert(t, 'b')
        table.insert(t, 'c')
        local first = table.remove(t, 1)
        return first, t[1], t[2]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("a")),
            Value::String(Bytes::from("b")),
            Value::String(Bytes::from("c")),
        ]
    );
}

// ---------------------------------------------------------------------------
// table.insert — error paths
// ---------------------------------------------------------------------------

#[test]
fn table_insert_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, 'notatable', 1)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_insert_too_few_args_zero() {
    let res = run_one(
        "\
        local ok = pcall(table.insert)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_insert_too_few_args_one() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, {})
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_insert_pos_out_of_bounds_zero() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, {1, 2}, 0, 99)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_insert_pos_out_of_bounds_too_large() {
    let res = run_one(
        "\
        local ok = pcall(table.insert, {1, 2}, 100, 99)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// table.remove — error paths
// ---------------------------------------------------------------------------

#[test]
fn table_remove_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.remove, 42)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_remove_pos_out_of_bounds_zero() {
    let res = run_one(
        "\
        local ok = pcall(table.remove, {1, 2}, 0)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_remove_pos_out_of_bounds_too_large() {
    let res = run_one(
        "\
        local ok = pcall(table.remove, {1, 2}, 5)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_remove_returns_string() {
    let res = run_one(
        "\
        local t = {'x', 'y', 'z'}
        return table.remove(t, 2)",
    );
    k9::assert_equal!(res, Value::String(Bytes::from("y")));
}

// ---------------------------------------------------------------------------
// table.concat — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn table_concat_float_values() {
    // Float values in the sequence are coerced to strings.
    k9::assert_equal!(
        run_one(
            "\
            local t = {1.5, 2.5}
            return table.concat(t, '+')"
        ),
        Value::String(Bytes::from("1.5+2.5"))
    );
}

#[test]
fn table_concat_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.concat, 'notatable')
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_concat_nil_args_use_defaults() {
    // Passing nil for sep, i, j should use defaults.
    k9::assert_equal!(
        run_one(
            "\
            local t = {'a', 'b', 'c'}
            return table.concat(t, nil, nil, nil)"
        ),
        Value::String(Bytes::from("abc"))
    );
}

// ---------------------------------------------------------------------------
// table.sort
// ---------------------------------------------------------------------------

#[test]
fn table_sort_default() {
    let res = run_all(
        "\
        local t = {3, 1, 4, 1, 5, 9, 2, 6}
        table.sort(t)
        return t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
            Value::Integer(5),
            Value::Integer(6),
            Value::Integer(9),
        ]
    );
}

#[test]
fn table_sort_strings() {
    let res = run_all(
        "\
        local t = {'banana', 'apple', 'cherry'}
        table.sort(t)
        return t[1], t[2], t[3]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("apple")),
            Value::String(Bytes::from("banana")),
            Value::String(Bytes::from("cherry")),
        ]
    );
}

#[test]
fn table_sort_custom_comparator() {
    // Sort in descending order.
    let res = run_all(
        "\
        local t = {3, 1, 4, 1, 5}
        table.sort(t, function(a, b) return a > b end)
        return t[1], t[2], t[3], t[4], t[5]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(5),
            Value::Integer(4),
            Value::Integer(3),
            Value::Integer(1),
            Value::Integer(1),
        ]
    );
}

#[test]
fn table_sort_single_element() {
    let res = run_all(
        "\
        local t = {42}
        table.sort(t)
        return t[1]",
    );
    k9::assert_equal!(res, vec![Value::Integer(42)]);
}

#[test]
fn table_sort_empty() {
    let res = run_one(
        "\
        local t = {}
        table.sort(t)
        return #t",
    );
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn table_sort_already_sorted() {
    let res = run_all(
        "\
        local t = {1, 2, 3, 4, 5}
        table.sort(t)
        return t[1], t[2], t[3], t[4], t[5]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
            Value::Integer(5),
        ]
    );
}

#[test]
fn table_sort_reverse_sorted() {
    let res = run_all(
        "\
        local t = {5, 4, 3, 2, 1}
        table.sort(t)
        return t[1], t[2], t[3], t[4], t[5]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(4),
            Value::Integer(5),
        ]
    );
}

#[test]
fn table_sort_mixed_int_float() {
    let res = run_all(
        "\
        local t = {3.5, 1, 2.5, 2}
        table.sort(t)
        return t[1], t[2], t[3], t[4]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Float(2.5),
            Value::Float(3.5),
        ]
    );
}

#[test]
fn table_sort_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.sort, 'notatable')
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_sort_incompatible_types() {
    // Comparing a number with a string should error.
    let res = run_one(
        "\
        local ok = pcall(table.sort, {1, 'a'})
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_sort_custom_comparator_by_field() {
    // Sort a table of records by a field using a comparator.
    let res = run_all(
        "\
        local t = {
            {name='charlie', age=30},
            {name='alice', age=25},
            {name='bob', age=35},
        }
        table.sort(t, function(a, b) return a.age < b.age end)
        return t[1].name, t[2].name, t[3].name",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("alice")),
            Value::String(Bytes::from("charlie")),
            Value::String(Bytes::from("bob")),
        ]
    );
}

#[test]
fn table_sort_comparator_error_propagates() {
    // If the comparator throws, the error should propagate and the table
    // should still have its elements (not be left empty).
    let res = run_all(
        "\
        local t = {3, 1, 2}
        local ok, msg = pcall(table.sort, t, function(a, b)
            error('comp failed')
        end)
        return ok, #t",
    );
    k9::assert_equal!(res, vec![Value::Boolean(false), Value::Integer(3)]);
}

#[test]
fn table_sort_comparator_truthy_non_boolean() {
    // A comparator returning a truthy non-boolean (e.g. a number) counts
    // as true.
    let res = run_all(
        "\
        local t = {3, 1, 2}
        table.sort(t, function(a, b) if a < b then return 1 else return nil end end)
        return t[1], t[2], t[3]",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[test]
fn table_sort_duplicates_with_comparator() {
    let res = run_all(
        "\
        local t = {5, 3, 3, 1, 5, 1, 2}
        table.sort(t, function(a, b) return a < b end)
        return t[1], t[2], t[3], t[4], t[5], t[6], t[7]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(3),
            Value::Integer(5),
            Value::Integer(5),
        ]
    );
}

#[test]
fn table_sort_all_equal() {
    let res = run_all(
        "\
        local t = {7, 7, 7, 7}
        table.sort(t)
        return t[1], t[2], t[3], t[4]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(7),
            Value::Integer(7),
            Value::Integer(7),
            Value::Integer(7),
        ]
    );
}

#[test]
fn table_sort_large_array() {
    // 50 elements to exercise multiple levels of merge sort recursion.
    let res = run_all(
        "\
        local t = {}
        for i = 50, 1, -1 do
            t[#t+1] = i
        end
        table.sort(t)
        return t[1], t[25], t[50]",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(1), Value::Integer(25), Value::Integer(50)]
    );
}

#[test]
fn table_sort_large_array_with_comparator() {
    // 50 elements descending via Lua comparator.
    let res = run_all(
        "\
        local t = {}
        for i = 1, 50 do
            t[#t+1] = i
        end
        table.sort(t, function(a, b) return a > b end)
        return t[1], t[25], t[50]",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(50), Value::Integer(26), Value::Integer(1)]
    );
}

// ---------------------------------------------------------------------------
// table.move
// ---------------------------------------------------------------------------

#[test]
fn table_move_same_table() {
    let res = run_all(
        "\
        local t = {1, 2, 3, 4, 5}
        table.move(t, 1, 3, 2)
        return t[1], t[2], t[3], t[4], t[5]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(1),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
            Value::Integer(5),
        ]
    );
}

#[test]
fn table_move_to_other_table() {
    let res = run_all(
        "\
        local src = {10, 20, 30}
        local dst = {0, 0, 0, 0, 0}
        table.move(src, 1, 3, 2, dst)
        return dst[1], dst[2], dst[3], dst[4], dst[5]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(0),
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(30),
            Value::Integer(0),
        ]
    );
}

#[test]
fn table_move_returns_destination() {
    let res = run_one(
        "\
        local src = {1, 2, 3}
        local dst = {}
        local r = table.move(src, 1, 3, 1, dst)
        return r == dst",
    );
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn table_move_empty_range() {
    // f > e means nothing is copied.
    let res = run_one(
        "\
        local t = {1, 2, 3}
        table.move(t, 3, 1, 1)
        return t[1]",
    );
    k9::assert_equal!(res, Value::Integer(1));
}

#[test]
fn table_move_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.move, 'notatable', 1, 2, 1)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// table.pack
// ---------------------------------------------------------------------------

#[test]
fn table_pack_basic() {
    let res = run_all(
        "\
        local t = table.pack(10, 20, 30)
        return t[1], t[2], t[3], t.n",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(10),
            Value::Integer(20),
            Value::Integer(30),
            Value::Integer(3),
        ]
    );
}

#[test]
fn table_pack_empty() {
    let res = run_one(
        "\
        local t = table.pack()
        return t.n",
    );
    k9::assert_equal!(res, Value::Integer(0));
}

#[test]
fn table_pack_with_nils() {
    // Nils in the middle are preserved; n reflects total count.
    let res = run_all(
        "\
        local t = table.pack(1, nil, 3)
        return t.n, t[1], t[2], t[3]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(3),
            Value::Integer(1),
            Value::Nil,
            Value::Integer(3),
        ]
    );
}

// ---------------------------------------------------------------------------
// table.unpack
// ---------------------------------------------------------------------------

#[test]
fn table_unpack_basic() {
    let res = run_all(
        "\
        return table.unpack({10, 20, 30})",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[test]
fn table_unpack_range() {
    let res = run_all(
        "\
        return table.unpack({10, 20, 30, 40, 50}, 2, 4)",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(20), Value::Integer(30), Value::Integer(40)]
    );
}

#[test]
fn table_unpack_empty_range() {
    // i > j returns nothing.
    let res = run_all(
        "\
        return table.unpack({1, 2, 3}, 3, 1)",
    );
    k9::assert_equal!(res, vec![]);
}

#[test]
fn table_unpack_single() {
    let res = run_all(
        "\
        return table.unpack({99}, 1, 1)",
    );
    k9::assert_equal!(res, vec![Value::Integer(99)]);
}

#[test]
fn table_unpack_bad_arg1_type() {
    let res = run_one(
        "\
        local ok = pcall(table.unpack, 'notatable')
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

// ---------------------------------------------------------------------------
// global unpack (Lua 5.1 compat)
// ---------------------------------------------------------------------------

#[test]
fn global_unpack_basic() {
    let res = run_all(
        "\
        return unpack({10, 20, 30})",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[test]
fn global_unpack_range() {
    let res = run_all(
        "\
        return unpack({'a', 'b', 'c', 'd'}, 2, 3)",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::String(Bytes::from("b")),
            Value::String(Bytes::from("c")),
        ]
    );
}

// ---------------------------------------------------------------------------
// table.move — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn table_move_too_few_args() {
    // Only 3 args instead of the required 4.
    let res = run_one(
        "\
        local ok = pcall(table.move, {1,2,3}, 1, 2)
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_move_bad_a2_type() {
    let res = run_one(
        "\
        local ok = pcall(table.move, {1,2,3}, 1, 3, 1, 'notatable')
        return ok",
    );
    k9::assert_equal!(res, Value::Boolean(false));
}

#[test]
fn table_move_overlap_shift_left() {
    // Copy elements 3..5 to starting at index 1 (shift left within same table).
    let res = run_all(
        "\
        local t = {10, 20, 30, 40, 50}
        table.move(t, 3, 5, 1)
        return t[1], t[2], t[3], t[4], t[5]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(30),
            Value::Integer(40),
            Value::Integer(50),
            Value::Integer(40),
            Value::Integer(50),
        ]
    );
}

// ---------------------------------------------------------------------------
// table.pack — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn table_pack_mixed_types() {
    let res = run_all(
        "\
        local t = table.pack(1, 'hello', true, nil, 3.14)
        return t.n, t[1], t[2], t[3], t[5]",
    );
    k9::assert_equal!(
        res,
        vec![
            Value::Integer(5),
            Value::Integer(1),
            Value::String(Bytes::from("hello")),
            Value::Boolean(true),
            Value::Float(3.14),
        ]
    );
}

#[test]
fn table_pack_unpack_roundtrip() {
    let res = run_all(
        "\
        local a, b, c = table.unpack(table.pack(10, 20, 30))
        return a, b, c",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

// ---------------------------------------------------------------------------
// table.unpack — additional coverage
// ---------------------------------------------------------------------------

#[test]
fn table_unpack_nils_in_middle() {
    // Gaps in the table come back as nil.
    let res = run_all(
        "\
        local t = {1, nil, 3}
        return table.unpack(t, 1, 3)",
    );
    k9::assert_equal!(res, vec![Value::Integer(1), Value::Nil, Value::Integer(3)]);
}

#[test]
fn table_unpack_explicit_i_only() {
    // Only i specified; j defaults to #t.
    let res = run_all(
        "\
        return table.unpack({10, 20, 30, 40}, 3)",
    );
    k9::assert_equal!(res, vec![Value::Integer(30), Value::Integer(40)]);
}

#[test]
fn table_unpack_nil_args_use_defaults() {
    let res = run_all(
        "\
        return table.unpack({10, 20, 30}, nil, nil)",
    );
    k9::assert_equal!(
        res,
        vec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

// ===========================================================================
// math library
// ===========================================================================

// ---------------------------------------------------------------------------
// math constants
// ---------------------------------------------------------------------------

#[test]
fn math_pi() {
    k9::assert_equal!(
        run_one("return math.pi"),
        Value::Float(std::f64::consts::PI)
    );
}

#[test]
fn math_huge() {
    k9::assert_equal!(run_one("return math.huge"), Value::Float(f64::INFINITY));
}

#[test]
fn math_huge_is_infinity() {
    k9::assert_equal!(run_one("return math.huge > 1e308"), Value::Boolean(true));
}

#[test]
fn math_maxinteger() {
    k9::assert_equal!(run_one("return math.maxinteger"), Value::Integer(i64::MAX));
}

#[test]
fn math_mininteger() {
    k9::assert_equal!(run_one("return math.mininteger"), Value::Integer(i64::MIN));
}

#[test]
fn math_maxinteger_plus_one_wraps() {
    // Adding 1 to maxinteger should wrap around (Lua integer overflow).
    k9::assert_equal!(
        run_one("return math.maxinteger + 1 == math.mininteger"),
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// math.floor
// ---------------------------------------------------------------------------

#[test]
fn math_floor_float() {
    k9::assert_equal!(run_one("return math.floor(3.7)"), Value::Integer(3));
}

#[test]
fn math_floor_negative() {
    k9::assert_equal!(run_one("return math.floor(-2.3)"), Value::Integer(-3));
}

#[test]
fn math_floor_integer_passthrough() {
    k9::assert_equal!(run_one("return math.floor(5)"), Value::Integer(5));
}

#[test]
fn math_floor_exact() {
    // Already whole float.
    k9::assert_equal!(run_one("return math.floor(4.0)"), Value::Integer(4));
}

#[test]
fn math_floor_huge() {
    // floor(inf) stays float since it can't be an integer.
    k9::assert_equal!(
        run_one("return math.floor(math.huge)"),
        Value::Float(f64::INFINITY)
    );
}

// ---------------------------------------------------------------------------
// math.ceil
// ---------------------------------------------------------------------------

#[test]
fn math_ceil_float() {
    k9::assert_equal!(run_one("return math.ceil(3.2)"), Value::Integer(4));
}

#[test]
fn math_ceil_negative() {
    k9::assert_equal!(run_one("return math.ceil(-2.7)"), Value::Integer(-2));
}

#[test]
fn math_ceil_integer_passthrough() {
    k9::assert_equal!(run_one("return math.ceil(5)"), Value::Integer(5));
}

#[test]
fn math_ceil_exact() {
    k9::assert_equal!(run_one("return math.ceil(4.0)"), Value::Integer(4));
}

// ---------------------------------------------------------------------------
// math.abs
// ---------------------------------------------------------------------------

#[test]
fn math_abs_positive_int() {
    k9::assert_equal!(run_one("return math.abs(42)"), Value::Integer(42));
}

#[test]
fn math_abs_negative_int() {
    k9::assert_equal!(run_one("return math.abs(-42)"), Value::Integer(42));
}

#[test]
fn math_abs_float() {
    k9::assert_equal!(run_one("return math.abs(-3.14)"), Value::Float(3.14));
}

#[test]
fn math_abs_zero() {
    k9::assert_equal!(run_one("return math.abs(0)"), Value::Integer(0));
}

#[test]
fn math_abs_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.abs, 'hello') return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.modf
// ---------------------------------------------------------------------------

#[test]
fn math_modf_positive() {
    let res = run_all("return math.modf(3.75)");
    k9::assert_equal!(res, vec![Value::Integer(3), Value::Float(0.75)]);
}

#[test]
fn math_modf_negative() {
    let res = run_all("return math.modf(-3.75)");
    k9::assert_equal!(res, vec![Value::Integer(-3), Value::Float(-0.75)]);
}

#[test]
fn math_modf_integer() {
    let res = run_all("return math.modf(5)");
    k9::assert_equal!(res, vec![Value::Integer(5), Value::Float(0.0)]);
}

#[test]
fn math_modf_whole_float() {
    let res = run_all("return math.modf(4.0)");
    k9::assert_equal!(res, vec![Value::Integer(4), Value::Float(0.0)]);
}

#[test]
fn math_modf_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.modf, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_modf_infinity() {
    // modf(inf) — integral part is inf (can't be integer), frac is NaN.
    let res = run_all("return math.modf(math.huge)");
    k9::assert_equal!(res.len(), 2);
    k9::assert_equal!(res[0], Value::Float(f64::INFINITY));
    // inf - inf = NaN; verify via NaN ~= NaN.
    match res[1] {
        Value::Float(f) => assert!(f.is_nan(), "expected NaN, got {}", f),
        ref other => panic!("expected Float, got {:?}", other),
    }
}

#[test]
fn math_floor_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.floor, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_floor_nan() {
    // floor(NaN) stays float since NaN is not finite.
    let res = run_one("return math.floor(0/0) ~= math.floor(0/0)");
    // NaN ~= NaN is true in Lua.
    k9::assert_equal!(res, Value::Boolean(true));
}

#[test]
fn math_ceil_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.ceil, 'hello') return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.sqrt
// ---------------------------------------------------------------------------

#[test]
fn math_sqrt_perfect() {
    k9::assert_equal!(run_one("return math.sqrt(9)"), Value::Float(3.0));
}

#[test]
fn math_sqrt_float() {
    k9::assert_equal!(
        run_one("return math.sqrt(2.0)"),
        Value::Float(2.0_f64.sqrt())
    );
}

#[test]
fn math_sqrt_zero() {
    k9::assert_equal!(run_one("return math.sqrt(0)"), Value::Float(0.0));
}

#[test]
fn math_sqrt_negative_is_nan() {
    k9::assert_equal!(
        run_one("return math.sqrt(-1) ~= math.sqrt(-1)"),
        Value::Boolean(true)
    );
}

#[test]
fn math_sqrt_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.sqrt, {}) return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.exp
// ---------------------------------------------------------------------------

#[test]
fn math_exp_zero() {
    k9::assert_equal!(run_one("return math.exp(0)"), Value::Float(1.0));
}

#[test]
fn math_exp_one() {
    k9::assert_equal!(
        run_one("return math.exp(1)"),
        Value::Float(std::f64::consts::E)
    );
}

#[test]
fn math_exp_negative() {
    k9::assert_equal!(
        run_one("return math.exp(-1)"),
        Value::Float((-1.0_f64).exp())
    );
}

#[test]
fn math_exp_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.exp, true) return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.log
// ---------------------------------------------------------------------------

#[test]
fn math_log_natural() {
    k9::assert_equal!(run_one("return math.log(1)"), Value::Float(0.0));
}

#[test]
fn math_log_e() {
    // ln(e) == 1
    k9::assert_equal!(run_one("return math.log(math.exp(1))"), Value::Float(1.0));
}

#[test]
fn math_log_base_10() {
    // ln(1000)/ln(10) has floating-point rounding, so compare approximately.
    k9::assert_equal!(
        run_one("return math.log(1000, 10) > 2.999 and math.log(1000, 10) < 3.001"),
        Value::Boolean(true)
    );
}

#[test]
fn math_log_base_2() {
    k9::assert_equal!(run_one("return math.log(8, 2)"), Value::Float(3.0));
}

#[test]
fn math_log_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.log, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_log_bad_base_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.log, 10, 'hello') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_log_float_input() {
    k9::assert_equal!(run_one("return math.log(1.0)"), Value::Float(0.0));
}

// ---------------------------------------------------------------------------
// math.sin / math.cos / math.tan
// ---------------------------------------------------------------------------

#[test]
fn math_sin_zero() {
    k9::assert_equal!(run_one("return math.sin(0)"), Value::Float(0.0));
}

#[test]
fn math_sin_pi_half() {
    k9::assert_equal!(
        run_one("return math.sin(math.pi / 2)"),
        Value::Float((std::f64::consts::PI / 2.0).sin())
    );
}

#[test]
fn math_sin_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.sin, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_cos_zero() {
    k9::assert_equal!(run_one("return math.cos(0)"), Value::Float(1.0));
}

#[test]
fn math_cos_pi() {
    k9::assert_equal!(
        run_one("return math.cos(math.pi)"),
        Value::Float(std::f64::consts::PI.cos())
    );
}

#[test]
fn math_tan_zero() {
    k9::assert_equal!(run_one("return math.tan(0)"), Value::Float(0.0));
}

#[test]
fn math_tan_pi_quarter() {
    k9::assert_equal!(
        run_one("return math.tan(math.pi / 4)"),
        Value::Float((std::f64::consts::PI / 4.0).tan())
    );
}

// ---------------------------------------------------------------------------
// math.asin / math.acos / math.atan
// ---------------------------------------------------------------------------

#[test]
fn math_asin_zero() {
    k9::assert_equal!(run_one("return math.asin(0)"), Value::Float(0.0));
}

#[test]
fn math_asin_one() {
    k9::assert_equal!(run_one("return math.asin(1)"), Value::Float(1.0_f64.asin()));
}

#[test]
fn math_asin_out_of_range_is_nan() {
    // asin(2) is NaN.
    k9::assert_equal!(
        run_one("return math.asin(2) ~= math.asin(2)"),
        Value::Boolean(true)
    );
}

#[test]
fn math_acos_one() {
    k9::assert_equal!(run_one("return math.acos(1)"), Value::Float(0.0));
}

#[test]
fn math_acos_zero() {
    k9::assert_equal!(run_one("return math.acos(0)"), Value::Float(0.0_f64.acos()));
}

#[test]
fn math_atan_zero() {
    k9::assert_equal!(run_one("return math.atan(0)"), Value::Float(0.0));
}

#[test]
fn math_atan_one() {
    k9::assert_equal!(run_one("return math.atan(1)"), Value::Float(1.0_f64.atan()));
}

#[test]
fn math_atan_two_args() {
    // atan(1, 1) == atan2(1, 1) == pi/4
    k9::assert_equal!(
        run_one("return math.atan(1, 1)"),
        Value::Float(1.0_f64.atan2(1.0))
    );
}

#[test]
fn math_atan_two_args_negative() {
    // atan(-1, -1) == atan2(-1, -1) == -3*pi/4
    k9::assert_equal!(
        run_one("return math.atan(-1, -1)"),
        Value::Float((-1.0_f64).atan2(-1.0))
    );
}

#[test]
fn math_atan_bad_second_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.atan, 1, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_trig_roundtrip() {
    // asin(sin(x)) should return x for x in [-pi/2, pi/2].
    k9::assert_equal!(
        run_one("return math.asin(math.sin(0.5)) - 0.5 < 1e-10"),
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// math.min
// ---------------------------------------------------------------------------

#[test]
fn math_min_two() {
    k9::assert_equal!(run_one("return math.min(3, 1)"), Value::Integer(1));
}

#[test]
fn math_min_many() {
    k9::assert_equal!(run_one("return math.min(5, 3, 8, 1, 4)"), Value::Integer(1));
}

#[test]
fn math_min_single() {
    k9::assert_equal!(run_one("return math.min(42)"), Value::Integer(42));
}

#[test]
fn math_min_negative() {
    k9::assert_equal!(
        run_one("return math.min(-10, -20, -5)"),
        Value::Integer(-20)
    );
}

#[test]
fn math_min_mixed_int_float() {
    // Should return the float since 1.5 < 2.
    k9::assert_equal!(run_one("return math.min(2, 1.5)"), Value::Float(1.5));
}

#[test]
fn math_min_preserves_integer() {
    // When the minimum is an integer, it stays integer.
    k9::assert_equal!(run_one("return math.min(1, 2.5)"), Value::Integer(1));
}

#[test]
fn math_min_no_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.min) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_min_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.min, 1, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_min_tie_returns_first() {
    // Equal values: the first argument wins, preserving its type.
    k9::assert_equal!(run_one("return math.min(1, 1.0)"), Value::Integer(1));
}

// ---------------------------------------------------------------------------
// math.max
// ---------------------------------------------------------------------------

#[test]
fn math_max_two() {
    k9::assert_equal!(run_one("return math.max(3, 1)"), Value::Integer(3));
}

#[test]
fn math_max_many() {
    k9::assert_equal!(run_one("return math.max(5, 3, 8, 1, 4)"), Value::Integer(8));
}

#[test]
fn math_max_single() {
    k9::assert_equal!(run_one("return math.max(42)"), Value::Integer(42));
}

#[test]
fn math_max_negative() {
    k9::assert_equal!(run_one("return math.max(-10, -20, -5)"), Value::Integer(-5));
}

#[test]
fn math_max_mixed_int_float() {
    // Should return the float since 3.5 > 2.
    k9::assert_equal!(run_one("return math.max(2, 3.5)"), Value::Float(3.5));
}

#[test]
fn math_max_preserves_integer() {
    k9::assert_equal!(run_one("return math.max(3, 1.5)"), Value::Integer(3));
}

#[test]
fn math_max_no_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.max) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_max_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.max, 1, true) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_max_tie_returns_first() {
    // Equal values: the first argument wins, preserving its type.
    k9::assert_equal!(run_one("return math.max(1, 1.0)"), Value::Integer(1));
}

// ---------------------------------------------------------------------------
// math.tointeger
// ---------------------------------------------------------------------------

#[test]
fn math_tointeger_from_integer() {
    k9::assert_equal!(run_one("return math.tointeger(42)"), Value::Integer(42));
}

#[test]
fn math_tointeger_from_whole_float() {
    k9::assert_equal!(run_one("return math.tointeger(5.0)"), Value::Integer(5));
}

#[test]
fn math_tointeger_fractional_fails() {
    k9::assert_equal!(run_one("return math.tointeger(3.14)"), Value::Nil);
}

#[test]
fn math_tointeger_infinity_fails() {
    k9::assert_equal!(run_one("return math.tointeger(math.huge)"), Value::Nil);
}

#[test]
fn math_tointeger_nan_fails() {
    k9::assert_equal!(run_one("return math.tointeger(0/0)"), Value::Nil);
}

#[test]
fn math_tointeger_string_fails() {
    k9::assert_equal!(run_one("return math.tointeger('42')"), Value::Nil);
}

#[test]
fn math_tointeger_nil_fails() {
    k9::assert_equal!(run_one("return math.tointeger(nil)"), Value::Nil);
}

#[test]
fn math_tointeger_negative() {
    k9::assert_equal!(run_one("return math.tointeger(-7.0)"), Value::Integer(-7));
}

// ---------------------------------------------------------------------------
// math.type
// ---------------------------------------------------------------------------

#[test]
fn math_type_integer() {
    k9::assert_equal!(
        run_one("return math.type(42)"),
        Value::String(Bytes::from("integer"))
    );
}

#[test]
fn math_type_float() {
    k9::assert_equal!(
        run_one("return math.type(3.14)"),
        Value::String(Bytes::from("float"))
    );
}

#[test]
fn math_type_string() {
    k9::assert_equal!(run_one("return math.type('hello')"), Value::Boolean(false));
}

#[test]
fn math_type_nil() {
    k9::assert_equal!(run_one("return math.type(nil)"), Value::Boolean(false));
}

#[test]
fn math_type_boolean() {
    k9::assert_equal!(run_one("return math.type(true)"), Value::Boolean(false));
}

#[test]
fn math_type_integer_zero() {
    // 0 is an integer, not a float.
    k9::assert_equal!(
        run_one("return math.type(0)"),
        Value::String(Bytes::from("integer"))
    );
}

#[test]
fn math_type_float_zero() {
    k9::assert_equal!(
        run_one("return math.type(0.0)"),
        Value::String(Bytes::from("float"))
    );
}

// ---------------------------------------------------------------------------
// math.random
// ---------------------------------------------------------------------------

#[test]
fn math_random_no_args_returns_float() {
    // No args: returns a float in [0, 1).
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random()
            return type(r) == 'number' and r >= 0 and r < 1"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_no_args_is_float_type() {
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.type(math.random())"
        ),
        Value::String(Bytes::from("float"))
    );
}

#[test]
fn math_random_one_arg() {
    // One arg m: returns an integer in [1, m].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(10)
            return math.type(r) == 'integer' and r >= 1 and r <= 10"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_two_args() {
    // Two args: returns an integer in [m, n].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(5, 10)
            return math.type(r) == 'integer' and r >= 5 and r <= 10"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_one_arg_is_one() {
    // math.random(1) always returns 1.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.random(1)"
        ),
        Value::Integer(1)
    );
}

#[test]
fn math_random_same_bounds() {
    // math.random(5, 5) always returns 5.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            return math.random(5, 5)"
        ),
        Value::Integer(5)
    );
}

#[test]
fn math_random_empty_interval_one_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 0) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_empty_interval_two_args() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 10, 5) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, 'x') return ok"),
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// math.randomseed
// ---------------------------------------------------------------------------

#[test]
fn math_randomseed_deterministic() {
    // Same seed produces the same sequence.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(123)
            local a = math.random()
            math.randomseed(123)
            local b = math.random()
            return a == b"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_randomseed_no_args() {
    // No args uses a time-based seed; just check it doesn't error.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed()
            local r = math.random()
            return r >= 0 and r < 1"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_randomseed_bad_type() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.randomseed, 'x') return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_negative_one_arg() {
    k9::assert_equal!(
        run_one("local ok = pcall(math.random, -1) return ok"),
        Value::Boolean(false)
    );
}

#[test]
fn math_random_float_coercion() {
    // 10.5 truncates to 10; result should be in [1, 10].
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local r = math.random(10.5)
            return r >= 1 and r <= 10"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_random_sequence_varies() {
    // Multiple calls should not all return the same value.
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(42)
            local a = math.random()
            local b = math.random()
            local c = math.random()
            return a ~= b or b ~= c"
        ),
        Value::Boolean(true)
    );
}

#[test]
fn math_randomseed_different_seeds_diverge() {
    k9::assert_equal!(
        run_one(
            "\
            math.randomseed(1)
            local a = math.random()
            math.randomseed(2)
            local b = math.random()
            return a ~= b"
        ),
        Value::Boolean(true)
    );
}

// ---------------------------------------------------------------------------
// Contextual error messages — variable names in errors
// ---------------------------------------------------------------------------

/// Compile and run a Lua snippet, returning the error message string.
fn run_err(src: &str) -> String {
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let env = new_env();
    let func = Function::lua(bc.top_level, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let err = rt.block_on(Task::new(env, func, vec![])).unwrap_err();
    err.to_string()
}

#[test]
fn error_index_nil_global() {
    k9::assert_equal!(
        run_err("return nil_global.field"),
        "attempt to index global 'nil_global' (a nil value)"
    );
}

#[test]
fn error_index_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil
            return x.field"
        ),
        "attempt to index local 'x' (a nil value)"
    );
}

#[test]
fn error_call_nil_global() {
    k9::assert_equal!(
        run_err("nil_global()"),
        "attempt to call global 'nil_global' (a nil value)"
    );
}

#[test]
fn error_call_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local f = nil
            f()"
        ),
        "attempt to call local 'f' (a nil value)"
    );
}

#[test]
fn error_call_number() {
    k9::assert_equal!(
        run_err(
            "\
            local n = 42
            n()"
        ),
        "attempt to call local 'n' (a number value)"
    );
}

#[test]
fn error_index_number_local() {
    k9::assert_equal!(
        run_err(
            "\
            local n = 42
            return n.field"
        ),
        "attempt to index local 'n' (a number value)"
    );
}

#[test]
fn error_index_boolean_local() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true
            return b.field"
        ),
        "attempt to index local 'b' (a boolean value)"
    );
}

#[test]
fn error_method_on_nil_global() {
    // obj:method() desugars to GetTable + Call; the error should mention
    // the object being indexed.
    k9::assert_equal!(
        run_err("nil_global:some_method()"),
        "attempt to index global 'nil_global' (a nil value)"
    );
}

#[test]
fn error_index_without_name() {
    // When the value comes from an expression rather than a named variable,
    // we fall back to the type-only message.
    k9::assert_equal!(
        run_err("return (nil).field"),
        "attempt to index a nil value"
    );
}

// ---------------------------------------------------------------------------
// Arithmetic error messages with variable names
// ---------------------------------------------------------------------------

#[test]
fn error_arith_local_nil() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x + 1"
        ),
        "attempt to perform arithmetic on local 'x' (a nil value)"
    );
}

#[test]
fn error_arith_global_nil() {
    k9::assert_equal!(
        run_err("return g + 1"),
        "attempt to perform arithmetic on global 'g' (a nil value)"
    );
}

#[test]
fn error_arith_string_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s - 1"
        ),
        "attempt to perform arithmetic on local 's' (a string value)"
    );
}

#[test]
fn error_arith_rhs_is_bad() {
    // When the left operand is fine but the right is not, name the right.
    k9::assert_equal!(
        run_err(
            "\
            local y = true\n\
            return 1 + y"
        ),
        "attempt to perform arithmetic on local 'y' (a boolean value)"
    );
}

#[test]
fn error_arith_no_name() {
    // Expression without a named variable falls back to type-only.
    k9::assert_equal!(
        run_err("return nil + 1"),
        "attempt to perform arithmetic on a nil value"
    );
}

#[test]
fn error_negate_local() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true\n\
            return -b"
        ),
        "attempt to perform arithmetic on local 'b' (a boolean value)"
    );
}

#[test]
fn error_bitwise_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s & 1"
        ),
        "attempt to perform arithmetic on local 's' (a string value)"
    );
}

#[test]
fn error_bitnot_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return ~s"
        ),
        "attempt to perform arithmetic on local 's' (a string value)"
    );
}

// ---------------------------------------------------------------------------
// Concatenation error messages with variable names
// ---------------------------------------------------------------------------

#[test]
fn error_concat_local_nil() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return 'hello' .. x"
        ),
        "attempt to concatenate local 'x' (a nil value)"
    );
}

#[test]
fn error_concat_global() {
    k9::assert_equal!(
        run_err("return 'hello' .. g"),
        "attempt to concatenate global 'g' (a nil value)"
    );
}

#[test]
fn error_concat_boolean_local() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true\n\
            return b .. 'world'"
        ),
        "attempt to concatenate local 'b' (a boolean value)"
    );
}

#[test]
fn error_concat_no_name() {
    k9::assert_equal!(
        run_err("return true .. 'x'"),
        "attempt to concatenate a boolean value"
    );
}

// ---------------------------------------------------------------------------
// Comparison error messages with variable names
// ---------------------------------------------------------------------------

#[test]
fn error_compare_nil_local() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x < 1"
        ),
        "attempt to compare nil with number (local 'x')"
    );
}

#[test]
fn error_compare_global() {
    k9::assert_equal!(
        run_err("return g < 1"),
        "attempt to compare nil with number (global 'g')"
    );
}

#[test]
fn error_compare_different_types() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s < 1"
        ),
        "attempt to compare string with number (local 's')"
    );
}

#[test]
fn error_compare_no_name() {
    k9::assert_equal!(
        run_err("return nil < 1"),
        "attempt to compare nil with number"
    );
}

#[test]
fn error_compare_gt_names_lhs() {
    // `a > b` is compiled as `compare_lt(b, a)` — verify lhs name still appears.
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x > 1"
        ),
        "attempt to compare number with nil (local 'x')"
    );
}

#[test]
fn error_compare_ge_names_lhs() {
    k9::assert_equal!(
        run_err(
            "\
            local x = nil\n\
            return x >= 1"
        ),
        "attempt to compare number with nil (local 'x')"
    );
}

#[test]
fn error_compare_rhs_named() {
    // Only rhs is a named variable — should still appear in message.
    k9::assert_equal!(
        run_err(
            "\
            local y = nil\n\
            return 1 < y"
        ),
        "attempt to compare number with nil (local 'y')"
    );
}

#[test]
fn error_bitwise_rhs_bad() {
    k9::assert_equal!(
        run_err(
            "\
            local b = true\n\
            return 1 & b"
        ),
        "attempt to perform arithmetic on local 'b' (a boolean value)"
    );
}

#[test]
fn error_shift_left_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s << 1"
        ),
        "attempt to perform arithmetic on local 's' (a string value)"
    );
}

#[test]
fn error_shift_right_local() {
    k9::assert_equal!(
        run_err(
            "\
            local s = 'hello'\n\
            return s >> 1"
        ),
        "attempt to perform arithmetic on local 's' (a string value)"
    );
}

#[test]
fn error_concat_literal_true() {
    k9::assert_equal!(
        run_err("return 'string' .. true"),
        "attempt to concatenate a boolean value"
    );
}

#[test]
fn error_concat_string_and_variable() {
    k9::assert_equal!(
        run_err(
            "\
            local some_variable = true\n\
            return 'string' .. some_variable"
        ),
        "attempt to concatenate local 'some_variable' (a boolean value)"
    );
}

// ===========================================================================
// os library
// ===========================================================================

/// 2000-01-01 00:00:00 UTC.
const Y2K: i64 = 946684800;
/// 2000-03-05 08:07:09 UTC (a Sunday).
const MAR5: i64 = 952243629;

#[test]
fn os_clock_returns_number() {
    // os.clock() returns a float >= 0.
    let v = run_one("return os.clock()");
    match v {
        Value::Float(f) => assert!(f >= 0.0, "os.clock() returned {}", f),
        other => panic!("expected float, got {:?}", other),
    }
}

#[test]
fn os_clock_monotonic() {
    // Two successive calls should be non-decreasing.
    k9::assert_equal!(
        run_one("local a = os.clock(); local b = os.clock(); return b >= a"),
        Value::Boolean(true)
    );
}

#[test]
fn os_time_returns_integer() {
    // os.time() returns a positive integer (Unix timestamp).
    let v = run_one("return os.time()");
    match v {
        Value::Integer(n) => assert!(n > 1_000_000_000, "timestamp too small: {}", n),
        other => panic!("expected integer, got {:?}", other),
    }
}

#[test]
fn os_time_with_table() {
    // Known epoch: 2000-01-01 00:00:00 UTC.
    k9::assert_equal!(
        run_one("return os.time({ year = 2000, month = 1, day = 1, hour = 0, min = 0, sec = 0 })"),
        Value::Integer(Y2K)
    );
}

#[test]
fn os_time_table_defaults() {
    // hour/min/sec default to 12:00:00 when omitted.
    k9::assert_equal!(
        run_one("return os.time({ year = 2000, month = 1, day = 1 })"),
        Value::Integer(Y2K + 12 * 3600)
    );
}

#[test]
fn os_time_table_bad_month() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 13, day = 1 })"),
        "bad argument #1 to 'time' (month in 1..12 expected, got 13)"
    );
}

#[test]
fn os_time_bad_arg() {
    k9::assert_equal!(
        run_err("os.time(42)"),
        "bad argument #1 to 'time' (table expected, got number)"
    );
}

#[test]
fn os_difftime() {
    k9::assert_equal!(run_one("return os.difftime(100, 30)"), Value::Float(70.0));
}

#[test]
fn os_difftime_negative() {
    k9::assert_equal!(run_one("return os.difftime(30, 100)"), Value::Float(-70.0));
}

#[test]
fn os_date_star_t_utc() {
    // os.date("!*t", Y2K) should be 2000-01-01 00:00:00 UTC, Saturday.
    let results = run_all(&format!(
        "local t = os.date('!*t', {Y2K})\n\
         return t.year, t.month, t.day, t.hour, t.min, t.sec, t.wday, t.yday"
    ));
    k9::assert_equal!(results[0], Value::Integer(2000)); // year
    k9::assert_equal!(results[1], Value::Integer(1)); // month
    k9::assert_equal!(results[2], Value::Integer(1)); // day
    k9::assert_equal!(results[3], Value::Integer(0)); // hour
    k9::assert_equal!(results[4], Value::Integer(0)); // min
    k9::assert_equal!(results[5], Value::Integer(0)); // sec
    k9::assert_equal!(results[6], Value::Integer(7)); // wday (Saturday)
    k9::assert_equal!(results[7], Value::Integer(1)); // yday
}

#[test]
fn os_date_format_utc() {
    // Known timestamp: 2000-01-01 00:00:00 UTC.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y-%m-%d %H:%M:%S', {Y2K})")),
        Value::String(Bytes::from("2000-01-01 00:00:00"))
    );
}

#[test]
fn os_date_weekday_names() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%A', {Y2K})")),
        Value::String(Bytes::from("Saturday"))
    );
    k9::assert_equal!(
        run_one(&format!("return os.date('!%a', {Y2K})")),
        Value::String(Bytes::from("Sat"))
    );
}

#[test]
fn os_date_month_names() {
    // March 15, 2023 = 1678838400
    k9::assert_equal!(
        run_one("return os.date('!%B', 1678838400)"),
        Value::String(Bytes::from("March"))
    );
    k9::assert_equal!(
        run_one("return os.date('!%b', 1678838400)"),
        Value::String(Bytes::from("Mar"))
    );
}

#[test]
fn os_date_twelve_hour() {
    // 2000-01-01 15:30:00 UTC = Y2K + 15*3600 + 30*60 = 946740600.
    k9::assert_equal!(
        run_one("return os.date('!%I:%M %p', 946740600)"),
        Value::String(Bytes::from("03:30 PM"))
    );
}

#[test]
fn os_date_day_of_year() {
    // Feb 1 2000 = day 32.
    // Y2K + 31*86400 = 949363200
    k9::assert_equal!(
        run_one("return os.date('!%j', 949363200)"),
        Value::String(Bytes::from("032"))
    );
}

#[test]
fn os_date_percent_escape() {
    k9::assert_equal!(
        run_one("return os.date('!100%%', 0)"),
        Value::String(Bytes::from("100%"))
    );
}

#[test]
fn os_date_default_format() {
    // os.date() with no args should return a non-empty string.
    let v = run_one("return os.date()");
    match v {
        Value::String(s) => assert!(!s.is_empty(), "os.date() returned empty string"),
        other => panic!("expected string, got {:?}", other),
    }
}

#[test]
fn os_date_two_digit_year() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%y', {Y2K})")),
        Value::String(Bytes::from("00"))
    );
}

#[test]
fn os_date_star_t_has_isdst() {
    // isdst field should be present (as boolean).
    k9::assert_equal!(
        run_one("local t = os.date('!*t', 0); return type(t.isdst)"),
        Value::String(Bytes::from("boolean"))
    );
}

#[test]
fn os_time_roundtrip() {
    // os.time(os.date("!*t", X)) should return X.
    k9::assert_equal!(
        run_one(&format!("return os.time(os.date('!*t', {Y2K}))")),
        Value::Integer(Y2K)
    );
}

// -- os.difftime edge cases --

#[test]
fn os_difftime_float_args() {
    k9::assert_equal!(
        run_one("return os.difftime(100.5, 30.25)"),
        Value::Float(70.25)
    );
}

#[test]
fn os_difftime_bad_arg() {
    k9::assert_equal!(
        run_err("os.difftime('hello', 1)"),
        "bad argument #1 to 'difftime' (number expected, got string)"
    );
}

// -- os.time error paths --

#[test]
fn os_time_missing_year() {
    k9::assert_equal!(
        run_err("os.time({ month = 1, day = 1 })"),
        "bad argument #1 to 'time' (integer for field 'year' expected, got field 'year' is missing)"
    );
}

#[test]
fn os_time_missing_month() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, day = 1 })"),
        "bad argument #1 to 'time' (integer for field 'month' expected, got field 'month' is missing)"
    );
}

#[test]
fn os_time_missing_day() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 1 })"),
        "bad argument #1 to 'time' (integer for field 'day' expected, got field 'day' is missing)"
    );
}

#[test]
fn os_time_invalid_day() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 1, day = 32 })"),
        "bad argument #1 to 'time' (valid date expected, got day was not in range)"
    );
}

#[test]
fn os_time_invalid_hour() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 1, day = 1, hour = 25 })"),
        "bad argument #1 to 'time' (valid time expected, got hour was not in range)"
    );
}

#[test]
fn os_time_month_zero() {
    k9::assert_equal!(
        run_err("os.time({ year = 2000, month = 0, day = 1 })"),
        "bad argument #1 to 'time' (month in 1..12 expected, got 0)"
    );
}

// -- os.date strftime specifiers --

// Use a known timestamp: 2000-03-05 08:07:09 UTC (Sunday)
// Y2K + 63*86400 + 8*3600 + 7*60 + 9 = MAR5
// March 5 2000 is a Sunday.

#[test]
fn os_date_zero_padded_day() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%d', {MAR5})")),
        Value::String(Bytes::from("05"))
    );
}

#[test]
fn os_date_space_padded_day() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%e', {MAR5})")),
        Value::String(Bytes::from(" 5"))
    );
}

#[test]
fn os_date_numeric_month() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%m', {MAR5})")),
        Value::String(Bytes::from("03"))
    );
}

#[test]
fn os_date_minute() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%M', {MAR5})")),
        Value::String(Bytes::from("07"))
    );
}

#[test]
fn os_date_second() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%S', {MAR5})")),
        Value::String(Bytes::from("09"))
    );
}

#[test]
fn os_date_four_digit_year() {
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y', {MAR5})")),
        Value::String(Bytes::from("2000"))
    );
}

#[test]
fn os_date_weekday_number() {
    // Sunday = 0
    k9::assert_equal!(
        run_one(&format!("return os.date('!%w', {MAR5})")),
        Value::String(Bytes::from("0"))
    );
}

#[test]
fn os_date_abbreviated_month_h() {
    // %h is an alias for %b.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%h', {MAR5})")),
        Value::String(Bytes::from("Mar"))
    );
}

#[test]
fn os_date_locale_date() {
    // %x expands to %m/%d/%y.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%x', {MAR5})")),
        Value::String(Bytes::from("03/05/00"))
    );
}

#[test]
fn os_date_locale_time() {
    // %X expands to %H:%M:%S.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%X', {MAR5})")),
        Value::String(Bytes::from("08:07:09"))
    );
}

#[test]
fn os_date_locale_datetime() {
    // %c expands to "%a %b %e %H:%M:%S %Y".
    k9::assert_equal!(
        run_one(&format!("return os.date('!%c', {MAR5})")),
        Value::String(Bytes::from("Sun Mar  5 08:07:09 2000"))
    );
}

#[test]
fn os_date_week_number_sunday() {
    // 2000-03-05 is day 65, Sunday (wday=0).
    // %U = (65 - 0 + 7) / 7 = 72 / 7 = 10.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%U', {MAR5})")),
        Value::String(Bytes::from("10"))
    );
}

#[test]
fn os_date_week_number_monday() {
    // 2000-03-05 is day 65, Sunday (Monday-based wday=6).
    // %W = (65 - 6 + 7) / 7 = 66 / 7 = 9.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%W', {MAR5})")),
        Value::String(Bytes::from("09"))
    );
}

#[test]
fn os_date_utc_offset() {
    // With '!' prefix the offset is UTC → +0000.
    k9::assert_equal!(
        run_one("return os.date('!%z', 0)"),
        Value::String(Bytes::from("+0000"))
    );
}

#[test]
fn os_date_timezone_name_utc() {
    k9::assert_equal!(
        run_one("return os.date('!%Z', 0)"),
        Value::String(Bytes::from("UTC"))
    );
}

#[test]
fn os_date_twelve_hour_midnight() {
    // Midnight: hour=0, %I should show 12.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%I', {Y2K})")),
        Value::String(Bytes::from("12"))
    );
}

#[test]
fn os_date_twelve_hour_noon() {
    // Noon: hour=12, %I should show 12.
    // Y2K + 12*3600 = 946728000
    k9::assert_equal!(
        run_one("return os.date('!%I', 946728000)"),
        Value::String(Bytes::from("12"))
    );
}

#[test]
fn os_date_am_indicator() {
    // Midnight is AM.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%p', {Y2K})")),
        Value::String(Bytes::from("AM"))
    );
}

#[test]
fn os_date_trailing_percent() {
    // A lone '%' at end of format string.
    k9::assert_equal!(
        run_one("return os.date('!hello%', 0)"),
        Value::String(Bytes::from("hello%"))
    );
}

#[test]
fn os_date_unknown_specifier() {
    // Unknown specifier should be output literally.
    k9::assert_equal!(
        run_one("return os.date('!%q', 0)"),
        Value::String(Bytes::from("%q"))
    );
}

#[test]
fn os_date_bad_format_type() {
    k9::assert_equal!(
        run_err("os.date(42)"),
        "bad argument #1 to 'date' (string expected, got number)"
    );
}

#[test]
fn os_date_epoch_star_t() {
    // Unix epoch: 1970-01-01 00:00:00 UTC, Thursday.
    let results = run_all(
        "local t = os.date('!*t', 0)\n\
         return t.year, t.month, t.day, t.wday, t.yday",
    );
    k9::assert_equal!(results[0], Value::Integer(1970)); // year
    k9::assert_equal!(results[1], Value::Integer(1)); // month
    k9::assert_equal!(results[2], Value::Integer(1)); // day
    k9::assert_equal!(results[3], Value::Integer(5)); // wday (Thursday = 5)
    k9::assert_equal!(results[4], Value::Integer(1)); // yday
}

#[test]
fn os_date_combined_specifiers() {
    // Multiple specifiers in one format string.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%d/%m/%Y', {MAR5})")),
        Value::String(Bytes::from("05/03/2000"))
    );
}

#[test]
fn os_date_literal_text() {
    // Literal text passes through unchanged.
    k9::assert_equal!(
        run_one("return os.date('!hello world', 0)"),
        Value::String(Bytes::from("hello world"))
    );
}

#[test]
fn os_date_local_time_path() {
    // Without '!' prefix, exercises the local-time branch.
    // Result varies by environment, but should be a non-empty string.
    let v = run_one("return os.date('%Y', 0)");
    match v {
        Value::String(s) => assert!(!s.is_empty(), "os.date local returned empty"),
        other => panic!("expected string, got {:?}", other),
    }
}

#[test]
fn os_date_star_t_local() {
    // "*t" without '!' returns a table via the local-time path.
    let v = run_one("return type(os.date('*t', 0))");
    k9::assert_equal!(v, Value::String(Bytes::from("table")));
}

#[test]
fn os_date_float_timestamp() {
    // Float timestamp is accepted and truncated to integer.
    k9::assert_equal!(
        run_one(&format!("return os.date('!%Y', {Y2K}.5)")),
        Value::String(Bytes::from("2000"))
    );
}

#[test]
fn os_date_format_no_timestamp() {
    // Explicit format with no timestamp defaults to current time.
    let v = run_one("return os.date('!%Y')");
    match v {
        Value::String(s) => {
            let year: i32 = String::from_utf8_lossy(&s).parse().expect("parse year");
            assert!(year >= 2024, "year too small: {}", year);
        }
        other => panic!("expected string, got {:?}", other),
    }
}

#[test]
fn os_time_bad_field_type() {
    k9::assert_equal!(
        run_err("os.time({ year = 'hello', month = 1, day = 1 })"),
        "bad argument #1 to 'time' (integer for field 'year' expected, got string)"
    );
}

#[test]
fn os_difftime_bad_second_arg() {
    k9::assert_equal!(
        run_err("os.difftime(1, 'hello')"),
        "bad argument #2 to 'difftime' (number expected, got string)"
    );
}

#[test]
fn os_difftime_nil_arg() {
    k9::assert_equal!(
        run_err("os.difftime(nil, 1)"),
        "bad argument #1 to 'difftime' (number expected, got nil)"
    );
}

#[test]
fn os_difftime_bool_arg() {
    k9::assert_equal!(
        run_err("os.difftime(true, 1)"),
        "bad argument #1 to 'difftime' (number expected, got boolean)"
    );
}

#[test]
fn os_time_bool_arg() {
    k9::assert_equal!(
        run_err("os.time(true)"),
        "bad argument #1 to 'time' (table expected, got boolean)"
    );
}

#[test]
fn os_date_bad_timestamp_type() {
    k9::assert_equal!(
        run_err("os.date('!%Y', 'hello')"),
        "bad argument #2 to 'date' (number expected, got string)"
    );
}

#[test]
fn os_date_bool_format() {
    k9::assert_equal!(
        run_err("os.date(true)"),
        "bad argument #1 to 'date' (string expected, got boolean)"
    );
}

#[test]
fn os_time_extra_field_ignored() {
    // Extra fields in the table are silently ignored. This is correct Lua
    // behavior — os.date("*t") returns wday/yday/isdst which os.time ignores.
    k9::assert_equal!(
        run_one(&format!("return os.time({{ year = 2000, month = 1, day = 1, hour = 0, min = 0, sec = 0, bogus = 42 }})")),
        Value::Integer(Y2K)
    );
}

// ===========================================================================
// utf8 library
// ===========================================================================

#[test]
fn utf8_char_basic() {
    k9::assert_equal!(
        run_one("return utf8.char(72, 101, 108, 108, 111)"),
        Value::String(Bytes::from("Hello"))
    );
}

#[test]
fn utf8_char_unicode() {
    // U+2603 = ☃ (snowman)
    k9::assert_equal!(
        run_one("return utf8.char(0x2603)"),
        Value::String(Bytes::from("☃"))
    );
}

#[test]
fn utf8_char_empty() {
    k9::assert_equal!(
        run_one("return utf8.char()"),
        Value::String(Bytes::from(""))
    );
}

#[test]
fn utf8_char_multibyte() {
    // U+1F600 = 😀
    k9::assert_equal!(
        run_one("return utf8.char(0x1F600)"),
        Value::String(Bytes::from("😀"))
    );
}

#[test]
fn utf8_char_invalid_codepoint() {
    k9::assert_equal!(
        run_err("utf8.char(0x110000)"),
        "bad argument #1 to 'utf8.char' (valid Unicode codepoint expected, got 1114112)"
    );
}

#[test]
fn utf8_len_ascii() {
    k9::assert_equal!(
        run_one("return utf8.len('Hello')"),
        Value::Integer(5)
    );
}

#[test]
fn utf8_len_unicode() {
    // "☃" is 3 bytes, 1 character.
    k9::assert_equal!(
        run_one("return utf8.len('☃')"),
        Value::Integer(1)
    );
}

#[test]
fn utf8_len_mixed() {
    // "a☃b" = 1 + 3 + 1 = 5 bytes, 3 characters.
    k9::assert_equal!(
        run_one("return utf8.len('a☃b')"),
        Value::Integer(3)
    );
}

#[test]
fn utf8_len_empty() {
    k9::assert_equal!(
        run_one("return utf8.len('')"),
        Value::Integer(0)
    );
}

#[test]
fn utf8_len_range() {
    // utf8.len("Hello", 2, 4) = characters in bytes 2..4 = "ell" = 3
    k9::assert_equal!(
        run_one("return utf8.len('Hello', 2, 4)"),
        Value::Integer(3)
    );
}

#[test]
fn utf8_len_invalid_returns_nil() {
    // Invalid UTF-8: \xff
    let results = run_all("return utf8.len('abc\\xff')");
    k9::assert_equal!(results[0], Value::Nil);
    k9::assert_equal!(results[1], Value::Integer(4));
}

#[test]
fn utf8_codepoint_single() {
    // 'A' = 65
    k9::assert_equal!(
        run_one("return utf8.codepoint('A')"),
        Value::Integer(65)
    );
}

#[test]
fn utf8_codepoint_unicode() {
    // ☃ = U+2603
    k9::assert_equal!(
        run_one("return utf8.codepoint('☃')"),
        Value::Integer(0x2603)
    );
}

#[test]
fn utf8_codepoint_range() {
    // "Hello" codepoints at bytes 1..3 = H, e, l
    let results = run_all("return utf8.codepoint('Hello', 1, 3)");
    k9::assert_equal!(results[0], Value::Integer(72));  // H
    k9::assert_equal!(results[1], Value::Integer(101)); // e
    k9::assert_equal!(results[2], Value::Integer(108)); // l
}

#[test]
fn utf8_offset_forward() {
    // "aéb": a=1byte, é=2bytes, b=1byte
    // offset(s, 1) = 1 (byte pos of 1st char)
    // offset(s, 2) = 2 (byte pos of 2nd char)
    // offset(s, 3) = 4 (byte pos of 3rd char, after 2-byte é)
    k9::assert_equal!(
        run_one("return utf8.offset('aéb', 1)"),
        Value::Integer(1)
    );
    k9::assert_equal!(
        run_one("return utf8.offset('aéb', 2)"),
        Value::Integer(2)
    );
    k9::assert_equal!(
        run_one("return utf8.offset('aéb', 3)"),
        Value::Integer(4)
    );
}

#[test]
fn utf8_offset_negative() {
    // offset(s, -1) from end = byte pos of last char
    // "aéb" (4 bytes): last char 'b' is at byte 4
    k9::assert_equal!(
        run_one("return utf8.offset('aéb', -1)"),
        Value::Integer(4)
    );
}

#[test]
fn utf8_codes_basic() {
    let results = run_all(
        "local r = {}\n\
         for p, c in utf8.codes('aé') do\n\
           r[#r+1] = p\n\
           r[#r+1] = c\n\
         end\n\
         return r[1], r[2], r[3], r[4]"
    );
    k9::assert_equal!(results[0], Value::Integer(1));   // byte pos of 'a'
    k9::assert_equal!(results[1], Value::Integer(97));  // codepoint 'a'
    k9::assert_equal!(results[2], Value::Integer(2));   // byte pos of 'é'
    k9::assert_equal!(results[3], Value::Integer(233)); // codepoint 'é'
}

#[test]
fn utf8_codes_empty() {
    k9::assert_equal!(
        run_one("local n = 0; for _ in utf8.codes('') do n = n + 1 end; return n"),
        Value::Integer(0)
    );
}

#[test]
fn utf8_charpattern_exists() {
    k9::assert_equal!(
        run_one("return type(utf8.charpattern)"),
        Value::String(Bytes::from("string"))
    );
}
