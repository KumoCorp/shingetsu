mod common;

use common::{run_all, run_err, run_one};
use shingetsu_vm::Value;

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
// __metatable protection (Lua 5.2+)
// ---------------------------------------------------------------------------

#[test]
fn setmetatable_rejects_protected_metatable() {
    // When the current metatable has `__metatable`, setmetatable must
    // refuse to replace it.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = "locked"})
setmetatable(t, {})"#,
    );
    k9::assert_equal!(err, "cannot change a protected metatable");
}

#[test]
fn setmetatable_protection_applies_to_nil_replacement() {
    // Even setting the metatable to nil is rejected.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = "locked"})
setmetatable(t, nil)"#,
    );
    k9::assert_equal!(err, "cannot change a protected metatable");
}

#[test]
fn setmetatable_protection_accepts_false_as_guard() {
    // Any non-nil `__metatable` value protects, including `false`.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = false})
setmetatable(t, {})"#,
    );
    k9::assert_equal!(err, "cannot change a protected metatable");
}

#[test]
fn setmetatable_no_metatable_is_unprotected() {
    // A table without any metatable has nothing to protect — setmetatable
    // proceeds normally.
    let res = run_one(
        "local t = {}\n\
         setmetatable(t, {x = 1})\n\
         local got = getmetatable(t)\n\
         return got.x",
    );
    k9::assert_equal!(res, Value::Integer(1));
}

#[test]
fn setmetatable_metatable_without_guard_allows_replacement() {
    // Having a metatable doesn't protect it — only `__metatable` does.
    let res = run_one(
        "local t = setmetatable({}, {__index = function() end})\n\
         setmetatable(t, {x = 2})\n\
         local got = getmetatable(t)\n\
         return got.x",
    );
    k9::assert_equal!(res, Value::Integer(2));
}

#[test]
fn getmetatable_returns_protection_value() {
    // Already covered elsewhere, but include here for the symmetry pair.
    let res = run_one(
        r#"local t = setmetatable({}, {__metatable = "hidden"})
return getmetatable(t)"#,
    );
    k9::assert_equal!(res, Value::String(bytes::Bytes::from_static(b"hidden")));
}

#[test]
fn setmetatable_protection_precedes_freeze_error() {
    // If both a __metatable guard and freeze apply, the protection
    // message is the one surfaced — the guard is the more specific
    // user-level contract.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = "locked"})
table.freeze(t)
setmetatable(t, {})"#,
    );
    k9::assert_equal!(err, "cannot change a protected metatable");
}
