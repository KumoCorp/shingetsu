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

### Prefix attributes

The attribute can also appear *before* the name list, in which case
it applies to every name in the declaration:

```lua
local <const> width, height = 1920, 1080
-- equivalent to: local width <const>, height <const> = 1920, 1080
```

A prefix attribute can be combined with per-name attributes as long
as they agree. Mixing different attributes on the same name (for
example `local <const> x <close> = ...`) is rejected at compile
time.

!!! note "For Lua and Luau users"

    Prefix attributes are a Lua 5.5 addition. Stock Lua 5.4 only
    supports the per-name form.

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

A name that is assigned to without being declared `local` is a
global. Globals live on the special table `_G` and are visible from
anywhere.

```lua
counter = 0          -- global
counter = counter + 1
```

In most embeddings, globals are discouraged: a sandbox often gives
each script its own restricted view of `_G`, and treating everything
as local makes intent clearer. Reach for `local` by default; use a
global only when you genuinely want a value to outlive the current
file or to be visible across modules sharing the same environment.

### The `global` keyword and strict mode

A bare assignment like `counter = 0` always succeeds, which means a
typed name (`countr = 0`) silently creates a *new* global instead
of reporting an error. To opt into strict checking, declare each
global the chunk uses with the `global` keyword:

```lua
global counter
counter = 0
countr = 1   -- error: undeclared global 'countr'
```

A chunk enters strict mode the first time it uses `global`. From
that point every read or write of a free name (one that is not a
local, parameter, or upvalue) must reference a name that the chunk
has declared. The check is purely chunk-local; nothing in the
host-provided environment is consulted.

A `global` declaration can also include an initialiser, which is
emitted as an ordinary global write:

```lua
global version = "1.0"
global x, y = 0, 0
```

The same attribute syntax that applies to locals applies to
globals, with the exception that `<close>` is rejected (it has no
meaning at chunk scope). A `<const>` global can only be assigned at
its declaration; subsequent writes anywhere in the chunk are
compile-time errors:

```lua
global <const> APP_NAME = "shingetsu"
APP_NAME = "other"   -- error: attempt to assign to const variable 'APP_NAME'
```

A `global *` wildcard relaxes the check from its lexical position
onward in the chunk. It is the way to opt back into classic-Lua
behaviour when a chunk has used `global` for some declarations but
wants to leave the rest of its globals undeclared:

```lua
global logger
global *           -- everything below this line behaves as in classic Lua
something_else = 1 -- ok, no longer checked
```

!!! note "For Lua and Luau users"

    The `global` keyword and `global *` wildcard are Lua 5.5
    additions. Without any `global` declaration in the chunk
    Shingetsu behaves like classic Lua: free names are unchecked.
    For users coming from Luau, this is broadly the same idea as
    the `--!strict` mode flag for globals, but is opted into
    per-chunk via the keyword rather than via a file-level
    directive.

    Most code is best written by binding modules into locals
    (`local string = require 'string'`) and avoiding free names
    entirely. `global` is most useful for the small number of
    chunk-level values that genuinely need to escape the file.

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
