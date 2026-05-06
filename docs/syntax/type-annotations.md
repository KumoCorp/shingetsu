---
title: Type annotations
---

# Type annotations

Shingetsu lets you optionally annotate variables, function parameters,
and function return types. The annotations are inspired by Luau and
help tools (and human readers) understand what kinds of values flow
through your code.

Annotations are optional. Unannotated code is a perfectly normal
Shingetsu program.

## Why bother?

Shingetsu is a dynamic language: a variable can hold a number now and
a string later, and most mistakes are only discovered when the code
actually runs. Type annotations give the compiler enough information
to catch a class of common mistakes *before* a single line executes.

With annotations, the compiler can flag things like:

- Calling a function with the wrong number of arguments.
- Passing a string where a number is expected.
- Assigning a value to a variable whose declared type doesn't fit.
- Returning the wrong shape of value from a function.
- Using a field that doesn't exist on a record.

These show up as diagnostics at load time, with the same source
location and explanation you would get from a syntax error — you do
not have to wait for the offending code path to be exercised by
running the script.

A few other reasons annotations earn their keep:

- **Documentation that doesn't drift.** A type annotation in the
  source is checked by the compiler, so it cannot quietly become
  wrong the way a comment can.
- **Better editor support.** Tools like `luau-lsp` use the same
  annotation syntax to drive autocomplete, hover information, and
  go-to-definition.
- **Clearer intent at API boundaries.** When you write a module that
  others will `require`, annotated parameter and return types tell
  the caller exactly what is expected without making them read the
  body.

You do not have to annotate everything. A common style is to
annotate function signatures and module-level values — the things
other code interacts with — and let local variables be inferred.

## Annotating variables

Place a colon and a type after the name:

```lua
local count: number = 0
local name: string = "anon"
local active: boolean = true
```

If an annotation conflicts with the value being assigned, the compiler
reports a diagnostic.

## Annotating functions

Each parameter can carry a type, and the return type follows the
parameter list:

```lua
local function add(a: number, b: number): number
    return a + b
end
```

A function that returns several values uses parentheses:

```lua
local function divmod(a: number, b: number): (number, number)
    return a // b, a % b
end
```

A function that returns nothing uses `()`:

```lua
local function shout(msg: string): ()
    print(msg:upper())
end
```

## Built-in type names

The most common type names you will see:

- `nil`
- `boolean`
- `number`, `integer`, `float`
- `string`
- `userdata`
- `any` — accepts any value
- `unknown` — like `any`, but the compiler insists you narrow it
  before use
- `never` — a value that never exists; used for return types of
  functions that never return normally (always raise, loop forever,
  etc.)

`number` accepts both integer and float values. `integer` and
`float` are tighter — they reject the other kind — and are useful
when a function genuinely needs one specific numeric kind:

```lua
local function index(arr: { string }, i: integer): string
    return arr[i]
end
```

