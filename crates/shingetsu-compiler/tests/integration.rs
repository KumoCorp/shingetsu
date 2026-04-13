use shingetsu_compiler::{compile, CompileOptions, Dialect};
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

/// Compile and run a LuaU snippet, returning the first return value.
fn run_one_luau(src: &str) -> Value {
    let opts = CompileOptions {
        dialect: Dialect::LuaU,
        ..CompileOptions::default()
    };
    let bc = compile(src, &opts).expect("compile failed");
    let env = GlobalEnv::new();
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

/// Compile, run, and return all return values (LuaU dialect).
fn run_all_luau(src: &str) -> Vec<Value> {
    let opts = CompileOptions {
        dialect: Dialect::LuaU,
        ..CompileOptions::default()
    };
    let bc = compile(src, &opts).expect("compile failed");
    let env = GlobalEnv::new();
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env, func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed")
}

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
    use shingetsu_vm::{GlobalEnv, Task, Value};
    use std::sync::Arc;

    let env = GlobalEnv::new();
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
    use shingetsu_vm::{GlobalEnv, NativeFunction, Task, Value, VmError};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let finalized = Arc::new(AtomicBool::new(false));
    let env = GlobalEnv::new();

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
    use shingetsu_vm::{GlobalEnv, NativeFunction, Task, Value, VmError};
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    // Register a native that blocks forever (simulates I/O or sleep).
    let env = GlobalEnv::new();
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
            env.get_global(b"closed").unwrap_or(Value::Nil),
            Value::Integer(1)
        );
    });
}

#[test]
fn task_dispose_no_close_vars_is_noop() {
    // dispose() on a task with no <close> variables should complete cleanly.
    let env = GlobalEnv::new();
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
    use shingetsu::downcast_rs::DowncastSync;
    use shingetsu::{UserData, Value};
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
    use shingetsu::{userdata, Function, GlobalEnv, Task, Value};
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

    let env = GlobalEnv::new();
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
    use shingetsu::{module, Function, GlobalEnv, Task, Value};
    use shingetsu_compiler::{compile, CompileOptions, Dialect};

    #[module]
    mod testmod {
        #[function]
        fn add(a: i64, b: i64) -> i64 {
            a + b
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value};
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

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value};
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

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn multiply(&self, factor: i64) -> i64 {
            self.0 * factor
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_method]
        fn doubled(self: Arc<Self>) -> i64 {
            self.0 * 2
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value, VmError};
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

    let env = GlobalEnv::new();
    env.set_global("n", Value::Userdata(Arc::new(Num(42))));
    let res = run_with_env(env, "return n:checked_div(6)");
    k9::assert_equal!(res[0], Value::Integer(7));
}

#[test]
fn userdata_macro_method_result_err() {
    // A method with Result return — Err path surfaces as a Lua error.
    use shingetsu::{userdata, GlobalEnv, Value, VmError};
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

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, CallContext, GlobalEnv, Value};
    use std::sync::Arc;

    struct Doubler;

    #[userdata]
    impl Doubler {
        #[lua_method]
        fn run(&self, _ctx: CallContext, n: i64) -> i64 {
            n * 2
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value, Variadic};
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

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value};
    use std::sync::Arc;

    struct Named(String);

    #[userdata]
    impl Named {
        #[lua_metamethod(ToString)]
        fn to_str(&self) -> String {
            self.0.clone()
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, CallContext, GlobalEnv, Value, VmError};
    use std::sync::Arc;

    struct Num(i64);

    #[userdata]
    impl Num {
        #[lua_metamethod(Add)]
        fn add_mm(&self, rhs: i64) -> i64 {
            self.0 + rhs
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{module, GlobalEnv, Value};

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

    let env = GlobalEnv::new();
    mathmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return mathmod.checked_sqrt(4.0)");
    k9::assert_equal!(res[0], Value::Float(2.0));
}

// ---------------------------------------------------------------------------
// Module macro: async function
// ---------------------------------------------------------------------------

#[test]
fn module_macro_async_fn() {
    use shingetsu::{module, GlobalEnv, Value};

    #[module]
    mod asyncmod {
        #[function]
        async fn async_double(n: i64) -> i64 {
            n * 2
        }
    }

    let env = GlobalEnv::new();
    asyncmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return asyncmod.async_double(21)");
    k9::assert_equal!(res[0], Value::Integer(42));
}

// ---------------------------------------------------------------------------
// Module macro: function with CallContext parameter
// ---------------------------------------------------------------------------

#[test]
fn module_macro_callcontext() {
    use shingetsu::{module, GlobalEnv, Value};

    #[module]
    mod ctxmod {
        use shingetsu::CallContext;

        #[function]
        fn passthrough(_ctx: CallContext, n: i64) -> i64 {
            n
        }
    }

    let env = GlobalEnv::new();
    ctxmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return ctxmod.passthrough(99)");
    k9::assert_equal!(res[0], Value::Integer(99));
}

// ---------------------------------------------------------------------------
// Module macro: function with Variadic parameter
// ---------------------------------------------------------------------------

#[test]
fn module_macro_variadic() {
    use shingetsu::{module, GlobalEnv, Value};

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

    let env = GlobalEnv::new();
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
    use shingetsu::{module, GlobalEnv, Value};

    #[module]
    mod constmod {
        #[field]
        fn magic() -> i64 {
            42
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{module, GlobalEnv, Value};

    #[module]
    mod renmod {
        #[function(rename = "doThing")]
        fn do_thing(n: i64) -> i64 {
            n + 1
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{module, GlobalEnv, Value};

    #[module(name = "myMod")]
    mod internal {
        #[function]
        fn hello() -> i64 {
            1
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value};
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

    let env = GlobalEnv::new();
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
    use shingetsu::{userdata, GlobalEnv, Value};
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

    let env = GlobalEnv::new();
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
    use shingetsu::{module, GlobalEnv, Value};

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

    let env = GlobalEnv::new();
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
    use shingetsu::{module, GlobalEnv, Value};

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

    let env = GlobalEnv::new();
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
    use shingetsu::{module, FromLuaMulti, GlobalEnv, Value, Variadic};

    #[module]
    mod swapmod {
        use shingetsu::{Value, Variadic};

        #[function]
        fn swap(a: i64, b: i64) -> Variadic {
            Variadic(vec![Value::Integer(b), Value::Integer(a)])
        }
    }

    let env = GlobalEnv::new();
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
    use shingetsu::{module, FromLuaMulti, GlobalEnv, Value};

    #[module]
    mod divmod {
        #[function]
        fn divmod(a: i64, b: i64) -> (i64, i64) {
            (a / b, a % b)
        }
    }

    let env = GlobalEnv::new();
    divmod::register_global_module(&env).expect("register");
    let res = run_with_env(env, "return divmod.divmod(10, 3)");

    // Arity check on the raw Vec.
    k9::assert_equal!(res.len(), 2);

    // Typed extraction via FromLuaMulti.
    let (q, r) = <(i64, i64)>::from_lua_multi(res).expect("from_lua_multi");
    k9::assert_equal!(q, 3);
    k9::assert_equal!(r, 1);
}
