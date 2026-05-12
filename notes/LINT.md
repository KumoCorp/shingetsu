# Extensible Linting

Design spec for shingetsu's extensible lint system: data-driven lints, Lua
plugin lints, embedder-supplied type data, and the lowered AST schema exposed
to plugin authors.

## Goals

- Embedders (kumomta, wezterm, ...) can ship lints specific to their object
  model without rebuilding shingetsu.
- End users can write project-local lints in shingetsu's own flavor of Lua.
- `shingetsu check` can type-check and lint embedder scripts entirely offline,
  consuming pre-extracted type data plus pre-installed lint plugins.
- Lint definitions, severity overrides, and lint-set enablement are all
  expressible in `shingetsu.toml`.
- The same `DocModel` that powers `shingetsu doc` also powers type-checking
  and data-driven lints, and can be extracted from both Rust-registered
  globals and pure-Lua API source files.

## Non-goals

- Replacing the built-in lints (`unused_variable`, `shadowing`, etc.) with
  plugin equivalents. Built-ins stay in Rust.
- A query DSL on top of the AST. Selene-style visitor callbacks cover every
  custom lint we have a concrete need for; see the scenario survey below.
- Cross-file data-flow analysis as a first-class plugin primitive. Some
  scenarios (mutation after DKIM signing) genuinely need it; those are
  called out as gaps to revisit.
- Auto-fix / refactor suggestions. Diagnostics only, for now.

## Architecture

Three layers, each independently shippable:

### Layer A: embedder type data

The existing `shingetsu_docgen::DocModel` is the canonical description of the
host's Lua surface. `shingetsu check` learns to load one or more `DocModel`
JSON files and merge them into the type checker's environment view.

This single change enables `arg_type`, `arg_count`, `field_access`,
`assign_type`, `event_name_unknown`, and the `event_handler_*` lints to run
against kumomta scripts without any plugin code.

#### Sources of `DocModel`

- `shingetsu_docgen::extract(&env)` over a populated `GlobalEnv`. The
  existing flow; used by embedders that link shingetsu in-process.
- `shingetsu doc extract-lua` (new): runs the compiler/type checker over
  a set of Lua source files in "library" mode and emits a `DocModel`.
  Modules, functions, userdata-like tables, and doc comments are harvested
  from the source. Used by kumomta to document its Lua-side helpers.

#### Merging

`DocModel::merge(others: &[DocModel]) -> Result<DocModel, MergeError>`:

- Concatenates `modules`, `userdata_types`, `globals`, `events`.
- `schema_version` mismatch is a hard error.
- A `partial: bool` field is added to `ModuleDoc` and `UserdataDoc`. When
  one side of a conflict is `partial = true`, its fields/functions are
  merged into the other side. Conflicting field/function names within the
  merged result are an error -- there is no "later wins" override.
  (Per discussion: kumomta's surface is conflict-free, so error-on-conflict
  is the right default.)

CLI:

```text
shingetsu check --types kumomta-rust.json --types kumomta-lua.json file.lua
shingetsu doc render-markdown --input kumomta-rust.json --input kumomta-lua.json --out site/
```

`shingetsu.toml`:

```toml
[check]
types = ["./build/kumomta-rust.json", "./build/kumomta-lua.json"]
```

### Layer B: data-driven lints

A handful of generic lints are entirely driven by enriched `DocModel`
metadata, with no plugin code required:

- `deprecated` -- `FieldDoc.deprecated`, `FunctionDoc.deprecated`.
- `must_use` -- `FunctionDoc.must_use`.
- `restricted_module_paths` -- `ModuleDoc.restricted`.
- per-arg read/write semantics (`ParamDoc.observes`), feeding
  better unused-variable / write-only reasoning.

These are normal built-in lints in Rust; the data feeding them is what's
new.

### Layer C: Lua plugin lints

For genuinely custom checks (kumomta's `set_meta` warning, cross-file
TOML validation, etc.), plugins are written in shingetsu Lua, loaded from
project-relative paths declared in `shingetsu.toml`.

## Lint sets

There is no special "heavy" tier. Lints declare which named set(s) they
belong to, and `shingetsu.toml` plus CLI flags control which sets run.

