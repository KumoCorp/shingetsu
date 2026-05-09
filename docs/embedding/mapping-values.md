---
title: Mapping values
---

# Mapping values

Anything that crosses the Rust–Lua boundary goes through one of four
conversion traits:

- [`IntoLua`](../api/shingetsu/trait.IntoLua.html) — Rust value to a single Lua [`Value`](../api/shingetsu/enum.Value.html).
- [`FromLua`](../api/shingetsu/trait.FromLua.html) — single Lua `Value` to a Rust value.
- [`IntoLuaMulti`](../api/shingetsu/trait.IntoLuaMulti.html) — Rust value to a list of Lua values.
- [`FromLuaMulti`](../api/shingetsu/trait.FromLuaMulti.html) — list of Lua values to a Rust value.

This page covers the single-value cases.  The multi-value traits are
covered in [Multi-value returns](multi-values.md).

## The `Value` type

[`Value`](../api/shingetsu/enum.Value.html) is the runtime tag-and-payload used everywhere a value
crosses the boundary.  Its variants:

- `Value::Nil`
- `Value::Boolean(bool)`
- `Value::Integer(i64)`
- `Value::Float(f64)`
- `Value::String(Bytes)` — bytes, not UTF-8.  See [Strings and `Bytes`](#strings-and-bytes).
- `Value::Table(Arc<Table>)` — see [`Table`](../api/shingetsu/struct.Table.html).
- `Value::Function(Arc<Function>)` — see [`Function`](../api/shingetsu/struct.Function.html).
- `Value::Userdata(Arc<dyn Userdata>)` — see [`Userdata`](../api/shingetsu/trait.Userdata.html).

`Value` implements `Clone` cheaply: scalars are `Copy`-ish, strings
are `Bytes` (`Arc`-backed slice clones), and the rest are `Arc`
clones.  Treat it like you would `serde_json::Value` — passing one
around is fine; comparing one with `==` is structural.

You will rarely build a `Value` directly: `IntoLua` does it for you
for primitive types, and the [`valuevec!`](../api/shingetsu/macro.valuevec.html) macro takes `Value`
expressions when you do need to write them out:

```rust
use shingetsu::{Value, valuevec};

let args = valuevec![
    Value::Integer(7),
    Value::string("hello"),
    Value::Boolean(true),
];
```

`Value::string` accepts anything that converts into `Bytes`.

## `IntoLua` and `FromLua` for primitive types

The standard primitive types implement both directions out of the
box:

| Rust                    | Lua                                |
|-------------------------|------------------------------------|
| `bool`                  | boolean                            |
| `i64`, `i32`, `u32`     | integer                            |
| `usize`                 | integer (errors if it overflows)   |
| `f64`, `f32`            | float                              |
| `String`, `&str`        | string                             |
| `Bytes`                 | string                             |
| `Option<T>`             | `nil` or `T`                       |
| `Vec<T>`                | array-shaped table                 |
| `HashMap<K, V>` / `BTreeMap<K, V>` | hash-shaped table       |
| `Value`                 | identity                           |
| `Table`, `Function`     | as themselves                      |

[`Number`](../api/shingetsu/enum.Number.html) is a helper enum (`Integer(i64) | Float(f64)`) for code
that wants to accept either numeric kind without committing to one;
`FromLua` for `Number` accepts any Lua number, while `FromLua` for
`i64` rejects floats with a fractional part.

[`Never`](../api/shingetsu/enum.Never.html) is the uninhabited type (Rust's `!` under another name); use
it as a return type for closures that always raise an error.

## Strings and `Bytes`

Lua strings are byte sequences.  Shingetsu represents them as
[`Bytes`](../api/shingetsu/struct.Bytes.html), a small-string-optimised byte string defined in this
crate.  Strings of 23 bytes or fewer are stored inline with no
heap allocation; longer strings live in a reference-counted heap
buffer, so cloning a `Bytes` is always O(1) (either a tiny inline
copy or an atomic refcount bump).  Use `Bytes` whenever you can:

```rust
use shingetsu::{Bytes, Function, VmError};

let upper = Function::wrap("upper", |s: Bytes| -> Result<Bytes, VmError> {
    Ok(s.to_ascii_uppercase().into())
});
```

`String` and `&str` are also accepted, but they require a UTF-8
validation pass and (for `String`) an allocation.  Prefer `Bytes`
unless your function actually needs UTF-8 semantics.

## Building a `Function` from a closure

[`Function::wrap`](../api/shingetsu/struct.Function.html#method.wrap) lifts a typed Rust closure into a Lua-callable
function.  Parameter types are `FromLua` and the return type is
`IntoLuaMulti`; both extraction and return conversion are
generated for you:

```rust
use shingetsu::{Function, GlobalEnv, Value, VmError};

let env = GlobalEnv::new();

let add = Function::wrap("add", |a: i64, b: i64| -> Result<i64, VmError> {
    Ok(a + b)
});
env.set_global("add", Value::Function(add.into()));
```

If the first parameter is [`CallContext`](../api/shingetsu/struct.CallContext.html), it receives the call
context (used to access globals, call other functions, or look at
the call stack); remaining parameters come from the script.  Async
closures work the same way:

```rust
use shingetsu::{Bytes, CallContext, Function, VmError};

let fetch = Function::wrap(
    "fetch",
    |_ctx: CallContext, url: Bytes| async move {
        // ... await an HTTP client ...
        Ok::<Bytes, VmError>(Bytes::from(b"<body>".to_vec()))
    },
);
```

[Async host calls](async.md) goes deeper on what suspending across
the boundary actually does.

## Converting your own types

The most common case is a struct that should look like a Lua table.
For that, derive [`LuaTable`](../api/shingetsu/derive.LuaTable.html):

```rust
use shingetsu::LuaTable;

#[derive(LuaTable)]
struct Point {
    x: f64,
    y: f64,
}
```

`#[derive(LuaTable)]` is shorthand for `#[derive(FromLua, IntoLua,
LuaTyped)]`.  Now `Point` can be passed and returned by any
`Function::wrap` closure, and the type checker knows its shape:

```rust
let mid = Function::wrap("mid", |a: Point, b: Point| -> Result<Point, VmError> {
    Ok(Point {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    })
});
```

[Structs and enums as tables](tables-structs-enums.md) covers field
naming, optional fields, defaults, and the enum tagging modes in
detail.

For Rust types that should be opaque to the script — handles to
host objects, mutable resources, types with methods — derive
[`UserData`](../api/shingetsu/derive.UserData.html) instead.  See [Userdata](userdata.md).

## When extraction fails

`FromLua` returns `Result<T, VmError>`.  When `Function::wrap`'s
generated extraction code fails on parameter `n`, it tags the error
with the parameter's position and the function's name.  The script
sees a message like:

```
bad argument #2 to 'add' (number expected, got string)
```

You usually do not have to do anything to make that work; just
return a typed `Result` and the wrapping code handles the rest.

When you do need to return your own argument errors from inside a
function, see [Errors and diagnostics](errors-and-diagnostics.md).

## Type metadata and `LuaTyped`

[`LuaTyped`](../api/shingetsu/trait.LuaTyped.html) is the trait that produces compile-time type information
for the type checker.  It is derived automatically by `LuaTable`,
`UserData`, and the various `#[function]` / `#[lua_method]`
attributes.  You only implement it by hand for exotic cases — for
example, when wrapping a generic Rust type that has no obvious Lua
shape.

If you skip `LuaTyped` entirely, things still run: the script just
loses compile-time argument checking for that type.
