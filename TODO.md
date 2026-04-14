# TODO

## Status

**Phase 1** — complete (numeric core, arithmetic, control flow, locals/globals,
`goto`/`::label::`, `<const>`, `if`/`while`/`repeat`/`for`, logical ops).

**Phase 2** — complete (strings, tables, userdata, `<close>`, `break`,
method calls, chained indexing/calls, table constructors, string escapes,
dotted/method function declarations, `NativeFunction` with runtime type checking,
multiple return values, `select`, `#s`, varargs).

**Phase 3** — complete (upvalue capture multi-level, closures, pcall/xpcall,
error with levels, full metatables, generic for, type/tostring/tonumber/next/
pairs/ipairs builtins, Task::dispose, GC with cycle collection + finalizers).

**Phase 4 (LuaU)** — partial.  Compound assignments, `continue`, `if`
expressions done.  Type annotations and generic type params still open.

**Phase 5 (Embedding API & Standard Library)** — Steps 1–3 complete.
Step 4 (stdlib modules) is next.

**547 integration tests passing.**

---

## Phase 4 (LuaU / future)

- [x] Compound assignments (`+=`, `-=`, `*=`, `/=`, `//=`, `%=`, `^=`, `..=`)
- [x] `continue` statement (LuaU)
- [x] `if` expressions (LuaU)
- [ ] Generic type parameter tracking and constraint checking

### Runtime type checking

Infrastructure exists: `ParamSpec` has `runtime_type: Option<ValueType>` and
`lua_type: Option<LuaType>`.  `validate_args` in `task.rs` checks
`runtime_type` at call boundaries for `NativeFunction` calls.  However,
neither the compiler nor the proc macros populate these fields today —
`params` is always `Vec::new()` and every `runtime_type` is `None`.

#### 4a — Proc macro: emit `ParamSpec` with `runtime_type` for native functions

The `#[function]` proc macro already knows each parameter's Rust type
(`Table`, `Bytes`, `i64`, `f64`, `bool`, `Value`, `Option<T>`,
`Variadic`, `CallContext`).  It should emit a `ParamSpec` vec in the
generated `FunctionSignature` with the corresponding `ValueType`:

| Rust type | `ValueType` |
|---|---|
| `bool` | `Boolean` |
| `i64` / `i32` | `Integer` |
| `f64` / `f32` | `Float` |
| `Bytes` / `String` | `String` |
| `Table` | `Table` |
| `Function` | `Function` |
| `Value` | `Any` |
| `Option<T>` | derive from `T` but mark optional (skip nil) |
| `Variadic` | stop (remaining args unconstrained) |
| `CallContext` | skip (not a Lua arg) |

This makes all `#[function]` builtins and stdlib entries produce better
error messages without changing any calling code — `validate_args`
already runs before every `NativeFunction` call.

- [x] Extend `gen_native_fn` (module `#[function]`) to emit
  `params: vec![ParamSpec { ... }, ...]`
- [x] Extend userdata `#[lua_method]` / `#[lua_metamethod]` / `#[lua_field]`
  generation to emit `ParamSpec` with `runtime_type`.  Methods use
  `arg_offset: 1` in `FunctionSignature` so `validate_args` skips the
  implicit self.  Metamethods and field setters use inline type checks
  in `gen_call_body` with context-appropriate error messages
  (field setters: `"bad value in assignment to 'T.field' (...)"`;
  metamethods: `"bad argument #N to 'T:__mm' (...)"`).
- [x] Emit `variadic: true` only when the last Lua-visible param is `Variadic`

#### 4b — LuaType annotation parsing (compiler)

The compiler now parses LuaU type annotations and populates
`ParamSpec.lua_type` and `FunctionSignature.lua_returns`.

- [x] Parse LuaU parameter type annotations from `full_moon` AST into
  `ParamSpec.lua_type` — `type_convert.rs` handles `Basic`, `Optional`,
  `Union`, `Intersection`, `Callback`, `Array`, `Table`, `Generic`,
  `Tuple`, `Variadic`, `Module`, `Typeof`, string/boolean literals.
