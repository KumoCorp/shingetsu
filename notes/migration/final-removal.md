# Final-removal recipe

Once the host (kumomta or wezterm) has converted every Lua-facing
type and call site to `shingetsu_migrate`, the migration facade
is no longer load-bearing -- it's an indirection layer.  This
document is the recipe for removing it.

The full migration is documented in `MIGRATE.md` and the
host-specific playbooks (`kumomta.md`, `wezterm.md`).  This
recipe assumes those have run to completion.

## Pre-checks

Before starting, verify:

- The host's mlua dependency chain is reachable only via
  `shingetsu_migrate`.  Run `cargo tree -i mlua` on the host
  workspace; every entry should originate from
  `shingetsu_migrate`, not directly from a host crate.
- All `shingetsu_migrate::declare_event!` invocations have moved
  off the host's old `declare_event!` macro (kumomta's
  `config::declare_event!`, wezterm has no equivalent macro).
- Every `#[shingetsu_migrate::userdata]` impl block is the only
  Lua-binding form for its type.  If an `impl mlua::UserData`
  block still coexists, it gets removed in this recipe.
- All host tests pass with `shingetsu_migrate` configured to
  enable both `shingetsu-backend` and `mlua-backend` features.

If any pre-check fails, finish the host playbook before
continuing.

## Step 1: search-and-replace `shingetsu_migrate::` to `shingetsu::`

The facade re-exports `shingetsu`'s public API under its own
namespace, so replacing the prefix is a mechanical rename.  Do it
in one commit:

```bash
# from the host repo root
git grep -l 'shingetsu_migrate' \
  | xargs sed -i 's/shingetsu_migrate/shingetsu/g'
```

Cargo.toml dependencies need the same treatment:

```bash
# replace the dep entry name
sed -i 's/shingetsu_migrate /shingetsu /g; s/shingetsu-migrate /shingetsu /g' \
  $(git ls-files '*Cargo.toml')
```

Some attributes need spelling adjustments because the facade
diverged from the native names slightly:

| facade form                              | native form                        |
|------------------------------------------|------------------------------------|
| `#[shingetsu_migrate::userdata(snapshot)]` | `#[shingetsu::userdata]` + `#[lua_snapshot]` body, **or** keep the `(snapshot)` arg if the native macro accepts it (it does) |
| `shingetsu_migrate::declare_event!`        | `shingetsu::declare_event!` (same syntax) |
| `shingetsu_migrate::DynamicLua<T>`         | not portable -- `DynamicLua` is facade-only; see step 3 |

Run the host's full test suite at this point.  Anything that
fails is either a missed rename or a divergence between the
facade and the native API (rare; report upstream).

## Step 2: feature flags

The host crates that depended on `shingetsu_migrate` typically
had a feature flag (or features) enabling one of its backends.
Common shape:

```toml
# before
[dependencies]
shingetsu_migrate = { version = "...", features = ["shingetsu-backend", "mlua-backend"] }

# after
[dependencies]
shingetsu = { version = "..." }
```

Drop both backend features.  The native crate has no concept of
a backend toggle; its features are about what's optional in the
shingetsu side itself (`serde`, `dynamic`, etc.).  If the host
uses `DynamicLua<T>`, port that need to a host-local adapter in
step 3 below.

If the host had its own feature flags conditional on
`shingetsu_migrate` features, simplify them too:

```rust
// before
#[cfg(feature = "shingetsu-backend")]
fn run(...) { ... }

// after -- the conditional becomes unconditional
fn run(...) { ... }
```

## Step 3: handle `DynamicLua<T>` (wezterm only)

`DynamicLua<T>` is intentionally facade-only -- `wezterm-dynamic`
is wezterm's concern and shouldn't bleed into shingetsu (this is
recorded as a resolved decision in §5 of MIGRATE.md).  After
final removal, wezterm needs to keep the same adapter alive in
its own tree.