```toml
[check]
default_sets = ["builtins"]
optional_sets = ["kumomta_semantic", "kumomta_deep"]
```

- Built-in lints implicitly belong to `builtins`.
- Plugins declare `sets = {...}` in their `lint.declare {...}` block.
- `shingetsu check --enable kumomta_deep` adds a set; `--disable` removes
  one (overrides config).
- `shingetsu check --list-lints` shows id, set(s), default severity, source
  (built-in / plugin path), and description.

## Trust model

Plugins run in a separate shingetsu VM with `Libraries::SANDBOXED` plus
the `shingetsu.lint` host module. No `io`, `os`, `exec`, network, or
filesystem access by default.

Per-plugin opt-out for plugins that need to load external data (kumomta's
TOML cross-check):

```toml
[check]
plugins = [
    "./lints/kumomta_set_meta.lua",
    { path = "./lints/kumomta_validate.lua", unsandboxed = true },
]
```

`unsandboxed = true` extends the plugin env with `io`, `os`, `serde`
(toml/json), and read-only `fs`. The user accepts the trust expansion
explicitly per plugin.

## Plugin registration

One lint per file. The file is a normal shingetsu Lua module that uses the
`shingetsu.lint` host module to declare itself and register callbacks.

```lua
local lint = require("shingetsu.lint")

lint.declare {
    name = "kumomta_set_meta",
    description = "Warn on unknown / unprefixed Message metadata keys",
    default_severity = "warn",
    sets = { "kumomta_semantic" },
    min_schema = 1,
}

lint.on("method_call", function(call, ctx)
    if call.method ~= "set_meta" then return end
    if not ctx.is_instance_of(call.receiver, "Message") then return end

    local key = call.args[1]
    local v = ctx.constant_value(key)
    if type(v) ~= "string" then return end
    if KNOWN_META[v] or v:starts_with("x_") then return end

    ctx.warn(key.span,
        "metadata key '" .. v .. "' is not a known name and lacks 'x_' prefix")
end)
```

`lint.declare {...}` must be called exactly once per file. `lint.on(event, fn)`
may be called any number of times to register callbacks for visitor events
(see the schema section).

The lint name is the visible identifier used in `shingetsu.toml`, CLI
overrides, and `--# shingetsu: allow=...` directives. Duplicate names across
plugins are an error.

## Diagnostic API on `ctx`

- `ctx.warn(span_or_node, message, opts?)` -- emits a warning.
- `ctx.error(span_or_node, message, opts?)` -- emits an error.
- `opts.notes` -- list of `{span = ..., message = ...}` secondary labels.
- Severity is resolved through the same `lint_directives` pipeline as
  built-in lints; severity in source overrides plugin default.

## Type / value queries on `ctx`

- `ctx.type_of(expr_node)` -- a stringly description of the inferred type,
  matching what the type checker emits in messages.
- `ctx.is_instance_of(expr_node, type_name)` -- structural test against a
  module or userdata name from the merged `DocModel`.
- `ctx.constant_value(expr_node)` -- the literal value if the expression
  is statically known, else `nil`. Handles string/number/bool literals,
  `local x = "lit"; x`, and trivial concatenation of constants.
- `ctx.resolve(name_node)` -- `{ kind = "local"|"global"|"upvalue", binding_id, decl_span }`.
- `ctx.doc_model` -- read-only handle to the merged `DocModel`, indexed
  by module / userdata / event name. Lets plugins enumerate known names
  (e.g. iterate event signatures to validate a `host.on(...)` call).
- `ctx.config` -- the TOML config block declared for this lint in
  `shingetsu.toml`, decoded into a Lua table.

Trivia helpers:

- `ctx.is_same_line(node_a, node_b)`.
- `ctx.comments_between(span_a, span_b)`.

Traversal helpers:

- `ctx.walk(node, visitors)` -- runs the visitor table over a subtree.
  Visitors receive a `walker` with `walker:skip()` to prune descent.
- `ctx.nodes_equivalent(a, b)` -- structural equality ignoring spans,
  trivia, and parenthesization, but respecting identifier identity.

