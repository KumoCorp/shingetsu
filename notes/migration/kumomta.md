# kumomta migration playbook

This document is the operational guide for moving kumomta from
`mlua`-only Lua scripting onto `shingetsu` via the
`shingetsu_migrate` facade.  It assumes you have read the design
spec in `MIGRATE.md` -- particularly §3.7 (canonical migration
patterns A through D) and §6 (memoization readiness) -- and treats
both as authoritative source-of-truth.  This playbook does **not**
restate those rules; it sequences them, calls out kumomta-specific
fixups, and recommends a commit shape.

## Goal

End state: kumomta runs entirely on `shingetsu`, no `mlua`
dependency.  Intermediate state: every Lua-facing type accepts both
engines side-by-side via `shingetsu_migrate`, callers route
through the facade's typed APIs, and the existing mlua-based
runtime keeps working at every commit.

## Order of operations

Roughly in this order.  Each step lands as one or more commits
that compile, pass tests, and don't change observable behaviour.

1. **Pre-migration registry-key fixup** -- one-line change.
2. **Conversion derives** -- Pattern A (mechanical) and Pattern C
   (Pattern A is mechanical; Pattern C is *not* a universal swap
   for kumomta -- most `mlua::FromLua` sites are userdata
   round-trip types deferred to Step 4, see Pattern C below).
3. **Manual serde call sites** -- Pattern B (mechanical).
4. **Userdata blocks** -- Pattern D (one type per commit).  Also
   absorbs the userdata round-trip types whose `mlua::FromLua`
   derive was left untouched in Step 2: delete that derive, add
   `#[shingetsu_migrate::userdata]`, and move call sites to
   `UserDataRef<T>` (or `SerdeLua<T>` for table-or-userdata
   events).
5. **Event registry** -- `declare_event!` rename and
   `CallbackSignature::call` plumbing.
6. **mod-memoize port** -- per §6.6.
7. **Config pool integration** -- swap `mlua::Lua` construction
   for `shingetsu_migrate::Engine`.
8. **Final removal** -- see `final-removal.md`.

Steps 2 and 3 (Pattern A and Pattern B) are independent and can
be parallelised once the fixup in step 1 has landed.  Pattern C's
userdata round-trip types are *not* independent: they are
deferred into Step 4 (Pattern D), so that work is sequenced after
Step 2/3 for those types.  Steps 5 through 7 have ordering
dependencies and should be done in sequence.

## Step 1: registry-key fixup

`shingetsu_migrate::install_on` and `emit_event` both look up event
handlers under the `host-event-<name>` registry key on the mlua
side.  kumomta currently uses `kumomta-on-<name>`.  Until those
two strings agree, the facade and `CallbackSignature::call` write
to and read from disjoint registry slots and event handlers
registered through one path are invisible to the other.

The fix is a single line in `crates/config/src/lib.rs`:

```rust
// before
pub fn decorate_callback_name(name: &str) -> String {
    format!("kumomta-on-{name}")
}

// after
pub fn decorate_callback_name(name: &str) -> String {
    format!("host-event-{name}")
}
```

Land this as its own commit before anything else in this
migration.  No production behaviour changes -- the registry key is
purely internal -- but doing it first means every later commit can
mix facade and native registration freely.

Verify by running the existing kumomta test suite; nothing should
fail.  Any test that hard-codes the registry key string is the
test that needs updating, not the production code.

## Step 2: conversion derives (Pattern A and Pattern C)

These two patterns share a workflow: replace the mlua-side derive
or wrapper with the `shingetsu_migrate` equivalent and re-run the
host's existing tests.

### Pattern A -- `SerdeWrappedValue<T>` rename

Wherever kumomta wraps a `Serialize + DeserializeOwned` type via
`SerdeWrappedValue<T>` (or its in-tree equivalent), the migration
target is `shingetsu::SerdeLua<T>`.  The wrapping shape and deref
semantics are identical, so this is a pure import rename:

```rust
// before
use config::SerdeWrappedValue;
fn handle(req: SerdeWrappedValue<Request>) { ... }

// during migration
use shingetsu_migrate::SerdeLua;
fn handle(req: SerdeLua<Request>) { ... }
```

`SerdeLua<T>` is re-exported from `shingetsu_migrate` so the
single-import form works on both engines.  After final removal the
import becomes `use shingetsu::SerdeLua` (search-and-replace).

### Pattern C -- `derive(mlua::FromLua)`

**This pattern is NOT a mechanical, universal derive swap for
kumomta.**  An earlier draft of this playbook said "every
`#[derive(mlua::FromLua)]` becomes
`#[derive(shingetsu_migrate::LuaRepr)]`".  That is wrong for
kumomta and was reverted; the corrected guidance follows.

#### Why the naive swap fails

kumomta is on mlua 0.11, whose `#[derive(FromLua)]` is
*userdata-downcast only* -- mlua's own docs: "takes UserData
value, borrow it (of the Rust type) and clone."  It emits a
`FromLua` that matches `Value::UserData(ud) => ud.borrow::<Self>()`
and rejects everything else.  It does **not** read fields, does
**not** do structural/table conversion, and imposes **no**
per-field bounds.

The facade's `LuaRepr` / `FromLua` derive is the opposite: it
emits structural, field-by-field conversion (mlua-side table
walking plus a shingetsu-side structural `FromLua`), which:

- imposes `FieldType: FromLua` on **every** field, on both
  engines.  kumomta's real `mlua::FromLua`-deriving types cannot
  satisfy this -- e.g. `Memoized.to_value: Arc<dyn Fn(&Lua) ->
  mlua::Result<Value> + Send + Sync>` (no `FromLua` possible),
  and `QueueConfig` / `EgressPathConfig` carry enum-typed fields
  (`QueueStrategy`, `ConfigRefreshStrategy`, ...).
- emits `IntoLua` (via `LuaRepr`), which **collides** with
  mlua's blanket `impl<T: UserData> IntoLua for T` -- and these
  types all `impl UserData`.  This is exactly why kumomta's
  derive is `FromLua`-only.
- the facade derive **hard-errors on enums** entirely; several of
  these types are enums.

Also note, empirically: returning a *table-shaped* value from an
event handler whose return type is one of these
`mlua::FromLua`-derived types does **not** work in kumomta today
(the derive is userdata-only; every in-tree `get_queue_config`
handler returns `kumo.make_queue_config{}`, a userdata).  So
there is no table-shape capability to preserve for these types.
Table-shape returns work today only for serde-typed returns
(`SerdeWrappedValue` / `SerdeLua`), not the userdata-derived ones.

#### Correct classification

Triage each `#[derive(mlua::FromLua)]` site:

1. **Userdata round-trip types** (the type also `impl UserData` /
   `LuaUserData`; this is the common case -- `QueueConfig`,
   `EgressSource`, `EgressPool`, `Shaping`, `EgressPathConfig`,
   `Message`, `Signer`, `Memoized`, `HeaderMapWrapper`,
   `EsmtpDomain`, ...).  **Do nothing in Step 2.**  Leave
   `#[derive(mlua::FromLua)]` exactly as-is.  These migrate in
   **Step 4 (Pattern D)**: the type gets
   `#[shingetsu_migrate::userdata]`, the `mlua::FromLua` derive is
   *deleted*, and every call site that received the value by
   `T` (relying on the userdata downcast) switches to
   `shingetsu_migrate::UserDataRef<T>` -- the idiomatic native
   shingetsu way to take a userdata back into Rust (shingetsu's
   `#[userdata]` deliberately does **not** emit `FromLua for T`;
   `UserDataRef<T>` is runtime-checked and derefs to `&T`).  For
   an event whose handler should accept *either* a userdata or a
   table, use `SerdeLua<T>` as the signature return type instead
   (the `SerdeLua` mlua impl already walks a userdata's `__pairs`
   via the materialize fallback, so both shapes deserialize).

