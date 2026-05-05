---
title: Variables and scope
---

# Variables and scope

A variable is a name that stands for a value. In Shingetsu, names
come in two kinds: **local** names (visible only inside the block
where they are declared) and **global** names (visible everywhere).

## Local variables

Use `local` to introduce a name. The name lives until the end of the
surrounding block:

```lua
local greeting = "hello"
print(greeting)              -- hello

do
    local greeting = "hi"    -- shadows the outer name
    print(greeting)          -- hi
end

print(greeting)              -- hello
```

You can declare several names on one line:

```lua
local x, y, z = 1, 2, 3
```

If you provide fewer values than names, the leftover names are set to
`nil`. Extra values are discarded.

```lua
local a, b = 1            -- a = 1, b = nil
local c, d = 1, 2, 3      -- c = 1, d = 2; the 3 is dropped
```

## Constants and to-be-closed locals

A local can be marked with one of two attributes:

- `<const>` — the name cannot be reassigned.
- `<close>` — when the block ends, a `__close` metamethod on the
  value is called. Useful for releasing host resources at a known
  point.

```lua
local pi <const> = 3.14159
local file <close> = io.open("data.txt", "r")
-- file is automatically closed when this block ends
```

## The `const` keyword

As a more concise alternative to the `<const>` attribute, Shingetsu
accepts `const` as a keyword that introduces a constant binding:

```lua
const pi = 3.14159
const a, b = 1, 2
```

`const` works the same way `local x <const> = ...` does — the name
cannot be reassigned, including via [compound
assignment](operators.md#compound-assignment) — but reads more like
the equivalent in other languages and saves a few characters at
each declaration. There is no `const`-equivalent for the `<close>`
attribute; `<close>` still requires the attribute syntax.

!!! note "For Lua and Luau users"

    The `const` keyword is a Luau extension. The `<const>`
    attribute form is from Lua 5.4. Both work in Shingetsu and
    produce identical behaviour; pick whichever reads better at the
    declaration site.

## Global variables

A name that is assigned to without `local` is a global. Globals live
on the special table `_G` and are visible from anywhere.

```lua
counter = 0          -- global
counter = counter + 1
```

In most embeddings, globals are discouraged: a sandbox often gives
each script its own restricted view of `_G`, and treating everything
as local makes intent clearer. The Shingetsu linter will flag
unintended globals.

## Scope and blocks

A block is the body of a control-flow statement (`if`, `while`,
`for`, `repeat`), the body of a function, a `do ... end` group, or
the whole chunk. A `local` declaration is in scope from the line
after it appears to the end of its block.

```lua
if x > 0 then
    local sign = "positive"
    print(sign)
end
print(sign)   -- error: sign is no longer in scope
```

## Multiple assignment

Assignment can target several names at once. The right-hand side is
evaluated fully before any name is updated, which makes swapping
straightforward:

```lua
local a, b = 1, 2
a, b = b, a       -- now a = 2, b = 1
```
