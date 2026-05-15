# Ergonomic compile + type-check + lint + run entrypoint

## Goal

Give consumers of the `shingetsu` crate a single ergonomic entrypoint that
compiles Lua source with type checking and plugin linting enabled by default
("maximal safety"), and optionally runs it. Today this wiring exists only
inside the CLI (`shingetsu-cli/src/main.rs`); embedders must hand-assemble
`Compiler` + `LoadedPlugins` + directive validation + lint-config application
+ `Task` themselves.

## Established decisions

1. **Eager pre-pass for recursion.** The `require` graph is walked statically
   before execution; best-effort static resolution is acceptable (dynamically
   named `require(expr)` edges that can't be resolved are simply not recursed
   into). No loader-driven lazy linting.
2. **Errors block execution.** Any `Severity::Error` diagnostic (including
   parse/compile failures, see #5) prevents `check_and_run` from running the
   chunk.
3. **Lint/plugin processing failures are Warnings**, consistent with the
   existing error policy: a plugin that fails to compile/load/dispatch
   produces `Warning` diagnostics and is absent from the active set; it does
   not abort checking the user's code.
4. **`type_check` and `lint_recursion` are project policy** and live in
   `ProjectConfig` (`[check]`), not on per-call `Options`.
5. **Unified diagnostic stream.** `CompileError::Parse` (and other hard
   compile failures) are converted into `Error`-severity `Diagnostic`s so
   there is one uniform stream and "any Error severity blocks the run" covers
   parse errors. Diagnostics are always returned, even when execution is
   blocked or compilation failed.
6. **Linting implies type checking.** Lint IR is only lowered when
   `type_check` is true. If plugins are present but `type_check` is false,
   type checking is enabled internally; this is documented, not an error.
7. **Graceful config/plugin absence:**
   - No `shingetsu.toml` found -> not an error; behave as
     `ProjectConfig::default()` (no plugins, `type_check` defaults true).
   - Failure to read or parse a discovered `shingetsu.toml` -> hard error.
   - Zero plugin paths -> not an error; no plugin lints, continue with type
     checking if enabled.

## ProjectConfig changes

Add to `CheckConfig` (`[check]` in `shingetsu.toml`):

- `type_check: bool` — `serde(default = "default_true")`. Default `true`.
- `lint_recursion: ModuleLintScope` — `serde(default)`. Values:
  `non_recursive` (default), `first_level`, `fully_recursive`.

`ProjectConfig` already owns everything else needed:

- `lints.overrides` — per-lint `Severity` overrides.
- `check.default_sets` / `check.optional_sets` + `active_sets(enabled,
  disabled)` — lint set resolution.
- `check.plugins` (`resolved_plugins()`) + `check.plugin_configs` — exactly
  the two arguments to `LoadedPlugins::load_from_paths`.
- `check.types` (`resolved_types()`) — DocModel JSON for the type checker.
- `config_dir` — base for resolving relative paths and the `require`
  package path used by the eager pre-pass.

New enum, defined in the `project_config` module (same module as
`ProjectConfig`):

```rust
pub enum ModuleLintScope { NonRecursive, FirstLevel, FullyRecursive }
```

## Entrypoint API

New module in the `shingetsu` crate (e.g. `src/entrypoint.rs`, re-exported
from `lib.rs`).

`Options` owns all its fields (no lifetime). Shared plugins are modelled
as `Arc<LoadedPlugins>` so the load-once-lint-many path is a pointer
clone, not a deep copy of the per-plugin VM envs. `ProjectConfig` is
cloned once per `Options` (cheap relative to compilation); `source` is
an owned `String`.

```rust
pub struct Options {
    source: String,
    source_name: Arc<String>,
    config: ProjectConfig,
    plugins: Option<Arc<LoadedPlugins>>,
    enabled_sets: Vec<String>,   // invocation-level --enable
    disabled_sets: Vec<String>,  // invocation-level --disable
}

impl Options {
    /// Loads plugins from `config.resolved_plugins()` +
    /// `config.check.plugin_configs`. Per-plugin load failures are
    /// captured and replayed as Warning diagnostics on the first
    /// check call. Hard `Err` only for config/IO/discovery problems.
    pub async fn with_config(
        config: ProjectConfig,
        source: String,
        source_name: Arc<String>,
    ) -> Result<Self, EntrypointError>;

    /// Caller-supplied, pre-loaded plugins (load-once-lint-many).
    /// Never loads plugins itself. `None` => type-check only.
    pub fn with_plugins(
        config: ProjectConfig,
        plugins: Option<Arc<LoadedPlugins>>,
        source: String,
        source_name: Arc<String>,
    ) -> Self;

    pub fn enabled_sets(self, sets: Vec<String>) -> Self;   // builder
    pub fn disabled_sets(self, sets: Vec<String>) -> Self;  // builder

    /// Compile + type-check + lint (incl. recursive pre-pass per
    /// config.lint_recursion). Never runs the chunk.
    pub async fn check(&self, env: &GlobalEnv) -> Vec<Diagnostic>;

    /// check(), then run via Task unless an Error-severity diagnostic
    /// blocks execution. Diagnostics are always returned.
    pub async fn check_and_run<R: FromLuaMulti>(
        &self,
        env: &GlobalEnv,
        args: impl IntoLuaMulti,
    ) -> CheckRunOutcome<R>;
}

pub struct CheckRunOutcome<R> {
    pub diagnostics: Vec<Diagnostic>,
    pub result: Result<R, RunError>,
}

pub enum RunError {
    /// An Error-severity diagnostic prevented execution.
    Blocked,
    /// Parse/lower failure (also surfaced in `diagnostics`).
    Compile,
    /// Task returned a RuntimeError (incl. ExitRequested).
    Runtime(RuntimeError),
}
```

`with_config` is `async` because `LoadedPlugins::load_from_paths` is
async; it stores the result as `Arc<LoadedPlugins>`.

`EntrypointError` is a dedicated enum that wraps `ConfigError` and
exposes any diagnostics produced by failed plugin loading, so callers
can distinguish "project is misconfigured" (hard error) from
"some plugins were unusable" (Warning diagnostics) -- see decision #3 /
the orchestrator change below.

`check_and_run<R: FromLuaMulti>` covers the raw-values case too: a
caller wanting the unconverted result asks for `R = Variadic` (the
newtype over `ValueVec` that implements `FromLuaMulti`; the bare
`ValueVec`/`SmallVec` alias does not), so no separate raw-value entry
point is needed.

## Internal flow (check)

1. Resolve config: if constructed via `with_config`, plugins already
   loaded (Owned); per-plugin load-failure Warnings stashed for replay.
2. Compute active sets: `config.active_sets(enabled_sets, disabled_sets)`.
3. Build a `Compiler` with `type_check = config.check.type_check ||
   plugins_present`, DocModels from `resolved_types()`, module loader +
   package path derived from `source_name` / `config.config_dir`.
4. Eager require pre-pass per `config.check.lint_recursion`:
   - `NonRecursive`: entry chunk only.
   - `FirstLevel`: entry + direct `require` targets.
   - `FullyRecursive`: transitive closure.
   Each module compiled with its own `@path` source name (diagnostics
   stay source-tagged via `SourceLocation`). Unresolvable dynamic
   requires are skipped (best-effort).
5. For each compiled chunk: `compile_with_ast` -> type diagnostics +
   `lint_ir`. Convert any hard `CompileError` into an Error-severity
   `Diagnostic` (decision #5).
6. If plugins present: `lint_directives.validate_against_plugins(...)`
   then `LoadedPlugins::lint_chunk_in_sets(source_name, chunk,
   Some(&active_sets))`. Dispatch failures -> Warning diagnostics.
7. Apply lint config: port `apply_lint_config` (currently CLI-local at
   `main.rs:226`) into the `shingetsu` crate; apply
   `config.lints.overrides`.
8. Prepend stashed plugin-load Warnings. Return aggregated diagnostics.

## Internal flow (check_and_run)

1. `diagnostics = check(env)`.
2. If any `diagnostics` has `Severity::Error` -> `result =
   Err(RunError::Blocked)`, return (diagnostics still returned).
3. If a hard compile failure occurred -> `Err(RunError::Compile)`.
4. Else build `Function::lua(top_level, ...)`, run via `Task::new(env,
   func, args).await`, handling `ExitRequested` / `dispose()` /
   `flush_stdio` the way `main.rs` does today. Convert returned values
   via `FromLuaMulti`. Runtime failures -> `Err(RunError::Runtime(..))`.

## Orchestrator / plugin-loading change (scope item)

### Structured errors, not rendered strings

`load_plugin` / `load_plugin_with_source` (`mod.rs:711` / `mod.rs:721`)
currently return `Result<PluginDeclaration, String>` where the `String`
is an already-rendered diagnostic (`render_compile_error` /
`render_runtime_error` / registry `.to_string()`). Pre-rendered strings
cannot be folded into the unified diagnostic stream, re-severitied to
`Warning`, or run through `apply_lint_config`. Change the error type to
`Vec<Diagnostic>` (structured):

- Plugin parse/lower failure: convert `CompileError` to an
  `Error`-severity `Diagnostic` -- the same conversion machinery as
  decision #5; share one helper.
- Plugin load-time `RuntimeError`: convert to a `Diagnostic`.
- Registry `begin_load`/`end_load` and duplicate-name errors: convert
  to `Diagnostic`s; `render_duplicate_name_diagnostic`
  (`orchestrator.rs`) likewise stops returning a `String`.

Rendering moves to the edges (CLI) via the existing
`render_warnings`/`render_*_error`; the library layer only ever passes
structured `Diagnostic`s around.

### Collect failures instead of bailing

`LoadedPlugins::load_from_paths` (`orchestrator.rs:121`) currently stops
at the first failure. Change it (or add a sibling) to:

- Load every plugin that loads.
- Collect per-plugin failure `Vec<Diagnostic>` (read error, compile
  error, load-time runtime error, duplicate-name) so the entrypoint can
  re-severity them to `Warning` and replay them into the stream.
- Still return a hard `Err` only for caller/programming errors, not
  plugin-quality problems.

Update all existing callers of `load_plugin*` /
`render_duplicate_name_diagnostic` (CLI, orchestrator, tests) to the
structured-diagnostic signature.

## Test-helper consolidation (desirable outcome)

The fragmentation in `crates/shingetsu-compiler/tests/common.rs`
(~6 run variants, ~6 compile-error/diagnostic variants, 3 `type_check`
variants, `assert_plugin_diagnostics!` / `assert_plugin_load_error!`)
exists because there is no single path that does
compile + type-check + lint + run -- each helper hand-assembles a
different subset (env shape, globals, with/without lint, rendered vs.
structured output). A unified entrypoint collapses that axis, so a
desirable outcome of this work is landing on a much smaller, easier-to-
choose helper set, ideally:

- one assertion over `check(...)` -> the full `Vec<Diagnostic>`
  (replacing the `compile_err*` / `compile_diagnostics*` /
  `compile_diag` / `type_check*` family), and
- one assertion over `check_and_run(...)` -> `(result, diagnostics)`
  (replacing the `run_*` family),

both parameterized by a `ProjectConfig` / `GlobalEnv` rather than via
bespoke per-combination helpers. This stays consistent with the
project's assertion guidance: fewer helpers means the
"which macro do I use" decision in AGENTS.md gets shorter, and the
full-rendered-output rule still applies (assert on the whole returned
diagnostics vec / result, not a substring).

Notes:

- Plugin lint tests currently live under `shingetsu-compiler/tests`
  even though plugins live in the `shingetsu` crate; consolidation is
  also a chance to relocate them next to the entrypoint.
- `assert_plugin_load_error!` consumes the rendered-`String` plugin
  load error that phase 2a restructures to `Vec<Diagnostic>`; it must
  be updated/absorbed in lockstep with that change.
- This is a cleanup that *follows* the entrypoint landing; do not block
  the entrypoint phases on it.

## CLI refactor

`shingetsu-cli/src/main.rs` `run` and `xlint`/`check` paths re-implement
this sequence by hand (discover config, resolve plugins, load, active
sets, compile_with_ast, validate directives, lint_chunk_in_sets,
apply_lint_config, render, Task). After the entrypoint lands, both
subcommands should call it, deleting the duplicated wiring (incl. moving
`apply_lint_config` into the crate). CLI keeps only: arg parsing,
`RenderStyle` selection, `render_warnings`/`render_*_error`, exit codes.

## Resolved design questions

- `ModuleLintScope` lives in the `project_config` module.
- `with_config` returns `EntrypointError`: a dedicated enum wrapping
  `ConfigError` and exposing diagnostics from failed plugin loading.
- No separate raw-values entry point; callers use `R = Variadic` with
  `check_and_run` (`Variadic: FromLuaMulti`; the `ValueVec` alias
  itself has no such impl).
- `enabled_sets` / `disabled_sets` use the sketched builder methods.
- `Options` owns all fields (no lifetime); shared plugins via
  `Arc<LoadedPlugins>`.

## Phasing (pause for review after each)

1. `ProjectConfig` additions (`type_check`, `lint_recursion`,
   `ModuleLintScope`) + tests.
2a. `load_plugin*` / `render_duplicate_name_diagnostic` ->
   `Vec<Diagnostic>` (structured, not rendered) + update callers +
   tests.
2b. `load_from_paths` failure-collection change + tests.
3. Entrypoint module: `Options`, `check`, non-recursive path +
   `apply_lint_config` move into the crate + tests.
4. Eager require pre-pass (`FirstLevel`, `FullyRecursive`) + tests.
5. `check_and_run` (Task integration, RunError) + tests.
6. CLI refactor to call the entrypoint; delete duplicated wiring.
7. Test-helper consolidation in `common.rs` around `check` /
   `check_and_run`; relocate plugin tests; absorb
   `assert_plugin_load_error!` into the structured-diagnostic model.
