---
title: Errors and diagnostics
---

# Errors and diagnostics

Two kinds of error escape from the embedding pipeline:

- A **`CompileError`** comes from `Compiler::compile` and means the
  source did not turn into bytecode.  It carries source spans and
  one or more diagnostics.
- A **`RuntimeError`** comes out of `Task::await` and means the
  script raised an error or hit a VM-level fault.  It carries the
  underlying [`VmError`](../api/shingetsu/enum.VmError.html), a snapshot of the call stack, and any
  hints attached along the way.

Both render through `shingetsu::diagnostic`.

## Rendering

`shingetsu::diagnostic` exposes three functions you will use most
of the time:

```rust
use shingetsu::diagnostic::{
    RenderStyle, render_compile_error, render_runtime_error, render_warnings,
};
```

`RenderStyle::Colored` produces ANSI-coloured output for terminals;
`RenderStyle::Plain` produces unstyled text suitable for log files.
The pattern from [Basics](basics.md) is the canonical use:

```rust
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
```

`render_warnings` is the lint counterpart — `Bytecode::diagnostics`
holds compile-time lints (unused variables, shadowed locals, type
mismatches) that did not stop compilation; render and print those
the same way.

### What rendered output looks like

A few representative cases.  All examples below are the actual
output of `shingetsu run`, which uses these renderers — your
embedding will produce the same shape.

**Compile error** (a missing expression after a binary operator):

```text
error: unexpected token `+`, expected expression after binary operator
 --> script.lua:1:13
  |
1 | local x = 1 +
  |             ^ unexpected token `+`, expected expression after binary operator
```

**Runtime error with variable context** (indexing a `nil`).  The
renderer points at the offending receiver *and* at the line
where the variable was last bound, so the reader can see why it
is `nil`:

```text
error: attempt to index local 'cfg' (a nil value) with key 'host'
 --> script.lua:5:7
  |
4 | local cfg = build()
  |       --- defined here
5 | print(cfg.host)
  |       ^^^ attempt to index local 'cfg' (a nil value) with key 'host'
stack traceback:
	script.lua:5: in main chunk
```

**Bad argument from a standard-library call** — the position
and function name are in the message, and the caret span covers
the specific argument that triggered the error (here, the second
one):

```text
error: bad argument #2 to 'rep' (number expected, got string)
 --> script.lua:1:27
  |
1 | local n = string.rep("x", "three")
  |                           ^^^^^^^ bad argument #2 to 'rep' (number expected, got string)
stack traceback:
	script.lua:1: in main chunk
```

**Lint warning** rendered alongside (not in place of) source
— unused locals, shadowed names, and type-checker findings come
out of `Bytecode::diagnostics` and are passed to `render_warnings`:

```text
warning[unused_variable]: unused variable 'total'
 --> script.lua:2:7
  |
2 | local total = name + 10
  |       ^^^^^ unused variable 'total'
  |
help: prefix the name with '_' to suppress this warning: '_total'
```

The warning is printed *before* execution begins; if the lint
severity is `Severity::Error`, the embedder typically refuses to
run the chunk.

## Raising errors from the host side

Inside a [`Function::wrap`](../api/shingetsu/struct.Function.html#method.wrap) closure, `#[function]`, or `#[lua_method]`,
return a `Result<T, VmError>`.  The most common construction is
`VmError::LuaError`, which mirrors `error("message")` from a
script:

```rust
use shingetsu::{Function, Value, VmError};

let div = Function::wrap("div", |a: f64, b: f64| -> Result<f64, VmError> {
    if b == 0.0 {
        return Err(VmError::LuaError {
            display: "divide by zero".into(),
            value: Value::string("divide by zero"),
        });
    }
    Ok(a / b)
});
```

The `display` field is the human-readable message; `value` is what
a `pcall` from the script sees as the error value.  In most cases
they are the same string.

For more structured errors, `VmError` has named variants for the
common cases:

- `BadArgument { position, function, expected, got }` — wrong
  argument type or count.  You will rarely build this one yourself;
  it is what `Function::wrap`'s extraction code produces.
- `HostError { name, source }` — for failures inside host code that
  are not really Lua errors (an underlying I/O error, a database
  failure).  `source` is `anyhow::Error`.
- `IoError { ... }` — for I/O-flavoured failures with a path attached.
- `ExitRequested { code, close }` — produced by `os.exit`.  See
  the basics page for how the CLI handles this.

For everything else, fall back to `LuaError`.

## Argument-position errors with `VmResultExt`

When your function calls into another conversion that might fail,
you usually want the error to be tagged with the right argument
position.  The [`VmResultExt`](../api/shingetsu/trait.VmResultExt.html) trait does that without a manual
`map_err`:

```rust
use shingetsu::{
    CallContext, Function, FromLua, Table, Value, VmError, VmResultExt,
};

let parse = Function::wrap(
    "parse",
    |ctx: CallContext, raw: Value| -> Result<i64, VmError> {
        let table = Table::from_lua(raw).with_call_context(1, &ctx)?;
        let n = table.raw_get(&Value::string("count"))?;
        i64::from_lua(n).with_call_context(1, &ctx)
    },
);
```

`with_call_context(1, &ctx)` patches any `BadArgument` error coming
out of the conversion with position 1 and the current function's
name.  The script sees a coherent message of the form
`bad argument #1 to 'parse' (...)` instead of a generic conversion
error.

`VmResultExt::with_function_name` is the variant for paths that
know the function name statically (typical inside a userdata
method's generated wrapper) and do not need a full `CallContext`.

## Hints

A `Hint` attaches a `help:` annotation to the rendered error,
pointing at a specific source location.  Hints are how the runtime
suggests `.` instead of `:` when a method is called wrong, or
points at the nearest spelling when an unknown field is accessed.

You build a `RuntimeError` with hints by hand only in unusual
cases.  Usually you raise a `VmError`, the surrounding machinery
wraps it into a `RuntimeError` at the task boundary, and any hints
are attached to the bare `VmError` ahead of time via the
diagnostic helpers in `shingetsu::diagnostics::render_field_suggestion`.
This page does not cover building hints from scratch — the
mechanics are stable but the API surface for it is small enough
that direct rustdoc is the better reference.

## Inspecting an error

Sometimes the host wants to react to a specific kind of failure —
an `os.exit` should terminate the process, an out-of-memory might
trigger a different code path:

```rust
use shingetsu::{RuntimeError, VmError};

match re.error {
    VmError::ExitRequested { code, close } => {
        // Honour os.exit().
    }
    VmError::IoError { .. } => {
        // I/O-flavoured failure.
    }
    other => {
        eprintln!("script failed: {other}");
    }
}
```

`re.error` gives you the underlying `VmError`; `re.call_stack`
gives you the frames at the point of failure; `re.hints` gives you
any attached hints.  Most embedders only ever look at
`re.vm_error()` (or pattern-match `re.error` directly).

## Rendering for logs vs. terminals

For terminals, render with `RenderStyle::Colored` and let the user
read the structured output.  For log files or non-TTY pipelines,
render with `RenderStyle::Plain` so the output stays grep-friendly.

For programmatic post-processing — sending an error to a metrics
backend, or writing it to a database — pull the fields off the
`RuntimeError` directly rather than parsing the rendered string.
The render is for humans.
