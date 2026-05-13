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

- `deprecated` -- `FieldDoc.deprecated`, `FunctionDoc.deprecated`,
  `ModuleDoc.deprecated` (with sub-module inheritance through the
  parent's `FieldDef.deprecated` for `kumo.api`-style access chains).
- `must_use` -- `FunctionDoc.must_use`.

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

- [x] Type checker accepts an externally supplied `DocModel` and merges
      it into its environment view (modules, userdata, events, globals).
      `TypeRef::to_lua_type` + `DocModel::to_global_type_map` /
      `DocModel::to_userdata_type_registry` cover the reverse path.
- [x] `shingetsu check --types <path>` (repeatable).
- [x] `shingetsu.toml` `[check] types = [...]` honored by `check`
      (paths resolve relative to the config file).
- [x] Userdata method resolution: `LuaType::lookup_known_member` /
      `LuaType::lookup_member` take an optional `&UserdataTypeRegistry`
      and follow `LuaType::Named` references through it.  Compiler
      threads the registry from `GlobalEnv` + `--types` data.
- [x] End-to-end tests: `--types`-supplied module functions and
      userdata methods both drive `arg_count` diagnostics
      (`check_types_flag_adds_module`, `check_userdata_method_arg_count`
      in `crates/shingetsu-cli/tests/cli.rs`).  Broader `arg_type` /
      `field_access` / `event_name_unknown` coverage to follow.

### Phase 2: DocModel merge

- [x] `partial: bool` added to `ModuleDoc`, `UserdataDoc`. Schema
      version bumped to 10.
- [x] `DocModel::merge` with conflict-on-overlap semantics, except
      where one side is `partial`.  `MergeError` covers schema
      mismatch, duplicate module / userdata / global / event, and
      duplicate member during partial merge.
- [x] Multiple `--types` flags merge in declared order via
      `DocModel::merge` before the type checker runs.  End-to-end
      tests `check_types_partial_merges_modules` and
      `check_types_duplicate_module_errors`.
- [x] `shingetsu doc render-markdown --input ... --input ...`
      produces merged output (integration test
      `doc_render_markdown_merges_inputs`).

### Phase 3: Lua-source DocModel extraction

Landed as a standalone full_moon-based AST walker in
`crates/shingetsu-docgen/src/extract_lua.rs`.  Iterating in Phase 3b
to route extraction through the existing compiler so inline Luau
annotations (`function mod.foo(x: number): string`) are picked up
for free.

- [x] `shingetsu_docgen::extract_from_sources(&[PathBuf], &ExtractOptions)
      -> Result<(DocModel, Vec<Warning>)>`.
- [x] `shingetsu doc extract-lua [--root <dir>] [--out <file>] <sources...>`
      CLI subcommand.
- [x] Round-trip integration test: `doc_extract_lua_round_trip`
      extracts a Lua helper, feeds the JSON to `shingetsu check`
      via `--types`, and observes an `arg_count` diagnostic.
- [x] EmmyLua-style doc comments: `@param`, `@return`,
      `@deprecated`, `@nodiscard`, plus shingetsu-specific `@hidden`.
- [x] `FunctionDoc.deprecated`, `FunctionDoc.must_use`,
      `FieldDoc.deprecated` fields added (schema 11).  Phase 4 lints
      will consume them; Phase 3 populates them from Lua sources.
- [ ] Userdata-like Lua classes: out of scope by design.  Embedders
      that want pure-Lua userdata must hand-author a partial JSON.
- [ ] Typo-suggestion for unknown `@tag` (edit-distance against
      known set).  Cosmetic; defer until real-world tags surface.

### Phase 3b: route extraction through the compiler

Motivation: pick up inline Luau annotations (`function mod.foo(x:
number): string`) and inherit the compiler's scope / require
resolution.  The current AST walker treats all params as `Any`
unless overridden by `@param`.

Scan of `kumomta/assets/policy-extras/*.lua` (the real-world
target) confirms two patterns matter and two don't:

- **Used everywhere**: `local mod = {} ... function mod.foo() ...
  end ... return mod`.  The compiler already infers this and
  produces a `Table` with each function entry typed.
- **Used in 3 files**: `mod.bar = <expr>` field assignments
  (typing.lua, docker_utils.lua, queue.lua).  Currently invisible
  to `module_type_info.return_type`.
- **Not used**: `return { foo = function() end }` inline tables.
  Compiler returns `None` for these.  Documented as a follow-up
  gap rather than fixed now.
- **Not used**: multi-return / non-table return.

Tasks:

- [x] `Compiler::compile_with_ast(src) -> (Bytecode, Arc<ast::Ast>)`.
      The existing `compile()` stays as a thin wrapper.
- [x] Compiler: `compile_assignment` updates a local's
      `inferred_type` when the LHS is `local_name.field` and the
      local holds a `Table`.  Local-function references (`mod.fn =
      helper`) get their real `Function` type; call results land as
      `Any` (sufficient for surfacing the field name to docgen).
- [x] Rewrite `extract_lua.rs`: compile each file via `compile`,
      read `module_type_info.return_type` for the module shape and
      `TableField.doc` for each entry's doc text.  No second AST walk
      in docgen.  Inline Luau annotations (`function mod.foo(x:
      number): string`) flow through automatically.  Added test
      `emmylua_param_overrides_annotation` covering the
      annotation-override precedence.
- [x] Compiler captures preceding `---` doc-comment text onto
      `TableField.doc` during `compile_function_decl` and
      `compile_assignment`.  Test
      `table_accumulation_doc_comment_attached_to_field` confirms.
- [x] `shingetsu doc extract-lua` warnings are now real
      `Diagnostic` values (lint `module_shape`, severity Warning)
      anchored at the chunk's `return` statement (or EOF when no
      return is present).  Rendered through the existing
      `render_warnings` pipeline with full source pane, caret, lint
      id, and help block -- indistinguishable from a type-checker
      warning.  Required adding `ModuleTypeInfo.return_location`
      and `LintId::ModuleShape`.
