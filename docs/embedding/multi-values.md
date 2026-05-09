---
title: Multi-value returns
---

# Multi-value returns

Lua functions can return more than one value, and so can host
functions exposed to Lua.  The traits that handle multi-valued
boundaries are:

- [`IntoLuaMulti`](../api/shingetsu/trait.IntoLuaMulti.html) — Rust to a list of Lua values.
- [`FromLuaMulti`](../api/shingetsu/trait.FromLuaMulti.html) — list of Lua values to a Rust type.

Both are auto-implemented for any single-valued type and for tuples
up to arity 16, so most code never needs to think about them
directly.

## Tuples for fixed arity

The simplest multi-return is a tuple:

```rust
use shingetsu::{Function, VmError};

let divmod = Function::wrap("divmod", |a: i64, b: i64| -> Result<(i64, i64), VmError> {
    if b == 0 {
        return Err(VmError::LuaError {
            display: "divide by zero".into(),
            value: shingetsu::Value::string("divide by zero"),
        });
    }
    Ok((a / b, a % b))
});
```

The script sees two return values:

```lua
local q, r = divmod(17, 5)   -- q = 3, r = 2
```

A tuple parameter type does the inverse — it pulls the first N
arguments out of the call:

```rust
use shingetsu::Function;

let f = Function::wrap("pair", |(a, b): (i64, i64)| Ok::<i64, _>(a + b));
```

In practice you would write `|a: i64, b: i64|` instead; the tuple
form is mostly useful inside generic code that has to talk about
"the argument list".

## `Variadic` for arbitrary arity

For functions that take or return an unknown number of values, use
[`Variadic`](../api/shingetsu/struct.Variadic.html):

```rust
use shingetsu::{valuevec, Function, Value, Variadic};

let reverse = Function::wrap("reverse", |Variadic(mut vals): Variadic| {
    vals.reverse();
    Ok::<_, shingetsu::VmError>(Variadic(vals))
});
```

`Variadic` wraps a [`ValueVec`](../api/shingetsu/type.ValueVec.html) (which is a `SmallVec<[Value; 3]>` —
small varargs do not allocate).  When used as a parameter, it
collects every remaining argument; when used as a return type, it
splats its contents as multiple return values.

If you need typed elements, [`TypedVariadic<T>`](../api/shingetsu/struct.TypedVariadic.html) collects values of a
specific type and runs `FromLua::from_lua` on each:

```rust
use shingetsu::{Function, TypedVariadic};

let sum = Function::wrap("sum", |TypedVariadic(xs): TypedVariadic<i64>| {
    Ok::<i64, _>(xs.iter().sum())
});
```

## `ValueVec`

[`ValueVec`](../api/shingetsu/type.ValueVec.html) is the underlying container — a small-vector of [`Value`](../api/shingetsu/enum.Value.html).
You see it in three places:

- `Task::new`'s argument list.
- The output of `Task::await` — a `Result<ValueVec, RuntimeError>`.
- `Variadic`'s inner field.

The [`valuevec!`](../api/shingetsu/macro.valuevec.html) macro builds one from `Value` expressions:

```rust
use shingetsu::{valuevec, Value};

let args = valuevec![Value::Integer(1), Value::Integer(2), Value::Integer(3)];
```

To build one from typed Rust values, use `IntoLuaMulti::into_lua_multi`:

```rust
use shingetsu::{IntoLuaMulti, Value};

let args = (1_i64, "hello", true).into_lua_multi();
// args is a ValueVec of length 3.
```

That is exactly what `Function::wrap`-generated code does on your
behalf.

## Deriving multi-shape enums

A frequent pattern is "this function returns one of several
different shapes".  `IntoLuaMulti` can be derived on an enum to
express that directly:

```rust
use shingetsu::{IntoLuaMulti, Value, Variadic};

#[derive(IntoLuaMulti)]
enum FindResult {
    /// Found a single integer position.
    Match(i64),
    /// Found a position and any number of capture values.
    MatchCaptures(i64, Variadic),
    /// No match — produces a single nil.
    NotFound,
}
```

A function returning `FindResult` will produce 1 value, 1 + N
values, or 1 nil depending on which variant it constructs.  The
type checker still knows the union of possible shapes.

The mirror image is `FromLuaMulti`, which handles "this function
accepts one of several arities":

```rust
use shingetsu::{FromLuaMulti, Table, Value};

#[derive(FromLuaMulti)]
enum InsertArgs {
    /// table.insert(t, pos, value)
    AtPos(Table, i64, Value),
    /// table.insert(t, value)
    Append(Table, Value),
}
```

The macro tries variants longest first.  Inside the function you
match on the enum and dispatch:

```rust
use shingetsu::Function;

let insert = Function::wrap("insert", |args: InsertArgs| {
    match args {
        InsertArgs::AtPos(t, pos, v)  => { /* ... */ }
        InsertArgs::Append(t, v)      => { /* ... */ }
    }
    Ok::<(), shingetsu::VmError>(())
});
```

This is how Shingetsu's own `table.insert` is wired — overload
dispatch becomes a single `match`.

## When to reach for which

| You want                                 | Use this              |
|------------------------------------------|-----------------------|
| Fixed number of return values            | a tuple               |
| Splat an arbitrary list                  | `Variadic`            |
| Splat a typed list                       | `TypedVariadic<T>`    |
| One of several distinct return shapes    | `derive(IntoLuaMulti)`|
| One of several distinct argument arities | `derive(FromLuaMulti)`|

For everything that fits the `Function::wrap` and module-macro
shapes (most things), you do not interact with `ValueVec` directly
— the wrapping code handles the conversion in both directions.
