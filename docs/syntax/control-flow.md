---
title: Control flow
---

# Control flow

These statements decide which code runs and how many times.

## `if` / `elseif` / `else`

```lua
if score >= 90 then
    grade = "A"
elseif score >= 80 then
    grade = "B"
elseif score >= 70 then
    grade = "C"
else
    grade = "F"
end
```

The condition can be any value. Only `false` and `nil` count as
false; every other value (including `0` and `""`) is true.

## `if` as an expression

In addition to the statement form above, `if` can be used as an
expression that produces a value. The shape mirrors the statement,
but every branch must produce a value and the chain must end with
an `else`:

```lua
local grade = if score >= 90 then "A"
              elseif score >= 80 then "B"
              elseif score >= 70 then "C"
              else "F"

local label = if active then "on" else "off"
```

Use the expression form when you want to assign or return a value
that depends on a condition; reach for the statement form when each
branch needs to *do* something rather than produce a value.

!!! note "For Lua users"

    `if` expressions come from Luau; stock Lua 5.4 has only the
    statement form. Lua scripts often use `cond and a or b` to
    achieve the same thing, with the well-known pitfall that the
    pattern silently does the wrong thing when `a` itself can be
    false-y. The `if` expression has no such pitfall.

## `while`

The loop body runs while the condition is true:

```lua
local i = 1
while i <= 10 do
    print(i)
    i = i + 1
end
```

## `repeat ... until`

Like `while`, but the condition is checked at the *end* of each
iteration, so the body always runs at least once:

```lua
repeat
    line = read_line()
until line == "quit"
```

Note that the condition can refer to local variables declared inside
the loop body — its scope extends across the `until`.

## Numeric `for`

Steps a counter through a range. The step defaults to `1` and may
be negative:

```lua
for i = 1, 10 do        -- 1, 2, 3, ..., 10
    print(i)
end

for i = 10, 1, -1 do    -- counts down
    print(i)
end

for x = 0.0, 1.0, 0.25 do
    print(x)            -- 0.0, 0.25, 0.5, 0.75, 1.0
end
```

## Generic `for`

Walks values produced by an iterator function. The
[`pairs`](../reference/modules/builtins/pairs.md) and
[`ipairs`](../reference/modules/builtins/ipairs.md) builtins are the
ones you reach for most often:

```lua
for i, v in ipairs({ "a", "b", "c" }) do
    print(i, v)
end

for k, v in pairs({ x = 1, y = 2 }) do
    print(k, v)
end
```

You can also write your own iterator functions; see
[Functions](functions.md).

## `break` and `continue`

`break` exits the innermost loop immediately. `continue` skips to
the next iteration of the innermost loop without leaving it:

```lua
for i = 1, 10 do
    if i % 2 == 0 then continue end
    if i > 7 then break end
    print(i)              -- 1, 3, 5, 7
end
```

!!! note "For Lua users"

    Stock Lua 5.4 does not have `continue` — it uses `goto continue`
    with a label at the end of the loop. Shingetsu adopts Luau's
    `continue` keyword as a more readable alternative.

## `goto` and labels

A `goto` statement jumps to a named *label* declared elsewhere in
the same function. A label is written `::name::`. Most loop control
is better expressed with `break` and `continue`, but `goto` is
useful for two patterns:

- Breaking out of several nested loops at once.
- Jumping to a single shared cleanup block from multiple branches.

```lua
for i = 1, 10 do
    for j = 1, 10 do
        if grid[i][j] == target then
            goto found
        end
    end
end
print("not found")
goto done

::found::
print("found it")

::done::
```

A `goto` can only target labels in the same function and may not
jump *into* the scope of a local variable.

!!! warning "Labels and the `::` type-assertion operator"

    Shingetsu shares the `::` token between Lua-style labels
    (`::name::`) and Luau-style [type assertions](type-annotations.md#type-assertions)
    (`expr :: type`). When a label statement directly follows an
    expression-shaped statement — a `local` declaration or an
    assignment — the parser greedily reads the trailing `::` as a
    type assertion on that previous expression, which then makes the
    label fail to parse:

    ```lua
    local x = 42
    ::label::      -- parsed as: local x = (42 :: label) :: ???
    print(x)
    ```

    There are three easy ways to disambiguate:

    - End the previous statement with a `;`.
    - Place the label inside a `do ... end` block.
    - Put any non-expression statement (a function call, an `if`,
      another label, etc.) between the assignment and the label.

    ```lua
    local x = 42;          -- semicolon
    ::label::
    print(x)

    do local y = 42 end    -- or wrap in do...end
    ::other::
    ```

    A label that follows a function call, an `end`, or another
    statement that cannot be extended by `::` parses without
    trouble.

## `do ... end`

Groups several statements into a block, mainly to limit the scope of
local variables:

```lua
do
    local tmp = compute_something_expensive()
    use(tmp)
end
-- tmp is no longer in scope
```

## `return`

`return` ends the current function and optionally produces values for
the caller:

```lua
local function clamp(x, lo, hi)
    if x < lo then return lo end
    if x > hi then return hi end
    return x
end
```

A bare `return` may also appear at the very end of a chunk, but is
not required.
