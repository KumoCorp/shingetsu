# Migration Plan: shingetsu-migrate facade crate

This document describes a phased plan for building a facade crate that lets
wezterm and kumomta gradually migrate from `mlua` to `shingetsu` for embedded
scripting, with a runtime-selectable engine during the transition and a final
trivial search-and-replace step to drop the facade entirely.

> **Scope of this session.** Only shingetsu (this repo) is in scope. The
> migration of wezterm and kumomta themselves is a separate, later effort —
> we just need to make sure shingetsu and the facade are *ready* for it.

---

## 1. Goals and constraints

1. **Runtime selectability.** The host application must be able to choose
   `mlua` or `shingetsu` at runtime (or feature-flag time) and have all
   user-facing macros, conversions, and userdata behave equivalently on both.
2. **Incremental migration per type.** wezterm and kumomta must be able to
   convert one struct/module/userdata at a time. The facade cannot require
   a flag-day flip.
3. **Trivial final removal.** Once migration is complete, removing the
   facade should be a `s/shingetsu_compat/shingetsu/g`-style refactor (plus
   deletion of the facade dependency). The facade's public API surface must
   therefore mirror shingetsu's spelling exactly — no facade-specific renames.
4. **Don't fork features.** Features that are useful enough to absorb (e.g.
   `SerdeWrappedValue<T>`, `flatten`, `try_from`, callback signatures) belong
   in shingetsu proper, not in the facade. The facade is glue, not a feature
   home.
5. **License hygiene.** Any new dependency added during this work must be
   MIT/Apache-2.0 (per existing project rules).

A non-goal: absorbing wezterm/kumomta's lua context pooling, caching, or
config-loading lifecycle. Those stay in the host applications and integrate
with `shingetsu::GlobalEnv` directly.

---

## 2. Gap analysis

Below is what the consumer codebases use today (via `mlua`,
`mlua-extras`, `wezterm-dynamic`, `serde`, kumomta's `config` crate) that
shingetsu's macro/API surface does not yet support — i.e. the prerequisites
for a credible facade.

### 2.1 `derive(LuaTable)` / `derive(FromLua)` / `derive(IntoLua)` field attributes

Currently supported: `#[lua(rename = "x")]`, `#[lua(default = expr)]`.

Required for parity with `wezterm-dynamic`:

| Attribute | Effect | Used by |
|---|---|---|
| `#[lua(skip)]` | Field absent from both sides of the conversion. | wezterm-dynamic |
| `#[lua(flatten)]` | Inline a sub-struct's fields at this level. | wezterm-dynamic, serde |
| `#[lua(try_from = "T")]` | Field is read as `T`, then `TryFrom::try_from` produces the actual field type; symmetric `Into` on emit. | wezterm-dynamic, serde |
| `#[lua(into = "T")]` | On emit, convert via `Into<T>` before writing to the lua table. | wezterm-dynamic, serde |
| `#[lua(deprecated = "msg")]` | Issue a (configurable) warning when the field appears in incoming lua tables. | wezterm-dynamic |
| `#[lua(validate = "path::to::fn")]` | Run a validation function on the converted value, returning a typed error. | wezterm-dynamic |

Container (struct/enum) attributes:

| Attribute | Effect | Used by |
|---|---|---|
| `#[lua(try_from = "T", into = "T")]` | Whole type round-trips through an intermediate. | wezterm-dynamic, serde |
| `#[lua(default)]` / `#[lua(default = "path")]` | Whole-struct default for absent values. | wezterm-dynamic |
| `#[lua(deny_unknown_fields)]` | Inverse of the current "extra fields ignored" default. Optional but useful for kumomta-style strict configs. | serde |

Enum attributes (parity with `serde(untagged)` and friends):

| Attribute | Effect |
|---|---|
| `#[lua(untagged)]` | Existing behavior — tries variants in priority order. Make this the explicit default. |
| `#[lua(tag = "kind")]` / `#[lua(tag, content)]` | Internally/adjacently tagged — kumomta uses these via serde for queue config. |

The wezterm-dynamic `Color` enum and many kumomta serde types depend on
these — without them the facade cannot offer a search-and-replace path.

### 2.2 `derive(LuaTyped)` and the type checker

Once `flatten`/`try_from`/etc exist, `LuaTyped` must:

- Treat `flatten` by composing the inner struct's `LuaType::Table` fields
  into the outer one.
- Treat `try_from = "T"` as `<T as LuaTyped>::lua_type()` (since that is the
  shape the user actually writes in lua).
- Honor `#[lua(skip)]` (field absent from the typed view).

This is critical for keeping shingetsu's compile-time checking working when
hosts move richly-typed config structs over.

### 2.3 Serde bridge (kumomta's `SerdeWrappedValue<T>`)

kumomta has a generic adapter `SerdeWrappedValue<T>` that gives any
`Serialize + DeserializeOwned` Rust type a `FromLua`/`IntoLua` impl by
serializing through `mlua::LuaSerdeExt`. This is heavily used in kumomta
parameter and return types.

shingetsu needs the analogue, because:

1. There are far more existing `#[derive(Serialize, Deserialize)]` types in
   kumomta than there will be time to convert to `derive(LuaTable)`.
2. Some types genuinely want serde semantics (serde tags, container
   `try_from`, custom impls) and the analog lets them keep that.

Proposed: add `shingetsu::SerdeLua<T>` (name TBD) implementing
`FromLua + IntoLua + LuaTyped` for any `T: Serialize + DeserializeOwned`.
This means shingetsu picks up an optional serde feature flag.

A separate, narrower piece is needed for wezterm: a
`shingetsu::DynamicLua<T: FromDynamic + ToDynamic>` adapter would let the
facade emit a similar "go through `wezterm-dynamic`" bridge without forcing
wezterm-dynamic into shingetsu itself. **Open question** — see §6.

### 2.4 Module-level lazy fields

`#[shingetsu::module]` currently classifies item kinds as:

- `#[function]` — runtime function call.
- `#[field]` — eager field, evaluated once at table-construction time.

Both wezterm and kumomta have host-side state ("get current config",
"version string", "process info") that must be evaluated per access. We
need:

- `#[lazy_field]` — wraps the body in a 0-arg `__index` closure that fires
  each access. Distinct from `#[field]`.
- `#[getter]` / `#[setter]` at module level (paired) for read/write hooks
  on a module table key.

mlua-extras' `#[field]` (lazy) and `#[getter]`/`#[setter]` are the obvious
spelling parallels.

### 2.5 Event/callback infrastructure

Both consumers use the pattern:

```lua
wezterm.on('format-tab-title', function(tab, panes, config, hover, max_width) ... end)
kumo.on('get_queue_config',    function(domain, tenant, campaign, routing) ... end)
```

The Rust side then declares typed signatures (kumomta's `CallbackSignature<A, R>`
+ `declare_event!` macro) and dispatches. This pattern does not currently
exist in shingetsu and must be added because it is too central to leave to
each host.

shingetsu will gain:

- `shingetsu::CallbackRegistry` — owned by `GlobalEnv` (or a host-defined
  extension), maps event name → registered functions.
- `shingetsu::CallbackSignature<A, R>` — typed, with `register()`,
  `name()`, `allow_multiple()`, `call(env, args)`.
- `declare_event!` macro mirroring kumomta's, with the bonus that the
  parameter names *can* be propagated into shingetsu's type system so the
  type checker reports mismatches in user-written handler lambdas. (This
  is a strict superset of what mlua offers.)
- A built-in `on(name, fn)` registration entry point that hosts can mount
  under whatever module name they want (`wezterm.on`, `kumo.on`). Reuses
  shingetsu's existing module/global infrastructure.

This is the single largest new shingetsu feature in the plan and warrants
its own design pass before implementation (per the "ask before arch
decisions" rule).

### 2.6 Userdata snapshot (memoization prerequisite)

A new method on the `Userdata` trait — `fn snapshot(&self) -> Option<Snapshot>` —
plus `Snapshot(Arc<dyn Fn(&GlobalEnv) -> Result<Value, VmError> + Send + Sync>)`,
plus a `#[lua_snapshot]` macro attribute (and a `#[lua(snapshot)]` shorthand
on `derive(UserData)` for `Clone + IntoLua` types). See §6 for details and
rationale.

### 2.7 Did-you-mean diagnostics for unknown fields

wezterm-dynamic's `Error::possible_matches` (see
`../wezterm/wezterm-dynamic/src/error.rs`) uses `strsim::jaro_winkler` over
the known field-name list to suggest corrections when a user assigns to an
unknown struct field. Both wezterm and kumomta have very large config
structs where this is the difference between a usable and an unusable error
message. We must preserve this UX in shingetsu — in two places:

1. **Runtime, in the conversion impls.** When `derive(FromLua)` rejects an
   unknown field (under `#[lua(deny_unknown_fields)]`) or surfaces a
   `#[lua(deprecated = "...")]` warning, the produced `VmError` should
   include ranked suggestions and the full sorted list of remaining valid
   fields, matching wezterm's existing message shape.
2. **Compile time, in the type checker.** When the user assigns to a field
   that doesn't exist on a known `LuaType::Table` (or calls a method that
   doesn't exist on a `LuaType::Userdata`), the diagnostic should likewise
   suggest the closest match. This is a strict superset of mlua's runtime
   behavior — it lights up before the user even runs their config.

Design points:

- **Single helper crate-internal API.** Add
  `shingetsu_vm::diagnostics::suggest_field(used: &str, possible: &[Bytes])
  -> Vec<Bytes>` that encodes wezterm's algorithm (jaro_winkler > 0.8,
  ranked descending, then list the rest sorted). All callers — derive,
  type checker, the upcoming event registry — share it.