2. **Enums deriving `mlua::FromLua`** (`QueueStrategy`,
   `WakeupStrategy`, `MemoryReductionPolicy`,
   `ConfigRefreshStrategy`, `ReconnectStrategy`).  The facade has
   no enum conversion support yet, so leave these on
   `#[derive(mlua::FromLua)]` until facade enum support lands or
   they are restructured.  This is safe: these enum `FromLua`
   impls are never reached across the lua boundary (the enums
   only appear as field types inside parent userdata structs,
   which round-trip as opaque userdata handles -- no per-field
   conversion occurs).

3. **Genuinely table-shaped structs** -- a plain config struct
   that is *not* `impl UserData`, whose every field already has
   both-engine conversion, and contains no enum fields.  Only
   these become `#[derive(shingetsu_migrate::LuaRepr)]` with the
   serde->lua attribute substitutions below.  In practice kumomta
   has few or none of these among the `mlua::FromLua` sites;
   verify per-site before converting.

For the (rare) genuine Pattern C struct, the substitution is:

| mlua / serde attribute            | facade attribute              |
|-----------------------------------|-------------------------------|
| `#[serde(default)]`               | `#[lua(default)]`             |
| `#[serde(rename = "x")]`          | `#[lua(rename = "x")]`        |
| `#[serde(flatten)]`               | `#[lua(flatten)]`             |
| `#[serde(deny_unknown_fields)]`   | `#[lua(deny_unknown_fields)]` |
| `#[serde(try_from = "T")]`        | `#[lua(try_from = "T")]`      |

Keep the `serde::Serialize` / `serde::Deserialize` derives.
After final removal the only change is
`shingetsu_migrate::LuaRepr` -> `shingetsu::LuaRepr`.

## Step 3: manual serde call sites (Pattern B)

Hunt down every `lua.from_value(v)` / `lua.to_value_with(...)`
call that converts to or from a `Serialize + DeserializeOwned`
type.  Replace the call site's parameter type with `SerdeLua<T>`
and let the function-call dispatcher do the conversion:

```rust
// before
async fn pre_init(lua: &mlua::Lua, raw: mlua::Value) -> mlua::Result<()> {
    let cfg: PreInitConfig = lua.from_value(raw)?;
    ...
}

// during migration
async fn pre_init(cfg: shingetsu_migrate::SerdeLua<PreInitConfig>) -> Result<(), VmError> {
    let cfg: &PreInitConfig = &cfg;
    ...
}
```