You can also use [`typeof(expr)`](#typeof) as a type to mean "the
same type as this value".

### `typeof`

`typeof(expr)` can appear anywhere a type can. It stands for the
type of the given expression:

```lua
local default_config = { host = "localhost", port = 80 }

local function configure(c: typeof(default_config))
    -- c has the same shape as default_config
end
```

At the moment Shingetsu treats `typeof(...)` as an opaque type
(equivalent to `any`) for compile-time purposes — it parses and
moves on rather than reasoning structurally about the inner
expression. Future versions may narrow this.

## Optional values

A `?` after a type means "this type or `nil`":

```lua
local function find(needle: string, haystack: string): number?
    return string.find(haystack, needle, 1, true)
end
```

## Tables and arrays

Array-style and record-style table types use a familiar shape:

```lua
local names: { string } = { "alice", "bob" }

local point: { x: number, y: number } = { x = 1, y = 2 }
```

## Unions and intersections

A pipe `|` joins alternatives — a value of a union type satisfies
*either* side:

```lua
local id: string | number = read_id()
```

An ampersand `&` forms an intersection — a value satisfies an
intersection only when it satisfies *both* sides:

```lua
type Readable = { read: (self: Readable) -> string }
type Writable = { write: (self: Writable, s: string) -> () }

local function copy(src: Readable & Writable, dest: Writable)
    dest:write(src:read())
end
```

Intersections are most useful when combining record types whose
fields complement each other.

## Type assertions

Sometimes you know more about a value than the compiler does. A
*type assertion* tells the compiler "trust me, treat this expression
as the following type". The syntax is `expression :: type`:

```lua
local raw = io.read("*a")
local text = raw :: string   -- io.read can return nil; we know it didn't here
print(#text)
```

A type assertion has **no runtime cost and performs no runtime
check**. It is purely a hint to the compiler about how to type the
expression from that point onward. The inner expression is still
type-checked normally; only the *result* type is overridden.

When are assertions useful?

- **Narrowing an optional value** after you have proved it isn't
  `nil` in a way the compiler can't follow on its own.
- **Bridging `any` or `unknown`** when a value comes in from a less
  strict source (host data, JSON, a generic container) and you need
  to use it at a specific type.
- **Disambiguating a union** when a value is `string | number` and
  control flow has narrowed it to one branch.

```lua
local function describe(value: string | number)
    if type(value) == "number" then
        local n = value :: number
        return string.format("%.2f", n)
    else
        return value :: string
    end
end
```

Because assertions are not checked at runtime, an incorrect
assertion can hide a real bug — the compiler will believe you, and
the error will resurface later as something less specific (a method
call on `nil`, an arithmetic operation on a string, and so on).
Reach for assertions when you can clearly explain to a reader why
the assertion is safe.

!!! note "For Lua users"

    The `::` operator is a Luau extension; stock Lua 5.4 does not
    have type assertions because it has no static type system to
    assert against. Shingetsu adopts the Luau form.

!!! warning "Interaction with `::` labels"

    Shingetsu reuses the `::` token for both type assertions and
    Lua-style [`goto` labels](control-flow.md#goto-and-labels)
    (`::name::`). Because the parser cannot tell from the `::`
    alone which of the two is intended, a label written immediately
    after a `local` declaration or an assignment is misread as a
    type assertion on the preceding value:

    ```lua
    local x = 42
    ::label::      -- parser sees: local x = (42 :: label) :: ...
    ```

    The fix is to terminate the previous statement explicitly — with
    a `;`, with `do ... end`, or by placing any non-expression
    statement between the two. See the [`goto` and labels](control-flow.md#goto-and-labels)
    section for the full discussion.

## Generic functions and aliases

A function or type alias can declare type parameters in angle
brackets after the name. The compiler uses the parameter
everywhere it appears to keep the call types consistent:

```lua
local function identity<T>(x: T): T
    return x
end

print(identity(42))      -- compiler knows this returns a number
print(identity("hi"))    -- and this returns a string
```

Multiple type parameters are comma-separated:

```lua
local function map<T, U>(list: { T }, f: (T) -> U): { U }
    local out = {}
    for i, v in ipairs(list) do
        out[i] = f(v)
    end
    return out
end
```

A trailing `...` on a type parameter declares a *type pack* — a
stand-in for a variable-length list of types, used for forwarding
varargs:

```lua
local function first<T...>(...: T...): T...
    return ...
end
```

Type aliases can be generic too, and aliases (unlike functions)
additionally accept default type arguments:

```lua
type Box<T> = { value: T }
type Pair<A, B = A> = { first: A, second: B }

local counter: Box<number> = { value = 0 }
local twins: Pair<string> = { first = "x", second = "y" }
```

Generic type parameters are erased at runtime — they affect what
the compiler accepts but produce no code of their own. The
function or alias compiles down to exactly what an unannotated
version would.

### Explicit type instantiation

Luau-style explicit type instantiation at a call site is
accepted in both the free-standing form `f<<T>>(x)` and the
method form `obj:m<<T>>(x)`. The type arguments are erased at
runtime, so it behaves identically to the inferred call `f(x)`;
in practice the compiler can infer the parameters from the value
arguments and writing the explicit form is rarely necessary.

When explicit arguments are supplied, the type checker binds them
before inspecting the value arguments, so a mismatch is reported
as a conflict with the explicit binding rather than a fresh
inference:

```lua
local function id<T>(x: T): T return x end
id<<number>>("hello")
--   ^^^^^^^^^^^^^^^^^ type 'string' conflicts with type parameter 'T'
--                     (bound to 'number' by '<<...>>' instantiation)
```

The type-argument list is checked against the callee's declared
type parameters: passing too many, too few, or any explicit
arguments to a non-generic function is an error.

## Runtime type checking

Annotations are not only a compile-time concern. When an annotated
function is called with a value of the wrong type, Shingetsu raises
a runtime error at the call boundary, the same way the standard
library does for its own arguments:

```lua
local function add(x: number, y: number): number
    return x + y
end

local ok, err = pcall(add, 1, "two")
print(ok, err)
-- false   bad argument #2 to 'add' (number expected, got string)
```

The check happens once per call, on entry. Inside the body the
compiler then reasons about the parameters using the declared
types, so most subsequent type errors are caught at compile time
rather than at run time.

This means an annotation buys you two related things:

- The compiler rejects calls that *can be proved* wrong at
  compile time.
- The runtime rejects calls that slip past the compiler — typically
  values from `any`-typed paths, host data, dynamic dispatch, or
  other places where the static information was lost.

Unannotated parameters carry no runtime check, by design: if you
want a function to accept anything, leave the parameter
unannotated (or write `: any` for clarity).

## Type aliases

`type` introduces a named alias that can stand in for a longer type:

```lua
type Point = { x: number, y: number }

local function midpoint(a: Point, b: Point): Point
    return { x = (a.x + b.x) / 2, y = (a.y + b.y) / 2 }
end
```

!!! note "For Lua users"

    Type annotations are a Luau extension; stock Lua 5.4 does not have
    them. They are entirely optional in Shingetsu — leaving them off
    is normal.