- [x] Parse LuaU return type annotations into
  `FunctionSignature.lua_returns` — tuple returns become multi-element
  vecs.

#### 4c — `runtime_type` derivation from `LuaType` — complete

`derive_runtime_type(lt: &LuaType) -> Option<ValueType>` in
`shingetsu-vm/src/types.rs` converts simple type annotations into
runtime-checkable `ValueType` values.  The compiler calls it when
building `ParamSpec` for Lua functions, and `validate_args` enforces
types at all call boundaries (including the initial `Task::new_inner`
entry point, which was previously unchecked).

- [x] Add `fn derive_runtime_type(lt: &LuaType) -> Option<ValueType>`
- [x] Call it in the compiler when building `ParamSpec` for Lua functions
- [x] VM's `validate_args` enforces types for annotated Lua functions
  at call boundaries, including initial task entry

---

## Error message quality

The VM currently produces generic error messages like `"attempt to index a
nil value"` without naming the variable or field involved.  These should
include contextual information derived from debug info (`LocalDesc`,
constant pool, instruction operands) so the user can immediately see what
went wrong.

Each item below covers a class of error that needs a better message.
The `locals` vec in `Proto` maps register slots to variable names; the
constant pool contains field/global name strings.  The VM can consult
these when building error messages.

### Indexing / field access on wrong type

- [ ] `nil_global:method()` — e.g.
  `"attempt to index global 'nil_global' (a nil value)"`
- [ ] `nil_local.field` — e.g.
  `"attempt to index local 'x' (a nil value)"`
- [ ] `nil_local:method()` — e.g.
  `"attempt to call method 'method' on local 'x' (a nil value)"`
- [ ] `number_var.field` / `number_var:method()` — e.g.
  `"attempt to index local 'n' (a number value)"`
- [ ] `boolean_var.field` / `boolean_var:method()` — e.g.
  `"attempt to index local 'b' (a boolean value)"`
- [ ] `nil_global.field = value` (newindex on nil global) — e.g.
  `"attempt to index global 'g' (a nil value)"`
- [ ] `nil_local.field = value` (newindex on nil local) — e.g.
  `"attempt to index local 'x' (a nil value)"`

### Calling non-callable values

- [ ] `nil_var()` — e.g.
  `"attempt to call local 'f' (a nil value)"`
- [ ] `nil_global()` — e.g.
  `"attempt to call global 'g' (a nil value)"`
- [ ] `number_var()` — e.g.
  `"attempt to call local 'n' (a number value)"`
- [ ] `boolean_var()` — e.g.
  `"attempt to call local 'b' (a boolean value)"`

### Arithmetic / concatenation on wrong type

- [ ] `nil + 1` — e.g.
  `"attempt to perform arithmetic on local 'x' (a nil value)"`
- [ ] `true .. "hello"` — e.g.
  `"attempt to concatenate local 'b' (a boolean value)"`

### Comparison on incompatible types

- [ ] `nil < 1` — e.g.
  `"attempt to compare local 'x' (a nil value) with number"`

---

## Phase 5 — Embedding API & Standard Library

### Step 1 — Foundation ✓

- [x] Crate restructure (`shingetsu` lib, `shingetsu-cli` bin)
- [x] `downcast-rs`, `Variadic`, `MetaMethod` enum, `LuaType::Module`
- [x] `LuaTyped` trait, `FromLua`/`IntoLua`/`IntoLuaMulti`/`FromLuaMulti`
- [x] `VmError::BadArgument` widened (`got: String`)
- [x] `VmError::with_arg_and_call_context` — patches `BadArgument` errors
  with the correct 1-based position and function name from a `CallContext`.
  Used by proc-macro generated code and hand-written builtins.
- [x] `FromLuaMulti` tuple impls track 1-based position in `BadArgument`

