---
title: Embedding guide
---

# Embedding guide

This section is for the *host* side of Shingetsu: writing Rust code
that compiles and runs scripts, exposes host objects to them, and
turns Lua values back into Rust values.

If you are writing the script itself, the [Syntax guide](../syntax/index.md)
is the place to start; this section assumes you already know the
language.

## Mental model

A running embedded Shingetsu has three layers:

- A **[`GlobalEnv`](../api/shingetsu/struct.GlobalEnv.html)** — the shared, initialised environment.  It owns
  the global table, the registered standard libraries, the type map,
  and any host-provided modules and event signatures.  You typically
  build one of these per host process (or per tenant) and keep it
  for the lifetime of the program.
- A **compiler** — turns source text into a `Bytecode` chunk.  The
  compiler reads the type information attached to the `GlobalEnv` so
  that compile-time checks and lints know about the host's modules.
- A **[`Task`](../api/shingetsu/struct.Task.html)** — one running script.  A `Task` is a Rust `Future`
  whose `Output` is the script's return values (or an error).  Many
  `Task`s can run concurrently against a single `GlobalEnv`; spawning
  one is cheap.

Everything else in this section builds on those three.

## What you depend on

Add the umbrella crate to your `Cargo.toml`:

```toml
[dependencies]
shingetsu = "..."
tokio = { version = "1", features = ["rt", "macros"] }
```

The `shingetsu` crate re-exports everything an embedder normally
needs.  The lower-level crates (`shingetsu-vm`, `shingetsu-compiler`,
`shingetsu-derive`) exist for advanced use and are not covered here.

## What this guide covers

- [Basics](basics.md) — building a `GlobalEnv`, compiling a chunk,
  running it, and surfacing errors.
- [Mapping values](mapping-values.md) — moving primitive and
  collection types between Rust and Lua.
- [Multi-value returns](multi-values.md) —
  [`ValueVec`](../api/shingetsu/type.ValueVec.html),
  [`Variadic`](../api/shingetsu/struct.Variadic.html), and deriving
  [`IntoLuaMulti`](../api/shingetsu/trait.IntoLuaMulti.html) /
  [`FromLuaMulti`](../api/shingetsu/trait.FromLuaMulti.html) from an
  enum of result shapes.
- [Structs and enums as tables](tables-structs-enums.md) —
  [`#[derive(LuaRepr)]`](../api/shingetsu/derive.LuaRepr.html) and friends.
- [Userdata](userdata.md) — exposing Rust types with methods and
  metamethods.
- [Modules and functions](modules.md) — registering callable
  surfaces with `#[shingetsu::module]`.
- [Events](events.md) — type-safe host-defined callbacks via
  [`declare_event!`](../api/shingetsu/macro.declare_event.html).
- [Async host calls](async.md) — suspending and resuming a task
  while the host does work.
- [Errors and diagnostics](errors-and-diagnostics.md) — building
  rich runtime errors and rendering them.
- [Sandboxing](sandboxing.md) — choosing which capabilities scripts
  can see.
- [Custom module loaders](module-loaders.md) — sourcing modules from
  somewhere other than the filesystem.
- [Shared sync registry](sync-registry.md) — when and how to install
  a custom registry for `task.mutex` / `task.watch` / `task.channel`
  named primitives, and the reload-friendly reconfiguration model.

The pages are written to be read in order on a first pass, but each
one stands on its own once you know the basics.
