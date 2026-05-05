---
title: Error handling
---

# Error handling

When something goes wrong, Shingetsu raises an error. An uncaught
error stops the current script and is reported back to the host with
a diagnostic that includes the source location.

## Raising an error

[`error(message)`](../reference/modules/builtins/error.md) raises an
error from your own code:

```lua
local function withdraw(account, amount)
    if amount > account.balance then
        error("insufficient funds")
    end
    account.balance = account.balance - amount
end
```

The error value can be any value, but a string is the most common
choice.

## Asserting a condition

[`assert(v, message?)`](../reference/modules/builtins/assert.md)
raises an error if its first argument is `false` or `nil`. It is the
right tool for "this should never happen" checks and for unwrapping
optional return values:

```lua
local f = assert(io.open(path, "r"))
local n = assert(tonumber(input), "expected a number, got " .. tostring(input))
```

## Catching errors with `pcall`

`pcall` (protected call) runs a function and turns any error into a
return value instead of stopping the script. It returns `true,
results...` on success, or `false, err` on failure:

```lua
local ok, result = pcall(parse, input)
if ok then
    print("got", result)
else
    print("parse failed:", result)
end
```

## `xpcall` for custom error handling

`xpcall` works like `pcall`, but lets you supply a handler that runs
*at the moment of the error*, before the stack unwinds. This is the
right tool for capturing a stack trace:

```lua
local function handler(err)
    return debug.traceback(err, 2)
end

local ok, info = xpcall(do_work, handler)
if not ok then
    print("failed:\n" .. info)
end
```

See the [`debug`](../reference/modules/debug/index.md) module for
related inspection tools.

## Errors and resources

A `<close>` local (see [Variables and scope](variables-and-scope.md))
has its `__close` metamethod called when the surrounding block ends,
including when the block ends because of an error. This is the
recommended way to make sure host resources are released even if a
script raises.