## Node schema

The schema is versioned (`shingetsu.lint.SCHEMA_VERSION`). It is a lowered,
desugared IR that we control; not a thin wrapper over full_moon.

### Representation

- Nodes are userdata with `__index`. Read-only.
- Field access is lazy (the underlying compiler IR is not deeply
  materialized into Lua tables until the plugin asks for a field).
- Default `==` is handle identity. Use `ctx.nodes_equivalent` for structural
  equality.

### Common fields

- `node.kind` -- string tag identifying the variant (e.g. `"method_call"`).
- `node.span` -- a `Span` userdata: `start_byte`, `end_byte`, `start_line`,
  `start_col`, `end_line`, `end_col`, plus `:contains(other)` and `tostring`.

Sub-tokens with independent diagnostic targets get their own `*_span`
field, e.g. `method_call.method_span`, `binop.op_span`.

### Expression kinds

| kind                | fields                                                              |
| ------------------- | ------------------------------------------------------------------- |
| `string_literal`    | `value: string`, `is_long: bool`, `is_interp: bool`                 |
| `interp_string`     | `parts: [string|expr]`                                              |
| `number_literal`    | `value: number`, `raw: string`                                      |
| `bool_literal`      | `value: bool`                                                       |
| `nil_literal`       | --                                                                  |
| `vararg`            | --                                                                  |
| `name`              | `name: string`, `is_global: bool`, `is_local: bool`, `binding_id`   |
| `binop`             | `op: string`, `op_span`, `lhs: expr`, `rhs: expr`                   |
| `unop`              | `op: string`, `op_span`, `operand: expr`                            |
| `function_call`     | `callee: expr`, `args: [expr]`, `has_trailing_multret: bool`        |
| `method_call`       | `receiver: expr`, `method: string`, `method_span`, `args`, `has_trailing_multret` |
| `index`             | `target: expr`, `key: expr`                                         |
| `field`             | `target: expr`, `name: string`, `name_span`                         |
| `table_constructor` | `entries: [entry]`                                                  |
| `function_expr`     | `params: [param]`, `is_variadic: bool`, `body: block`               |

Every expression node also exposes `was_parenthesized: bool`. Parentheses
do not get their own node.

### Entry sub-shape (table constructors)

```
{ kind = "array",  value = expr }
{ kind = "named",  name = string, name_span, value = expr }
{ kind = "hash",   key = expr, value = expr }
```

### Param sub-shape

```
{ name = string, name_span, type_annotation = optional<type_ref>, default = optional<expr> }
```

### Statement kinds

| kind             | fields                                                                |
| ---------------- | --------------------------------------------------------------------- |
| `assign`         | `targets: [expr]`, `values: [expr]`                                   |
| `local_assign`   | `names: [param]`, `values: [expr]`, `attribs: [string]`               |
| `local_function` | `name: string`, `name_span`, `function: function_expr`                |
| `function_decl`  | `target: expr`, `is_method: bool`, `function: function_expr`          |
| `global_decl`    | `names: [string]`, `type_annotations: [optional<type_ref>]`           |
| `if`             | `branches: [{cond: expr, block: block}]`, `else_block: optional<block>` |
| `while`          | `cond: expr`, `block: block`                                          |
| `repeat`         | `block: block`, `cond: expr`                                          |
| `numeric_for`    | `var: param`, `start: expr`, `stop: expr`, `step: optional<expr>`, `block` |
| `generic_for`    | `vars: [param]`, `exprs: [expr]`, `block: block`                      |
| `do_block`       | `block: block`                                                        |
| `return`         | `values: [expr]`                                                      |
| `break`          | --                                                                    |
| `continue`       | --                                                                    |
| `goto`           | `label: string`                                                       |
| `label`          | `name: string`                                                        |
| `expr_statement` | `expr: expr`                                                          |

Calls used as statements: `expr_statement.expr` is a `function_call` or
`method_call`. Both `on_expr_statement` and the appropriate call callback
fire. Plugins that only care about calls register for the call event and
work in either context.

### Type-annotation references

