---
title: Tables
---

# Tables

Tables are the only built-in compound type. The same table can act
as an array, as a record (a map from string keys to values), or as a
mix of the two. For the standard operations on tables, see the
[`table`](../reference/modules/table/index.md) module.

## Constructors

A table is built with curly braces. Inside, you can list values, or
write `key = value` for string keys, or `[expr] = value` for any other
key type. Entries can be separated by either commas or semicolons.

```lua
-- array-style
local fruits = { "apple", "banana", "cherry" }

-- record-style
local point = { x = 1, y = 2 }

-- mixed
local mixed = {
    "first",
    "second",
    label = "important",
    [42]  = "the answer",
}

-- empty
local empty = {}
```

## Reading and writing

Use square brackets for any key, or dot notation as a shortcut for
identifier-shaped string keys:

```lua
print(fruits[1])     -- apple   (arrays are 1-indexed)
print(point.x)       -- 1
print(point["y"])    -- 2

point.z = 3          -- add a new field
point.x = nil        -- remove a field
```

Reading a key that is not present yields `nil`. Writing `nil` removes
the key.

!!! note "1-based indexing"

    Array-style tables are indexed starting at `1`, not `0`. For an
    array-style table, the `for i = 1, #t do ... end` idiom walks
    every entry. Tables that mix string keys with array entries, or
    that have holes, need one of the iterators in
    [Iteration](#iteration) below.

## Length

`#t` returns the *array length* of `t` — the largest `n` such that
`t[1] .. t[n]` are all non-nil. It does not count string-keyed
entries:

```lua
local t = { 10, 20, 30, name = "ben" }
print(#t)     -- 3
```

If you need to know how many entries a sparse or mixed table has,
iterate with `pairs` (see below) and count.

## Iteration

Two iterator functions cover the common cases:

- [`ipairs(t)`](../reference/modules/builtins/ipairs.md) walks the
  array part in order from `1` upward, stopping at the first `nil`.
- [`pairs(t)`](../reference/modules/builtins/pairs.md) walks every
  key-value pair, in unspecified order.

```lua
for i, v in ipairs(fruits) do
    print(i, v)
end

for k, v in pairs(point) do
    print(k, v)
end
```

## Tables as objects

Combined with the colon-call syntax described in
[Functions](functions.md) and the customisation hooks described in
[Metatables](metatables.md), tables can be used as object-like
values:

```lua
local greeter = { name = "world" }

function greeter:hello()
    print("hello, " .. self.name)
end

greeter:hello()      -- hello, world
```
