---
title: Modules
---

# Modules

A program of any size is usually split across several files. The
`require` builtin loads another file, runs it, and gives you back
whatever it returned.

## Writing a module

A module is just a file. Whatever it returns at the top level is
what callers will see. The conventional shape is to build up a table
of functions and return it:

```lua
-- file: greet.lua
local M = {}

function M.hello(name)
    return "hello, " .. name
end

function M.shout(name)
    return M.hello(name):upper() .. "!"
end

return M
```

## Loading a module

```lua
local greet = require("greet")

print(greet.hello("world"))   -- hello, world
print(greet.shout("world"))   -- HELLO, WORLD!
```

`require` caches its results: requiring the same name twice will
return the same value, and the module's top-level code runs only the
first time.

## Where modules come from

How a name like `"greet"` is resolved to a file is decided by the
host application. When the host enables it, `require` consults the
familiar `package.path` search list — a semicolon-separated set of
templates where `?` stands in for the requested name — to find a
matching file on disk. A host may also serve modules from an
in-memory table or some other source instead. The script itself
does not need to care.

!!! note "For Lua users"

    Shingetsu does not load native C modules: there is no equivalent
    of `package.cpath` or `package.loadlib`. Pure-script modules
    behave the way you would expect.

## Returning more than a table

A module can return any value, not just a table. Returning a single
function is a common pattern for utilities with one obvious entry
point:

```lua
-- file: parse.lua
return function(input)
    -- ... do the work ...
    return result
end
```

```lua
local parse = require("parse")
local data  = parse(input)
```

## Exporting types

A module can export named [type aliases](type-annotations.md)
alongside its values. Marking a `type` declaration with `export`
makes it visible to anyone who `require`s the module:

```lua
-- file: shapes.lua
local M = {}

export type Point = { x: number, y: number }
export type Rect  = { origin: Point, width: number, height: number }

function M.move(p: Point, dx: number, dy: number): Point
    return { x = p.x + dx, y = p.y + dy }
end

return M
```

```lua
local shapes = require("shapes")

-- exported type names are visible after the require above
local start: Point = { x = 0, y = 0 }
local moved = shapes.move(start, 10, 5)
```

Without `export`, a `type` declaration is private to the file it
appears in.