Where the source carries explicit type annotations (function params,
`global` declarations, return types), the node exposes a `type_ref`
userdata whose shape mirrors `shingetsu_docgen::TypeRef`. Plugins can
read it but generally won't -- `ctx.type_of` is the preferred query.

### Callback events

Initial set (callable via `lint.on(eventname, fn)`):

- `chunk_begin`, `chunk_end`
- `statement` -- fires for every statement before kind-specific events
- `expr_statement`
- `assign`, `local_assign`, `local_function`, `function_decl`, `global_decl`
- `if`, `while`, `repeat`, `numeric_for`, `generic_for`, `do_block`
- `return`, `break`, `continue`, `goto`, `label`
- `function_call`, `method_call`
- `binop`, `unop`
- `name` -- every name reference
- `global_read`, `global_write` -- specialised name-reference events
- `string_literal`, `interp_string`, `number_literal`
- `table_constructor`
- `function_expr`
- `require` -- specialised `function_call` for `require("...")` patterns

A plugin registering `function_call` sees method calls only if it also
registers `method_call`; the two events are distinct.

### Schema versioning

- `shingetsu.lint.SCHEMA_VERSION` is exposed to plugins.
- `lint.declare { min_schema = N }` makes a plugin refuse to load against
  an older host.
- Additive changes (new event, new optional field) bump the version but
  do not break existing plugins.
- Removal / rename is a major bump; the current surface is frozen by
  default once published.

## Coverage of kumomta scenarios

The list of concrete kumomta-side lints requested in design discussion,
mapped to mechanisms above:

1. **Logger header extraction** -- visitor on `function_call` matching
   `kumo.configure_local_logs`, inspect `headers` field of arg-1
   `table_constructor`. Pure visitor, sandboxed.
2. **Cert/key paths that don't exist** -- visitor on `function_call`,
   string-literal arg extraction, filesystem `stat`. Needs
   `unsandboxed = true`.
3. **TOML cross-check between `queue` and `source` helpers** --
   visitor on `require` + setup-call patterns; collect filename string
   literals; load TOMLs and compare. Needs `unsandboxed = true`. Belongs
   in an optional lint set.
4. **Global where local would do** -- `on_global_write`. Becomes a
   built-in once Lua 5.5 `global` adoption is universal; until then,
   ship as a plugin or as an opt-in built-in.
5. **Constant lookup table inside a loop/function** -- visitor on
   `table_constructor` whose entries are all `ctx.constant_value`-able,
   with the plugin tracking enclosing-loop state via closure during
   `ctx.walk`. Doable but slightly awkward; revisit whether to expose
   `ctx.enclosing(kind)` if this pattern repeats.
6. **Bespoke caching via globals / weak tables** -- visitor on
   `table_constructor` looking for `__mode` entries; visitor on
   `on_global_write` for the global-cache pattern.
7. **Excessive file-scope computation** -- visitor at `chunk_end`,
   summarize statements that aren't `local`/`function`/`require`/literal
   assigns. Heuristic, but expressible.
8. **O(n) key lookup via `pairs`** -- visitor on `generic_for` whose
   body is an `if` comparing the loop key against a value.
9. **Mutating a `Message` after DKIM/ARC signing** -- requires intra-
   procedural data flow that the visitor model does not provide on its
   own. **Gap.** Two options for a follow-up phase: expose CFG/SSA
   from the compiler, or ship this as a built-in. Defer.
10. **Credential literals in source** -- visitor on `function_call`
    matching known auth-config entry points, check for `string_literal`
    args.

All but scenario 9 fit the visitor model. Scenario 9 is the canonical
example of "needs flow analysis" and is the planned trigger for a
future control-flow-graph extension.

Two `ctx` helpers worth considering once we have real plugin code in
hand, based on scenarios 5 and 7:

- `ctx.is_constant_expr(node) -> bool`
- `ctx.enclosing(node, kind) -> node?` (closest ancestor matching `kind`)

Leaving these out of v1; add them only if real lints would be ugly
without them.

## Implementation phases and checklist

Each phase is independently shippable. Pause for review at phase
boundaries per project convention.

### Phase 1: DocModel as type source

- [ ] Type checker accepts an externally supplied `DocModel` and merges
      it into its environment view (modules, userdata, events, globals).
