---
title: Strings
---

# Strings

A string in Shingetsu is a sequence of bytes. It does not have to be
valid UTF-8, although you can choose to treat it that way.

For operations on strings — searching, slicing, formatting — see the
[`string`](../reference/modules/string/index.md) module. For
codepoint-aware operations on UTF-8 text, see the
[`utf8`](../reference/modules/utf8/index.md) module.

## Quoted literals

Strings can be written between matching single or double quotes. The
two forms are equivalent; pick whichever needs less escaping.

```lua
local a = "hello"
local b = 'hello'
local c = "she said \"hi\""
local d = 'she said "hi"'
```

### Escape sequences

Inside quoted strings, `\` introduces an escape:

| Escape       | Means                              |
| ------------ | ---------------------------------- |
| `\n`         | newline                            |
| `\t`         | tab                                |
| `\r`         | carriage return                    |
| `\\`         | a literal backslash                |
| `\"`, `\'`   | a literal quote                    |
| `\0`         | a NUL byte                         |
| `\a`, `\b`, `\f`, `\v` | other ASCII control bytes |
| `\xNN`       | the byte with hexadecimal value `NN` |
| `\ddd`       | the byte with decimal value `ddd` (1–3 digits) |
| `\u{NNNN}`   | the UTF-8 encoding of the given codepoint |
| `\z`         | skip the next run of whitespace, including newlines |

```lua
local greeting = "hello,\n\tworld"
local heart    = "\u{2764}"      -- ❤ encoded as UTF-8
```

## Long strings

A string between matching `[[` and `]]` runs across as many lines as
you like, ignores escape sequences, and trims a single leading
newline:

```lua
local poem = [[
roses are red
violets are blue
]]
```

If your text contains `]]`, use one or more `=` signs in the brackets
and match them at the close:

```lua
local snippet = [==[
    contains ]] without ending the string
]==]
```

## Interpolated strings

A string between matching backticks is *interpolated*: you can drop
an expression directly into the string by wrapping it in `{ ... }`.
The expression is evaluated and converted to a string at the point
where it appears.

```lua
local name = "world"
print(`hello, {name}!`)              -- hello, world!
print(`1 + 2 = {1 + 2}`)             -- 1 + 2 = 3
print(`upper: {name:upper()}`)       -- upper: WORLD
```

Non-string values are converted using their `tostring` form (which a
[metatable](metatables.md) can customise via `__tostring`).

To include a literal `` ` ``, `{`, or `\` inside an interpolated
string, escape it with a backslash:

```lua
print(`a literal \` and \{ and \\`)   -- a literal ` and { and \
```

For more control over how values are formatted — width, precision,
hexadecimal, padding, and so on — use
[`string.format`](../reference/modules/string/format.md):

```lua
print(string.format("%-10s %6.2f", "weight", 3.5))
-- weight       3.50
```

!!! note "For Lua users"

    Interpolated (backtick) strings come from Luau; stock Lua 5.4
    does not have them. They are a more readable alternative to
    chains of `..` and explicit `tostring`/`string.format` calls.

## Concatenation and length

Use `..` to join strings and `#` to ask how many bytes they hold. See
[Operators](operators.md).

```lua
local who = "world"
local msg = "hello, " .. who .. "!"
print(#msg)           -- 13
```

## Method-style calls

Because the string type's `__index` points at the `string` module,
every function in that module can also be called as a method:

```lua
print(("hello"):upper())     -- HELLO
print(string.upper("hello")) -- HELLO (equivalent)
```
