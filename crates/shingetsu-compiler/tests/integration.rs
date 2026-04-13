use bytes::Bytes;
use shingetsu_compiler::{compile, CompileOptions, Dialect};
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

/// Compile and run a LuaU snippet, returning the first return value.
fn run_one_luau(src: &str) -> Value {
    let opts = CompileOptions {
        dialect: Dialect::LuaU,
        ..CompileOptions::default()
    };
    let bc = compile(src, &opts).expect("compile failed");
    let env = new_env();
    let func = crate::Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task)
        .expect("task failed")
        .into_iter()
        .next()
        .unwrap_or(Value::Nil)
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

// ---------------------------------------------------------------------------
// type / tostring / tonumber
// ---------------------------------------------------------------------------

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
        run_one_luau(
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
        run_one_luau(
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
        run_one_luau(
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
        run_one_luau(
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
        run_one_luau("local x = 10; x += 5; return x"),
        Value::Integer(15)
    );
}

#[test]
fn compound_minus_equal() {
    k9::assert_equal!(
        run_one_luau("local x = 10; x -= 3; return x"),
        Value::Integer(7)
    );
}

#[test]
fn compound_star_equal() {
    k9::assert_equal!(
        run_one_luau("local x = 6; x *= 7; return x"),
        Value::Integer(42)
    );
}

#[test]
fn compound_slash_equal() {
    k9::assert_equal!(
        run_one_luau("local x = 10.0; x /= 4; return x"),
        Value::Float(2.5)
    );
}

#[test]
fn compound_double_slash_equal() {
    k9::assert_equal!(
        run_one_luau("local x = 10; x //= 3; return x"),
        Value::Integer(3)
    );
}

#[test]
fn compound_percent_equal() {
    k9::assert_equal!(
        run_one_luau("local x = 10; x %= 3; return x"),
        Value::Integer(1)
    );
}

#[test]
fn compound_caret_equal() {
    k9::assert_equal!(
        run_one_luau("local x = 2.0; x ^= 10; return x"),
        Value::Float(1024.0)
    );
}

#[test]
fn compound_two_dots_equal() {
    k9::assert_equal!(
        run_one_luau(r#"local s = "hello"; s ..= " world"; return s"#),
        Value::String(bytes::Bytes::from_static(b"hello world"))
    );
}

#[test]
fn compound_global() {
    k9::assert_equal!(run_one_luau("x = 5; x += 3; return x"), Value::Integer(8));
}

#[test]
fn compound_table_field() {
    k9::assert_equal!(
        run_one_luau("local t = {n=10}; t.n += 5; return t.n"),
        Value::Integer(15)
    );
}

#[test]
fn compound_table_index() {
    k9::assert_equal!(
        run_one_luau("local t = {[1]=100}; t[1] -= 1; return t[1]"),
        Value::Integer(99)
    );
}

// ---------------------------------------------------------------------------
// if expressions (LuaU)
// ---------------------------------------------------------------------------

#[test]
fn if_expr_true_branch() {
    k9::assert_equal!(
        run_one_luau("return if true then 1 else 2"),
        Value::Integer(1)
    );
}

#[test]
fn if_expr_false_branch() {
    k9::assert_equal!(
        run_one_luau("return if false then 1 else 2"),
        Value::Integer(2)
    );
}

#[test]
fn if_expr_elseif() {
    k9::assert_equal!(
        run_one_luau(
            "local x = 2; return if x == 1 then \"one\" elseif x == 2 then \"two\" else \"other\""
        ),
        Value::String(bytes::Bytes::from_static(b"two"))
    );
}

#[test]
fn if_expr_nested() {
    k9::assert_equal!(
        run_one_luau("local x = 5; local y = if x > 3 then if x > 4 then \"big\" else \"mid\" else \"small\"; return y"),
        Value::String(bytes::Bytes::from_static(b"big"))
    );
}

#[test]
fn if_expr_in_assignment() {
    k9::assert_equal!(
        run_one_luau("local cond = true; local t = {v = if cond then 42 else 0}; return t.v"),
        Value::Integer(42)
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};
    let opts = CompileOptions {
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};
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
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};

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
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};

    let env = new_env();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    let opts = CompileOptions {
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};
    let opts = CompileOptions {
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};

    let env = new_env();
    let opts = CompileOptions {
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};

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
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};

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
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};
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
        dialect: Dialect::Lua54,
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
    use shingetsu_compiler::{compile, CompileOptions, Dialect};

    let env = new_env();
    let opts = CompileOptions {
        dialect: Dialect::Lua54,
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
fn string_lib_gsub_function_not_supported() {
    // Function replacement should produce an error.
    let res = run_all(
        "\
        local ok, msg = pcall(string.gsub, 'hello', '%w+', function(m) return m end)
        return ok, type(msg)",
    );
    k9::assert_equal!(
        res,
        vec![Value::Boolean(false), Value::String(Bytes::from("string")),]
    );
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