- [x] Every kumomta `policy-extras/*.lua` file extracts cleanly
      (no warnings, sensible function / field surfaces).
- [ ] **Known follow-up gap**: inline `return { foo = function() end
      }` returns `None` from `infer_table_constructor_type`.  Fix by
      extending that helper to infer function-expression and literal
      values.  Not used by kumomta; documented here.
- [ ] **Known follow-up gap**: `infer_expr_type` doesn't yet handle
      `FunctionCall` return types.  `mod.bar = kumo.memoize(...)`
      lands as `Any` rather than the call's actual return type.
      Cosmetic for docgen surfacing; relevant once Phase 4 lints
      consume the actual type.

### Phase 3c: doc-comment hygiene and EmmyLua class surface

A pass of small, related changes driven by real kumomta files
(`policy-extras/policy_utils.lua` and `typing.lua`).  Each item is
self-contained; together they make doc extraction useful for the
pre-migration Lua codebase and lay groundwork for plugin-authored
lints that compare docs to runtime declarations.

- [x] **Interrupted doc-comment warning**.  `harvest_doc_comment`
      keeps walking past `--` lines to detect an orphaned `---`
      block; emits `LintId::InterruptedDocComment` pointing at the
      `--` line.  Surfaces through `shingetsu doc extract-lua`
      (filtered to docgen-relevant diagnostics).  Tests:
      `interrupted_doc_comment_warning` in compiler suite plus
      `warns_on_interrupted_doc_comment` in extract_lua.
- [x] **Generalise doc-comment harvest to local assignments**.
      `compile_local_assignment`, `compile_const_assignment`,
      `compile_local_function`, and `compile_const_function` now
      run `harvest_doc_comment` and attach the text to
      `Local.doc` on the first declared name.  Interrupted-doc
      diagnostics fire from these paths too (verified by
      `interrupted_doc_comment_on_local_assignment` and
      `interrupted_doc_comment_on_local_function` tests).  Storage
      is internal to the compiler scope; the Phase 5 plugin API
      will expose it via a node helper.
- [x] **EmmyLua `@class` / `@field` tag parsing in extract-lua**.
      `@class Name [: Parent]` on a top-level local declaration
      produces a `UserdataDoc` entry in
      `DocModel.userdata_types`; `@field name type [desc]` lines
      populate its `fields`.  Doc-only for now -- the type checker
      does not yet treat `@class`-named types as members of the
      type registry; that lands when Phase 5 wires up `@type`
      annotations.  Compiler exposes documented top-level locals
      via `ModuleTypeInfo.documented_locals`; docgen iterates them.
      Test: `class_annotation_produces_userdata_doc`.
- [x] **typing.lua canonical recipe documented**.  See
      `notes/LUA-ANNOTATIONS.md` for the author-facing reference,
      including the recommended pattern for combining
      `typing.record(...)` calls with `@class` / `@field`
      annotations.  Phase 5 lints will consume this.

### Phase 4: Data-driven lints

- [x] `deprecated` lint.  Reads
      `FunctionDoc.deprecated` / `FieldDoc.deprecated` from DocModel
      and the matching fields on `FunctionSignature` /
      `FunctionLuaType` / `FieldDef` in the compiler's type registry.
      Emits `LintId::Deprecated` at call sites for functions marked
      `@deprecated`.  End-to-end test
      `check_deprecated_function_warns` covers a `--types`-supplied
      deprecation.
- [x] Same lint for deprecated field accesses.  New
      `LuaType::lookup_member_deprecation` surfaces the field's
      deprecation flag for non-function members; check_var_expression
      emits the `Deprecated` warning when set.  Covers `Module.fields`
      and `Userdata.fields`.  Test:
      `check_deprecated_field_warns`.