The facade source for `DynamicLua<T>` lives at
`shingetsu_migrate/src/dynamic.rs` and depends only on
`shingetsu` and `wezterm-dynamic`.  Copy it into a wezterm crate
(typically `luahelper` or `config`) and rename:

```rust
// in wezterm
pub struct DynamicLua<T>(pub T);

impl<T: FromDynamic + ToDynamic> shingetsu::FromLua for DynamicLua<T> { ... }
impl<T: FromDynamic + ToDynamic> shingetsu::IntoLua for DynamicLua<T> { ... }
```

The mlua-side impls drop entirely once mlua is gone.

For kumomta this step is a no-op -- kumomta doesn't use
`DynamicLua<T>`.

## Step 4: drop the migration facade dependency

After steps 1 through 3, no host code references
`shingetsu_migrate` directly.  Verify with a final grep:

```bash
git grep shingetsu_migrate || echo "clean"
```

If the grep returns hits, those are misses from step 1 -- fix
them before continuing.  Then drop the facade dependency from
`Cargo.toml` and `Cargo.lock`:

```bash
git ls-files '*Cargo.toml' \
  | xargs sed -i '/shingetsu_migrate\|shingetsu-migrate/d'
cargo update -p shingetsu_migrate --precise=  2>/dev/null || true
cargo build
```

The build pulls in only `shingetsu`; mlua is gone from
`Cargo.lock` unless something else (host-side tooling, an
unrelated dependency) still references it.

## Step 5: drop coexisting `impl mlua::UserData` blocks

If the host kept `impl mlua::UserData for T` alongside
`#[shingetsu::userdata] impl T` during the migration (per
Pattern D), remove the mlua-side impls now.  They have no
runtime path -- mlua is no longer in the dependency tree -- so
any leftover `impl UserData` is dead code waiting to fail
compilation.

## Step 6: drop `__wezterm_to_dynamic` (wezterm only)

If any userdata still registers `__wezterm_to_dynamic` (which
shouldn't happen after the wezterm playbook step 5), remove the
metamethod registration.  The replacement adapter from step 3
handles dynamic round-tripping without it.

## Step 7: drop `decorate_callback_name` indirection (kumomta only)

After the kumomta playbook step 1 swapped `kumomta-on-` for
`host-event-`, the `decorate_callback_name` function in
`crates/config/src/lib.rs` is a one-liner that prepends
`host-event-`.  After final removal, the same prepending happens
inside shingetsu's event-registry code.  The kumomta-side
function can come out:

```rust
// remove this
pub fn decorate_callback_name(name: &str) -> String {
    format!("host-event-{name}")
}

// every caller used the decorated form to build a string and pass
// it into mlua.  Now they call shingetsu's event API directly,
// which decorates internally.
```

This removal is not strictly required -- the function still
returns a correct string -- but it's dead code after the
migration, so cleaning it up tightens the surface.

## Step 8: bump the host's version

Final removal is a deliberate breaking change for any external
users of the host's plugin / lua-api surface.  The version bump
is the host's call (semver minor or major depending on the
host's policy).  Mention `shingetsu` in the changelog so users
who want to write Rust extensions know which crate to depend on.

## Verification

- `cargo tree -i mlua` returns nothing (mlua is gone).
- `cargo tree -i shingetsu_migrate` returns nothing (facade is
  gone).
- The host's full test suite passes.
- The host's documentation generator -- if it consumes
  `shingetsu-docgen` output -- still produces the same pages it
  did before final removal.  Per-event reference pages, userdata
  pages, and module pages are stable across the rename.

## Rollback

If something fails late in the recipe, the rollback path is the
inverse of step 1 -- a single `git revert` of the rename commit
restores the facade-namespaced names.  Steps 2 onwards are
mostly Cargo edits that can be unwound by reverting their
commits.

The migration facade is designed so that the rename in step 1 is
the only step that touches a non-trivial fraction of the host's
Rust files.  Everything before step 1 (the host playbook) is
incremental -- one type or one pattern per commit -- and
everything after step 1 is small and local.  If the rename
itself goes wrong, the rollback is a one-commit revert.