- **Field-list propagation.** `derive(LuaTyped)` already produces a
  `LuaType::Table` with a known field list; that list flows to both the
  type checker and (via a new `FromLua`-side static slice) to the runtime
  error path. The macro emits the static `&[&'static [u8]]` once and
  references it from both sides.
- **Threshold and tuning are shared.** wezterm's 0.8 threshold is the
  default; expose it as a const so docgen and tests reference the same
  value.
- **Reuse for event names — with an openness policy** (see §2.7.1).
- **Reuse for callback parameter names**, e.g. typed `#[function]`
  keyword-argument names if/when those land.
- **Dependency.** Adds `strsim` (MIT) to shingetsu — small, stable,
  no-std-friendly. License is fine.

This is small in implementation surface but high-leverage for the
migration UX, so it should land in Phase 0 alongside the other
`derive(LuaTable)` parity work, and be wired into the type checker as
part of Phase 1 once the field metadata is reachable from the
diagnostic site.

#### 2.7.1 Open vs. closed name sets (event registry)

Event names are not a closed set in the way struct fields are. Two
important counter-examples:

- **kumomta dynamic constructors.** A user writes
  `kumo.on('my.constructor', fn)` and elsewhere in their config tells
  the engine "use `my.constructor` to build a thing". There is no
  formal Rust-side `CallbackSignature` for `my.constructor` — its
  signature is only known when the constructor is invoked.
- **wezterm user-emitted events.** A user can `wezterm.emit('my.thing',
  ...)` and `wezterm.on('my.thing', fn)` for events that have no host
  declaration at all.

In both cases, naïvely flagging "unknown event name" with a
Jaro-Winkler suggestion would generate false positives whenever a user
legitimately picks a fresh name. The suggester must therefore be aware
of an **openness policy** per registry namespace.

Proposed model on the `CallbackRegistry`:

```rust
pub enum NamePolicy {
    /// Every name must be statically registered; misspells are errors.
    Closed,
    /// Unknown names are accepted; emit a suggestion *only* when there is
    /// a close static match AND the typed name is not itself in the
    /// registry. Suggestions are advisory (warn-level), not errors.
    /// This is the default for hosts like kumo/wezterm.
    OpenWithSuggestions,
    /// Unknown names are accepted silently; never suggest.
    Open,
}
```

Differentiating runtime and compile-time visibility:

- **Runtime.** The set of "known" names at runtime can grow as the host
  initializes — e.g. when kumomta's Rust-side configuration parses a
  reference to `my.constructor`, the host can call
  `registry.declare_dynamic(name)` to register the name as expected
  before the lua chunk runs (or before `kumo.on` is called from lua).
  Only names not in either the static or dynamic sets are candidates
  for suggestion under `OpenWithSuggestions`.
- **Compile time.** The type checker only sees the static set (via
  `declare_event!`). It must therefore treat compile-time event-name
  suggestions as **soft warnings**, not errors, regardless of policy —
  it lacks the dynamic set. A `--deny-unknown-events` lint flag can
  promote them to errors for projects that have made the trade-off to
  declare every event statically.
- **Threshold tuning for events.** The 0.8 jaro_winkler threshold is
  appropriate for struct fields where suggesters fire on every
  unknown key. For events under `OpenWithSuggestions`, we should bump
  the threshold (provisionally 0.9) to bias against false positives;
  expose this as a separate const so it can be tuned independently.
- **Opt-in fast-path.** A host can call
  `registry.declare_dynamic(name)` proactively; the same call can also
  attach a `CallbackSignature` later, upgrading the entry from
  "dynamic, untyped" to "dynamic, typed". This makes the
  late-registration kumomta pattern explicit rather than implicit.

Facade implications: `shingetsu_migrate::declare_event!` writes to the
static set. `shingetsu_migrate::declare_dynamic_event(name, [sig])`
becomes the migration target for kumomta's current pattern of
"register name through configuration". On the mlua side the call is
a no-op; on the shingetsu side it populates the dynamic set so that
type-checking and did-you-mean stay accurate.

#### 2.7.2 User-defined event opt-out at registration time

The §2.7.1 model assumes the host can declare a dynamic name *before*
the user's `.on()` call. That breaks down when names are produced by
n-th-order data dependencies — e.g. one event handler computes a value
that names a constructor that a later handler registers. The host
cannot declare what it doesn't yet know.

The registry therefore exposes a per-call opt-out:

```rust
impl CallbackRegistry {
    /// Standard registration. Under `OpenWithSuggestions`, an unknown name
    /// that is close to a known one emits an advisory warning. The name
    /// is then added to the dynamic set.
    pub fn register(&self, name: Bytes, func: Function) -> Result<(), VmError>;

    /// Registration that asserts the name is intentionally user-defined.
    /// The suggester check for *this* call is skipped; the name is still
    /// added to the dynamic set so future lookups and registrations can
    /// use it as a suggestion target.
    pub fn register_user_defined(&self, name: Bytes, func: Function) -> Result<(), VmError>;
}
```

The marker is per-call, not name-scoped. If a user registers
`'my.constructor'` as user-defined and later registers `'my.constructo'`
without the marker, the second call still trips the suggester against
the first — the typo is caught.

**Host-facing spelling.** Hosts choose between equivalent shapes:

- *Marker argument*: `kumo.on(name, fn, { user_defined = true })` or
  `kumo.on(name, fn, kumo.USER_DEFINED)`.
- *Separate function*: `kumo.on_user(name, fn)`,
  `wezterm.on_user(name, fn)`.

The facade supplies both via thin helpers; per-host migration playbooks
pick a canonical spelling. Recommendation: marker-argument form, because
it composes with future flags and keeps a single registration entry
point per host module.

**Explicitly rejected: naming-convention magic.** A rule like "names
starting with `user.` automatically opt out" is invisible at the call
site, ungreppable, and hostile to migration across hosts that pick
different prefixes. The opt-out must be explicit at the call site.

### 2.8 Smaller userdata / module gaps

These are minor but have come up while reading the consumer codebases:

- **Verify `&Lua`-equivalent injection in `#[function]`.** Both crates' free
  functions take `&Lua` to construct tables. shingetsu's analog is
  `&GlobalEnv`. Confirm the macro recognizes that param and skips it from
  the lua-visible signature.
- **Variadic** is supported (`#[function(variadic)]`); should also be
  available on userdata methods. Audit and document.
- **Error context.** kumomta uses `mlua::Error::external`. The facade will
  funnel through shingetsu's `VmError` + `VmResultExt` (per project rule).
- **`&'static str` keys in `CallbackSignature` argument tuples.** kumomta's
  `declare_event!` accepts `&'static str` because mlua's `IntoLuaMulti`
  has impls for it. shingetsu needs the same impls (audit needed).

---

## 3. Facade crate design (`shingetsu-migrate`)

Approach: **emit both mlua and shingetsu impls** behind feature flags, since
the migration goal is *runtime* selection (both engines live in the same
binary at once).

### 3.1 Crate layout

```
crates/shingetsu-migrate/
  Cargo.toml         # deps: shingetsu, mlua (optional), mlua-extras (optional, git pin),
                     #       wezterm-dynamic (optional), serde
  src/lib.rs
  src/serde.rs       # SerdeLua<T> wrappers for both backends
  src/dynamic.rs     # DynamicLua<T> wrappers (wezterm-dynamic interop)
  src/event.rs       # CallbackSignature<A,R> + on() shim that targets either backend
  src/memoize.rs     # Memoized + impl_memoize shims (§6.6)
  src/runtime.rs     # Engine trait + selector
crates/shingetsu-migrate-derive/   # proc-macro crate
  src/lib.rs         # re-exports / facade-flavored macros
  src/module.rs
  src/userdata.rs
  src/lua_struct.rs
```

### 3.2 Cargo features

```
[features]
default = ["mlua-backend", "shingetsu-backend"]
mlua-backend       = ["dep:mlua", "dep:mlua-extras", ...]
shingetsu-backend  = ["dep:shingetsu", "dep:shingetsu-derive"]
wezterm-dynamic    = ["dep:wezterm-dynamic"]   # for DynamicLua<T>
serde              = ["dep:serde"]             # for SerdeLua<T>
async              = [...]
```

When **both** backends are enabled (the migration steady state), every
generated impl produces both an mlua impl and a shingetsu impl. The host
chooses at runtime which engine to drive.

