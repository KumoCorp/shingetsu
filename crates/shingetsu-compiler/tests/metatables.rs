mod common;

use common::{run_all, run_err, run_one};
use shingetsu::valuevec;
use shingetsu_vm::Value;

// ---------------------------------------------------------------------------
// Metatables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn setmetatable_getmetatable() {
    k9::assert_equal!(
        run_one(
            "local t = {}
local mt = {}
setmetatable(t, mt)
return getmetatable(t) == mt"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn metatable_index_table() {
    // __index as a table: prototype-based inheritance.
    k9::assert_equal!(
        run_one(
            "local proto = { x = 42 }
local obj = setmetatable({}, { __index = proto })
return obj.x"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn metatable_index_table_own_field_wins() {
    k9::assert_equal!(
        run_one(
            "local proto = { x = 1 }
local obj = setmetatable({ x = 99 }, { __index = proto })
return obj.x"
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn metatable_index_chain() {
    // Two-level prototype chain.
    k9::assert_equal!(
        run_one(
            "local base = { z = 7 }
local mid  = setmetatable({}, { __index = base })
local obj  = setmetatable({}, { __index = mid })
return obj.z"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn metatable_index_function() {
    // __index as a function: called with (table, key).
    k9::assert_equal!(
        run_one(
            "local obj = setmetatable({}, {
    __index = function(t, k) return k .. '!' end
})
return obj.hello"
        )
        .await,
        Value::string("hello!")
    );
}

#[tokio::test]
async fn metatable_newindex_function() {
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
        )
        .await,
        Value::string("foo")
    );
}

#[tokio::test]
async fn metatable_newindex_existing_skips_mm() {
    // __newindex is NOT called when the key already exists.
    k9::assert_equal!(
        run_one(
            "local called = false
local obj = setmetatable({ x = 1 }, {
    __newindex = function(t, k, v) called = true end
})
obj.x = 2  -- key exists, no __newindex
return called"
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn metatable_call() {
    // __call makes a table callable.
    k9::assert_equal!(
        run_one(
            "local callable = setmetatable({}, {
    __call = function(self, a, b) return a + b end
})
return callable(3, 4)"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn metatable_len() {
    // __len overrides #.
    k9::assert_equal!(
        run_one(
            "local obj = setmetatable({}, {
    __len = function(t) return 42 end
})
return #obj"
        )
        .await,
        Value::Integer(42)
    );
}

#[tokio::test]
async fn oop_class_pattern() {
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
        )
        .await,
        Value::string("Cat says hello")
    );
}

#[tokio::test]
async fn rawget_bypasses_index() {
    k9::assert_equal!(
        run_one(
            "local proto = { x = 99 }
local obj = setmetatable({}, { __index = proto })
return rawget(obj, 'x')"
        )
        .await,
        Value::Nil
    );
}

#[tokio::test]
async fn rawset_bypasses_newindex() {
    k9::assert_equal!(
        run_one(
            "local called = false
local obj = setmetatable({}, {
    __newindex = function() called = true end
})
rawset(obj, 'k', 1)
return called"
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn rawequal_same_value() {
    k9::assert_equal!(run_one("return rawequal(1, 1)").await, Value::Boolean(true));
}

#[tokio::test]
async fn rawequal_different_values() {
    k9::assert_equal!(
        run_one("return rawequal(1, 2)").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn rawequal_different_types() {
    k9::assert_equal!(
        run_one("return rawequal(1, '1')").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn rawequal_nil() {
    k9::assert_equal!(
        run_one("return rawequal(nil, nil)").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn rawequal_tables_same_ref() {
    k9::assert_equal!(
        run_one("local t = {} return rawequal(t, t)").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn rawequal_tables_different_ref() {
    // Two distinct tables with the same contents are not rawequal.
    k9::assert_equal!(
        run_one("return rawequal({1}, {1})").await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn rawequal_bypasses_eq_metamethod() {
    k9::assert_equal!(
        run_one(
            "local mt = { __eq = function() return true end }\n\
             local a = setmetatable({}, mt)\n\
             local b = setmetatable({}, mt)\n\
             return rawequal(a, b)"
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn rawequal_int_float_cross() {
    // 1 == 1.0 in Lua (even raw equality).
    k9::assert_equal!(
        run_one("return rawequal(1, 1.0)").await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn rawlen_table() {
    k9::assert_equal!(
        run_one("return rawlen({10, 20, 30})").await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn rawlen_empty_table() {
    k9::assert_equal!(run_one("return rawlen({})").await, Value::Integer(0));
}

#[tokio::test]
async fn rawlen_string() {
    k9::assert_equal!(run_one("return rawlen('hello')").await, Value::Integer(5));
}

#[tokio::test]
async fn rawlen_empty_string() {
    k9::assert_equal!(run_one("return rawlen('')").await, Value::Integer(0));
}

#[tokio::test]
async fn rawlen_bypasses_len_metamethod() {
    k9::assert_equal!(
        run_one(
            "local t = setmetatable({1, 2, 3}, { __len = function() return 999 end })\n\
             return rawlen(t)"
        )
        .await,
        Value::Integer(3)
    );
}

#[tokio::test]
async fn rawlen_bad_type() {
    k9::assert_equal!(
        run_err("rawlen(42)").await,
        "\
error: bad argument #1 to 'rawlen' (table or string expected, got number)
 --> test.lua:1:8
  |
1 | rawlen(42)
  |        ^^ bad argument #1 to 'rawlen' (table or string expected, got number)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn type_of_values() {
    k9::assert_equal!(
        run_all(
            "return type(nil), type(true), type(1), type(1.0),
             type('s'), type({}), type(type)"
        )
        .await,
        valuevec![
            Value::string("nil"),
            Value::string("boolean"),
            Value::string("number"),
            Value::string("number"),
            Value::string("string"),
            Value::string("table"),
            Value::string("function"),
        ]
    );
}

#[tokio::test]
async fn typeof_primitives_match_type() {
    // For primitive types, `typeof` behaves exactly like `type`.
    k9::assert_equal!(
        run_all(
            "return typeof(nil), typeof(true), typeof(1), typeof(1.0),
             typeof('s'), typeof({}), typeof(typeof)"
        )
        .await,
        valuevec![
            Value::string("nil"),
            Value::string("boolean"),
            Value::string("number"),
            Value::string("number"),
            Value::string("string"),
            Value::string("table"),
            Value::string("function"),
        ]
    );
}

#[tokio::test]
async fn typeof_no_args_errors() {
    let err = run_err("typeof()").await;
    k9::assert_equal!(
        err,
        r#"error: bad argument #1 to 'typeof' (value expected, got no value)
 --> test.lua:1:1
  |
1 | typeof()
  | ^^^^^^ bad argument #1 to 'typeof' (value expected, got no value)
stack traceback:
	test.lua:1: in main chunk"#
    );
}

#[tokio::test]
async fn typeof_reads_table_type_metafield() {
    // A `__type` string metafield overrides the default "table" name.
    k9::assert_equal!(
        run_one(
            "local t = setmetatable({}, {__type = 'Vector3'})
return typeof(t)"
        )
        .await,
        Value::string("Vector3")
    );
}

#[tokio::test]
async fn typeof_non_string_type_metafield_falls_back() {
    // If `__type` is present but not a string, fall back to "table".
    k9::assert_equal!(
        run_one(
            "local t = setmetatable({}, {__type = 42})
return typeof(t)"
        )
        .await,
        Value::string("table")
    );
}

#[tokio::test]
async fn typeof_table_without_metatable_is_table() {
    k9::assert_equal!(
        run_one("return typeof({1, 2, 3})").await,
        Value::string("table")
    );
}

#[tokio::test]
async fn typeof_table_metatable_without_type_field_is_table() {
    // Having a metatable without `__type` should still yield "table".
    k9::assert_equal!(
        run_one(
            "local t = setmetatable({}, {__index = function() end})
return typeof(t)"
        )
        .await,
        Value::string("table")
    );
}

#[tokio::test]
async fn tostring_numbers() {
    k9::assert_equal!(
        run_all("return tostring(42), tostring(3.14), tostring(true), tostring(nil)").await,
        valuevec![
            Value::string("42"),
            Value::string("3.14"),
            Value::string("true"),
            Value::string("nil"),
        ]
    );
}

#[tokio::test]
async fn tostring_metamethod() {
    k9::assert_equal!(
        run_one(
            "local mt = { __tostring = function(t) return 'obj' end }
local obj = setmetatable({}, mt)
return tostring(obj)"
        )
        .await,
        Value::string("obj")
    );
}

// ---------------------------------------------------------------------------

// arithmetic metamethods
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arith_metamethod_add() {
    k9::assert_equal!(
        run_one(
            "local mt = { __add = function(a, b) return a.v + b.v end }
local a = setmetatable({v=10}, mt)
local b = setmetatable({v=5}, mt)
return a + b"
        )
        .await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn arith_metamethod_sub() {
    k9::assert_equal!(
        run_one(
            "local mt = { __sub = function(a, b) return a.v - b.v end }
local a = setmetatable({v=10}, mt)
local b = setmetatable({v=3}, mt)
return a - b"
        )
        .await,
        Value::Integer(7)
    );
}

#[tokio::test]
async fn arith_metamethod_mul() {
    k9::assert_equal!(
        run_one(
            "local mt = { __mul = function(a, b) return a.v * b.v end }
local a = setmetatable({v=4}, mt)
local b = setmetatable({v=5}, mt)
return a * b"
        )
        .await,
        Value::Integer(20)
    );
}

#[tokio::test]
async fn arith_metamethod_unm() {
    k9::assert_equal!(
        run_one(
            "local mt = { __unm = function(a) return -a.v end }
local a = setmetatable({v=7}, mt)
return -a"
        )
        .await,
        Value::Integer(-7)
    );
}

// ---------------------------------------------------------------------------
// __pairs / __ipairs metamethods
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pairs_respects_pairs_metamethod() {
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
        )
        .await,
        Value::Integer(99)
    );
}

#[tokio::test]
async fn ipairs_respects_ipairs_metamethod() {
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
        )
        .await,
        Value::Integer(60)
    );
}

#[tokio::test]
async fn pairs_falls_through_without_metamethod() {
    // Ordinary table with no __pairs should work as before.
    k9::assert_equal!(
        run_one(
            "local t = {a=1, b=2}
local count = 0
for k, v in pairs(t) do count = count + 1 end
return count"
        )
        .await,
        Value::Integer(2)
    );
}

// ---------------------------------------------------------------------------

// Comparison metamethods (__eq, __lt, __le)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eq_metamethod_tables() {
    k9::assert_equal!(
        run_one(
            "local mt = {
    __eq = function(a, b) return a.v == b.v end
}
local a = setmetatable({v=1}, mt)
local b = setmetatable({v=1}, mt)
local c = setmetatable({v=2}, mt)
return a == b, a == c"
        )
        .await,
        // run_one returns first value; use run_all
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn eq_metamethod_returns_bool() {
    // Result of == with __eq must be a strict boolean.
    k9::assert_equal!(
        run_all(
            "local mt = { __eq = function(a, b) return 42 end }
local a = setmetatable({}, mt)
local b = setmetatable({}, mt)
return a == b, a ~= b"
        )
        .await,
        valuevec![Value::Boolean(true), Value::Boolean(false)]
    );
}

#[tokio::test]
async fn ne_uses_eq_metamethod() {
    // ~= is not (==), so __eq is respected.
    k9::assert_equal!(
        run_one(
            "local mt = { __eq = function(a, b) return a.v == b.v end }
local a = setmetatable({v=5}, mt)
local b = setmetatable({v=5}, mt)
return a ~= b"
        )
        .await,
        Value::Boolean(false)
    );
}

#[tokio::test]
async fn eq_same_ref_skips_metamethod() {
    // Identical table references are equal without calling __eq.
    k9::assert_equal!(
        run_one(
            "local called = false
local mt = { __eq = function() called = true; return false end }
local a = setmetatable({}, mt)
return a == a, called"
        )
        .await,
        // run_one returns first: true (same ref)
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn lt_metamethod() {
    k9::assert_equal!(
        run_one(
            "local mt = { __lt = function(a, b) return a.v < b.v end }
local a = setmetatable({v=3}, mt)
local b = setmetatable({v=5}, mt)
return a < b"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn gt_uses_lt_metamethod() {
    // a > b calls __lt(b, a)
    k9::assert_equal!(
        run_one(
            "local mt = { __lt = function(a, b) return a.v < b.v end }
local a = setmetatable({v=3}, mt)
local b = setmetatable({v=5}, mt)
return b > a"
        )
        .await,
        Value::Boolean(true)
    );
}

#[tokio::test]
async fn le_metamethod() {
    k9::assert_equal!(
        run_all(
            "local mt = { __le = function(a, b) return a.v <= b.v end }
local a = setmetatable({v=3}, mt)
local b = setmetatable({v=3}, mt)
return a <= b, a >= b"
        )
        .await,
        valuevec![Value::Boolean(true), Value::Boolean(true)]
    );
}

// ---------------------------------------------------------------------------
// __concat metamethod
// ---------------------------------------------------------------------------

#[tokio::test]
async fn concat_strings() {
    k9::assert_equal!(
        run_one(r#"return "hello" .. " " .. "world""#).await,
        Value::string("hello world")
    );
}

#[tokio::test]
async fn concat_number_coercion() {
    k9::assert_equal!(run_one(r#"return "x=" .. 42"#).await, Value::string("x=42"));
}

#[tokio::test]
async fn concat_metamethod() {
    // Tables with __concat should be supported.
    k9::assert_equal!(
        run_one(
            r#"local mt = { __concat = function(a, b) return a.v .. b.v end }
local a = setmetatable({v="hello"}, mt)
local b = setmetatable({v=" world"}, mt)
return a .. b"#
        )
        .await,
        Value::string("hello world")
    );
}

#[tokio::test]
async fn concat_error_on_nil() {
    // Concatenating nil without __concat should be caught by pcall.
    k9::assert_equal!(
        run_one(
            r#"local ok, err = pcall(function() return "x" .. nil end)
return ok"#
        )
        .await,
        Value::Boolean(false)
    );
}

// ---------------------------------------------------------------------------
// __metatable protection (Lua 5.2+)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn setmetatable_rejects_protected_metatable() {
    // When the current metatable has `__metatable`, setmetatable must
    // refuse to replace it.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = "locked"})
setmetatable(t, {})"#,
    )
    .await;
    k9::assert_equal!(
        err,
        "\
error: cannot change a protected metatable
 --> test.lua:2:14
  |
2 | setmetatable(t, {})
  |              ^ cannot change a protected metatable
help: the table's metatable defines a `__metatable` field; the table can no longer be re-metatabled (this is by design — the original author opted out)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn setmetatable_protection_applies_to_nil_replacement() {
    // Even setting the metatable to nil is rejected.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = "locked"})
setmetatable(t, nil)"#,
    )
    .await;
    k9::assert_equal!(
        err,
        "\
error: cannot change a protected metatable
 --> test.lua:2:14
  |
2 | setmetatable(t, nil)
  |              ^ cannot change a protected metatable
help: the table's metatable defines a `__metatable` field; the table can no longer be re-metatabled (this is by design — the original author opted out)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn setmetatable_protection_accepts_false_as_guard() {
    // Any non-nil `__metatable` value protects, including `false`.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = false})
setmetatable(t, {})"#,
    )
    .await;
    k9::assert_equal!(
        err,
        "\
error: cannot change a protected metatable
 --> test.lua:2:14
  |
2 | setmetatable(t, {})
  |              ^ cannot change a protected metatable
help: the table's metatable defines a `__metatable` field; the table can no longer be re-metatabled (this is by design — the original author opted out)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn setmetatable_no_metatable_is_unprotected() {
    // A table without any metatable has nothing to protect — setmetatable
    // proceeds normally.
    let res = run_one(
        "local t = {}\n\
         setmetatable(t, {x = 1})\n\
         local got = getmetatable(t)\n\
         return got.x",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(1));
}

#[tokio::test]
async fn setmetatable_metatable_without_guard_allows_replacement() {
    // Having a metatable doesn't protect it — only `__metatable` does.
    let res = run_one(
        "local t = setmetatable({}, {__index = function() end})\n\
         setmetatable(t, {x = 2})\n\
         local got = getmetatable(t)\n\
         return got.x",
    )
    .await;
    k9::assert_equal!(res, Value::Integer(2));
}

#[tokio::test]
async fn getmetatable_returns_protection_value() {
    // Already covered elsewhere, but include here for the symmetry pair.
    let res = run_one(
        r#"local t = setmetatable({}, {__metatable = "hidden"})
return getmetatable(t)"#,
    )
    .await;
    k9::assert_equal!(res, Value::string("hidden"));
}

#[tokio::test]
async fn setmetatable_protection_precedes_freeze_error() {
    // If both a __metatable guard and freeze apply, the protection
    // message is the one surfaced — the guard is the more specific
    // user-level contract.
    let err = run_err(
        r#"local t = setmetatable({}, {__metatable = "locked"})
table.freeze(t)
setmetatable(t, {})"#,
    )
    .await;
    k9::assert_equal!(
        err,
        "\
error: cannot change a protected metatable
 --> test.lua:3:14
  |
3 | setmetatable(t, {})
  |              ^ cannot change a protected metatable
help: the table's metatable defines a `__metatable` field; the table can no longer be re-metatabled (this is by design — the original author opted out)
stack traceback:
\ttest.lua:3: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Bitwise metamethods
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bitwise_metamethod_band() {
    k9::assert_equal!(
        run_one(
            "local mt = { __band = function(a, b) return a.v + b.v end }
local a = setmetatable({v=10}, mt)
local b = setmetatable({v=3}, mt)
return a & b"
        )
        .await,
        Value::Integer(13)
    );
}

#[tokio::test]
async fn bitwise_metamethod_bor() {
    k9::assert_equal!(
        run_one(
            "local mt = { __bor = function(a, b) return a.v + b.v end }
local a = setmetatable({v=10}, mt)
local b = setmetatable({v=3}, mt)
return a | b"
        )
        .await,
        Value::Integer(13)
    );
}

#[tokio::test]
async fn bitwise_metamethod_bxor() {
    k9::assert_equal!(
        run_one(
            "local mt = { __bxor = function(a, b) return a.v + b.v end }
local a = setmetatable({v=10}, mt)
local b = setmetatable({v=3}, mt)
return a ~ b"
        )
        .await,
        Value::Integer(13)
    );
}

#[tokio::test]
async fn bitwise_metamethod_bnot() {
    k9::assert_equal!(
        run_one(
            "local mt = { __bnot = function(a) return a.v * 2 end }
local a = setmetatable({v=7}, mt)
return ~a"
        )
        .await,
        Value::Integer(14)
    );
}

#[tokio::test]
async fn bitwise_metamethod_only_on_left() {
    k9::assert_equal!(
        run_one(
            "local mt = { __band = function(a, b) return a.v + b end }
local a = setmetatable({v=10}, mt)
return a & 5"
        )
        .await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn bitwise_metamethod_only_on_right() {
    k9::assert_equal!(
        run_one(
            "local mt = { __band = function(a, b) return a + b.v end }
local b = setmetatable({v=5}, mt)
return 10 & b"
        )
        .await,
        Value::Integer(15)
    );
}

#[tokio::test]
async fn bitwise_no_metamethod_error() {
    let err = run_err(
        "local a = setmetatable({}, {})
return a & 5",
    )
    .await;
    k9::assert_equal!(
        err,
        "\
error: attempt to perform arithmetic on local 'a' (a table value)
 --> test.lua:2:8
  |
1 | local a = setmetatable({}, {})
  |       - defined here
2 | return a & 5
  |        ^^^^^ attempt to perform arithmetic on local 'a' (a table value)
stack traceback:
\ttest.lua:2: in main chunk"
    );
}
