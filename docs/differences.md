---
title: Differences from Lua and Luau
---

# Differences from Lua and Luau

Shingetsu is a deliberate blend: most of the syntax
comes from Lua 5.5, the type-annotation system comes from Luau,
and the runtime is built on the host's async executor.  This page
collects every meaningful difference in one place — for readers
coming from Lua 5.4/5.5 or from Luau, and as a reference when
something behaves unexpectedly.

A note on framing: Luau is officially
[based on Lua 5.1](https://luau.org/compatibility) and selectively
backports features from later Lua versions.  As a result Luau
itself diverges from stock Lua 5.4 in significant ways, so where
it matters this page distinguishes "Lua 5.4 has it, Shingetsu
does not" from "Luau has it, Shingetsu does not."

The notes throughout the [Syntax guide](syntax/index.md) and
[Embedding guide](embedding/index.md) cover the same material in
context; this page is the consolidated view.

## At a glance

### Things stock Lua 5.4 has that Shingetsu does not

- The `coroutine` library and the `thread` value type.  Async
  host calls fill the same niche; see
  [Async host calls](embedding/async.md).
- The C API for embedding.  Embedding is via the Rust `shingetsu`
  crate; see the [Embedding guide](embedding/index.md).
- `<<` and `>>` shift operators — those tokens are reserved for
  Luau-style type instantiation (`f<<T>>(x)`).
- Generational and incremental GC modes, and the
  `collectgarbage("generational"/"incremental")` knobs.
- `os.setlocale`.
- `math.deg`, `math.rad`, `math.ult`.
- `string.dump`.
- The `warn` global.
- The full `debug` library.  Shingetsu exposes `debug.info`,
  `debug.getinfo`, `debug.traceback`, `debug.pretty_print` at
  all times; `debug.getlocal`, `debug.getupvalue`,
  `debug.setupvalue`, `debug.upvalueid` are gated by
  `Libraries::DEBUG`.  Hook installation
  (`gethook`/`sethook`), the registry, user-values,
  upvalue-join, and the interactive `debug.debug` are not
  present.

### Things Shingetsu adds on top of Lua 5.4

From Lua 5.5:

- Prefix attributes — `local <const> a, b = ...` instead of
  `local a <const>, b <const> = ...`.
- The `global` keyword and `global *` wildcard for opt-in
  free-name strictness within a chunk.

From Luau:

- The `continue` keyword in loops.
- `if`/`else` *expressions* (the existing statement form is
  unchanged).
- Compound assignment: `+=`, `-=`, `*=`, `/=`, `//=`, `%=`,
  `^=`, `..=`.
- Backtick string interpolation: `` `x is {x}` ``.
- Type annotations on locals, function parameters, and returns,
  plus `type` aliases and the type checker.
- The `const` keyword as a more concise alternative to `<const>`.
- Type assertions (`expr :: type`).
- Explicit type instantiation at call sites (`f<<T>>(x)`).
- `typeof`.
- `string.split`.
- `table.create`, `table.clone`, `table.clear`, `table.find`,
  `table.freeze`, `table.isfrozen`.
- `math.clamp`, `math.round`, `math.sign`.
- `bit32` library (Lua 5.2 addition, retained in Luau, removed
  from Lua 5.3 in favor of bitwise operators; Shingetsu provides
  both).
- `debug.info` (the positional-return form, alongside the
  table-returning `debug.getinfo`).

Shingetsu-specific:

- `debug.pretty_print`.

### Things Luau has that Shingetsu does not

Because Luau is based on Lua 5.1, it retains things Lua itself
removed in 5.2 or later:

- `setfenv`, `getfenv` — function-environment access (Lua 5.2
  removed these; Luau kept them).
- `gcinfo` — heap-size accessor (Lua 5.2 removed this; Luau
  kept it).
- `newproxy` — typed userdata constructor.
- `math.lerp`, `math.map`, `math.noise`.

Luau-specific libraries Shingetsu does not provide:

- `buffer` library — fixed-size mutable byte buffers with typed
  read and write operations.
- `vector` library — built-in 3- or 4-component vector type.

Other Luau additions Shingetsu does not currently provide:

- `__iter` metamethod (Luau's replacement for the older
  `__pairs`/`__ipairs`, which Shingetsu retains instead).
- `math.lerp`, `math.map`, `math.noise`.
- `math.deg`, `math.rad`, and the legacy Lua 5.1 transcendental
  helpers `math.frexp`, `math.ldexp`, `math.pow`, `math.log10`,
  `math.sinh`, `math.cosh`, `math.tanh`, `math.atan2` — Luau
  retained these for backwards compatibility; Shingetsu follows
  Lua 5.4's lead and omits them.  `math.atan` accepts an
  optional second argument for the `atan2` use case.

### Things Shingetsu has that Luau does not

Lua features Luau dropped that Shingetsu retains:

- 64-bit integer numeric subtype.  Luau is float-only;
  Shingetsu inherits Lua 5.3's integer/float split.
- Bitwise operators (`&`, `|`, `~`, unary `~`).  Luau uses the
  `bit32` library for bit operations because it has no
  integers; Shingetsu has both operators *and* the `bit32`
  library for Luau parity.
- `goto` statement and `::label::` form.
- `<close>`-attributed locals and the `__close` metamethod.
- `<const>` attribute syntax (Luau supports only the
  keyword-style `const x = ...`; both forms work in Shingetsu).
- `__pairs` and `__ipairs` metamethods (Luau has `__iter`
  instead).
- Substantial `os`, `io`, `package`/`require`, and `debug`
  libraries.  Luau is intentionally minimal here for sandboxing
  reasons; Shingetsu gates capabilities by library flag instead.
  See [Sandboxing](embedding/sandboxing.md).
- `loadfile`, `dofile` — Luau dropped these for sandboxing;
  Shingetsu gates them behind `Libraries::LOAD`.

## Language details

### Operators

- Arithmetic, comparison, concat, and length operators are as in
  Lua 5.4.
- Bitwise operators `&`, `|`, `~`, and unary `~` are present
  (from Lua 5.3).  The shift tokens `<<` and `>>` are *not*
  operators in Shingetsu; they are reserved for type
  instantiation (`f<<T>>(x)`).
- Compound assignment (`+=` etc.) is from Luau.

### Control flow

- `continue` is a keyword (from Luau).  Lua 5.4's
  `goto continue` workaround still works but is unnecessary.
- `if`/`else` works as both a statement (Lua 5.4) and an
  expression (Luau).
- `goto` and `::label::` work as in Lua 5.4 (Luau dropped them).
  The `::` token is shared with type assertions, which can
  confuse the parser when a label immediately follows an
  assignment — see
  [labels and the type-assertion operator](syntax/control-flow.md#goto-and-labels).

### Variables and scope

- Per-name attributes (`local x <const> = ...`) work as in Lua 5.4.
- Prefix attributes (`local <const> x, y = ...`) are from Lua 5.5.
- The `const` keyword is a Luau-flavoured alias for the `<const>`
  attribute.  No keyword equivalent for `<close>` exists; use the
  attribute syntax.
- Free-name strictness via `global` / `global *` is from Lua 5.5.
  Without any `global` declaration, a chunk behaves like classic
  Lua and free names are unchecked.

### Strings

- Interpolated strings (`` `total: {n}` ``) come from Luau.
- `string.split` is present (Luau extension over Lua 5.4).
- `string.dump` is *not* present (matching Luau; Lua 5.4 has it).
- The host-side string type is `Bytes`, a small-string-optimised
  byte string with O(1) clone.  This is an embedding detail —
  script-visible string semantics are unchanged.

### Numbers

- Integer and float subtypes, as in Lua 5.3+.  `math.type`
  reports which subtype a number has.  Luau is float-only and
  does not have this distinction.
- Integers are 64-bit two's-complement; `math.maxinteger` is
  `2^63 - 1`.

### Type system

- Annotations and the type checker come from Luau.
- All type information is erased at runtime; annotations and
  assertions cost nothing at execution time and never produce
  type errors at runtime.
- The type checker runs when the embedder sets
  `CompileOptions::type_check = true` (recommended for new code).

### Metatables

- All Lua 5.4 metamethods are honoured: arithmetic, comparison,
  `__index`, `__newindex`, `__call`, `__len`, `__concat`,
  `__tostring`, `__metatable`, `__eq`, `__lt`, `__le`, `__pairs`,
  `__close`.
- `__ipairs` (a Lua 5.2 extension dropped in 5.3) is *retained*
  by Shingetsu's `ipairs` because it remains useful for proxy
  tables.  Luau uses `__iter` for this niche, which Shingetsu
  does not currently support.
- `ipairs` does *not* consult `__index`, matching the Lua 5.3+
  spec.
- For userdata, `__close` is auto-dispatched at scope exit.
  `__gc` is *not* — userdata cleanup runs through Rust's `Drop`.
  See [Userdata lifecycle and
  cleanup](embedding/userdata.md#lifecycle-and-cleanup).

## Standard library

Differences library-by-library, against Lua 5.4 unless otherwise
noted.

### Globals

| Function          | Lua 5.4 | Luau | Shingetsu |
|-------------------|:-------:|:----:|:---------:|
| `assert`          | yes     | yes  | yes       |
| `collectgarbage`  | yes     | partial | yes (mode-restricted) |
| `dofile`          | yes     | no   | yes (`LOAD`) |
| `error`           | yes     | yes  | yes       |
| `gcinfo`          | no      | yes  | no        |
| `getfenv`         | no      | yes  | no        |
| `getmetatable`    | yes     | yes  | yes       |
| `ipairs`          | yes     | yes  | yes       |
| `load`            | yes     | yes  | yes (`LOAD`) |
| `loadfile`        | yes     | no   | yes (`LOAD`) |
| `newproxy`        | no      | yes  | no        |
| `next`            | yes     | yes  | yes       |
| `pairs`           | yes     | yes  | yes       |
| `pcall`           | yes     | yes  | yes       |
| `print`           | yes     | yes  | yes       |
| `rawequal/get/len/set` | yes | yes  | yes       |
| `require`         | yes (in `package`) | no | yes (`PACKAGE`) |
| `select`          | yes     | yes  | yes       |
| `setfenv`         | no      | yes  | no        |
| `setmetatable`    | yes     | yes  | yes       |
| `tonumber`        | yes     | yes  | yes       |
| `tostring`        | yes     | yes  | yes       |
| `type`            | yes     | yes  | yes       |
| `typeof`          | no      | yes  | yes       |
| `warn`            | yes     | no   | no        |
| `xpcall`          | yes     | yes  | yes       |

### `string`

Functions shared with Lua 5.4: `byte`, `char`, `find`, `format`,
`gmatch`, `gsub`, `len`, `lower`, `match`, `pack`, `packsize`,
`rep`, `reverse`, `sub`, `unpack`, `upper`.

- `string.split` — Luau extension, present in Shingetsu.
- `string.dump` — Lua 5.4 has it; Shingetsu and Luau do not.

### `table`

Lua 5.4 set retained: `concat`, `insert`, `move`, `pack`,
`remove`, `sort`, `unpack`.

- `table.create`, `table.clone`, `table.clear`, `table.find`,
  `table.freeze`, `table.isfrozen` — Luau extensions, all
  present in Shingetsu.
- `table.foreach`, `table.foreachi`, `table.getn`, `table.maxn`
  — Lua 5.1 deprecated functions retained by Luau; Shingetsu
  does not include them.

### `math`

Compatible with Lua 5.4: `abs`, `acos`, `asin`, `atan` (with
optional second arg for `atan2` semantics), `ceil`, `cos`,
`exp`, `floor`, `fmod`, `huge`, `log`, `max`, `maxinteger`,
`min`, `mininteger`, `modf`, `pi`, `random`, `randomseed`,
`sin`, `sqrt`, `tan`, `tointeger`, `type`.

- `math.clamp`, `math.round`, `math.sign` — Luau extensions,
  present in Shingetsu.
- `math.deg`, `math.rad`, `math.ult` — Lua 5.4 functions *not*
  present in Shingetsu.
- `math.lerp`, `math.map`, `math.noise` — Luau extensions *not*
  present in Shingetsu.
- `math.frexp`, `math.ldexp`, `math.pow`, `math.log10`,
  `math.sinh`, `math.cosh`, `math.tanh`, `math.atan2` — Lua 5.1
  legacy functions retained by Luau but not present in
  Shingetsu (use `^`, `math.log`, `math.atan(y, x)`).
- `math.random` uses a per-environment RNG; concurrent VMs do
  not share state.

### `os`

Compatible with Lua 5.4 minus `setlocale`: `clock`, `date`,
`difftime`, `execute`, `exit`, `getenv`, `remove`, `rename`,
`time`, `tmpname`.  Luau provides only `clock`, `date`,
`difftime`, `time`.

Process-affecting and filesystem-touching functions are gated
by separate library flags — see
[Sandboxing](embedding/sandboxing.md).

### `io`

The Lua 5.4 `io` functions are all present: `close`, `flush`,
`input`, `lines`, `open`, `output`, `popen`, `read`, `stderr`,
`stdin`, `stdout`, `tmpfile`, `type`, `write`.  Luau does not
have an `io` library.  Capabilities are gated by `Libraries::IO`,
`Libraries::STDIO`, and `Libraries::EXEC`.

### `utf8`

Identical across all three: `char`, `charpattern`,
`codepoint`, `codes`, `len`, `offset`.

### `debug`

Always-available subset (sandbox-safe): `info`, `getinfo`,
`traceback`, `pretty_print`.  Behind `Libraries::DEBUG`:
`getlocal`, `getupvalue`, `setupvalue`, `upvalueid`.

Lua 5.4 has more here that Shingetsu does not implement:
`debug.debug`, `gethook`, `sethook`, `getregistry`,
`getuservalue`, `setuservalue`, `setlocal`, `upvaluejoin`, the
debug-side `getmetatable`/`setmetatable`.

Luau exposes only `info` and `traceback`.

### `package` / `require`

Present and gated by `Libraries::PACKAGE`.  Preloaded modules
(registered by the host) are looked up before any filesystem
search; see [Custom module loaders](embedding/module-loaders.md).
Luau does not provide a `package` library.

### `bit32`

Full Luau `bit32` library: `band`, `bor`, `bxor`, `bnot`,
`btest`, `lshift`, `rshift`, `arshift`, `lrotate`, `rrotate`,
`extract`, `replace`, `countlz`, `countrz`, `byteswap`.

Shingetsu has native bitwise *operators* (`&`, `|`, `~`,
unary `~`) in addition to `bit32`.  The `<<` and `>>` *shift
operators* that Lua 5.3–5.5 have are **not** present (the
tokens are reserved for type instantiation); use `bit32.lshift`,
`bit32.rshift`, or `bit32.arshift` instead.

### Absent libraries

- `coroutine` — entire library.  Async host calls replace it.
- `buffer` (Luau) — not present.
- `vector` (Luau) — not present.
- Roblox's `task` library — not part of Luau, never present in
  Shingetsu.

## Runtime model

### Concurrency

A `GlobalEnv` is shared between many concurrent `Task` futures.
Each task has its own call stack, locals, and pending state, but
shares globals, modules, the type map, and registered libraries.

There are no coroutines.  When a script calls into an async host
function, the underlying `Task` parks on the future and yields
to the executor; other tasks against the same env keep making
progress.  See [Async host calls](embedding/async.md).

### Garbage collection

- Tables and Lua closures are tracked by a mark-and-sweep cycle
  collector.  `__gc` on a table is invoked from the collector
  during `collect_cycles()` / `dispose()`.
- Userdata is reference-counted via `Arc<T>`.  When the last
  reference is released, Rust's `Drop` runs synchronously and
  deterministically.  Shingetsu does not run `__gc` on userdata.
- There are no incremental or generational modes.
  `collectgarbage("incremental")` and `"generational"` from Lua
  5.4 are not honoured.

### Error messages

Compile and runtime errors are not byte-for-byte compatible
with reference Lua.  Shingetsu prioritises clear,
source-located diagnostics over mimicking Lua's wording
exactly.  See [Errors and diagnostics](embedding/errors-and-diagnostics.md).

## Specific gotchas

A short list of footguns where Shingetsu differs subtly enough
to surprise:

- **No `<<` / `>>` operators** — the tokens are taken by type
  instantiation.  Use `bit32.lshift`, `bit32.rshift`, or
  `bit32.arshift` for bit shifts.
- **Userdata `__gc` does not auto-fire.**  Cleanup goes through
  `Drop` (synchronous) or `__close` (async, scope-bounded).
- **`::` is shared between labels and type assertions** — a
  label written immediately after a `local` declaration or an
  assignment is misread as a type assertion on the previous
  expression.  Insert a no-op statement between them, or move
  the label.
- **`atan2` is spelled `math.atan(y, x)`** — Lua 5.4 merged the
  two, Luau retained both spellings.  Shingetsu follows Lua
  5.4.