When only `shingetsu-backend` is enabled (post-migration), the macros are
identical to the underlying `shingetsu::*` macros and removing the facade
becomes a search-and-replace of `shingetsu_compat::` → `shingetsu::`.

### 3.3 Macro surface (must mirror shingetsu's spelling)

Re-exports under `shingetsu_compat`:

| Facade name | Expands to |
|---|---|
| `#[shingetsu_compat::module]` | `#[shingetsu::module]` + mlua-extras `Module` registration |
| `#[shingetsu_compat::userdata]` | `#[shingetsu::userdata]` + `#[mlua_extras::user_data_impl]` |
| `#[derive(shingetsu_compat::UserData)]` | `derive(shingetsu::UserData)` + `derive(mlua_extras::UserData)` |
| `#[derive(shingetsu_compat::LuaTable)]` | `derive(shingetsu::LuaTable)` + corresponding mlua glue |
| `#[derive(shingetsu_compat::FromLua)]`, `IntoLua`, `LuaTyped` | parallel duals |
| `#[derive(shingetsu_compat::IntoLuaMulti)]`, `FromLuaMulti` | parallel duals |
| `shingetsu_compat::declare_event!` | one signature object per backend |

Inner attributes (`#[function]`, `#[field]`, `#[lazy_field]`, `#[lua_method]`,
`#[lua_field]`, `#[lua_metamethod]`, `#[lua(rename = ...)]`, …) keep their
shingetsu spellings. The facade's macros parse them, then re-emit them
unchanged into the shingetsu invocation while translating into the
mlua-extras spelling for the mlua side.

### 3.4 Bridge types

- `shingetsu_compat::SerdeLua<T>`: when `mlua-backend` is on, implements
  `mlua::FromLua + mlua::IntoLua` via `LuaSerdeExt`. When `shingetsu-backend`
  is on, delegates to `shingetsu::SerdeLua<T>`. Drop-in for kumomta's
  `SerdeWrappedValue<T>` — kumomta migrates by `s/SerdeWrappedValue/SerdeLua/g`.
- `shingetsu_compat::DynamicLua<T>` for `T: FromDynamic + ToDynamic`:
  on the mlua side, calls into `luahelper`'s `to_lua`/`from_lua`; on the
  shingetsu side, walks the dynamic value through shingetsu's `Value` tree.
  This replaces wezterm's `impl_lua_conversion_dynamic!` macro.
- `shingetsu_compat::CallbackSignature<A, R>`: thin wrapper around either
  kumomta-style `mlua::CallbackSignature` or `shingetsu::CallbackSignature`,
  picked at runtime via the engine selector.

### 3.5 Runtime engine selection

```rust
pub enum Engine {
    Mlua(MluaContext),
    Shingetsu(ShingetsuContext),
}
```

Hosts hold an `Engine`, and the facade exposes:

- `Engine::load(path)`, `Engine::call_event(sig, args)`,
  `Engine::with_globals(|g| ...)`.
