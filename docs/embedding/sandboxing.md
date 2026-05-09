---
title: Sandboxing
---

# Sandboxing

Shingetsu's default posture is "the script sees nothing the host
did not put in front of it."  There is no implicit access to the
filesystem, the network, environment variables, or the process
itself.  Standard-library surface is opt-in, capability by
capability.

This page covers what the standard libraries actually expose, how
to choose them, and the patterns hosts use when they need to
expose host capabilities themselves.

## The `Libraries` bitflag

[`shingetsu::register_libs(env, flags)`](../api/shingetsu/fn.register_libs.html) installs a chosen set of
standard libraries into a [`GlobalEnv`](../api/shingetsu/struct.GlobalEnv.html).
The [`Libraries`](../api/shingetsu/struct.Libraries.html) bitflag has one entry per
capability:

| Flag        | What it adds                                                     |
|-------------|------------------------------------------------------------------|
| `BUILTINS`  | `print`, `type`, `pcall`, `tostring`, `math`, `string`, `table`, `utf8` |
| `OS`        | `os.clock`, `os.time`, `os.date`, `os.difftime`                  |
| `IO`        | `io.open`, `io.tmpfile`, plus `os.remove`, `os.rename`, `os.tmpname` |
| `STDIO`     | `io.stdin`, `io.stdout`, `io.stderr`, `io.read`, `io.write`, `io.flush` (implies `IO`) |
| `EXEC`      | `io.popen`, `os.execute` (implies `IO`)                          |
| `ENV`       | `os.getenv`                                                      |
| `EXIT`      | `os.exit`                                                        |
| `DEBUG`     | `debug.getlocal`, `debug.getupvalue`, `debug.setupvalue`         |
| `PACKAGE`   | filesystem-based `require` (searches `package.path`)             |
| `LOAD`      | `load()` — compile and execute strings at runtime                |
| `ALL`       | everything except `DEBUG`                                        |
| `SANDBOXED` | just `BUILTINS`                                                  |

Two convenience aliases are worth calling out:

- **`SANDBOXED`** is the right starting point for untrusted scripts.
  It exposes only side-effect-free libraries — pure functions on
  numbers, strings, tables, and UTF-8 — plus `print` for diagnostic
  output and `pcall` / `error` for the script's own error handling.
- **`ALL`** is the right starting point for trusted scripts in a
  controlled environment, like the embedder's own test harness or
  a developer-facing REPL.  Note that it does *not* include
  `DEBUG`: introspection of frame locals and upvalues is its own
  opt-in even within "all", because exposing it weakens otherwise
  airtight encapsulation.

## Why each capability is its own flag

The flags are split fine-grained on purpose.  A few examples of
the design intent:

- `EXEC` is separate from `OS` because spawning processes is a
  qualitatively different capability from reading the wall clock.
- `ENV` is separate because environment variables routinely carry
  credentials, paths, and host fingerprinting data — a script
  asking for the time should not also be able to read
  `AWS_SECRET_ACCESS_KEY`.
- `EXIT` is separate because surrendering control of the host
  process to the script is a much bigger deal than letting it ask
  what time it is.
- `LOAD` is separate because compiling and running a string at
  runtime defeats every static check the type checker performs;
  you want to make a conscious choice to allow it.
- `DEBUG` is separate (and excluded from `ALL`) because frame-local
  introspection lets a script reach across encapsulation boundaries
  that the host probably did not intend to weaken.

## Always-on, regardless of flags

A small core is registered no matter what you ask for:

- The sandbox-safe parts of `debug` — `debug.traceback`,
  `debug.info`, `debug.getinfo` — are always present.  They expose
  call-site information, never frame contents.
- After you call `register_libs`, the `loaded` cache is populated
  for any registered library so that `require("os")` works without
  filesystem access.

## Choosing per tenant

The flag set can differ between tenants or per-script even within
the same host process, because each `GlobalEnv` carries its own
library registration.  Build one env per trust domain:

```rust
use shingetsu::{GlobalEnv, Libraries};

fn untrusted_env() -> anyhow::Result<GlobalEnv> {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::SANDBOXED)?;
    Ok(env)
}

fn trusted_env() -> anyhow::Result<GlobalEnv> {
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        Libraries::BUILTINS | Libraries::OS | Libraries::IO | Libraries::PACKAGE,
    )?;
    Ok(env)
}
```

A clone of either env is cheap and safe to share with many tasks.

## Exposing host capabilities

Sandboxing is not "register fewer flags."  It is also "expose only
the host functions that match the script's mandate."  If a script
needs to read its tenant's configuration but not files in general,
do not turn on `IO`; instead expose a `config.get(name)` host
function that reads from a value the host already trusts:

```rust
use shingetsu::{module, VmError};

#[module(name = "config")]
mod config_mod {
    use shingetsu::CallContext;

    /// Read a configuration value by key.
    #[function]
    fn get(_ctx: CallContext, name: String) -> Option<String> {
        // ... look it up from your trusted source ...
        let _ = name;
        None
    }
}
```

This is the path most embeddings end up taking.  Standard-library
flags handle the obvious cases; everything else is a host module.

## Hooks the embedder controls

Some host integrations bypass the standard library entirely:

- [`PrintCapture`](../api/shingetsu/struct.PrintCapture.html) — register one as an extension on the `GlobalEnv`
  and `print` writes lines to it instead of the process stdout.
  Useful for capturing script output into logs, test runners, or
  documentation builds without ever giving the script `STDIO`.
- Custom module loaders — `Libraries::PACKAGE` defaults to
  filesystem-based search, but you can replace the loader entirely.
  See [Custom module loaders](module-loaders.md).

## Resource limits

Sandboxing is about *what* a script can reach, not *how much*.
For "how much" — preventing a runaway script from chewing CPU or
memory — the embedder typically wraps `Task::await` in a timeout
(see [Async host calls](async.md)) and enforces memory limits at
the host process level.  The VM does not currently have built-in
instruction or allocation limits.

## A starting checklist

For a new embedding, walk through:

1. Pick a starting set: usually `SANDBOXED`, sometimes `ALL`
   minus a few flags, rarely `ALL`.
2. Decide whether `LOAD` is allowed.  If scripts will be reviewed
   ahead of time, leave it off; the type checker is far more
   useful that way.
3. Decide whether `PACKAGE` is allowed and, if so, what
   `package.path` should be.  The default points at `./?.lua` and
   `./?.luau`, which is almost never what you want in production.
4. List the host capabilities the script genuinely needs and
   expose them as your own modules.  Pass the *minimum* of what
   the script's job requires.
5. If your scripts need event hooks, use [events](events.md)
   rather than asking scripts to register globals — it is easier
   to audit and the type checker validates the handler signature.
