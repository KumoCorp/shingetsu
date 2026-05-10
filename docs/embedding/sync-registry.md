---
title: Shared sync registry
---

# Shared sync registry

The `task` module ships async-aware synchronization primitives
(mutex, rwlock, semaphore, notify, watch, bounded/unbounded channel,
oneshot) plus a process-wide registry that lets named instances
of those primitives survive configuration reloads and coordinate
across many [`GlobalEnv`](../api/shingetsu/struct.GlobalEnv.html)
instances in the same host process.

This page covers the host-side view: when a host needs to install a
custom registry, what the lifetime guarantees are, and how to wire
named primitives into a reload-friendly configuration model.

## Default behaviour

By default a host does *not* need to install anything.  Every
`GlobalEnv` lazily refers to the same process-global
[`SharedRegistry`](../api/shingetsu/struct.SharedRegistry.html), so a
script that calls `task.mutex("cache")` in one VM and a script that
calls `task.mutex("cache")` in a *different* VM both receive the same
mutex.  Names persist for the lifetime of the host process.

If you only need cross-VM coordination — for example because your
host pools many `GlobalEnv` instances against a single config — the
default registry is what you want.

## When to install a custom registry

[`GlobalEnv::install_shared_registry`](../api/shingetsu/struct.GlobalEnv.html#method.install_shared_registry)
replaces the default with an isolated registry on a single env.  The
common reasons to do that:

- **Multi-tenant isolation.**  When a single host process serves
  several tenants, each tenant's VMs should share state with each
  other but not with another tenant's.  Build one
  [`SharedRegistry`](../api/shingetsu/struct.SharedRegistry.html) per
  tenant and install it on every `GlobalEnv` you create for that
  tenant.

- **Test isolation.**  Each integration test that uses named
  primitives should install its own fresh
  [`SharedRegistry`](../api/shingetsu/struct.SharedRegistry.html) so
  that names registered by one test do not leak into another running
  in parallel.

- **Resetting state at well-defined points.**  Dropping the only
  reference to a custom registry releases every primitive it held.
  This is rarely needed in production but useful in long-running
  test loops or development tools.

`install_shared_registry` must be called *before* the env's registry
is observed (e.g. before any script that uses a named primitive
runs).  After observation, calls return `false` and the install is
rejected.

```rust
use shingetsu::{GlobalEnv, SharedRegistry};
use std::sync::Arc;

let env = GlobalEnv::new();
let registry = Arc::new(SharedRegistry::new());
let installed = env.install_shared_registry(registry);
assert!(installed, "must install before any registry observation");

// register libraries, compile and run scripts as usual...
```

## Reload-friendly configuration

The registry is the mechanism that makes scripts reload-safe.  A
script that uses named primitives can be re-evaluated as many times
as needed during a reload; each call to a constructor returns the
*existing* primitive (created on the first run) rather than a fresh
one.  State held in those primitives — locks held, watches set,
queued channel values — survives the reload.

Reload semantics by primitive:

| Primitive               | First call              | Subsequent calls with the same name |
| ----------------------- | ----------------------- | ----------------------------------- |
| `task.mutex`            | Create                  | Return existing                     |
| `task.rwlock`           | Create                  | Return existing                     |
| `task.notify`           | Create                  | Return existing                     |
| `task.watch`            | Create with `initial`   | Return existing; `initial` ignored  |
| `task.semaphore`        | Create with `permits`   | Return existing; if requested permits is *higher* than configured, grow; if *lower*, log and keep existing |
| `task.bounded_channel`  | Create with `capacity`  | Return existing; if requested capacity differs, log and keep existing (tokio's mpsc capacity is fixed at construction) |
| `task.unbounded_channel`| Create                  | Return existing                     |

Type mismatch — for example calling `task.mutex("x")` after a
previous call registered `"x"` as an `rwlock` — is a hard error.
That is a programming bug in the calling code, not a tunable;
reload-loop reasoning does not apply because the calling code cannot
function with the wrong primitive type.

### Worked example: a reload-survival pattern

Imagine a host that pools 4 VMs and reloads the user's configuration
when a file changes.  The user wants a global rate limiter that
caps outbound work at 10 in flight, but they want to be able to
adjust the cap without restarting.

```lua
-- config.lua, evaluated once per VM on every reload

-- Returns the same Arc<Semaphore> on every reload, growing the
-- configured permits if the user bumps the cap.
local outbound_limit = task.semaphore(10, "outbound")

function on_request(req)
    local permit = outbound_limit:acquire()
    do_outbound_work(req)
    -- permit released when the function returns
end
```

Concurrent reload while requests are in flight:

1. The user changes `task.semaphore(10, ...)` to
   `task.semaphore(20, ...)` and triggers a reload.
2. Each pooled VM re-evaluates `config.lua`.  The first call into
   `task.semaphore(20, "outbound")` grows the existing semaphore by
   10 permits; subsequent calls in the other pooled VMs see the
   already-grown semaphore and do nothing.
3. In-flight `:acquire()` calls are unaffected.  Newly arriving
   requests can now acquire up to 20 permits.

If the user instead asks for `task.semaphore(5, "outbound")` (a
shrink), the existing 10-permit semaphore is preserved and a
warning is logged.  The script keeps running and serving requests;
the operator sees the warning in their log and can decide whether
to restart for the lower cap to take effect.

## Cross-VM value transport

Values that pass through `task.watch:set` or
`task.bounded_channel:send` are *snapshotted* before being stored,
so that consumers in any other `GlobalEnv` see a fresh deep copy
rather than aliasing the producer's tables.

Snapshotting is built on the
[`SnapshotValue`](../api/shingetsu/enum.SnapshotValue.html) type and
covers:

- All primitives plus strings (cheap).
- Tables recursively, so long as keys are integers or strings and
  the table contains no cycles.
- Userdata that opts in via the
  [`Userdata::snapshot`](../api/shingetsu/trait.Userdata.html#method.snapshot)
  trait method.

Functions and userdata that did not opt in are rejected with a
clear diagnostic at the send/set site.  This catches cross-VM
aliasing bugs before they cause action at a distance.

Hosts that want their own userdata types to be transportable across
VMs should override `snapshot()`; the closure it returns must be
able to rebuild an equivalent value in any
[`GlobalEnv`](../api/shingetsu/struct.GlobalEnv.html).

`task.oneshot()` is the exception: it is anonymous-only and stores
plain `Value`s by reference.  Because the sender/receiver pair
cannot escape its creating env, aliasing is intentional and safe.

## Warning routing

Reload-time warnings (semaphore shrink, channel capacity mismatch)
go through the [`log`](https://docs.rs/log) crate when the
`log` cargo feature is enabled on the `shingetsu` crate; otherwise
they fall back to `eprintln!`.  Hosts that already plumb `log`
should enable the feature so the messages flow through their
existing log infrastructure:

```toml
[dependencies]
shingetsu = { version = "...", features = ["log"] }
```

Each named entry tracks the most recent value passed to its
constructor and only warns when the requested value changes from
the last warned value, so a busy reload path does not flood the
log.
