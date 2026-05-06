---
title: Metatables
---

# Metatables

A metatable is a second table attached to a value that customises how
the value responds to a handful of built-in operations — arithmetic,
indexing, calling, comparison, and so on. The functions stored in the
metatable under specific keys are called *metamethods*.

## Attaching a metatable

```lua
local point = { x = 1, y = 2 }
local mt = {}
setmetatable(point, mt)

print(getmetatable(point) == mt)   -- true
```

See [`setmetatable`](../reference/modules/builtins/setmetatable.md)
and [`getmetatable`](../reference/modules/builtins/getmetatable.md).

## Common metamethods

| Key         | Triggered by                                       |
| ----------- | -------------------------------------------------- |
| `__index`   | reading a field that is not present                |
| `__newindex`| writing to a field that is not present             |
| `__call`    | calling the value like a function                  |
| `__tostring`| `tostring(v)` and `print(v)`                       |
| `__len`     | `#v`                                               |
| `__eq`      | `a == b` when both sides share a metatable         |
| `__lt`      | `a < b`                                            |
| `__le`      | `a <= b`                                           |
| `__add`, `__sub`, `__mul`, `__div`, `__mod`, `__pow`, `__unm`, `__idiv` | the matching arithmetic operator |
| `__band`, `__bor`, `__bxor`, `__bnot` | bitwise operators |
| `__concat`  | `..`                                               |
| `__close`   | end-of-scope cleanup for `<close>` locals          |

`__index` is the most-used metamethod. It can hold either a function
or another table. When it is a table, missing-field reads fall
through to it — the basis of most prototype-style "classes":

```lua
local Animal = {}
Animal.__index = Animal

function Animal.new(name)
    local a = setmetatable({}, Animal)
    a.name = name
    return a
end

function Animal:describe()
    return self.name .. " is an animal"
end

local cat = Animal.new("Mittens")
print(cat:describe())   -- Mittens is an animal
```

## Bypassing metamethods

Sometimes you want to talk to the table directly, ignoring whatever
the metatable says. The `raw*` builtins do that:

- [`rawget(t, k)`](../reference/modules/builtins/rawget.md) — read a
  field without consulting `__index`.
- [`rawset(t, k, v)`](../reference/modules/builtins/rawset.md) —
  write a field without consulting `__newindex`.
- [`rawequal(a, b)`](../reference/modules/builtins/rawequal.md) —
  compare without `__eq`.
- [`rawlen(v)`](../reference/modules/builtins/rawlen.md) — length
  without `__len`.