`PreInitConfig` already had `Serialize + DeserializeOwned` derives
(that's what made the original `from_value` call valid); they
satisfy `SerdeLua<T>`'s bounds without further changes.

The before/after is structurally smaller because every manual
conversion call disappears from the body.  Group call sites by
type so the diff for one type lands as one commit.

## Step 4: userdata blocks (Pattern D)

Each `impl mlua::UserData for T { fn add_methods, fn add_fields }`
becomes `#[shingetsu_migrate::userdata] impl T { ... }` with each
method tagged `#[lua_method]`, fields tagged `#[lua_field]`,
metamethods tagged `#[lua_metamethod(Name)]`.

This is the only pattern that doesn't yield to search-and-replace.
Each rewrite is structural: kumomta's `add_methods` registers
against an mlua-style builder; the shingetsu form is per-method
attribute-driven.  Plan one userdata type per commit.  Both impls
can coexist on the same Rust type during the transition (mlua
uses the `impl UserData`, shingetsu uses the
`#[shingetsu_migrate::userdata]` form), so each commit is
independently testable on both engines.

### Memoization opt-in

Types that participated in `mod-memoize` need a snapshot hook on
the shingetsu side.  Two ways to declare it:

- `#[shingetsu_migrate::userdata(snapshot)]` -- auto-generates
  `Userdata::snapshot` from the type's serde representation.  The
  type must already be `Serialize + DeserializeOwned`.
- `#[lua_snapshot] fn snapshot(&self) -> Snapshot { ... }` --
  hand-written body, for types that need a non-serde
  representation (e.g. interior mutable state that isn't in the
  serde shape).

The mlua-side `__memoize` metamethod is registered automatically
under the same attribute, so memoizing call sites work on both
engines during the transition.  See §6 of MIGRATE.md for the
underlying contract.

### Per-arg rustdoc

While you're rewriting the impl, drop `# Parameters` markdown
sections and put doc comments directly on the parameters
themselves -- the proc-macro accepts both, but per-arg `///` is
local to the parameter and doesn't drift:

```rust
#[lua_method]
fn defer_until(
    &self,
    /// retry deadline as an RFC 3339 timestamp
    deadline: String,
    /// optional reason logged with the deferral
    reason: Option<String>,
) -> Result<(), VmError> {
    ...
}
```

The captured docs flow into `TypedParam.doc` and surface in both
the type checker (handler-arg validation) and `shingetsu-docgen`
(reference pages, definition files).

### `mod-sqlite`, `mod-filesystem`, and other module-style crates

These crates use the pattern of returning a userdata from a
module function.  The userdata becomes
`#[shingetsu_migrate::userdata]` per the rules above; the
returning module function becomes
`#[shingetsu_migrate::module]` with each
`fn open(...) -> SqliteHandle` marked `#[function]`.  No new
glue is needed -- both pieces compose.

## Step 5: event registry

After the registry-key fixup in step 1, `declare_event!` invocations
can move from kumomta's macro to the facade's:

```rust
// before
config::declare_event! {
    pub static GET_QUEUE_CONFIG: Multiple(
        "get_queue_config",
        domain: String,
        tenant: Option<String>,
    ) -> QueueConfig;
}

// during migration
shingetsu_migrate::declare_event! {
    /// Resolve queue config for a domain + tenant pair.
    /// Multiple handlers run in registration order; the first
    /// non-empty result wins.
    #[returns = "Resolved queue configuration."]
    pub static GET_QUEUE_CONFIG: Multiple(
        "get_queue_config",
        /// Recipient domain.
        domain: String,
        /// Optional tenant scope.
        tenant: Option<String>,
    ) -> QueueConfig;
}
```

Note the new opportunities: rustdoc on the `static`, on each
parameter, and via `#[returns = "..."]` -- the facade captures
all three and `shingetsu-docgen` renders them as per-event
reference pages.  This is a good moment to add rustdoc that the
old macro had no slot for.

`CallbackSignature::call(env, args)` (the shingetsu-side dispatch
entry point) replaces the kumomta-side `sig.call(...)` /
`sig.call_callback(...)` family.  The migration facade exposes
`EventSignature::call` with the same shape on both engines.

For wezterm-style "fire and forget across all handlers" dispatch
(where every handler runs even when one returns a value),
`shingetsu_migrate::emit_event(target, name, args)` is the
broadcasting alternative.  kumomta historically uses the
typed-call form; broadcast is rarely needed.

## Step 6: mod-memoize port

`mod-memoize`'s `MemoizedTable` proxy depends on userdata's
ability to snapshot itself for cache-key derivation.  The
shingetsu-side hooks (`Userdata::snapshot`, `__memoize`
metamethod) are wired in step 4 above.  After step 4 lands for
every type that goes through memoization, port mod-memoize's
front-end:

- The cache key derivation and serde_json bridge stay identical;
  see §6.2 of MIGRATE.md.
- The `Memoize` trait moves from a kumomta-side trait to
  consuming `Userdata::snapshot` directly.
- The `__memoize` metamethod is now auto-emitted by the userdata
  derive, so the explicit registration in mod-memoize's table
  proxy can come out.

Land mod-memoize as a single commit once every userdata it
touches has snapshot support.

## Step 7: config pool integration

The kumomta config pool today constructs `mlua::Lua` instances and
calls `lua.load(...).exec()` to run the user's config files.  The
shingetsu equivalent is `shingetsu::Compiler::compile` followed by
`shingetsu::Task::new(env, fn, args).await`.

The facade exposes `Engine::from_shingetsu(env)` /
`Engine::from_mlua(lua)` so the pool can hold either backend
behind a uniform handle.  Configure the pool to construct
`Engine::from_shingetsu` instances; the existing mlua-based code
paths can be deleted in the same commit.

This is the commit that flips kumomta from "mlua with shingetsu
side-by-side" to "shingetsu with the facade re-exporting mlua
shapes for compatibility".  After this, the mlua dependency stays
only because `shingetsu_migrate` brings it in.

## Verification at each step

- `cargo nextest run --workspace` (or kumomta's equivalent) at
  every commit.
- The kumomta integration test suite should pass without
  modification; if a test fails, the migration step changed
  observable behaviour and needs to be unwound.
- For event-related steps, check the runtime registry by
  registering a handler from a Lua config and dispatching from
  Rust -- both paths must hit the same `host-event-<name>`
  slot.

## Deferred work (post-Step-8)

- **Structural per-field `__index` for the `serde_index` userdata
  types (`EgressPathConfig`, `QueueConfig`, `EsmtpDomain`).**
  Status: *deliberately deferred; not delivered by the migration.*
  `serde_index` was made JSON-free in the (b)/(c) follow-on
  (`serde_ser::to_value` instead of `serde_json::to_value` +
  `value_from_json`), but the shingetsu-side `__index` /
  `__pairs` / `__len` still serialize the **entire struct** on
  **every field access** from Lua policy.  Step 8 only renames
  `shingetsu_migrate:: -> shingetsu::` and deletes the mlua
  parity shim; it does **not** change this mechanism, so without
  a dedicated follow-up `serde_index` silently becomes the
  permanent (and O(all-fields)-per-access) read path.

  This is a hot path for an MTA (policy reads these configs
  constantly), so schedule a focused post-Step-8 perf pass.
  Options, increasing payoff/effort:
  1. Leave it (permanent full-struct serialize per access).
  2. Serde-attr-aware structural index: serialize only the
     requested field (keeps `with=` / fidelity, O(1) not
     O(all-fields)).  The hard part is `#[serde(flatten)]` --
     a flattened key is not a named field, so the derive must
     model serde's flatten.  This is where the originally
     rejected "hand-rolled per-field derive" design work
     actually belongs, once shingetsu is the only engine and
     there is no dual-engine parity constraint to preserve.
  3. Make these native tables instead of userdata (no `__index`
     metamethod at all).  Fastest, but reverses the Pattern D
     userdata decision and needs its own justification.
  Recommended: option 2, after the migration completes.

## Common pitfalls

- **Forgetting the registry-key fixup.** Symptoms: handlers
  register successfully but never fire.  Fix: do step 1 before
  anything else.
- **Mixing `mlua::Result` and `Result<_, VmError>` in
  handler bodies during step 4.** The facade's userdata derive
  expects `Result<_, VmError>`; the mlua side translates
  internally.  If a method body uses `?` against an `mlua::Error`
  source, convert the error explicitly.
- **Treating Pattern C as a universal `mlua::FromLua` ->
  `LuaRepr` swap.** kumomta's `mlua::FromLua` derive is a
  userdata downcast, not a structural derive; most sites are
  userdata round-trip types that must defer to Step 4 (Pattern
  D), not convert in Step 2.  See the Pattern C section for the
  triage.  Swapping these blindly produces `IntoLua` blanket-impl
  collisions, unsatisfiable per-field `FromLua` bounds, and enum
  hard-errors.
- **`#[lua(...)]` attribute typos** silently fall back to the
  default behaviour.  After landing a Pattern C commit, exercise
  the affected struct from Lua to make sure custom attributes
  (`rename`, `default`, etc.) actually applied.
- **Memoization across migration boundary.** A type that's
  memoized via mod-memoize must have snapshot support on both
  engines simultaneously, otherwise cache lookups diverge between
  the two paths.  Land snapshot-bearing userdata types before
  switching their callers to the facade.