- A small set of trait objects (`dyn EventHost`) so individual host
  features (e.g. wezterm's `format-tab-title`) can be coded once against
  the facade.

The selector is a pure dispatch shim — no heavy abstraction. The host's
existing pool/cache layer wraps an `Engine` instead of an `mlua::Lua`.

### 3.6 Per-type incrementality

Every conversion lives entirely in derive output. A struct that has
`#[derive(shingetsu_compat::LuaTable)]` works on both engines simultaneously
without touching any sibling type. So a host can convert types one at a
time, mixing converted and unconverted types in the same Lua context: the
unconverted `mlua::FromLua`/`IntoLua` types still compile and run on the
mlua engine, and converted ones additionally run on shingetsu.

Rule of thumb during migration:

- "Convert to facade derive" = both engines work, but the type still has
  `mlua::Lua`-only callers if any.
- "Convert callers" = call sites use `Engine::*` instead of raw `mlua::Lua`.
- Do these two passes independently; don't gate one on the other.

### 3.7 Canonical migration patterns

This is the catalogue of mlua-side patterns hosts will encounter
during migration and the facade-supported target form for each.
The Phase 8 playbooks reference this section instead of restating
the rules.

**Pattern A: `SerdeWrappedValue<T>` (or equivalent serde wrapper)**

The host wraps a `Serialize + DeserializeOwned` type to expose it
through mlua's serde bridge.

Migration target: `shingetsu::SerdeLua<T>`.  Same wrapping shape,
same deref semantics, same fallibility caveats (§6.2 in this
document).  Mechanical rename plus a `use shingetsu::SerdeLua` import
(or facade re-export during the transition).

**Pattern B: bare `T: Serialize + DeserializeOwned` with manual
`from_lua_value(lua, v)` / `to_value_with(...)` call sites**

The host converts lua values to and from a serde-friendly type at
specific call sites instead of wrapping them.  The resulting code is
repetitive (every call site repeats the conversion) and tightly
couples each call to mlua's API.

Migration target: replace the parameter type at the call site with
`SerdeLua<T>` and let the function-call dispatcher handle the
conversion automatically.  No more manual conversion calls.  Keep the
`Serialize + DeserializeOwned` derives; they're what `SerdeLua<T>`
requires anyway.

**Pattern C: `derive(mlua::FromLua)`**

mlua provides a `FromLua` derive but no corresponding `IntoLua`
derive (round-trip is one-way unless you implement the other side
by hand).  Hosts using this derive typically have either a
hand-written `IntoLua` companion or just don't need to round-trip.

Migration target: `#[derive(shingetsu::LuaTable)]`, which produces
`FromLua`, `IntoLua`, and `LuaTyped` in one go and supports the
full `#[lua(...)]` attribute set (`rename`, `default`, `flatten`,
`try_from`, `into`, `validate`, `deny_unknown_fields`, enum
tagging).  During the transition, use
`#[derive(shingetsu_migrate::LuaTable)]` instead — same attribute
surface, but the derive emits both shingetsu-side and mlua-side
impls from a single source of truth, so the two engines stay in
lockstep without parallel `#[serde(...)]` annotations.  Migration
is a search-and-replace of `shingetsu_migrate::` → `shingetsu::`.

**Pattern D: `impl mlua::UserData for T { fn add_methods, fn add_fields }`**

mlua's `UserData` trait is implemented as a *single impl block*
with methods like `add_methods` that receive a registry builder.
The layout is structurally different from shingetsu's per-method
attribute model.

Migration target: `#[shingetsu::userdata] impl T { ... }` with each
method marked `#[lua_method]`, fields marked `#[lua_field]`,
metamethods marked `#[lua_metamethod(Name)]`.  Memoization
opt-in becomes `#[lua_snapshot]` or `#[lua(snapshot)]` (§6.6).
The rewrite is structural — no search-and-replace covers it — but
the per-host playbooks call out a recommended commit shape
(typically: one userdata type per commit; the type goes through
the migration alongside its callers in the same commit).

The `impl mlua::UserData` block can stay in place during the
transition (mlua engine continues to use it) while the
`#[shingetsu::userdata]` form is added alongside.  Both can
coexist on the same Rust type until the host flips off the mlua
engine.

---

## 4. Phased delivery

Each phase ends with a pause for review (per project rule). Tick boxes
as work lands; phase headings carry a status marker (🔴 not started /
🟡 in progress / ✅ complete) so progress is scannable from the index.

**Status overview**

- [x] ✅ Phase 0 — Prerequisites in `shingetsu-derive`
- [x] ✅ Phase 1 — Bridge types in shingetsu
- [x] ✅ Phase 1.5 — Userdata snapshot / memoization primitives
- [x] ✅ Phase 2 — Facade scaffolding
- [x] ✅ Phase 3 — Conversion derive facade
- [ ] 🟡 Phase 4 — `#[module]` and `#[userdata]` facade
- [ ] 🔴 Phase 5 — wezterm-dynamic interop
- [ ] 🔴 Phase 6 — Event registry facade
- [ ] 🔴 Phase 7 — Docgen and definition-file generation
- [ ] 🔴 Phase 8 — Migration playbooks (docs only)

### Phase 0 — Prerequisites in `shingetsu-derive` ✅

Goal: bring `derive(LuaTable)` and `#[module]` to feature parity with
wezterm-dynamic and mlua-extras for the attributes that consumers actually
use today.

- [x] **Field attributes**: add `skip`, `flatten`, `try_from`, `into`,
      `deprecated`, `validate`. Update `gen_type_fields` so `LuaTyped`
      honors them.
- [x] **Container attributes**: add `try_from`, `into`, `default`,
      `deny_unknown_fields`.
- [x] **Enum tagging**: add `tag` / `tag + content` modes; document
      `untagged` as the default.
- [x] **Module-level**: add `#[lazy_field]` and `#[getter]`/`#[setter]`.
- [x] **Did-you-mean diagnostics (runtime)**: add the `suggest_field`
      helper in `shingetsu-vm` (jaro_winkler > 0.8, ranked, then sorted
      remainder), thread the field list from `derive(LuaTyped)` into
      the static metadata used by `derive(FromLua)`, and emit
      suggestions in the unknown-field / deprecated-field error paths.
      See §2.7. Adds `strsim` (MIT) as a workspace dependency.
- [x] **Audit**: confirm `&GlobalEnv` injection works in `#[function]`
      bodies; confirm `#[function(variadic)]` parity on userdata
      methods.
- [x] **Tests**: extend the existing trybuild + integration tests for
      each new attribute. Add a fixture that exercises wezterm-style
      typo suggestions against a struct with ≥50 named fields and
      asserts the rendered error matches expected output via
      `k9::assert_equal!`. No facade work yet.

### Phase 1 — Bridge types in shingetsu ✅

- [x] Add the `Value ↔ serde_json::Value` bridge behind a `serde`
      feature.
- [x] Add `shingetsu::SerdeLua<T>` on top of that bridge; integrate
      with `LuaTyped`.
- [x] Add `shingetsu::CallbackRegistry`, `CallbackSignature<A, R>`,
      `declare_event!`. Wire the registry into `GlobalEnv` (or a
      default extension) and provide an `on(name, fn)` registration
      helper.
- [x] Type-check integration: a registered event signature contributes
      its parameter types to shingetsu's compiler so handler lambdas
      are checked on load. Two-stage delivery:

      - **Stage A (done)**: `CallbackSignature` and `declare_event!`
        capture parameter names + per-param `LuaType` + return
        `LuaType` so the compiler has metadata to read.
      - **Stage B**: wire signature lookup into the compiler's
        function-call checker.  When a host's `on(name, fn)` global
        is called with a string-literal name and a lambda, the
        compiler validates the lambda against the looked-up
        signature.

      Stage B rules:

      - **Arity is forward-compatible.** A handler accepting *fewer*
        parameters than the signature is OK — lets signatures grow
        new optional parameters without breaking existing scripts.
        A handler accepting *more* than the signature triggers a
        warning since extra positions will always be `nil`.
      - **Variadic handlers** (`function(...)`) skip arity / name
        checks entirely.
      - **Parameter name matching is abbreviation-tolerant** (see
        below).  We do not require user scripts to use the canonical
        name from the signature.  Migration must not force users to
        rename `function(msg)` to `function(message)` just because
        the host happens to spell the parameter `message` in Rust.
      - **Findings emit as a new `LintId::EventHandlerSignature`**,
        default `Warning`, suppressible via
        `--allow event_handler_signature` and project lint config.

      Parameter-name matching strategy:

      1. Run pairwise Jaro-Winkler between each handler parameter
         name and each signature parameter name.
      2. For each handler parameter at position `i`, find the
         signature parameter most similar to it.
      3. If the best match is at the same position (or no name in
         the signature exceeds a loose similarity threshold), accept
         silently.  This covers `msg` ↔ `message`, `req` ↔ `request`,
         `s` ↔ `state`, etc.  Single-letter or short abbreviations
         that fall under the threshold are also silently accepted
         — the goal is *catching swaps*, not *enforcing canonical
         names*.
      4. If the best match is at a *different* position **and** the
         handler's counterpart at that other position is itself best
         matched back to position `i`, flag as a likely
         transposition with a did-you-mean-style suggestion naming
         the suspected swap.
      5. Otherwise (handler used a wholly novel name with no close
         match anywhere), accept silently.

      Test matrix Stage B must cover:

      | Signature | Handler | Expected |
      |---|---|---|
      | `(message)` | `function(msg)` | accept silently |
      | `(message)` | `function(text)` | accept silently |
      | `(message)` | `function(m)` | accept silently |
      | `(message, domain)` | `function(domain, message)` | warn: transposition |
      | `(message, domain)` | `function(msg, dom)` | accept silently (same positions, abbreviated) |
      | `(message, domain)` | `function(dom, msg)` | warn: transposition (abbreviations on swapped positions) |
      | `(message)` | `function(...)` | accept silently (variadic) |
      | `(message, domain)` | `function(message)` | accept silently (forward-compat fewer params) |
      | `(message)` | `function(message, extra)` | warn: extra parameters always nil |

      **Pause for design discussion before implementing Stage B**
      — the matching strategy goes beyond mlua's behavior.

      **Stage B follow-ups (deferred)**:

      - **Cross-function return-value tracking.**  When a handler is
        sourced from a factory pattern —
        ```lua
        local helper = setup_with_automation { ... }
        host.on('ev', helper.get_egress_path_config)
        ```
        — the type checker would need to track that `helper` holds the
        return value of `setup_with_automation()`, that the return
        value is a table whose `get_egress_path_config` field aliases
        an inner `local function`, and propagate the inner function's
        parameter info to the registration site.  This pattern is
        widely used in the kumomta policy-extras corpus, so the
        check currently misses a meaningful fraction of real-world
        registrations.  Solving it requires either (a) limited
        whole-chunk dataflow that records function return-value
        shapes and field-alias chains, or (b) a heuristic pass that
        scans `local NAME = expr` patterns to record
        `b"helper.foo"` keys when `helper` is assigned a literal
        table containing function-typed fields.  Pick when this
        starts blocking real migrations — not before.
- [x] **Did-you-mean diagnostics (compile time)**: wire the
      `suggest_field` helper into the type checker so unknown-field
      assignments and unknown-method calls on typed tables/userdata
      produce ranked suggestions. For event names, honor the
      registry's `NamePolicy` (§2.7.1) — only emit suggestions for
      `Closed` namespaces or as soft warnings under
      `OpenWithSuggestions`, and never under `Open`. Surface
      `--deny-unknown-events` as an opt-in lint that promotes those
      soft warnings to errors. Reuses the helper added in Phase 0.
- [x] **Dynamic event declaration API**: add
      `CallbackRegistry::declare_dynamic(name)` and a typed-upgrade
      variant for hosts to register names parsed out of their
      configuration before the lua chunk runs. Document the kumomta
      constructor and wezterm `emit` patterns as the canonical use
      cases.
- [x] **Per-call user-defined opt-out**: add
      `CallbackRegistry::register_user_defined(name, func)` for the
      n-th-order dependency case where the host cannot pre-declare
      the name (§2.7.2). Verify that subsequent typo registrations
      against the same name still trigger suggestions. Document the
      recommended marker-argument spelling for hosts.

### Phase 1.5 — Userdata snapshot / memoization primitives ✅

- [x] Add `Userdata::snapshot()` and the `Snapshot` newtype to the
      core `Userdata` trait.
- [x] Add `#[lua_snapshot]` attribute support to
      `#[shingetsu::userdata]` and a `#[lua(snapshot)]` shorthand on
      `derive(UserData)`.
- [ ] Have the macro additionally register a `__memoize`-named
      metamethod that delegates to `snapshot()`, so existing mlua-side
      conventions keep working through the facade.

      **Deferred to the migration-facade work**: the lua-callable
      `__memoize` metamethod is the convention used by the existing
      mlua-side cache walkers.  In shingetsu, host code that owns
      the cache can simply call `userdata.snapshot()` from Rust —
      no metamethod indirection required.  Hosts migrating from
      mlua keep working via the facade (which can polyfill
      `__memoize` -> `snapshot()` on the mlua backend); shingetsu-
      native hosts skip the metamethod entirely.  Revisit if a use
      case appears for a lua-level inspection of "is this userdata
      memoizable" that can't go through Rust.
- [x] Audit `__pairs` support on userdata so kumomta's `MemoizedTable`
      analog can be built host-side without shingetsu changes.

      **Audit finding (2026-05-08)**: the `pairs()` builtin currently
      takes a `Table` argument and rejects userdata at conversion
      time, so the `__pairs` metamethod on a userdata is unreachable
      through `for k, v in pairs(ud) do ... end`.  The metamethod
      *is* dispatched by `Userdata::dispatch("__pairs", ...)` if
      something looks for it directly, but there is no path from
      lua-level `pairs()` to that dispatch.

      **Follow-up needed before a userdata-iterable cache can be
      built**: relax `pairs(value)` to accept either a `Table` or a
      `Userdata`.  When given a userdata, look up its `__pairs`
      metamethod via `Userdata::dispatch` (or a sync fast path),
      call it, and return its results.  Mirror the same change for
      `ipairs` and the implicit `for ... in obj do` desugaring if
      it bypasses `pairs`.  Tracked here so it lands alongside the
      first cache-style userdata that depends on it.
- [x] Tests: a userdata round-trips through a snapshot into a fresh
      `GlobalEnv` and exposes the same fields/methods.

No cache, lruttl, metrics, or epoch logic enters shingetsu — those
remain entirely in kumomta's `mod-memoize`.

### Phase 2 — Facade scaffolding ✅

- [x] Create the empty `shingetsu-migrate` and `…-derive` crates with
      feature flags wired but no real codegen yet.
- [x] Add a smoke test: a `derive(LuaTable)` struct that compiles on
      each feature combination.

### Phase 3 — Conversion derive facade ✅

- [x] `derive(LuaTable)`, `FromLua`, `IntoLua`, `LuaTyped`,
      `IntoLuaMulti`, `FromLuaMulti` re-exported. Each emits both a
      shingetsu impl and an mlua-extras / serde-based mlua impl.

      Approach taken: a unified `shingetsu_migrate::LuaTable` derive
      that emits both shingetsu-side and mlua-side impls from a
      single `#[lua(...)]` source of truth.  No parallel
      `#[serde(...)]` annotations needed; both engines see the same
      attribute set and produce identical observable behavior.

      ```rust
      use shingetsu_migrate::LuaTable;
      #[derive(LuaTable)]
      struct Config {
          #[lua(rename = "x-pos")]
          x: i64,
          #[lua(default = 7)]
          y: i64,
      }
      ```

      Migration step: search-and-replace `shingetsu_migrate::` for
      `shingetsu::` (or change the `use` import).  The derive name
      stays `LuaTable`.

      Implementation lives in a new `shingetsu-derive-impl` library
      crate that holds the codegen for both engines.
      `shingetsu-derive` and `shingetsu-migrate-derive` are now thin
      proc-macro wrappers over that library; the shingetsu-side
      output is bit-identical to the previous codegen, and the
      mlua-side output mirrors the same `#[lua(...)]` semantics via
      mlua's `Table::get`/`set` API.

      Initial mlua-side coverage is structs with all the field- and
      container-level attributes (`rename`, `default`, `skip`,
      `flatten`, `try_from`, `into`, `validate`,
      `deny_unknown_fields`, container `try_from`/`into`/`default`).
      Enum tagging is stubbed with a clear compile error pointing
      at this MIGRATE.md note; it lands once a test corpus exercises
      tagged enums through both engines.
- [x] Property test: a fixed corpus of structs (tagged enums,
      optional fields, flattened nested structs, try_from-wrapped
      types) round-trips through both engines and produces identical
      observable behavior.

      Initial corpus covers: simple structs, structs with `Option`,
      structs with float fields.  Tagged enums, flattened, and
      try_from-wrapped types are deferred to follow-up turns once
      the corresponding `#[lua(...)]` ↔ `#[serde(...)]` attribute
      translation is documented for each.

### Phase 4 — `#[module]` and `#[userdata]` facade 🟡

- [ ] `#[shingetsu_migrate::module]` parses the shingetsu-style body,
      re-emits it for shingetsu, and emits an mlua-extras `Module`
      registration for mlua. Async, variadic, and lazy fields
      supported on both sides.
- [ ] `#[shingetsu_migrate::userdata]` similar, with metamethod
      mapping between shingetsu's `Add`/`Sub`/… spelling and
      mlua-extras' `MetaMethod::Add`/etc.

### Phase 5 — wezterm-dynamic interop 🔴

- [ ] `DynamicLua<T>` adapter. Verify it round-trips the wezterm
      `Config` struct on both engines (using a copy of wezterm's test
      fixtures, not modifying wezterm).
- [ ] Document the `impl_lua_conversion_dynamic!` →
      `derive(LuaTable)` (or `DynamicLua<T>`) translation patterns
      for the wezterm migration team.

### Phase 6 — Event registry facade 🔴

- [ ] `shingetsu_migrate::declare_event!` produces signature objects
      whose methods dispatch to either backend.
- [ ] `Engine::call_event(sig, args)` integration tests that exercise
      both `allow_multiple` and single-handler events, confirming
      behavior parity with kumomta's existing `CallbackSignature::call`.

### Phase 7 — Docgen and definition-file generation 🔴

- [ ] Verify shingetsu's existing docgen still works through the
      facade (the shingetsu macro side is untouched).
- [ ] Optionally emit `mlua-extras` `DefinitionFileGenerator` hooks so
      consumers get LuaLS `.d.lua` files for free during the
      transition.
- [ ] **`declare_event!` doc capture.** Extend the macro to accept and
      preserve documentation about the event — a summary describing
      when the host invokes the handler, per-parameter docs, and an
      optional return-value description — then surface that metadata
      on the generated `CallbackSignature` so it flows into shingetsu
      docgen output. The shape should mirror what `#[function]` and
      `#[lua_method]` already capture from rustdoc on the underlying
      Rust item, so docgen sees event handlers as documented entry
      points alongside functions and methods. This is the analog of
      kumomta's existing per-event docs hook in its `declare_event!`.

### Phase 8 — Migration playbooks (docs only) 🔴

- [ ] wezterm playbook: type-by-type conversion order, suggested
      commit shape, what to do about `__wezterm_to_dynamic` metamethod
      consumers, and the canonical patterns from §3.7 as they apply
      (wezterm leans more heavily on `wezterm-dynamic` than on
      direct serde, so Pattern A / B are less common; Pattern C
      and D dominate).
- [ ] kumomta playbook: walk through each canonical pattern from
      §3.7 in order — `SerdeWrappedValue` (Pattern A), manual
      `from_lua_value` call sites (Pattern B), `derive(mlua::FromLua)`
      occurrences (Pattern C), `impl mlua::UserData` blocks
      (Pattern D) — with concrete examples drawn from the kumomta
      tree.  Then `declare_event!` →
      `shingetsu_migrate::declare_event!`, then `mod-memoize` port
      (per §6.6), then config pool integration.  Recommend a
      one-userdata-per-commit cadence for Pattern D since each
      rewrite is structural.
- [ ] Final-removal recipe: search-and-replace pattern
      (`shingetsu_migrate` → `shingetsu`), feature-flag flip, facade
      dependency removal.

---

## 5. Resolved decisions

1. **`SerdeLua<T>` lives in `shingetsu`** behind a `serde` feature flag.
   It is generally useful and remains a first-class option after the
   facade is removed.
2. **Event registry lives in `shingetsu` core.** It is a primary feature
   and one of the motivations for type-checked handler lambdas.
3. **`DynamicLua<T>` is facade-only.** wezterm-dynamic is wezterm's
   concern and shouldn't bleed into shingetsu.
4. **Type checking of event handler lambdas is a primary goal.** Phase 1
   delivers it as part of the event-registry work, not as a stretch goal.
5. **`mlua-extras` is pinned to a specific git rev** in the facade's
   `Cargo.toml`. We accept the pre-release status; we will bump the pin
   intentionally rather than tracking `main`.
6. **Facade crate name: `shingetsu-migrate`.**

## 6. Memoization-readiness

kumomta's `mod-memoize` (see `../kumomta/crates/mod-memoize/src/lib.rs`)
lets the host cache lua return values across `mlua::Lua` lifetimes. The
memoization layer itself stays in kumomta — it is tightly coupled to
`lruttl`, prometheus metrics, and the `ConfigEpoch` system. What we need
from shingetsu is the same set of primitives that mlua exposes today, so
kumomta can port `mod-memoize` to shingetsu without reinventing
cross-context plumbing.

Four primitives are required:

### 6.1 Userdata snapshot hook

In mlua, kumomta uses a `__memoize` metamethod that returns a
`Memoized { Arc<dyn Fn(&Lua) -> Value + Send + Sync> }` closure. That
closure can rebuild the userdata in any future `Lua` context.

Shingetsu equivalent — a first-class trait method:

```rust
pub trait Userdata: DowncastSync {
    // ... existing methods ...

    /// Produce a closure that can re-materialize this value in a different
    /// `GlobalEnv`. Returning `None` (the default) means the value is not
    /// memoizable.
    fn snapshot(&self) -> Option<Snapshot> { None }
}

pub struct Snapshot(pub Arc<dyn Fn(&GlobalEnv) -> Result<Value, VmError> + Send + Sync>);
```

Derivation support:

- `#[lua_snapshot]` attribute on a method inside a `#[shingetsu::userdata]`
  impl block, OR
- a `#[lua(snapshot)]` flag on `derive(UserData)` for `Clone` types that
  just want "clone self, then `IntoLua` later" (kumomta's most common
  case).

The shingetsu macro should also emit a `__memoize`-named metamethod entry
when the snapshot hook is present, so any host code that already speaks
the mlua convention (including wrapper crates) can detect snapshottable
values uniformly.

This is a small, well-defined addition to the core `Userdata` trait. It
does not pull `lruttl` or any cache machinery into shingetsu.

### 6.2 Lua value ↔ `serde_json::Value` bridge

kumomta's `CacheValue::Json` and its hash-key derivation both depend on
`mlua::LuaSerdeExt`'s `to_value` / `from_value`. shingetsu has no
equivalent today.

Add (behind the `serde` feature, alongside `SerdeLua<T>`):

```rust
pub fn value_to_serde<S: serde::Serializer>(v: &Value, s: S) -> Result<S::Ok, S::Error>;
pub fn value_from_serde<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Value, D::Error>;
// or, more ergonomically:
pub fn value_to_json(v: &Value) -> Result<serde_json::Value, VmError>;
pub fn value_from_json(env: &GlobalEnv, j: serde_json::Value) -> Result<Value, VmError>;
```

`SerdeLua<T>` should be implemented in terms of these primitives so the
two features share a single conversion path.

### 6.3 Send + Sync + 'static guarantees on userdata

shingetsu's `Userdata` trait already requires `DowncastSync` (effectively
`Send + Sync + 'static`), and the snapshot closure above is
`Send + Sync + 'static`, so no change is required here. Worth calling
out because it's a hard prerequisite for cross-VM memoization to work.

