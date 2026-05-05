---
title: Values and types
---

# Values and types

Every value a Shingetsu script handles has one of a small number of
types. You can ask any value what its type is using the global
[`type`](../reference/modules/builtins/type.md) function.

## The basic types

### `nil`

`nil` is the absence of a value. An unset variable, a missing field
in a table, or a function that did not return anything all read as
`nil`. There is exactly one `nil` value.

```lua
local x       -- x is nil
print(x)      -- prints: nil
```

### `boolean`

The two values `true` and `false`. Used by the
[logical operators](operators.md) and by `if`/`while` conditions.

In a condition, only `false` and `nil` are treated as false. Every
other value — including `0` and the empty string — counts as true.

### `number`

Numbers come in two flavours that share the same type:

- **integers** — whole numbers, written without a decimal point.
- **floats** — numbers with a decimal point or an exponent.

```lua
local count = 42        -- integer
local ratio = 3.14      -- float
local big   = 1e9       -- float (one billion)
local hex   = 0xFF      -- integer (255)
local half  = .5        -- float (leading zero is optional)
local bits  = 0x1p10    -- float (hex significand, binary exponent: 1024.0)
```

Most arithmetic preserves the integer/float distinction; division
with `/` always produces a float, and floor division `//` produces an
integer when both operands are integers.

The [`math`](../reference/modules/math/index.md) module provides
common numeric functions.

### `string`

A sequence of bytes — *not* necessarily text. Shingetsu does not
assume strings are UTF-8. See the [strings page](strings.md) for the
literal syntax, and the
[`string`](../reference/modules/string/index.md) module for the
operations.

For text-aware (codepoint-level) operations on UTF-8, see the
[`utf8`](../reference/modules/utf8/index.md) module.

### `table`

The all-purpose container: tables can be used as arrays, as records
(string-keyed maps), as sets, and as the building block for
object-like values. See the [tables page](tables.md) and the
[`table`](../reference/modules/table/index.md) module.

### `function`

Functions are values. They can be stored in variables, passed as
arguments, and returned from other functions. See the
[functions page](functions.md).

### `userdata`

A handle to a value owned by the host (the program embedding
Shingetsu). The host decides what fields and methods such a value
exposes. From a script's point of view, a userdata behaves much like
an opaque object.

!!! note "For Lua and Luau users"

    Shingetsu does not provide the `coroutine` library, and there is
    no `thread` value type. Suspending and resuming work is done
    through asynchronous host functions instead.

## Checking a type

```lua
print(type(nil))       -- nil
print(type(true))      -- boolean
print(type(42))        -- number
print(type("hi"))      -- string
print(type({}))        -- table
print(type(print))     -- function
```

The closely related [`typeof`](../reference/modules/builtins/typeof.md)
function additionally reports the *named* type of a value when one is
available — for example, the specific kind of userdata.
