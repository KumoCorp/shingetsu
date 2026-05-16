# wezterm migration playbook

This document is the operational guide for moving wezterm from
`mlua`-only Lua scripting onto `shingetsu` via the
`shingetsu_migrate` facade.  It assumes you have read the design
spec in `MIGRATE.md` -- particularly §3.7 (canonical migration
patterns A through D) and §6 (memoization readiness) -- and treats
both as authoritative source-of-truth.  This playbook does **not**
restate those rules; it sequences them, calls out wezterm-specific
fixups, and recommends a commit shape.

## Goal

End state: wezterm runs entirely on `shingetsu`, no `mlua`
dependency.  `wezterm-dynamic` continues to exist as wezterm's own
configuration-and-RPC type system; the migration deliberately does
**not** remove it.

## Where wezterm differs from kumomta

wezterm's mlua usage skews differently from kumomta's:

- **Patterns A and B (serde wrappers / manual `from_lua_value`)**
  are rare.  wezterm prefers `wezterm-dynamic` round-tripping
  for almost every config-shaped value.  When you encounter a
  serde wrapper, it usually appears in lua-api crates that came
  later than the dynamic-bridge era.
- **Pattern C (`derive(mlua::FromLua)`)** is moderately common,
  particularly in lua-api crates (`logging`, `time`, `procinfo`,
  etc.) where small parameter structs round-trip through Lua.
- **Pattern D (`impl mlua::UserData`)** dominates: every
  long-lived object exposed to Lua -- `Window`, `Pane`, `Tab`,
  `MuxDomain`, color-parser handles, configuration handles --
  is a userdata.

The two wezterm-specific things you'll meet that kumomta doesn't:

1. **`__wezterm_to_dynamic` metamethod.** Userdata that round-trip
   through `wezterm-dynamic` register this metamethod so the
   `lua_value_to_dynamic` walker (in `luahelper`) can convert
   them.  Migration target: the `shingetsu_migrate::DynamicLua<T>`
   adapter (gated on the `dynamic` feature flag).  See step 3.
2. **`wezterm-event-` registry-key prefix.** Same shape as
   kumomta's `kumomta-on-` prefix and same fixup -- swap to
   `host-event-` so the facade and existing dispatch paths share
   a registry slot during the transition.  See step 1.

## Order of operations

1. **Pre-migration registry-key fixup.**
2. **`luahelper` conversion to `DynamicLua<T>`.**
3. **Pattern C derives (small ones first).**
4. **Pattern A / B (rare; mostly in lua-api crates).**
5. **Pattern D userdata** -- one type per commit, in dependency
   order (see step 5).
6. **Final removal** -- see `final-removal.md`.

## Step 1: registry-key fixup

`shingetsu_migrate::install_on` and `emit_event` look up event
handlers under the `host-event-<name>` registry key on the mlua
side.  wezterm currently uses `wezterm-event-<name>`.

The fix lives in `config/src/lua.rs`, four call sites that all
build the same string:

```rust
// before -- four sites in config/src/lua.rs
let decorated_name = format!("wezterm-event-{}", name);

// after
let decorated_name = format!("host-event-{}", name);
```

Land this as one commit before anything else.  No production
behaviour changes; the registry key is internal to wezterm.

The wezterm test suite hard-codes this string in a couple of
places.  Update those at the same time.

## Step 2: convert `luahelper` to `DynamicLua<T>`

`wezterm-dynamic` round-tripping today goes through hand-rolled
walkers in `luahelper::{lua_value_to_dynamic, dynamic_to_lua}`.
The migration target is `shingetsu_migrate::DynamicLua<T>` -- a
generic adapter that bridges between the two engines' value model
and `wezterm_dynamic::Value` for any `T: FromDynamic + ToDynamic`.

The `dynamic` feature on `shingetsu_migrate` is opt-in; enable it
in `config/Cargo.toml`:

```toml
[dependencies]
shingetsu_migrate = { workspace = true, features = ["dynamic"] }
```

Then convert `luahelper`'s public functions to thin wrappers
around `DynamicLua<T>::from_lua` / `IntoLua::into_lua`:

```rust
// before
pub fn from_lua<'lua, T: FromDynamic>(value: mlua::Value<'lua>) -> Result<T, mlua::Error> {
    let value = lua_value_to_dynamic(value).map_err(...)?;
    T::from_dynamic(&value, FromDynamicOptions::default()).map_err(...)
}

// during migration
pub fn from_lua<T: FromDynamic + ToDynamic>(
    value: mlua::Value,
) -> Result<T, mlua::Error> {
    let dl: shingetsu_migrate::DynamicLua<T> = mlua::FromLua::from_lua(value, /* lua */)?;
    Ok(dl.into_inner())
}
```

The `__wezterm_to_dynamic` metamethod becomes a no-op:
`DynamicLua<T>` walks userdata via `Userdata::snapshot` on the
shingetsu side and via `wezterm_dynamic::Value::Object` round-trip
on the mlua side without needing the metamethod hook.  Once every
userdata that previously registered `__wezterm_to_dynamic` has
been migrated to Pattern D (step 5), the metamethod can be
removed entirely.  Until then, both code paths coexist.

## Step 3: Pattern C derives (small structs first)

Every `#[derive(mlua::FromLua)]` on a small parameter struct
becomes `#[derive(shingetsu_migrate::LuaRepr)]`.  The facade's
`LuaRepr` derive emits both engines' `FromLua` / `IntoLua` plus
the shingetsu `LuaTyped` impl from a single source of truth.

```rust
// before
#[derive(Debug, Clone, mlua::FromLua, FromDynamic, ToDynamic)]
struct LogParams {
    level: String,
    message: String,
}

// during migration
#[derive(Debug, Clone, shingetsu_migrate::LuaRepr, FromDynamic, ToDynamic)]
struct LogParams {
    level: String,
    message: String,
}
```

The `FromDynamic` / `ToDynamic` derives stay; nothing in this
playbook removes them.  The `#[lua(...)]` attribute set is
documented in `shingetsu`'s reference docs; `#[serde(...)]`
attributes don't apply because the facade derive doesn't go
through serde.

If a struct already has `FromDynamic + ToDynamic` and you don't
need it as a strict mlua-derived param (i.e. it can flow through
the dynamic bridge), prefer wrapping the field type in
`DynamicLua<T>` at the call site instead of converting the
struct's derive.  That keeps `wezterm-dynamic` as the source of
truth for shape decisions.

Recommended order: lua-api crates first (small, isolated),
config-tree types last (touched by everything).

## Step 4: Pattern A / B (rare)

Where wezterm uses `lua.from_value(v)` or its own serde-based
adapters outside the `wezterm-dynamic` flow, replace the call site
parameter with `shingetsu::SerdeLua<T>` (or `DynamicLua<T>` if the
type already has `FromDynamic + ToDynamic`).  See the kumomta
playbook for the mechanical shape; in wezterm these sites are
sparse and best done on demand.

## Step 5: Pattern D userdata

Each `impl mlua::UserData for T` becomes
`#[shingetsu_migrate::userdata] impl T { ... }` per §3.7
Pattern D.  In wezterm this is the bulk of the migration work:
every long-lived host object goes through this rewrite.

### Recommended dependency order

Convert types in roughly this order so each commit's tests have
something to verify against:

1. **Leaf types** that don't reference other userdata: color
   parsers, simple handles, date/time wrappers.
2. **Mux domain handles** (`MuxDomain`, `SshDomain`, etc.).
3. **`Pane`** -- depends on domain handles.
4. **`Tab`** -- depends on `Pane`.
5. **`Window`** -- depends on `Tab`.
6. **`Mux` root** -- depends on everything else.

This dependency order means each commit can rewrite one type and
its callers in the same commit without forward references to
not-yet-migrated types.  Leaving `Window` for last is deliberate:
it sits at the top of the dependency tree, so converting it earlier
forces accommodating mlua-shaped Tabs and Panes in its method
bodies.

### Per-arg rustdoc

While rewriting an impl, drop `# Parameters` markdown sections in
favour of per-arg `///` next to the parameter:

```rust
#[lua_method]
fn split(
    &self,
    /// orientation: "Horizontal" or "Vertical"
    direction: String,
    /// optional pane to split; defaults to the active pane
    target: Option<PaneRef>,
) -> Result<PaneRef, VmError> {
    ...
}
```

The captured docs flow into `TypedParam.doc` and surface in the
type checker (handler-arg validation against `wezterm.on(...)`)
and `shingetsu-docgen` (per-event reference pages, definition
files for editor tooling).

### `__wezterm_to_dynamic` removal during conversion

When you convert a userdata type to Pattern D, drop the
`__wezterm_to_dynamic` metamethod registration in the same
commit.  The conversion path now goes through
`shingetsu_migrate::DynamicLua<T>` (step 2) instead.

If the type previously had a hand-written `__wezterm_to_dynamic`
that did something non-trivial (e.g. excluding a field from
serialisation), preserve that behaviour by:

- adding `#[lua(skip)]` to the field on the `LuaRepr` derive,
  if the field is part of a struct field list, or
- implementing `Userdata::snapshot` by hand and excluding the
  field there.

### Memoization opt-in

wezterm doesn't currently use `mod-memoize`-style memoization
(that's a kumomta concept), so most userdata types skip the
snapshot derivation.  If a type is consumed by hashing /
caching elsewhere in wezterm, opt in via
`#[shingetsu_migrate::userdata(snapshot)]` (auto-derive from
serde) or `#[lua_snapshot] fn snapshot(&self) -> Snapshot`
(hand-written).  See §6 of MIGRATE.md.

## Step 6: event registry

After step 1, wezterm's `emit_sync_callback` /
`emit_async_callback` continue to work as-is -- they walk the
mlua named-registry under the new shared `host-event-<name>` key.
No further changes required for the existing dispatch paths.

If you want the type checker to validate event-handler lambdas
written in user config, declare event signatures via
`shingetsu_migrate::declare_event!`:

```rust
shingetsu_migrate::declare_event! {
    /// Fired when a window resize completes.
    pub static WINDOW_RESIZED: Multiple(
        "window-resized",
        /// the window that resized
        window: WindowRef,
        /// the new dimensions
        size: ResizeDims,
    ) -> ();
}
```

This is optional during the migration -- wezterm has lived
without compile-time handler validation for years -- but
worthwhile for new events because the captured rustdoc renders
into per-event reference pages via `shingetsu-docgen`.

For the broadcast pattern wezterm uses today (every handler runs
even when one returns a value), `shingetsu_migrate::emit_event`
is the typed-tuple alternative to `emit_sync_callback`.  Both
walk the same `host-event-<name>` slot; pick whichever has the
more convenient signature for the call site.

## Verification at each step

- `cargo build --all-features` after each commit (wezterm has a
  large feature surface; `--all-features` exercises the gated
  paths).
- The wezterm integration test suite should pass without
  modification.  Tests that assert on `wezterm-event-` strings are
  the tests that need updating, not the production code.
- For Pattern D commits, exercise the converted userdata from a
  Lua config (`wezterm.action_callback`, custom keybinding
  handlers) on both engines if you've kept them coexisting.

## Common pitfalls

- **Forgetting the registry-key fixup.** Symptoms: handlers
  register successfully but never fire.  Fix: step 1 first.
- **`DynamicLua<T>` requires the `dynamic` feature.** Forgetting
  to enable it produces a compile error pointing at `DynamicLua`
  not being in scope; the fix is the Cargo.toml line shown in
  step 2.
- **Removing `__wezterm_to_dynamic` before converting all its
  consumers** breaks dynamic round-tripping silently (the value
  becomes a userdata reference instead of a `Value::Object`).
  Pair the metamethod removal with the userdata type's Pattern D
  commit, never as a separate cleanup.
- **Window / Pane / Tab cycle commits.** Converting `Window`
  before its dependencies forces the in-flight commit to
  accommodate two engine shapes for `Pane`, doubling complexity.
  Stick to the leaf-first order in step 5.