### 6.4 Metamethods needed by `MemoizedTable`-style proxies

kumomta builds a copy-on-write `MemoizedTable` userdata exposing
`__index`, `__newindex`, `__len`, and `__pairs`. shingetsu already
supports all four (`MetaMethod::{Index, NewIndex, Len, Pairs}`) — so
kumomta can build the analogous host-side proxy with no shingetsu
changes. We just need to keep `__pairs` working for userdata returning
iterators (audit during Phase 0).

### 6.5 What stays out of shingetsu

Not in scope for shingetsu:

- The cache itself (`lruttl`, TTLs, capacity, name registry).
- Per-cache prometheus metrics.
- `ConfigEpoch` / cache-invalidation semantics.
- The `kumo.memoize` registration entrypoint.
- The `MemoizedTable` proxy userdata — kumomta builds this on top of
  shingetsu's metamethod support.

This matches kumomta's structure: `mod-memoize` is a kumomta-internal
crate that consumes scripting-engine primitives, and the migration step
is to swap those primitives from `mlua` to `shingetsu`.

### 6.6 Migration impact

When kumomta ports `mod-memoize` to shingetsu:

- `Memoized::impl_memoize(methods)` calls in kumomta's userdata impls
  become a single `#[lua_snapshot]` attribute (or, for trivial
  `Clone + IntoLua` types, a `#[lua(snapshot)]` flag on `derive(UserData)`).
