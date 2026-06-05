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

Each `#[lua(...)]` annotation on a field controls one aspect of how
that field crosses the boundary.  The common ones first:

```rust
use shingetsu::LuaRepr;

#[derive(LuaRepr)]
struct Request {
    /// Use a different Lua key.
    #[lua(rename = "URL")]
    url: String,

    /// If absent, fall back to a default expression.  Bare
    /// `#[lua(default)]` is shorthand for `T::default()`.
    #[lua(default = String::from("GET"))]
    method: String,

    /// Optional fields are omitted when None and accept nil/missing
    /// when extracting.
    auth_token: Option<String>,
}
```

The full set of field options is:

| Attribute                  | Effect                                                                                         |
|----------------------------|------------------------------------------------------------------------------------------------|
| `rename = "x"`             | Lua key name (default: the Rust ident).                                                        |
| `default` / `default = expr` | Fallback when the key is nil/absent.  Bare flag uses `T::default()`.                         |
| `skip`                     | Omit from `FromLua`/`IntoLua`/`LuaTyped`.  `FromLua` fills it with `T::default()`.             |
| `flatten`                  | Inline the inner struct's fields at this level (the field's type must itself derive `LuaRepr`). |
| `try_from = "T"`           | Read as `T`, then convert via `<FieldType as TryFrom<T>>::try_from`.  Symmetric `IntoLua` uses `Into<T>`. |
| `into = "T"`               | `IntoLua` only: convert via `Into<T>` before writing.                                          |
| `validate = "path::to::fn"`| After `FromLua`, call `fn(&T) -> Result<(), impl Display>` to validate.                        |
| `deprecated = "reason"`    | Record a deprecation reason for the type-checker lint.                                         |

A `None` `Option<T>` is *not* inserted as `nil` on the way out — it
is omitted entirely.  This matches how Lua code treats absence and
avoids polluting tables with explicit nils.

### Extra fields are tolerated by default

A table passed to a `FromLua`-derived struct may carry fields the
struct does not declare; they are silently ignored.  This is
deliberate — it matches the structural (width-subtyping) typing
model the language uses.  It also means common idioms like
`os.time(os.date("*t", ts))` work, even though `os.date` returns
fields (`wday`, `yday`, `isdst`) that `os.time` does not consume.

Opt out with `#[lua(deny_unknown_fields)]` on the container (see
below) when you want strict input validation.

## Container attributes

Annotations on the struct itself control conversion as a whole:

```rust
use shingetsu::LuaRepr;

#[derive(LuaRepr)]
#[lua(rename_all = "kebab-case", deny_unknown_fields)]
struct RetryPolicy {
    max_retries: i64,
    backoff_ms: i64,
    #[lua(rename = "jitter%")]
    jitter_pct: i64,
}
```

This accepts and produces tables like
`{ ["max-retries"] = 3, ["backoff-ms"] = 250, ["jitter%"] = 10 }`,
and rejects tables with unknown keys.  Per-field `#[lua(rename =
"...")]` overrides the container's `rename_all` for that field.

The full set of container options is:

| Attribute                  | Effect                                                                                       |
|----------------------------|----------------------------------------------------------------------------------------------|
| `rename_all = "casing"`    | Default case conversion for field names.  See the casing list under [Enums](#enums) below.   |
| `deny_unknown_fields`      | Reject tables containing keys not declared on the struct.                                    |
| `default` / `default = "path::to::fn"` | If the whole Lua value is `nil`, build the struct from `Default::default()` (bare flag) or from the named zero-argument function. |
| `try_from = "T"`           | Decode the Lua value as `T`, then convert via `Self::try_from(T)`.  Symmetric `IntoLua` uses `Into<T>`. |
| `into = "T"`               | `IntoLua` only: convert to `T` before emitting.                                              |

`try_from` and `into` delegate the whole struct to a different
type, so they are mutually exclusive with `deny_unknown_fields` and
`rename_all` (which would have nothing to apply to).

## Enums

Two enum shapes are supported.

### Unit-string enums

When every variant is a unit (data-less) variant, the enum is
encoded as a string:

```rust
use shingetsu::LuaRepr;

#[derive(Clone, Copy, PartialEq, Eq, LuaRepr)]
#[lua(rename_all = "kebab-case")]
enum ResizePolicy {
    No,             // <-> "no"
    SmallestWins,   // <-> "smallest-wins"
    #[lua(rename = "custom-wins")]
    CustomOverride, // <-> "custom-wins"
}
```

`rename_all` on the container accepts the standard serde-style
casings: `"lowercase"`, `"UPPERCASE"`, `"PascalCase"`,
`"camelCase"`, `"snake_case"`, `"SCREAMING_SNAKE_CASE"`,
`"kebab-case"`, `"SCREAMING-KEBAB-CASE"`.  These apply to struct
fields too.  Per-variant `#[lua(rename = "...")]` overrides the
container default.

### Newtype enums

When variants carry data, each variant must be a newtype — exactly
one unnamed inner field.  The container `#[lua(...)]` attribute
picks one of three tagging modes.

#### Untagged (the default)

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

A `#[lua(nil)]` unit variant inside an otherwise untagged newtype
enum maps to Lua `nil`:

```rust
#[derive(shingetsu::IntoLua, shingetsu::LuaTyped)]
enum Maybe {
    Value(i64),
    #[lua(nil)]
    Missing,
}
```

#### Internally tagged

Add a tag field on the table that names the variant:

```rust
use shingetsu::{FromLua, IntoLua, LuaTyped, LuaRepr};

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

#### Adjacently tagged

The variant name and the inner value live in two named fields,
which lets the inner type be anything — including a scalar:

```rust
#[derive(shingetsu::FromLua, shingetsu::IntoLua, shingetsu::LuaTyped)]
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

`#[lua(rename = "...")]` overrides the variant's Lua-facing name —
the string for unit-string enums, the tag value for tagged newtype
enums.  Combine with `#[lua(rename_all = "...")]` on the container
to set a default casing:

```rust
use shingetsu::LuaRepr;

#[derive(LuaRepr)]
struct LoginEvent { user: String }

#[derive(LuaRepr)]
struct LogoutEvent { user: String }

#[derive(shingetsu::FromLua, shingetsu::IntoLua, shingetsu::LuaTyped)]
#[lua(tag = "type", rename_all = "snake_case")]
enum Event {
    UserLogin(LoginEvent),     // tag = "user_login"
    #[lua(rename = "user.logout")]
    UserLogout(LogoutEvent),   // tag = "user.logout"
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
