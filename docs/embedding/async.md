---
title: Async host calls
---

# Async host calls and suspension

A central goal of Shingetsu is that a script can call into an async
Rust function, the script's task suspends while the host does
work, and execution resumes with the result when the future is
ready — all without the script having to do anything special.

This page covers how that works in practice and the corner cases
worth knowing about.

## The basic shape

Any [`Function::wrap`](../api/shingetsu/struct.Function.html#method.wrap) closure can be `async`:

```rust
use shingetsu::{Bytes, Function, VmError};

let sleep = Function::wrap(
    "sleep",
    |secs: f64| async move {
        tokio::time::sleep(std::time::Duration::from_secs_f64(secs)).await;
        Ok::<(), VmError>(())
    },
);
```

The same is true of `#[function]` inside a `#[module]` block, of
`#[lua_method]` on a `#[userdata]` impl, and of an event handler
return path: anywhere a Rust callable is exposed to Lua, it can be
async.

Script-side, calling it looks identical to calling a synchronous
function:

```lua
sleep(0.5)
print("done")
```

There is no `await` keyword, no callback, and no coroutine
threading.  The script just calls the function; the underlying
`Task` parks on the future, and when the future resolves, the VM
resumes at the next instruction.

## What suspension actually does

A [`Task`](../api/shingetsu/struct.Task.html) is a `Future`.  When a script calls an async host
function, the VM stores the future in the `Task` and returns
`Poll::Pending` from `Task::poll`.  The host's executor (typically
tokio) wakes the task when the inner future is ready, the VM
extracts its return values, and execution continues.

A consequence: while a task is suspended on a host future, no Lua
opcodes run.  Other tasks against the same `GlobalEnv` keep
making progress — they have their own `Task` futures, polled
independently.

## Spawning many tasks

[`GlobalEnv::clone()`](../api/shingetsu/struct.GlobalEnv.html) is cheap (an `Arc` bump).  A typical pattern
is one env per process and many concurrent tasks:

```rust
use shingetsu::{GlobalEnv, Task, valuevec};

async fn fanout(env: GlobalEnv, scripts: Vec<shingetsu::Function>) {
    let mut handles = Vec::new();
    for func in scripts {
        let env = env.clone();
        handles.push(tokio::spawn(async move {
            let task = Task::new(env, func, valuevec![]);
            task.await
        }));
    }
    for h in handles {
        let _ = h.await;
    }
}
```

The env's globals, modules, type map, and registered libraries are
shared; each task has its own call stack, locals, and pending
state.

## Cancellation

Dropping a `Task` cancels it without running `<close>` finalisers
on whatever locals it was holding.  When you actually want the
finalisers to run — to close file handles, return a connection
to a pool, release any other host resource — call
[`Task::dispose().await`](../api/shingetsu/struct.Task.html#method.dispose) instead.  `dispose` walks the still-open
frames, runs each `__close` handler, and then resolves.

`Task` is a `Future` and is `Unpin`, so you can poll it through a
`&mut` reference and recover ownership afterwards.  That's what
lets you combine it with `tokio::time::timeout` and dispose on
the timeout branch:

```rust
use std::time::Duration;
use shingetsu::{RuntimeError, Task, ValueVec};

/// Run `task` to completion, or dispose it after `dur`.
async fn run_with_timeout(
    mut task: Task,
    dur: Duration,
) -> Option<Result<ValueVec, RuntimeError>> {
    match tokio::time::timeout(dur, &mut task).await {
        Ok(result) => Some(result),
        Err(_elapsed) => {
            // Timed out; run __close handlers on still-open
            // <close> locals before discarding the task.
            task.dispose().await;
            None
        }
    }
}
```

`tokio::time::timeout` borrows the task for the duration of its
own future; once the timeout future resolves — success or elapse
— the borrow is released and `task` is owned again, free to be
`dispose`d.

## Async and userdata methods

A common pattern: a userdata representing a remote resource, with
async methods that talk to it:

```rust
use shingetsu::{userdata, Bytes};

#[userdata]
impl HttpClient {
    /// Fetch the body of a URL.
    #[lua_method]
    async fn get(&self, url: String) -> Bytes {
        self.get_inner(&url).await
    }
}
```

Scripts call `client:get("https://example.com/")` synchronously;
under the hood the task suspends on the HTTP future and resumes
with the response.

If the host method needs to return a `Result<T, VmError>`, the
shape is the same:

```rust
use shingetsu::{userdata, VmError};

#[userdata]
impl Db {
    #[lua_method]
    async fn get(&self, id: i64) -> Result<Option<String>, VmError> {
        // ... await some db query, map errors into VmError ...
        Ok(None)
    }
}
```

## Restrictions while a task is suspended

Lua state — locals, the call stack, upvalues, pending results —
crosses a suspension boundary without ceremony; the VM stores
what it needs to resume.  Two things on the *Rust* side are
worth being deliberate about, though.

### Locks held across an `await`

Holding a synchronous lock across `.await` keeps the lock for as
long as the future takes to resolve, blocking every other task
waiting for it and pinning the executor thread.  Bare
`parking_lot::MutexGuard` is `Send`, so this mistake compiles
cleanly with parking_lot's primitives and only surfaces under
load — the compiler does not catch it for you.

Shingetsu ships a drop-in replacement that does:
`shingetsu::sync::Mutex` and `shingetsu::sync::RwLock`.  These
wrap parking_lot underneath (same fast path, same no-poison
behaviour) but their guards are deliberately `!Send`.  Because
every async native registered with shingetsu is stored as a
`BoxFuture<'static, ... + Send>`, holding a `shingetsu::sync`
guard across an `.await` makes the future `!Send` and the
coercion fails at compile time, pointing right at the
offending site.

Use `shingetsu::sync` for any state that might be touched from
an `async fn` exposed to Lua:

```rust
use shingetsu::sync::RwLock;
use std::collections::HashMap;

struct Cache {
    entries: RwLock<HashMap<String, String>>,
}
```

For state that only needs to be touched during synchronous
work, release the guard before awaiting either way:

```rust
let cached = {
    let guard = self.entries.read();
    guard.get(&key).cloned()
}; // guard dropped here, before any .await

match cached {
    Some(v) => Some(v),
    None => self.fetch(&key).await,
}
```

For state that genuinely needs to stay locked across an
`.await`, neither sync lock fits — reach for
`tokio::sync::Mutex` (or `RwLock`) instead.  Its guards
cooperate with the runtime: while one task holds the guard,
others waiting for it can park without monopolising a worker
thread.

### Reaching back into the calling Lua frame

When the host function takes [`CallContext`](../api/shingetsu/struct.CallContext.html) as a parameter, the
context is moved into the future and travels through suspension
with you — it is cheap to clone (`Arc` internally), so the
dispatcher hands you an owned copy.  From inside the future you
can freely call `ctx.call_function(...)`, walk
`ctx.call_stack()`, attach hints to errors, and so on.

What you cannot do is reach into the *calling Lua frame's
locals* mid-suspension — those are part of the VM state, not
part of `CallContext`, and the host code does not have a handle
to them.  If a host function needs a value from the calling
scope, take it as an explicit parameter instead of trying to
fish it out from inside the future.

## Choosing async vs sync

A function should be `async fn` exactly when it would otherwise
block — I/O, network, sleeping, channel waits.  Synchronous
functions are slightly cheaper and simpler.  Mixing freely in the
same module is fine.
