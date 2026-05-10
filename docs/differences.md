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

The [Standard library](#standard-library) section below tabulates
every library function name; the headlines here only call out
broad categories so you can scan quickly.

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
- `os.setlocale`, `string.dump`, the `warn` global.
- Most of the `debug` library — see [`debug`](#debug) for the
  retained subset.

### Things Shingetsu adds on top of Lua 5.4

From Lua 5.5:

- Prefix attributes — `local <const> a, b = ...` instead of
  `local a <const>, b <const> = ...`.
- The `global` keyword and `global *` wildcard for opt-in
  free-name strictness within a chunk.

From Luau (syntax and types):

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

Library additions from Luau and Shingetsu-specific functions are
catalogued in the [Standard library](#standard-library) tables
below.

### Things Luau has that Shingetsu does not

- `setfenv`, `getfenv`, `gcinfo`, `newproxy` — Lua 5.1 globals
  Luau retained.  See [Globals](#globals).
- The `buffer` library (fixed-size mutable byte buffers).
- The `vector` library (built-in 3- or 4-component vector type).
- The `__iter` metamethod (Luau's replacement for the older
  `__pairs`/`__ipairs`, which Shingetsu retains instead).
- `math.noise` (Perlin noise).

### Things Shingetsu has that Luau does not

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

The tables below enumerate every standard-library name.  The
columns are:

- **Lua** — the range of Lua versions in which the name exists
  in the official reference implementation.  `5.1` means
  "Lua 5.1 only" (removed in 5.2); `5.1+` means "5.1 and every
  later version up to and including 5.5"; `5.3+` means "added in
  5.3, still present"; `5.1–5.2` means "added in 5.1, removed
  in 5.3", and so on.  This column is about *name* availability,
  not signature or behavioural compatibility.
- **Luau** — `yes` if the name is in Luau's standard library,
  `no` otherwise.
- **Shingetsu** — `yes` if always available, `no` if absent, or
  `yes (FLAG)` if gated behind a `Libraries` flag.  See
  [Sandboxing](embedding/sandboxing.md).

### Globals

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `assert`, `error`, `getmetatable`, `ipairs`, `next`, `pairs`, `pcall`, `print`, `rawequal`, `rawget`, `rawset`, `select`, `setmetatable`, `tonumber`, `tostring`, `type`, `xpcall` | 5.1+ | yes | yes |
| `rawlen` | 5.2+ | yes | yes |
| `collectgarbage` | 5.1+ | partial | yes (mode-restricted) |
| `require` | 5.1+ | no | yes (`PACKAGE`) |
| `dofile`, `loadfile` | 5.1+ | no | yes (`LOAD`) |
| `load` | 5.2+ | no | yes (`LOAD`) |
| `loadstring` | 5.1 | no | no |
| `unpack` | 5.1 | yes | no (use `table.unpack`) |
| `gcinfo`, `getfenv`, `setfenv`, `newproxy` | 5.1 | yes | no |
| `typeof` | — | yes | yes |
| `warn` | 5.4+ | no | no |

### `string`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `byte`, `char`, `find`, `format`, `gmatch`, `gsub`, `len`, `lower`, `match`, `rep`, `reverse`, `sub`, `upper` | 5.1+ | yes | yes |
| `pack`, `packsize`, `unpack` | 5.3+ | yes | yes |
| `dump` | 5.1+ | no | no |
| `split` | — | yes | yes |

### `table`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `concat`, `insert`, `remove`, `sort` | 5.1+ | yes | yes |
| `pack`, `unpack` | 5.2+ | yes | yes |
| `move` | 5.3+ | yes | yes |
| `create`, `clone`, `clear`, `find`, `freeze`, `isfrozen` | — | yes | yes |
| `foreach`, `foreachi`, `getn`, `maxn` | 5.1 | yes | no |

### `math`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `abs`, `acos`, `asin`, `atan`, `ceil`, `cos`, `deg`, `exp`, `floor`, `fmod`, `huge`, `log`, `max`, `min`, `modf`, `pi`, `rad`, `random`, `randomseed`, `sin`, `sqrt`, `tan` | 5.1+ | yes | yes |
| `tointeger`, `type`, `ult`, `maxinteger`, `mininteger` | 5.3+ | no | yes |
| `atan2`, `cosh`, `sinh`, `tanh`, `frexp`, `ldexp`, `log10`, `pow` | 5.1–5.2 | yes | yes |
| `clamp`, `lerp`, `map`, `round`, `sign`, `isnan`, `isinf`, `isfinite`, `e`, `phi`, `sqrt2`, `tau`, `nan` | — | yes | yes |
| `noise` | — | yes | no |

`math.random` uses a per-environment RNG; concurrent VMs do not
share state.

### `os`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `clock`, `date`, `difftime`, `time` | 5.1+ | yes | yes |
| `execute`, `exit`, `getenv`, `remove`, `rename`, `tmpname` | 5.1+ | no | yes |
| `setlocale` | 5.1+ | no | no |

Process-affecting and filesystem-touching functions are gated
by separate library flags — see
[Sandboxing](embedding/sandboxing.md).

### `io`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `close`, `flush`, `input`, `lines`, `open`, `output`, `popen`, `read`, `stderr`, `stdin`, `stdout`, `tmpfile`, `type`, `write` | 5.1+ | no | yes |

Capabilities within `io` are further gated by `Libraries::IO`,
`Libraries::STDIO`, and `Libraries::EXEC`.

### `utf8`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `char`, `charpattern`, `codepoint`, `codes`, `len`, `offset` | 5.3+ | yes | yes |

### `debug`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `traceback` | 5.1+ | yes | yes |
| `getinfo` | 5.1+ | no | yes |
| `info` (positional return form) | — | yes | yes |
| `pretty_print` | — | no | yes |
| `getlocal`, `getupvalue`, `setupvalue`, `upvalueid` | 5.1+ | no | yes (`DEBUG`) |
| `debug`, `gethook`, `sethook`, `getregistry`, `getuservalue`, `setuservalue`, `setlocal`, `upvaluejoin`, `getmetatable`, `setmetatable` | 5.1+ | no | no |

### `package` / `require`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `package` library and `require` global | 5.1+ | no | yes (`PACKAGE`) |

Preloaded modules (registered by the host) are looked up before
any filesystem search; see
[Custom module loaders](embedding/module-loaders.md).

### `bit32`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| `band`, `bor`, `bxor`, `bnot`, `btest`, `lshift`, `rshift`, `arshift`, `lrotate`, `rrotate`, `extract`, `replace` | 5.2 | yes | yes |
| `countlz`, `countrz`, `byteswap` | — | yes | yes |

Shingetsu has native bitwise *operators* (`&`, `|`, `~`,
unary `~`) in addition to `bit32`.  The `<<` and `>>` *shift
operators* that Lua 5.3–5.5 have are **not** present (the
tokens are reserved for type instantiation); use `bit32.lshift`,
`bit32.rshift`, or `bit32.arshift` instead.

### `coroutine`

| Name(s) | Lua | Luau | Shingetsu |
|---|---|---|---|
| entire library | 5.1+ | yes | no |

Async host calls replace coroutines.  See
[Async host calls](embedding/async.md).

### Other Luau libraries

| Library | Luau | Shingetsu |
|---|---|---|
| `buffer` (fixed-size mutable byte buffers) | yes | no |
| `vector` (3- or 4-component vector type) | yes | no |

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
- **`atan2` is available as both `math.atan(y, x)` and `math.atan2(y, x)`** — Lua 5.4 merged the two,
  Luau retained both spellings.  Shingetsu provides both.