### Step 2 — Proc macros (`shingetsu-derive`) ✓

- [x] `#[shingetsu::module]` — generates `build_module_table`,
  `register_global_module`, `register_preload`.
- [x] `#[shingetsu::userdata]` + `#[derive(UserData)]`
- [x] Proc-macro generated `FromLua` extractions automatically call
  `with_arg_and_call_context` for correct error context.

### Step 3 — Builtin migration & `require` ✓

- [x] **`require` builtin** — preload/loaded infrastructure; `FromLuaMulti`
  for argument parsing.
- [x] **Builtin migration to `#[module]` macro** — 13 builtins moved to
  `shingetsu::builtins` module (`type`, `rawget`, `rawset`, `tonumber`,
  `tostring`, `next`, `getmetatable`, `setmetatable`, `select`, `error`,
  `assert`, `pairs`, `collectgarbage`).  Uses `extern crate self as
  shingetsu` trick for `::shingetsu::` path resolution within the crate.
- [x] **Remaining hand-written builtins** — `pcall`, `xpcall`, `ipairs`,
  `require` stay in `GlobalEnv::register_builtins` (need private internals
  or custom calling conventions).
- [x] **`GlobalEnv::register_from_table`** — installs all string-keyed
  entries from a table as globals; used by `shingetsu::builtins::register`.
- [x] **`shingetsu::builtins::register(&GlobalEnv)`** — one-call
  registration of all macro-generated builtins.
- [x] **Ergonomics** — `get_metamethod` and `get_global` accept
  `impl AsRef<[u8]>` (plain `"strings"` instead of `b"bytes"`).

### Step 4 — Standard Library Modules

Stdlib modules are defined in the `shingetsu` crate (under `src/`) using
the `#[module]` proc macro.  `shingetsu::builtins::register` calls each
module's registration function so all stdlib is available after one call.

Functions returning variable numbers of values use `Variadic` (not
`Vec<Value>`) since `Vec<T: IntoLua>` converts to a table via the blanket
`IntoLuaMulti` impl.

#### `string` library

- [x] **`string` module scaffold** — register `string` global table; set as
  the metatable `__index` for all string values so `("hello"):upper()` works.
  `GlobalEnvInner.string_metatable` stores the shared metatable; VM's
  `GetTable` instruction consults it for `Value::String`.
- [x] **Length & inspection** — `string.len(s)`, `string.byte(s [,i [,j]])`,
  `string.char(...)`.
- [x] **Case & reversal** — `string.upper(s)`, `string.lower(s)`,
  `string.reverse(s)`.
- [x] **Substrings & repetition** — `string.sub(s, i [,j])`,
  `string.rep(s, n [,sep])`.
- [x] **Search & match** — `string.find(s, pattern [,init [,plain]])`,
  `string.match(s, pattern [,init])`.  Uses in-house `lua_pattern` module
  (Lua pattern → regex translator) + `regex` crate for matching.
- [x] **Iterators** — `string.gmatch(s, pattern)`.  Returns a native
  iterator function with captured state.
- [x] **Replacement** — `string.gsub(s, pattern, repl [,n])`.  String and
  table replacement supported.
- [x] **`string.gsub` function replacement** — `repl` can be a function
  called with captures; return value becomes the replacement.
- [x] **Formatting** — `string.format(fmt, ...)`.  Supports `%d`, `%i`,
  `%u`, `%f`, `%e`, `%g`, `%x`, `%X`, `%o`, `%s`, `%c`, `%q`, `%%`.

#### `table` library

- [x] **Sequential operations** — `table.insert(t, [pos,] v)`,
  `table.remove(t [,pos])`, `table.concat(t [,sep [,i [,j]]])`.
- [x] **Sorting** — `table.sort(t [,comp])`.  `comp` is an optional Lua
  comparator function; uses async merge sort (O(n log n)) with
  `swap_array` for zero-copy.
