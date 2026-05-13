# Lua annotation reference

This document is the author-facing guide for the `---` doc-comment
and annotation conventions that `shingetsu doc extract-lua` (and the
forthcoming lint plugins) understand.  All of this is compatible with
[EmmyLua] / `lua-language-server`, so editors that already speak
EmmyLua will give you hover, completion, and inline diagnostics for
free.

Scope: pure-Lua source files (kumomta's `policy-extras/*.lua`,
wezterm's user-side helpers, etc.).  Rust-side modules
(`#[shingetsu::module]`, `#[shingetsu::userdata]`) use rustdoc
comments instead and are out of scope for this document.

[EmmyLua]: https://emmylua.github.io/

## Quick reference

| Tag | Where it goes | Effect |
| --- | ------------- | ------ |
| `---` (prose) | above any declaration | Free-form summary |
| `@param name type [desc]` | above a function | Override / supplement param type & description |
| `@return type [desc]` | above a function | Declare a return type (one tag per return) |
| `@deprecated [msg]` | above a function or field | Marks deprecated; consumed by the `deprecated` lint |
| `@nodiscard [reason]` | above a function | Result must not be discarded; consumed by `must_use` lint |
| `@hidden` | above a declaration | Omits the declaration from extracted docs |
| `@class Name [: Parent]` | above a `local Name = ...` | Declares a named type |
| `@field name type [desc]` | inside an `@class` block | Adds a field to the enclosing `@class` |

Tags that aren't recognized are silently ignored (so adding new
EmmyLua tags later doesn't break extraction).

## Module shape

Shingetsu's extractor supports two module shapes:

**Canonical**: `local mod = {} ... return mod`.  This is what
kumomta's `policy-extras` files use and what we recommend.

```lua
local mod = {}

--- Configure the queue from a TOML file.
--- @param path string  path to the TOML config
--- @return boolean
function mod.configure(path)
    return true
end

return mod
```

**Inline table return**: `return { foo = function() ... end }`.
Supported by the extractor but rarely used.  See the known-gaps
list in `notes/LINT.md`.

Other shapes (no `return`, returning a non-table, returning multiple
values) produce a `module_shape` warning and an empty extracted
module.

## Doc comments use `---` (three dashes)

The EmmyLua / `lua-language-server` convention: triple-dash for doc,
double-dash for plain comments.  Shingetsu follows this strictly.

```lua
-- This comment is not documentation.  It will be ignored.
--- This comment IS documentation.
function mod.foo() end
```

Mixing the two patterns next to a declaration triggers an
`interrupted_doc_comment` warning:

```lua
--- This summary is intended as documentation,
--- but the next line breaks the attachment.
-- This `--` line orphans the `---` block above.
function mod.bar() end          -- warning: doc block was lost
```

Fix by promoting the offending line to `---`, moving it into the
function body, or deleting it.

## Function annotations

### Parameter types

The function's inline Luau annotation is the primary source of
truth:

```lua
function mod.greet(name: string): boolean
    return #name > 0
end
```

Use `@param` / `@return` when:
- the source has no inline annotation (un-migrated code),
- you want a description in addition to the type,
- you want to override the inline type (rare; usually a smell).

```lua
--- Compute a checksum over a buffer.
--- @param buf string  raw bytes (may contain NULs)
--- @return integer  16-bit checksum
function mod.checksum(buf)
    -- ...
end
```

`@param` and `@return` tags appearing alongside an inline annotation
override the inline type.  When the override changes the apparent
semantics (e.g. `function f(x: number)` annotated `@param x integer`),
shingetsu's docs will reflect the tag.

### Deprecation and must-use

```lua
--- @deprecated use `new_func` instead
function mod.old_func() end

--- Compute a hash; discarding the result is almost always a bug.
--- @nodiscard the hash result is the whole point
function mod.message_hash(msg)
    -- ...
end
```

`@deprecated` and `@nodiscard` will drive the `deprecated` and
`must_use` lints once Phase 4 lands.

### Hiding

```lua
--- @hidden
function mod._internal_helper() end
```

`@hidden` removes the declaration from extracted docs entirely.  Use
it for exports that exist for backwards compatibility but shouldn't
be promoted to users.

## Declaring named types

Use `---@class Name` on a top-level `local` declaration.  The `@class`
block can stand on its own — shingetsu does not require the local's
value to be anything in particular:

```lua
--- A 2D point.
--- @class Point
--- @field x number  the horizontal coordinate
--- @field y number  the vertical coordinate
local Point = {}
```

`@class` surfaces in `DocModel.userdata_types` as a `UserdataDoc`.
Future lint plugins (Phase 5) will use it to validate uses of the
type elsewhere in the codebase.

## Combining `@class` with `typing.lua` (or any runtime type system)

kumomta's `policy-extras/typing.lua` provides a runtime type system
(`mod.record`, `mod.enum`, etc.).  The runtime side is opaque to
shingetsu — there's no static check that two record fields you wrote
match each other.  The recommendation is to add a `@class` block
alongside each `mod.record` call, treating the annotation as the
canonical declaration and the runtime call as the validator:

```lua
local typing = require 'policy-extras.typing'

--- A 2D point.
--- @class Point
--- @field x number
--- @field y number
local Point = typing.record('Point', {
    x = typing.number,
    y = typing.number,
})

--- A user record nested inside an example.
--- @class Example
--- @field point Point
--- @field layer Layer | nil
local Example = typing.record('Example', {
    point = Point,
    layer = typing.option(Layer),
})
```

The `@class` declaration:
- Drives extracted documentation.
- Is what editor tooling (lua-language-server, IntelliJ) shows on
  hover.
- Will drive a planned Phase 5 lint that compares the `@field`
  annotations against the `typing.record(name, {fields})` argument
  table — catching drift between the runtime declaration and the
  annotation.

The duplication is the cost.  The payoff is type checking + docs +
editor support, all from one source file.

## What's not yet supported

These tags / behaviours are not recognized today but may land in
future phases:

- **`@type Name` on a local** to declare an instance type
  (e.g. `local pt = Point({...}) ---@type Point`).  Planned for the
  Phase 5 type-checker integration so that `pt.x = "bad"` can be
  flagged.
- **`@class`-driven completion** in `shingetsu repl`.  Today the
  REPL completes against runtime fields only; a future change can
  read `@class` declarations from loaded files.
- **Generic classes** (`@class Foo<T>`).  Out of scope until the
  underlying type system grows generics.

Tags from EmmyLua that we don't recognize (`@async`, `@cast`,
`@enum`, etc.) are silently ignored.  Adding support for any of them
is straightforward; open a request when you have a use case.