- [ ] `shingetsu check --types <path>` (repeatable).
- [ ] `shingetsu.toml` `[check] types = [...]` honored by `check`.
- [ ] End-to-end test: a small embedder-style `DocModel` JSON drives
      `arg_type` / `field_access` / `event_name_unknown` diagnostics
      against a sample script.

### Phase 2: DocModel merge

- [ ] `partial: bool` added to `ModuleDoc`, `UserdataDoc`. Schema version
      bumped.
- [ ] `DocModel::merge` with conflict-on-overlap semantics, except where
      one side is `partial`.
- [ ] Multiple `--types` and `--input` flags merge in declared order.
- [ ] `shingetsu doc render-markdown --input ... --input ...` produces
      merged output.

### Phase 3: Lua-source DocModel extraction

- [ ] `shingetsu_docgen::extract_from_sources(&[Path]) -> DocModel`.
      Runs compiler + type checker in library mode; harvests modules,
      functions, userdata-like tables, and doc comments.
- [ ] `shingetsu doc extract-lua --out file.json <sources...>` CLI.
- [ ] Round-trip test: extract from a Lua file, feed back into
      `shingetsu check` against a caller script.

### Phase 4: Data-driven lints

- [ ] `deprecated` lint (reads `FieldDoc.deprecated`,
      `FunctionDoc.deprecated`).
- [ ] `must_use` lint.
- [ ] `restricted_module_paths` lint.
- [ ] `ParamDoc.observes` field; consumed by unused-variable and
      assign-type reasoning.

### Phase 5: Plugin loader and minimal API

- [ ] Separate `GlobalEnv` for plugins with sandboxed library set.
- [ ] `shingetsu.lint` host module with `declare` and `on`.
- [ ] `LintId` becomes `BuiltIn(...) | Plugin(Arc<str>)`; all lint
      pipelines (severity, directives, diagnostic rendering) accept
      either.
- [ ] Plugin loading from `shingetsu.toml [check] plugins`.
- [ ] First three events wired: `method_call`, `function_call`,
      `assign`.
- [ ] Diagnostic API on `ctx`: `warn`, `error`, span/node accepted.
- [ ] `kumomta_set_meta`-shaped integration test.

### Phase 6: Full visitor coverage

- [ ] Remaining callback events from the schema list.
- [ ] `ctx.type_of`, `ctx.is_instance_of`, `ctx.constant_value`,
      `ctx.resolve`, `ctx.doc_model`, `ctx.config`.
- [ ] `ctx.walk` with `walker:skip()`.
- [ ] `ctx.nodes_equivalent`.
- [ ] Trivia helpers (`is_same_line`, `comments_between`).
- [ ] Schema versioning (`SCHEMA_VERSION`, `min_schema`).

### Phase 7: Lint sets

- [ ] `sets` field on plugin declarations; `builtins` implicit for
      built-ins.
- [ ] `default_sets` / `optional_sets` in `shingetsu.toml`.
- [ ] `--enable` / `--disable` CLI flags.
- [ ] `shingetsu check --list-lints` enumerates id / sets / severity /
      source / description.

### Phase 8: Unsandboxed plugins

- [ ] Per-plugin `unsandboxed = true` opt-in in `shingetsu.toml`.
- [ ] Unsandboxed env exposes `io`, `os`, `serde` (toml/json), read-only
      `fs`.
- [ ] Documentation warning on the trust expansion.
- [ ] Cross-file TOML cross-check scenario as an integration test.

### Phase 9 (deferred): flow-sensitive analysis

- [ ] Decide: expose CFG/SSA to plugins, or ship the DKIM mutation lint
      as a built-in.
- [ ] Revisit once phases 1-8 are in real use.

## Open questions

- Whether `ctx.is_constant_expr` and `ctx.enclosing(kind)` should be in
  v1. Defer to real plugin code.
- Exact `--enable` / `--disable` precedence vs `default_sets` / `optional_sets`
  when they conflict. Lean: CLI wins, last flag wins.
- Whether merged `DocModel` should be cached on disk between `check`
  invocations. Out of scope for v1.