- [x] **Movement & packing** — `table.move(a1, f, e, t [,a2])`,
  `table.pack(...)`, `table.unpack(t [,i [,j]])` (also exposed as global
  `unpack` for Lua 5.1 compat).

#### `math` library

- [x] **Constants** — `math.pi`, `math.huge`, `math.maxinteger`,
  `math.mininteger`.
- [x] **Rounding & sign** — `math.floor(x)`, `math.ceil(x)`,
  `math.abs(x)`, `math.modf(x)` (returns integral + fractional parts).
- [x] **Exponential & logarithmic** — `math.sqrt(x)`, `math.exp(x)`,
  `math.log(x [,base])`.
- [x] **Trigonometric** — `math.sin(x)`, `math.cos(x)`, `math.tan(x)`,
  `math.asin(x)`, `math.acos(x)`, `math.atan(y [,x])`.
- [x] **Min / max / clamp** — `math.min(...)`, `math.max(...)`.
- [x] **Integer operations** — `math.tointeger(x)`, `math.type(x)`
  (`"integer"`, `"float"`, or `fail`).
- [x] **Random** — `math.random([m [,n]])`, `math.randomseed([x [,y]])`.

#### Missing global builtins

- [x] `print(...)` — write tab-separated `tostring()` of each arg to stdout
- [ ] `rawequal(v1, v2)` — equality without metamethods
- [ ] `rawlen(v)` — length without metamethods
- [ ] `load(chunk [,chunkname [,mode [,env]]])` — load a chunk from string.
  *Not in LuaU (sandboxing).*
- [ ] `dofile([filename])` / `loadfile([filename [,mode [,env]]])`.
  *Not in LuaU (sandboxing).*
- [ ] `warn(msg1 [,...])` — warning system (Lua 5.4 only).
  *Not in LuaU.*

#### Missing stdlib modules

- _deferred_ `coroutine` — `create`, `resume`, `yield`, `status`, `wrap`,
  `isyieldable`, `close`.  *In LuaU.*  Out of scope for now.
- [ ] `io` — file I/O (`open`, `read`, `write`, `lines`, `close`, etc.).
  *Not in LuaU (sandboxing).*
- [ ] `os` — LuaU only exposes `os.clock`, `os.date`, `os.time`,
  `os.difftime`.  Lua 5.4 also has `execute`, `exit`, `getenv`, `remove`,
  `rename`, `tmpname` — *not in LuaU (sandboxing).*
- _deferred_ `package` — `path`, `cpath`, `config`, `loaded`, `preload`,
  `searchpath`.  *Not in LuaU (sandboxing).*  Currently only `require` +
  preload registry exist.  Out of scope for now.
- [ ] `utf8` — `char`, `codes`, `codepoint`, `len`, `offset`.  *In LuaU.*
- _deferred_ `debug` — LuaU only exposes `debug.info` and
  `debug.traceback`; Lua 5.4 has `getinfo`, `sethook`, etc.
  Out of scope for now.
- [ ] `bit32` — `band`, `bor`, `bxor`, `bnot`, `lshift`, `rshift`,
  `arshift`, `lrotate`, `rrotate`, `extract`, `replace`, `btest`,
  `countlz`, `countrz`.  *In LuaU (replaces Lua 5.3+ bitwise operators).*
- [ ] `string.pack` / `string.unpack` / `string.packsize` — binary
  packing.  *In both Lua 5.3+ and LuaU.*

#### LuaU-specific extensions (not in Lua 5.4)

- [ ] `table.create(n [,v])`, `table.find(t, v [,init])`,
  `table.clear(t)`, `table.freeze(t)`, `table.isfrozen(t)`,
  `table.clone(t)`
- [ ] `string.split(s [,sep])`
- [ ] `typeof(obj)` — returns host-defined type name for userdata
- [ ] `buffer` library — fixed-size mutable byte arrays
- [ ] `vector` library — SIMD vector type