- [x] `must_use` lint.  Fires when a function with
      `FunctionLuaType.must_use = Some(_)` (`@nodiscard` in DocModel)
      is called in statement position (return value discarded).
      End-to-end test `check_must_use_function_warns` covers both
      the warning case (`hash()`) and the no-warning case
      (`local _h = hash()`).
- [x] Module-level `@deprecated`.  `ModuleType.deprecated` and
      `ModuleDoc.deprecated` (schema 12), `#[module(deprecated = "...")]`
      macro option, Lua-side harvesting of `@deprecated` from the
      doc-comment on the chunk's returned local.  Field-access lint
      inherits the message from a sub-module via
      `lookup_member_deprecation` so `kumo.api`-style chains fire
      at the access site without a new lint hook.  Selene's
      `restricted_module_paths` use case (deprecating a whole
      module) is covered by this annotation-driven path.
      Test: `deprecated_submodule_access_through_parent_warns`,
      `field_own_deprecation_wins_over_submodule`,
      `module_macro_deprecated_attribute`,
      `module_level_deprecated_annotation`.

## Plugin implementation decisions (Phase 5 design)

These decisions were settled after the original Plugin Node Schema
section above; they specify the *implementation strategy* the schema
rests on.  Capture so a future compaction can resume Phase 5 without
re-litigating.

### IR construction and retention

- The lowered lint AST is built as a small second pass over the
  AST inside `Compiler::compile_with_ast`, not in `lower.rs`
  itself.  Keeps lowering / codegen separated from lint-shape
  concerns at the cost of one extra walk over the AST (cheap
  relative to lowering proper).
- IR construction is gated on `CompileOptions::type_check`: when
  `type_check: true`, `compile_with_ast` builds the IR; when
  `false`, the IR is omitted.  In practice kumomta and most
  embedded uses run with `type_check: true` because the enhanced
  diagnostics are valuable, so the IR will almost always be
  present in practice -- but `shingetsu run` paths that don't
  need diagnostics skip the cost.
- `Compiler::compile_with_ast` is reintroduced (it was removed
  earlier as unused) and returns a small struct holding the
  `Ast`, the (optional) lint IR, and the resulting `Bytecode`.
  `shingetsu check` callers that drive plugins take this path;
  `compile` keeps its current signature (returning `Bytecode`)
  for `run` / `shingetsu run` paths.
- Doc-comment harvesting is extended to every doc-able statement
  node -- `local_assign`, `local_function`, `function_decl`,
  `assign` (multi-target), at minimum.  The compiler stores the
  harvested raw text on the corresponding lint-IR node so
  `node:doc_comment()` is a direct field read.  Already covered
  for `local_assign` and `TableField` by Phase 3c; extension to
  the others is part of Phase 5.

### Plugin VM lifecycle

- One plugin `GlobalEnv` per `shingetsu check` invocation.  All
  plugins declared in `[check] plugins` load into that single VM,
  and every source file under check is walked through it.  Cheap
  to construct, no per-file VM churn.
- Plugin paths in `shingetsu.toml` resolve relative to the config
  file -- the existing project-config relative-path loader covers
  this.  No glob support in v1; paths are explicit.

### LintId migration

- Big-bang refactor: `LintId` becomes
  `BuiltIn(BuiltInLintId) | Plugin(Arc<str>)` (current enum becomes
  the `BuiltInLintId` inner enum), with the conversion landed in
  one PR before plugin loading lands.  All severity tables,
  `lint_directives` filter logic, and diagnostic rendering accept
  either variant.
- Diagnostic-id rendering for plugin lints uses a `project:`
  namespace prefix: `warning[project:my_plugin_lint]: ...`.  The
  same spelling is what users write in `--# shingetsu: allow=...`
  directives.
- Unknown lint names in `--# shingetsu: allow=...` (or the
  matching `disable`/`warn` forms) produce a diagnostic with
  did-you-mean suggestions drawn from the union of loaded
  built-in and plugin lints.  Severity: **error** when the
  unknown name has no `project:` prefix (it claims to be a
  built-in), **warning** when prefixed with `project:` (the
  plugin may simply be temporarily disabled in this run).

### Plugin error handling

- A Lua error raised inside a plugin callback is caught, converted
  to a `Diagnostic` with `Severity::Warning` against the user's
  source at the visited node's span (so the user sees something
  actionable about where the plugin choked), and the run
  continues with other lints.  The plugin's Lua traceback goes
  into the diagnostic's secondary spans / help so plugin authors
  can debug.  Reasoning: a single buggy plugin shouldn't blackhole
  `check`.

### `chunk_begin` / `chunk_end` and per-file state

- Plugins manage per-file state via closures / upvalues, not via
  a `ctx`-side scratchpad.  Keeps the `ctx`/node surface minimal.
