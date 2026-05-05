---
title: Operators
---

# Operators

This page lists the operators built into the language. Many of them
can be customised for table values via [metatables](metatables.md).

## Arithmetic

| Operator | Meaning                              |
| -------- | ------------------------------------ |
| `a + b`  | addition                             |
| `a - b`  | subtraction                          |
| `a * b`  | multiplication                       |
| `a / b`  | division (always produces a float)   |
| `a // b` | floor division                       |
| `a % b`  | modulo (remainder)                   |
| `a ^ b`  | exponentiation (always a float)      |
| `-a`     | negation                             |

For most operators, two integers produce an integer; if either side
is a float, the result is a float.

A string operand is automatically converted to a number when used
with an arithmetic operator, so `"5" + 3` is `8` and `"2.5" * 2` is
`5.0`. The conversion uses the same rules as the number literal
syntax — hex, exponent, and leading-dot forms all work — and the
resulting kind (integer or float) is determined by what the string
parses as. A string that doesn't look like a number raises an
error. This coercion only happens for arithmetic operators; the
bitwise and comparison operators do not coerce.

## Comparison

| Operator | Meaning                |
| -------- | ---------------------- |
| `a == b` | equal                  |
| `a ~= b` | not equal              |
| `a < b`  | less than              |
| `a <= b` | less than or equal     |
| `a > b`  | greater than           |
| `a >= b` | greater than or equal  |

Comparisons return `true` or `false`. Numbers compare numerically;
strings compare byte by byte; other types only support `==` and `~=`
unless a metatable provides more.

## Logical

| Operator  | Meaning                                                  |
| --------- | -------------------------------------------------------- |
| `a and b` | `a` if `a` is false-y, otherwise `b`                     |
| `a or b`  | `a` if `a` is true-y, otherwise `b`                      |
| `not a`   | `true` if `a` is `false` or `nil`, otherwise `false`     |

`and` and `or` short-circuit: they only evaluate `b` if needed. This
makes `or` handy for default values:

```lua
local name = user_name or "anonymous"
```

## String

| Operator   | Meaning                |
| ---------- | ---------------------- |
| `a .. b`   | concatenate two strings |
| `#s`       | length of `s` in bytes  |

Numbers are automatically converted to strings when concatenated, but
no other implicit conversion happens.

## Bitwise

These operate on integers:

| Operator | Meaning              |
| -------- | -------------------- |
| `a & b`  | bitwise and          |
| <code>a &#124; b</code> | bitwise or           |
| `a ~ b`  | bitwise xor          |
| `~a`     | bitwise not          |
| `a << b` | left shift           |
| `a >> b` | right shift          |

## Compound assignment

For every arithmetic operator and the concat operator, there is a
compound form that updates the left-hand side in place:

| Operator   | Equivalent to       |
| ---------- | ------------------- |
| `x += y`   | `x = x + y`         |
| `x -= y`   | `x = x - y`         |
| `x *= y`   | `x = x * y`         |
| `x /= y`   | `x = x / y`         |
| `x //= y`  | `x = x // y`        |
| `x %= y`   | `x = x % y`         |
| `x ^= y`   | `x = x ^ y`         |
| `x ..= y`  | `x = x .. y`        |

The target on the left can be a local, a global, a table field, or
a table index — anything that can appear on the left of an ordinary
assignment.

```lua
local counters = { hits = 0 }
counters.hits += 1

local greeting = "hello"
greeting ..= ", world"
```

!!! note "For Lua and Luau users"

    Compound assignment is a Luau extension; stock Lua 5.4 does not
    have it. Bitwise compound forms (`&=`, `|=`, `<<=`, etc.) are
    not part of Luau and are not accepted by Shingetsu either; use
    the long form (`x = x & y`) for those.

## Length

`#` returns the length of a string in bytes or the array-length of a
table:

```lua
print(#"hello")        -- 5
print(#{ 10, 20, 30 }) -- 3
```

## Precedence

From highest to lowest:

```
^
unary: not  #  -  ~
*  /  //  %
+  -
..
<<  >>
&
~
|
<  >  <=  >=  ==  ~=
and
or
```

`^` and `..` group right-to-left; everything else groups left-to-right.
