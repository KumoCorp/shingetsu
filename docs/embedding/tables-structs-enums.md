---
title: Structs and enums as tables
---

# Structs and enums as Lua tables

Most "plain data" Rust types should look like Lua tables on the
script side.  The [`LuaRepr`](../api/shingetsu/derive.LuaRepr.html) derive sets that up in one line:

```rust
use shingetsu::LuaRepr;

#[derive(LuaRepr)]
struct Point {
    x: f64,
    y: f64,
}
```

`#[derive(LuaRepr)]` expands to `#[derive(FromLua, IntoLua,
LuaTyped)]` (see
[`FromLua`](../api/shingetsu/derive.FromLua.html),
[`IntoLua`](../api/shingetsu/derive.IntoLua.html), and
[`LuaTyped`](../api/shingetsu/derive.LuaTyped.html)): a struct with
`LuaRepr` is a full participant in the boundary, including in
compile-time type checks.

## Field attributes

Two attributes are common:

```rust
use shingetsu::LuaRepr;

#[derive(LuaRepr)]
struct Request {
    /// Use a different Lua key.
    #[lua(rename = "URL")]
    url: String,

    /// If absent, fall back to a default.
    #[lua(default = String::from("GET"))]
    method: String,

    /// Optional fields are omitted when None and accept nil/missing
    /// when extracting.
    auth_token: Option<String>,
}
```

`#[lua(default = ...)]` accepts any expression of the field's type
and is evaluated on extraction when the table key is missing or
nil.

`Option<T>` fields are special on the way out as well: a `None`
field is *not* inserted as `nil`, it is omitted entirely.  This
matches how Lua code treats absence and avoids polluting tables
with explicit nils.

### Extra fields are tolerated

A table passed to a `FromLua`-derived struct may carry fields the
struct does not declare; they are silently ignored.  This is
deliberate — it matches the structural (width-subtyping) typing
model the language uses.  It also means common idioms like
`os.time(os.date("*t", ts))` work, even though `os.date` returns
fields (`wday`, `yday`, `isdst`) that `os.time` does not consume.

## Enums

Enums are derived the same way, with one extra knob: how the
variant is encoded.  Each variant must be a newtype — exactly one
unnamed inner field.

### Untagged (the default)

The macro tries each variant's inner `FromLua` in priority order,
narrower types first:

```rust
use shingetsu::{FromLua, IntoLua, LuaTyped};

#[derive(FromLua, IntoLua, LuaTyped)]
enum Stringy {
    Number(i64),
    Text(String),
}
```

A Lua integer becomes `Stringy::Number`; anything else goes to
`Stringy::Text`.  Variants whose inner types overlap ambiguously
produce a compile-time error.

### Internally tagged

Add a tag field on the table that names the variant:

```rust
use shingetsu::{FromLua, IntoLua, LuaTyped};

#[derive(LuaRepr)]
struct LiteralBody { value: String }

#[derive(LuaRepr)]
struct FileBody    { path: String }

#[derive(FromLua, IntoLua, LuaTyped)]
#[lua(tag = "kind")]
enum Body {
    Literal(LiteralBody),
    File(FileBody),
}
```

Lua side:

```lua
{ kind = "Literal", value = "hello" }
{ kind = "File",    path  = "/etc/motd" }
```

The inner type for an internally-tagged variant must produce a
table from `IntoLua` (the macro adds the tag field to it).  Any
struct derived with `LuaRepr` qualifies; raw scalars do not.

### Adjacently tagged

The variant name and the inner value live in two named fields,
which lets the inner type be anything — including a scalar:

```rust
#[derive(FromLua, IntoLua, LuaTyped)]
#[lua(tag = "kind", content = "data")]
enum Token {
    Word(String),
    Count(i64),
}
```

Lua side:

```lua
{ kind = "Word",  data = "shingetsu" }
{ kind = "Count", data = 7 }
```

### Renaming variants

`#[lua(rename = "...")]` works on each variant the same way it does
on a struct field:

```rust
#[derive(FromLua, IntoLua, LuaTyped)]
#[lua(tag = "type")]
enum Event {
    #[lua(rename = "user.login")]
    UserLogin(LoginEvent),
    #[lua(rename = "user.logout")]
    UserLogout(LogoutEvent),
}
```

## Putting it together

A more realistic example: a host-defined "filter rule" that accepts
either a literal string match or a regex.

```rust
use shingetsu::{Function, LuaRepr, FromLua, IntoLua, LuaTyped, VmError};

#[derive(LuaRepr)]
struct LiteralRule { equals: String }

#[derive(LuaRepr)]
struct RegexRule { matches: String }

#[derive(FromLua, IntoLua, LuaTyped)]
#[lua(tag = "kind")]
enum Rule {
    Literal(LiteralRule),
    Regex(RegexRule),
}

let apply = Function::wrap(
    "apply",
    |rule: Rule, input: String| -> Result<bool, VmError> {
        match rule {
            Rule::Literal(LiteralRule { equals })  => Ok(input == equals),
            Rule::Regex(RegexRule { matches: _ })  => Ok(false), // placeholder
        }
    },
);
```

Script-side use:

```lua
apply({ kind = "Literal", equals = "hello" }, "hello")  -- true
apply({ kind = "Regex",   matches = "h.l." }, "hello")  -- (not implemented above)
```

## When the derive does not fit

Two escape hatches:

- **[`SerdeLua`](../api/shingetsu/struct.SerdeLua.html)** — a wrapper type that bridges a serde-implementing
  Rust value through Lua tables.  Useful for types you do not own
  or that already have a `serde` representation you want to reuse.
  Use sparingly — it does not produce `LuaTyped` metadata.
- **Hand-written impls** — implement `IntoLua`, `FromLua`, and
  `LuaTyped` directly.  Worth doing for performance-critical types
  or types whose Lua shape really has no resemblance to their Rust
  layout.

For mutable, identity-bearing values — handles, file descriptors,
host objects — use [`Userdata`](../api/shingetsu/trait.Userdata.html) instead, covered next.
