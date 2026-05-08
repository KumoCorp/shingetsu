---
title: Modules and functions
---

# Modules, functions, fields, registration

A *module* is a named table full of functions, constants, and
related data.  The standard library — `string`, `table`, `math`,
`os`, `io`, `utf8` — is structured this way, and host code uses the
same pattern to expose its own surface to scripts.

The `#[shingetsu::module]` attribute turns a Rust `mod { ... }`
block into a registerable Lua module.

## A first module

```rust
use shingetsu::module;

/// A small math module.
#[module(name = "smallmath")]
mod smallmath {
    /// Return the larger of two numbers.
    #[function]
    fn max(a: f64, b: f64) -> f64 {
        if a > b { a } else { b }
    }

    /// Return the smaller of two numbers.
    #[function]
    fn min(a: f64, b: f64) -> f64 {
        if a < b { a } else { b }
    }

    /// Module version.
    #[field]
    fn version() -> String {
        "1.0".to_owned()
    }
}
```

The macro generates three things inside the module:

- `pub fn build_module_table(env: &GlobalEnv) -> Result<Table, VmError>`
  — build the table without installing it.
- `pub fn register_global_module(env: &GlobalEnv) -> Result<(), VmError>`
  — install it as a global named `smallmath`.
- `pub fn register_preload(env: &GlobalEnv)` — install it so that
  `require("smallmath")` returns it lazily on first use.

