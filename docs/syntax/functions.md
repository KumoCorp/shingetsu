---
title: Functions
---

# Functions

Functions are values. You can store them in variables, pass them as
arguments, return them from other functions, and call them.

## Defining a function

The `function` keyword defines a function. The most common form
binds it to a local name:

```lua
local function add(a, b)
    return a + b
end

print(add(2, 3))     -- 5
```

`local function` is a convenient shorthand for
`local add = function(a, b) ... end`.

You can also assign an anonymous function to anything:

```lua
local square = function(x) return x * x end
```

## Calling a function

Parentheses surround the arguments:

```lua
add(1, 2)
print("hello")
```

There are two shorthands for single-argument calls:

```lua
print "hello"          -- single string literal
print { 1, 2, 3 }      -- single table constructor
```

## Multiple return values

A function can return more than one value. The caller can ignore
extras, or capture as many as they like:

```lua
local function divmod(a, b)
    return a // b, a % b
end

local q, r = divmod(17, 5)
print(q, r)            -- 3, 2

local q2 = divmod(17, 5)   -- only the first return is kept
```

When a multi-return call is in the middle of an expression list, only
its first value is used. Wrapping it in parentheses also limits it
to one value:

```lua
print(divmod(17, 5))       -- 3, 2 (last in list, all values used)
print(divmod(17, 5), "!")  -- 3, ! (not last; only first is used)
print((divmod(17, 5)))     -- 3
```

## Varargs

A function whose parameter list ends with `...` accepts any number of
extra arguments. Inside the body, `...` expands to those extras:

```lua
local function greet(...)
    for i, name in ipairs({ ... }) do
        print("hello, " .. name)
    end
end

greet("alice", "bob", "carol")
```

The [`select`](../reference/modules/builtins/select.md) builtin gives
more precise access to the varargs.

## Defining functions on tables

A `function` declaration may name a chain of fields, optionally
ending with a colon and a method name. This is purely shorthand for
an assignment, but it reads more naturally for libraries and
object-like tables:

```lua
local app = { ui = {} }

function app.start()        -- assigns app.start = function() ... end
    print("starting")
end

function app.ui.refresh()   -- arbitrary depth of dot fields
    print("refreshing")
end

function app.ui:close()     -- colon adds an implicit `self`
    print("closing", self)
end
```

The colon form is equivalent to writing the same function with an
explicit first parameter named `self`.

## `const function`

Prefixing a function declaration with `const` introduces a binding
that cannot be reassigned later, the same way [`const`
bindings](variables-and-scope.md#the-const-keyword) work for
values:

```lua
const function double(x)
    return x * 2
end

double = nil   -- error: attempt to assign to const variable 'double'
```

This is useful for module-level helpers that should not be
overridden once defined.

## Method definitions

Two pieces of syntactic sugar make it easy to write object-like
values:

```lua
local greeter = {}

function greeter.hello(self)
    print("hello, " .. self.name)
end

-- equivalent: the colon adds an implicit `self` parameter
function greeter:hello()
    print("hello, " .. self.name)
end
```

Method *calls* with `:` similarly pass the receiver as the first
argument:

```lua
greeter.name = "world"
greeter:hello()           -- prints: hello, world
greeter.hello(greeter)    -- equivalent
```

See [Tables](tables.md) and [Metatables](metatables.md) for more on
table-as-object patterns.

## Nested functions and closures

A function defined inside another function can read and update local
variables of its enclosing function. This is called a closure:

```lua
local function make_counter()
    local n = 0
    return function()
        n = n + 1
        return n
    end
end

local next_id = make_counter()
print(next_id())   -- 1
print(next_id())   -- 2
```

## Recursion

`local function` allows the function to refer to itself by name:

```lua
local function fact(n)
    if n <= 1 then return 1 end
    return n * fact(n - 1)
end
```