- `chunk_begin` / `chunk_end` defer to Phase 6.  Neither v1
  integration test needs them, and they're trivially additive
  later.

### Plugin file shape

- One lint per plugin file.  The chunk's `lint.declare {...}` call
  is required exactly once at the top level, before any
  `lint.on(...)` calls.  The chunk itself does not need to
  return anything; the loader cares only about side effects on
  the `shingetsu.lint` host module's registry.
- The plugin loader enforces "exactly one `declare` per file".
  A second `declare` in the same chunk is a load-time error.

### Doc-comment access on call expressions

- `node:doc_comment()` on a call expression (`function_call`,
  `method_call`) is **liberal by default**: it walks to the
  immediate enclosing statement and returns that statement's doc
  comment.  This makes lints like
  `kumomta_record_doc_matches_runtime` -- which visit
  `function_call` looking for `mod.record(name, {...})` -- read
  the doc without manually navigating to the enclosing
  `local_assign` / `expr_statement`.
- An optional table param tightens the behaviour:
  `call:doc_comment { strict = true }` returns only the node's
  own doc-comment (currently always `nil` for call expressions,
  but the API is stable as the harvesting surface grows).
  Discerning lints that need to ignore inherited statement-level
  docs pass `strict = true`.
- Edge case to be aware of: in `local x = outer(inner())` both
  call expressions inherit the same `local_assign` doc.  A naive
  visitor on `function_call` fires twice with the same doc.  Lint
  authors who care about "top-level call only" can either guard
  with `strict = true` (and explicitly look up the statement) or
  filter on enclosing-statement kind once `ctx.enclosing` lands.

### Severity overrides for plugin lints

- A plugin's `lint.declare { default_severity = "warn" }` sets
  the baseline.
- `[check.lints]` in `shingetsu.toml` overrides per lint name:
  `"project:my_plugin_lint" = "error"`.
- Source-level `--# shingetsu: warn=project:my_plugin_lint` (or
  `error=`, `allow=`) overrides again at the file scope.
- Resolution order matches built-in lints: source directive >
  TOML override > plugin default.  The same
  `lint_directives.rs` filter pipeline accepts plugin ids.

### Phase 5: Plugin loader and minimal API

- [ ] Reintroduce `Compiler::compile_with_ast` returning a struct
      `{ ast, lint_ir, bytecode }` and route `shingetsu check`
      through it.
- [ ] Lint IR module: node taxonomy from the schema section
      above, built during the existing lowering pass.
- [ ] Big-bang `LintId` migration: `BuiltIn(BuiltInLintId) |
      Plugin(Arc<str>)`, all match sites updated.
- [ ] Separate `GlobalEnv` for plugins with sandboxed library set
      (`Libraries::SANDBOXED` + `shingetsu.lint` host module).
      One VM per `check` invocation.
- [ ] `shingetsu.lint` host module with `declare` and `on`.
- [ ] Plugin loading from `shingetsu.toml [check] plugins`
      (relative-to-config-file path resolution).
- [ ] First three events wired: `method_call`, `function_call`,
      `assign`.
- [ ] Diagnostic API on `ctx`: `warn`, `error`, span/node accepted.
      Plugin-emitted diagnostics use a `project:<lint_name>` id.
- [ ] **Doc-comment access on visited nodes**.  Visited statement
      nodes expose `node:doc_comment() -> string?` returning the
      raw text the compiler harvested.  Extends harvesting to
      `local_function`, `function_decl`, and `assign` beyond the
      Phase 3c-covered `local_assign` / `TableField`.  Call
      expressions delegate to their enclosing statement.
- [ ] Plugin error policy: callback errors become `Warning`
      diagnostics at the visited site; the run continues.
- [ ] Lint-directive validation: unknown lint name in
      `--# shingetsu: allow=...` emits a did-you-mean diagnostic.
      Error severity for non-`project:` names, warning for
      `project:` names.
- [ ] Two integration tests:
      - `kumomta_set_meta`: visitor on `method_call`, plain
        constant-arg check.
      - `kumomta_record_doc_matches_runtime`: visitor on
        `function_call`, walks the `mod.record(name, {fields})`
        argument table and compares against parsed `@class` /
        `@field` tags on the preceding doc comment.  Validates that
        the plugin API can express "runtime declaration vs.
        annotation drift" lints.

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
  v1. Defer to real plugin code.  Note: once `ctx.enclosing` lands,
  it pairs naturally with `node:doc_comment { strict = true }` to
  reach for the right doc-bearing ancestor.
- Exact `--enable` / `--disable` precedence vs `default_sets` / `optional_sets`
  when they conflict. Lean: CLI wins, last flag wins.
- Whether merged `DocModel` should be cached on disk between `check`
  invocations. Out of scope for v1.
