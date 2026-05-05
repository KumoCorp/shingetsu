---
title: Comments
---

# Comments

Comments are notes for human readers. Outside of the special syntax for [linting](linting.md), the compiler ignores them.

## Line comments

Two consecutive hyphens start a comment that runs to the end of the
line:

```lua
-- this whole line is a comment
local x = 1   -- and so is this trailing note
```

## Block comments

A block comment starts with `--[[` and ends with `]]`. It can span
multiple lines and may sit in the middle of a line:

```lua
--[[
  This is a longer note that spans
  several lines.
]]

local y = 1 --[[ inline ]] + 2
```

If you need to include `]]` inside a block comment, use one or more
equals signs between the brackets and match them at the close:

```lua
--[==[
  This block can contain ]] without ending the comment.
]==]
```

The number of `=` signs in the opener and the closer must match.

## A common idiom: commenting out code

Wrapping a block of code in `--[[ ... ]]` is a quick way to disable it
without deleting it. A handy variant uses three hyphens to toggle the
block on and off by editing one character:

```lua
---[[
print("this prints")
--]]
```

Removing the leading `-` from the first line turns the same text into
a real block comment, and the `print` is skipped.
