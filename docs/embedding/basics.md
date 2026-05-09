---
title: Basics
---

# Basics

This page walks through the smallest useful embedding: build an
environment, compile a script, run it, and report any error.
Everything else in this section assumes you have this skeleton in
place.

## Building a `GlobalEnv`

A [`GlobalEnv`](../api/shingetsu/struct.GlobalEnv.html) is the long-lived,
shared environment that scripts run against.  Construct one with
`GlobalEnv::new()` and ask
[`shingetsu::register_libs`](../api/shingetsu/fn.register_libs.html) to
install whichever standard libraries you want scripts to see:

```rust
use shingetsu::{GlobalEnv, Libraries};

let env = GlobalEnv::new();
shingetsu::register_libs(&env, Libraries::SANDBOXED)?;
```

[`Libraries`](../api/shingetsu/struct.Libraries.html) is a bitflag.  `SANDBOXED` gives you only `BUILTINS` —
`print`, `pcall`, `string`, `table`, `math`, `utf8`, and similar
pure functions.  `ALL` adds `os`, `io`, `stdio`, `package`, and
`load`.  Each capability is a separate flag so that a host can, for
example, hand a script the calendar (`OS`) without also handing it
the ability to spawn processes (`EXEC`) or read environment
variables (`ENV`).  See [Sandboxing](sandboxing.md) for the full
breakdown.

The `GlobalEnv` is cheap to clone (it is internally reference
counted) and can be shared across threads and tasks.  One per host
program is normal.

## Compiling a chunk

Compilation is async because the compiler may need to load other
modules through the configured module loader.  The compiler reads
type information from the `GlobalEnv` so that compile-time
diagnostics know about whatever modules the host has registered:

```rust
use std::sync::Arc;
use shingetsu::compiler::{CompileOptions, Compiler};

let opts = CompileOptions {
    debug_info: true,
    source_name: Arc::new("=hello".to_string()),
    type_check: true,
};

let compiler = Compiler::new(opts, env.global_type_map());
let bytecode = compiler.compile("return 1 + 2").await?;
```

`source_name` is the label that shows up in stack traces and error
messages.  By convention, names that begin with `=` are shown
verbatim and names that begin with `@` are treated as file paths.

The result is a `Bytecode` value.  You can run it once and discard
it, or keep it around and run it many times against many different
tasks — a `Bytecode` is independent of any one execution.

## Running a `Task`

To execute the compiled chunk, turn it into a callable
[`Function`](../api/shingetsu/struct.Function.html) and hand it to
[`Task::new`](../api/shingetsu/struct.Task.html#method.new):

```rust
use shingetsu::{Task, valuevec};

let func = bytecode.into_function();
let task = Task::new(env.clone(), func, valuevec![]);
let results = task.await?;
```

`Task` implements `Future`, so you `.await` it like any other
future.  Its output is `Result<ValueVec, RuntimeError>`: the
`ValueVec` holds the values returned by the chunk's top-level
`return` statement (empty if the chunk did not return anything).

Putting it all together, here is a complete `tokio::main`:

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

    let opts = CompileOptions {
        debug_info: true,
        source_name: Arc::new("=hello".to_string()),
        type_check: true,
    };
    let compiler = Compiler::new(opts, env.global_type_map());
    let bytecode = compiler.compile("return 1 + 2").await?;

    let task = Task::new(env, bytecode.into_function(), valuevec![]);
    let results = task.await?;

    for v in &results {
        println!("{v}");
    }
    Ok(())
}
```

Run this and it prints `3`.

## Passing arguments to the chunk

The third argument to `Task::new` is a
[`ValueVec`](../api/shingetsu/type.ValueVec.html) of arguments that the
chunk receives as `...`:

```rust
use shingetsu::{Value, valuevec};

let bytecode = compiler.compile("local x, y = ...; return x + y").await?;
let task = Task::new(
    env,
    bytecode.into_function(),
    valuevec![Value::Integer(2), Value::Integer(3)],
);
let results = task.await?;
```

The [`valuevec!`](../api/shingetsu/macro.valuevec.html) macro builds a
`ValueVec` from [`Value`](../api/shingetsu/enum.Value.html) expressions.
[Mapping values](mapping-values.md) covers richer ways to build
argument lists from arbitrary Rust types via
[`IntoLuaMulti`](../api/shingetsu/trait.IntoLuaMulti.html).

## Handling errors

Two kinds of errors come out of this pipeline:

- **Compile errors** — a `CompileError` returned by
  `Compiler::compile`.  They carry source spans and turn into a
  pretty diagnostic via `shingetsu::diagnostic::render_compile_error`.
- **Runtime errors** — a `RuntimeError` returned when a `Task` ends
  in an error.  They carry the offending
  [`VmError`](../api/shingetsu/enum.VmError.html), a call stack, and
  any hints attached along the way; render them with
  `shingetsu::diagnostic::render_runtime_error`.

The renderer takes a `RenderStyle` so the host can choose between
ANSI-coloured output for terminals and plain text for logs:

```rust
use shingetsu::diagnostic::{
    RenderStyle, render_compile_error, render_runtime_error,
};

let style = if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
    RenderStyle::Colored
} else {
    RenderStyle::Plain
};

let bytecode = match compiler.compile(&source).await {
    Ok(bc) => bc,
    Err(e) => {
        eprint!("{}", render_compile_error(&e, &source, style));
        return Ok(());
    }
};

let task = Task::new(env, bytecode.into_function(), valuevec![]);
match task.await {
    Ok(results) => {
        for v in &results {
            println!("{v}");
        }
    }
    Err(re) => {
        eprint!("{}", render_runtime_error(&re, style));
    }
}
```

The renderer needs the original source text for compile errors so
that it can quote the offending lines; runtime errors carry their
own source references inside the call stack.

`RuntimeError` is the type you will see most often as you build
host bindings.  The [Errors and diagnostics](errors-and-diagnostics.md)
page covers how to construct one yourself when a host function
needs to fail with a useful message.

## Cancelling a task

Dropping a `Task` cancels it without running any to-be-closed
finalisers.  If the script may be holding `<close>` resources whose
cleanup you care about, call `Task::dispose().await` instead.
`dispose` walks the still-open frames, runs each `__close` handler,
and then resolves.  Errors raised by handlers are silently dropped
in favour of the original cancellation.

## What's next

The skeleton above runs scripts, but it does not yet let scripts
talk to the host or vice versa.  The next page,
[Mapping values](mapping-values.md), covers the conversion traits
that bridge the two sides.