Pick the registration form that matches how scripts should reach
the module.  For the standard library style ("always available as a
global"), use `register_global_module`.  For a module that should
only be loaded when asked for, use `register_preload`.

## Calling it from a script

```rust
use std::sync::Arc;
use shingetsu::{
    GlobalEnv, Libraries, Task, valuevec,
    compiler::{CompileOptions, Compiler},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::SANDBOXED)?;
    smallmath::register_global_module(&env)?;

    let opts = CompileOptions {
        debug_info: true,
        source_name: Arc::new("=demo".to_string()),
        type_check: true,
    };
    let compiler = Compiler::new(opts, env.global_type_map());
    let bc = compiler
        .compile("return smallmath.max(3, 7), smallmath.version")
        .await?;

    let task = Task::new(env, bc.into_function(), valuevec![]);
    let results = task.await?;
    for v in &results { println!("{v}"); }
    Ok(())
}
```

Output:

```
7
1.0
```

## Item annotations

Inside a `#[module]` block, each item carries an attribute that
decides how it appears in the generated Lua table.

### `#[function]`

A plain free function.  Parameters use `FromLua`, the return
value uses `IntoLuaMulti`.  This is the same shape as
`Function::wrap`, just attached to a module item:

```rust
use shingetsu::module;

#[module(name = "strutil")]
mod strutil {
    /// Repeat `s` `n` times.
    #[function]
    fn repeat(s: String, n: i64) -> String {
        s.repeat(n.max(0) as usize)
    }
}
```

### `#[function(rename = "...")]`

Override the Lua name when the Rust identifier is not what you
want scripts to see — typically because the Lua name would
clash with a Rust keyword:

```rust
use shingetsu::module;

#[module(name = "flow")]
mod flow {
    /// Lua-side name is `loop`, even though Rust won't accept
    /// `fn loop` as an identifier.
    #[function(rename = "loop")]
    fn lua_loop(n: i64) -> i64 {
        n
    }
}
```

### `#[function(variadic)]`

The last parameter is decoded via `FromLuaMulti` instead of
`FromLua`.  The most useful pairing is with an enum derived
`FromLuaMulti`, which gives you arity-overloaded dispatch in a
single function:

```rust
use shingetsu::{module, FromLuaMulti, Table, Value, VmError};

/// Argument shapes for `seq.insert(t, [pos,] value)`.
#[derive(FromLuaMulti)]
enum InsertArgs {
    AtPos { list: Table, pos: i64, value: Value },
    Append { list: Table, value: Value },
}

#[module(name = "seq")]
mod seq {
    use super::InsertArgs;
    use shingetsu::{Value, VmError};

    /// `seq.insert(t, value)` appends; `seq.insert(t, pos, value)`
    /// inserts at a position.
    #[function(variadic)]
    fn insert(args: InsertArgs) -> Result<(), VmError> {
        match args {
            InsertArgs::Append { list, value } => {
                let n = list.raw_len();
                list.raw_set(Value::Integer(n + 1), value)?;
            }
            InsertArgs::AtPos { list, pos, value } => {
                list.raw_set(Value::Integer(pos), value)?;
            }
        }
        Ok(())
    }
}
```

The `FromLuaMulti` derive tries variants longest-first, so
`seq.insert(t, 2, x)` matches `AtPos` and `seq.insert(t, x)`
falls through to `Append`.  See [Multi-value
returns](multi-values.md) for the derive's full behaviour.

### `#[field]` and `#[lazy_field]`

`#[field]` evaluates a zero-argument function *once*, when the
module table is built, and stores the result.  Use it for
constants and pre-computed values.  `#[lazy_field]` calls the
function *every time* the field is read — use it for values
that may change over the lifetime of the env or that are
expensive enough that computing them at startup would matter.

```rust
use shingetsu::module;

#[module(name = "build")]
mod build {
    /// Stamped at module-table construction time.
    #[field]
    fn version() -> String {
        env!("CARGO_PKG_VERSION").to_owned()
    }

    /// Recomputed on every read.
    #[lazy_field]
    fn uptime_seconds() -> f64 {
        // ... compute from a host clock ...
        0.0
    }
}
```

Lua-side, both look like properties:

```lua
print(build.version)         -- "0.1.0"
print(build.uptime_seconds)  -- 0.0  (read again later: a different value)
```

### `#[getter("name")]` and `#[setter("name")]`

Paired read/write accessors.  When both annotations name the
same Lua key, the field becomes read-write; either may appear
alone for read-only or write-only.  A bare `#[getter]` /
`#[setter]` strips the `get_` / `set_` prefix from the function
identifier as the Lua name:

```rust
use parking_lot::RwLock;
use shingetsu::module;
use std::sync::LazyLock;

static LEVEL: LazyLock<RwLock<i64>> = LazyLock::new(|| RwLock::new(0));

#[module(name = "log")]
mod log_mod {
    use super::LEVEL;

    #[getter("level")]
    fn get_level() -> i64 {
        *LEVEL.read()
    }

    #[setter("level")]
    fn set_level(v: i64) {
        *LEVEL.write() = v;
    }
}
```

```lua
print(log.level)   -- 0
log.level = 3
print(log.level)   -- 3
```

This is the same mechanism used for userdata properties (see
[Userdata](userdata.md#field-accessors-and-properties)); use
`#[getter]`/`#[setter]` on a module when the field needs both
read and write semantics, and `#[field]` / `#[lazy_field]` when
read-only is enough.

The default Lua name for `#[function]`, `#[field]`, and
`#[lazy_field]` is the Rust identifier.  Use `rename = "..."` to
override.

## Auto-injected parameters

A `#[function]` may take a `CallContext` or `GlobalEnv` parameter.
These are filled in by the dispatch wrapper, not by the script:

```rust
use shingetsu::module;

#[module(name = "hostinfo")]
mod hostinfo {
    use shingetsu::{Bytes, CallContext};

    /// Returns the name under which this native was registered.
    #[function]
    fn whoami(ctx: CallContext) -> Option<Bytes> {
        ctx.native_name.clone()
    }
}
```

`ctx.native_name` is the name of *this* native function — the
thing currently executing.  To learn about the *caller*, walk
the call stack: `ctx.call_stack()` exposes frames top-down, with
the topmost being this native and the next one down being
whoever invoked it.

`CallContext` is the right choice when the function needs to
call back into Lua (`ctx.call_function(...)`), inspect the call
stack, or attach hints to errors.  `GlobalEnv` alone is enough
when you just need to read globals or installed modules.  The
script does not see these parameters.

## Hand-rolled registration

When the `#[module]` shape genuinely does not fit — for example,
when the table's contents are decided at runtime from
configuration — you can build the table yourself:

```rust
use shingetsu::{Function, GlobalEnv, Table, Value, VmError};

fn install_random(env: &GlobalEnv) -> Result<(), VmError> {
    let table = Table::new();
    let u32_fn = Function::wrap(
        "random.u32",
        || -> Result<i64, VmError> { Ok(rand::random::<u32>() as i64) },
    );
    table.raw_set(Value::string("u32"), Value::Function(u32_fn))?;
    env.set_global("random", Value::Table(table));
    Ok(())
}
```

!!! warning "Hand-rolled registration loses type information"

    The `#[module]` macro emits compile-time type metadata
    alongside the runtime table: parameter and return types,
    field types, doc strings, examples.  All of that feeds the
    type checker, the documentation generator, and the IDE
    integration.  Hand-rolled registration produces only the
    runtime table — scripts can call your functions, but the
    type checker cannot validate argument types at compile
    time, the docs site has no entry for the module, and
    `--list` output does not see it.

    Reach for hand-rolled registration only when the macro
    truly can't express what you need.  In almost every other
    case, lifting the function bodies into a `mod` annotated
    with `#[shingetsu::module]` will be both shorter and more
    capable.

## Putting modules where scripts expect them

Two common patterns:

1. **Always-on global**, like `string` or `math` —
   `register_global_module`.  Scripts do `mymod.foo()` directly.
2. **Lazy-loaded**, like a third-party Lua module —
   `register_preload`.  Scripts do `local m = require("mymod"); m.foo()`.
   This requires `Libraries::PACKAGE` for `require` to be in scope,
   though preloaded modules are looked up *before* any filesystem
   search, so they work even with no `package.path`.

For loading modules from places other than the filesystem (a
database, a bundled archive), see [Custom module
loaders](module-loaders.md).