- `CacheValue::from_lua` calls `userdata.snapshot()` instead of
  `mt.get::<Function>("__memoize")` + `func.call(...)`.
- `multi_value_to_json_value` / `from_lua_value` / `to_value_with` calls
  become `value_to_json` / `value_from_json` / `serde_options()` calls
  on shingetsu.

The facade should provide compatibility shims so individual userdata
types can be migrated independently:

- `shingetsu_migrate::Memoized` — a re-export that resolves to the
  appropriate per-engine type at runtime.
- `shingetsu_migrate::impl_memoize<T>(methods)` — a no-op on the
  shingetsu side (the snapshot is on the trait), and the existing
  `Memoized::impl_memoize(methods)` glue on the mlua side.

This adds a small **Phase 1.5** to the plan (between bridge types and
the facade scaffolding) for snapshot trait + serde_json bridge work.

---

## 7. What this plan deliberately does *not* do

- Does not change wezterm or kumomta. Those efforts happen later, against
  a stable facade.
- Does not add lua-context pooling/caching to shingetsu; the facade
  composes with whatever the host already has.
- Does not commit to a final fate for `wezterm-dynamic`. The facade
  bridges it; whether wezterm eventually drops `wezterm-dynamic` entirely
  is wezterm's call.
- Does not unify `mlua::Error` and `shingetsu::VmError` beyond what's
  needed for the facade boundary; each engine keeps its own diagnostic
  story.
