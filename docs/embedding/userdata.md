---
title: Userdata
---

# Userdata

A *userdata* is a Rust value that scripts can hold but cannot
inspect or modify directly.  Use it for anything that has identity,
holds resources, or needs to enforce invariants — file handles,
database connections, mutable buffers, host-owned configuration
objects.

Compared with a [`LuaRepr`](../api/shingetsu/derive.LuaRepr.html)-derived struct:

| `LuaRepr` struct                      | [`Userdata`](../api/shingetsu/trait.Userdata.html)                        |
|----------------------------------------|-----------------------------------|
| Plain data, copied through a table     | Identity-bearing handle           |
| Fields readable from Lua               | Only what `#[lua_method]` exposes |
| Mutated by re-assignment from Lua      | Mutated by calling host methods   |
| `clone()` makes a separate value       | `clone()` is an `Arc` bump        |

## A complete example

Here is a counter that scripts can increment and read.  The whole
type — including its method dispatch, type info, and downcast
plumbing — comes out of one attribute macro
([`#[userdata]`](../api/shingetsu/attr.userdata.html)):

```rust
use shingetsu::{userdata, GlobalEnv, Value, VmError};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

struct Counter {
    value: AtomicI64,
}

#[userdata]
impl Counter {
    /// The current value of the counter.
    #[lua_field]
    fn value(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Add `amount` and return the new value.
    #[lua_method]
    fn increment(&self, amount: i64) -> i64 {
        self.value.fetch_add(amount, Ordering::Relaxed) + amount
    }

    /// Reset to zero.
    #[lua_method]
    fn reset(&self) {
        self.value.store(0, Ordering::Relaxed);
    }
}
```

To hand a `Counter` to a script, install it on the env:

```rust
fn install(env: &GlobalEnv) {
    let counter = Arc::new(Counter {
        value: AtomicI64::new(0),
    });
    env.set_global("counter", counter.into());
}
```

`Arc<T>` where `T: Userdata` converts directly to a `Value`.
`Value::userdata(arc)` is the explicit form, useful when the
surrounding context can't infer the target type.

Script-side, `counter` looks like an object with a property and two
methods:

```lua
counter:increment(5)
counter:increment(3)
print(counter.value)        -- 8
counter:reset()
print(counter.value)        -- 0
```

Note the `:` in `counter:increment(5)` — colon-call syntax passes
the receiver as the implicit first argument.  Calling
`counter.increment(5)` (with a dot) is an error: the macro-generated
`#[lua_method]` always expects the receiver as the first Lua
argument.

## Field accessors and properties

A Rust function on a userdata can show up Lua-side as either a
*property* (read like `obj.name`) or a *method* (called like
`obj:name()`).  The annotation chooses which:

- `#[lua_field]` on `fn name(&self) -> T` — read-only property.
  No call syntax needed Lua-side.
- `#[lua_field(setter)]` on `fn set_name(&self, v: T)` — paired
  setter; combined with a getter of the same Lua name it makes the
  field read-write.
- `#[lua_method]` on `fn name(&self, ...) -> T` — a callable.
  Even when it takes no arguments beyond `&self`, callers must
  invoke it with `()`.

The distinction matters because a zero-argument `#[lua_method]`
is *not* the same as a `#[lua_field]`:

```rust
use shingetsu::userdata;
use std::sync::atomic::{AtomicI64, Ordering};

struct Sensor {
    value: AtomicI64,
}

#[userdata]
impl Sensor {
    /// Read as `sensor.reading`.
    #[lua_field]
    fn reading(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Call as `sensor:sample()`.  Same body, different syntax.
    #[lua_method]
    fn sample(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }
}
```

Lua side:

```lua
print(sensor.reading)    -- 42
print(sensor:sample())   -- 42
print(sensor.sample)     -- function: ...    (the bound method itself)
print(sensor:reading())  -- error: attempt to call a number value
```

Adding a setter turns the property read-write:

```rust
#[userdata]
impl Sensor {
    #[lua_field]
    fn reading(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }

    #[lua_field(setter)]
    fn set_reading(&self, v: i64) {
        self.value.store(v, Ordering::Relaxed);
    }
}
```

```lua
sensor.reading = 100
print(sensor.reading)    -- 100
```

Rule of thumb: pick `#[lua_field]` when the name reads like a
noun and the access is essentially storage; pick `#[lua_method]`
when the name reads like a verb or the call performs work.

## Methods and `&self` / `&mut self`

Userdata methods take `&self`, never `&mut self`.  A single
userdata can be referenced from multiple script tasks running
concurrently against the same [`GlobalEnv`](../api/shingetsu/struct.GlobalEnv.html), so the dispatcher only
hands out shared references.  Mutation goes through interior
mutability — `Mutex`, `RwLock`, `AtomicI64`, `Cell`.

For a single value of a primitive type, an atomic is the cheapest
choice (this is what the `Counter` example above uses).  For
anything more structured, a `Mutex` or `RwLock` around the whole
state is usually clearest — use the ones in `shingetsu::sync`
rather than `parking_lot` directly so the compiler catches
holding a guard across an `.await`:

```rust
use shingetsu::sync::RwLock;
use shingetsu::userdata;
use std::collections::HashMap;

struct Cache {
    entries: RwLock<HashMap<String, String>>,
}

#[userdata]
impl Cache {
    /// Read a value, returning `nil` if the key is absent.
    #[lua_method]
    fn get(&self, key: String) -> Option<String> {
        self.entries.read().get(&key).cloned()
    }

    /// Insert or replace a value.
    #[lua_method]
    fn set(&self, key: String, value: String) {
        self.entries.write().insert(key, value);
    }

    /// Number of entries currently in the cache.
    #[lua_field]
    fn size(&self) -> i64 {
        self.entries.read().len() as i64
    }
}
```

Lua side:

```lua
cache:set("name", "shingetsu")
print(cache:get("name"))   -- shingetsu
print(cache.size)          -- 1
```

The `RwLock` is held only for the duration of each method call,
so concurrent script tasks can read the cache in parallel.
Guards from `shingetsu::sync` are `!Send`, so if a future
version of these methods became `async` and you accidentally
held a guard across an `.await`, the compiler would refuse the
coercion to shingetsu's `Send`-bounded native-call slot — see
[Async host calls](async.md#locks-held-across-an-await).

For owned-receiver methods (`self`), take [`Ud<Self>`](../api/shingetsu/struct.Ud.html) instead
of `&self` when the operation should consume the handle.  This is
unusual; most host-bound types are designed to be shared.

## Async methods

`#[lua_method]` also works on `async fn`:

```rust
use shingetsu::userdata;
use std::time::Duration;

struct Db;

#[userdata]
impl Db {
    /// Look up a row by primary key.
    #[lua_method]
    async fn get(&self, id: i64) -> Option<String> {
        // Stand-in for a real async client call.
        tokio::time::sleep(Duration::from_millis(50)).await;
        Some(format!("row {id}"))
    }
}
```

Calling `db:get(7)` from a script suspends the task at the
`await` point, lets the executor run other tasks for those 50
ms, and resumes with the result when the sleep completes.  The
script sees a normal blocking-looking call — there is no `await`
keyword on the Lua side:

```lua
local row = db:get(7)
print(row)   -- row 7
```

See [Async host calls](async.md) for what suspension actually
does and the corner cases worth knowing about.

## Metamethods

In Lua, a *metamethod* is the host-side hook that runs when a
built-in operator or operation is applied to a value.  When you
write `a + b` in a script, the VM first looks for an `__add`
metamethod on either operand; if one is found, it calls that
instead of failing with a type error.  The same mechanism backs
`tostring`, `#x` (length), `<`, `==`, `[]` indexing, and several
others.

For a userdata, metamethods are how you make Lua's syntax work
naturally with your Rust type — a `Money` value that supports
`m1 + m2`, prints sensibly under `tostring`, or compares with `<`.
Without metamethods, scripts can only interact with the userdata
through the methods you expose; with them, the userdata fits
into Lua's existing operator vocabulary.

`#[lua_metamethod]` annotates a method as the implementation of
a specific metamethod:

```rust
use shingetsu::{userdata, BinOpSide, MetaMethod};

struct Money(i64);

#[userdata]
impl Money {
    /// `tostring(money)`
    #[lua_metamethod(ToString)]
    fn to_string(&self) -> String {
        format!("${}", self.0)
    }

    /// `money + n` and `n + money` (commutative).
    #[lua_metamethod(Add)]
    fn add(&self, rhs: i64) -> i64 {
        self.0 + rhs
    }

    /// `money - n` and `n - money` (non-commutative).
    #[lua_metamethod(Sub)]
    fn sub(&self, other: BinOpSide<i64>) -> i64 {
        other.impl_sub(self.0)
    }

    /// `money < n` and `n < money`.
    #[lua_metamethod(Lt)]
    fn lt(&self, other: BinOpSide<i64>) -> bool {
        other.impl_lt(self.0)
    }
}
```

`BinOpSide<T>` carries the other operand together with which side
of the operator it sat on.  For commutative operations (`Add`,
`Mul`, `BitAnd`, `BitOr`, `BitXor`) order does not matter and a
plain `T` parameter is fine; for everything else, take a
`BinOpSide<T>` and use one of its `impl_sub` / `impl_lt` /
`impl_div` / ... helpers, which dispatch to the correct
Rust operation with the operands in the right order.

The [`MetaMethod`](../api/shingetsu/enum.MetaMethod.html) enum lists every metamethod the VM dispatches.
The `#[lua_metamethod(Name)]` argument is one of its variants.

Two of those variants — `Close` and `Gc` — are about *cleanup*
rather than operator overloading, and they behave differently from
reference Lua.  The next section covers them.

## Lifecycle and cleanup

A userdata in shingetsu is held behind an `Arc<T>`.  When the last
reference to it goes away — from a Lua local, an upvalue, the
registry, a host-side handle, or anywhere else — the `Arc`
refcount hits zero and Rust's `Drop` for `T` runs synchronously,
right then.  This is the primary cleanup mechanism for userdata.
Write a `Drop` impl on your type and you have deterministic
release for whatever it owns:

```rust
use shingetsu::sync::Mutex;

struct Connection {
    socket: Mutex<Option<std::net::TcpStream>>,
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(sock) = self.socket.lock().take() {
            // Best-effort shutdown; we're synchronous here.
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
    }
}
```

That covers most needs.  Two metamethods extend it for cases
where `Drop` is the wrong tool.

### `__close` for scoped, async-capable cleanup

`Drop` has two limits: it is synchronous (you cannot `.await` from
it), and it runs only when the last reference goes — which, in a
long-running task with deeply nested scopes or long-lived loops,
can be substantially later than you would like.

The `__close` metamethod addresses both.  Shingetsu lets a local
be marked `<close>`:

```lua
local conn <close> = pool:acquire()
conn:send("hello")
-- when this scope exits (normally, via return, break, or error),
-- conn's __close metamethod runs immediately.
```

On the host side, that metamethod is just an `async fn`:

```rust
use shingetsu::{userdata, Variadic, VmError};

#[userdata]
impl Connection {
    /// Release the connection back to the pool.  Async, runs at
    /// scope exit, errors are surfaced.
    #[lua_metamethod(Close)]
    async fn lua_close(&self) -> Result<Variadic, VmError> {
        self.graceful_shutdown().await?;
        Ok(Variadic::default())
    }
}
```

Two benefits over `Drop`:

1. **Async cleanup.**  `__close` is dispatched through the VM's
   async machinery, so it can `.await` — send a goodbye frame,
   flush a buffer, return the connection to a pool, commit a
   transaction.  Synchronous `Drop` cannot do any of that.
2. **Deterministic, scope-bounded release.**  The metamethod
   fires when the lexical scope ends, including on `break`,
   `return`, and error unwind.  In a script with deeply nested
   control flow or a tight loop that holds a resource for one
   iteration, this can be far sooner than the surrounding
   `Task`'s drop point would allow.

If both `__close` and `Drop` are defined, `__close` runs first
(at scope exit, during normal execution) and `Drop` runs later
(when the last `Arc` goes).  Make sure your `Drop` is idempotent
so a double-cleanup is safe — the file userdata, for example,
takes its inner state in both paths and is a no-op if it has
already been released.

!!! warning "`Task` drop vs `Task::dispose()`"

    `__close` is only guaranteed to run when its scope exits
    *during execution*.  If the host abandons a running task by
    simply dropping it (a tokio cancel, a future being aborted),
    any `<close>` locals that were still in scope will not have
    their `__close` invoked — only `Drop` will run as the values
    fall out of memory.

    To make sure `__close` runs on still-open locals during
    abandonment, call [`Task::dispose().await`](../api/shingetsu/struct.Task.html#method.dispose) instead of
    dropping.  `dispose` walks the open frames, dispatches each
    `__close`, and then resolves.  Use `Drop` for cleanup that
    must always happen; reach for `__close` when you want async
    capability or scope-bounded release during normal flow.

### `__gc` is *not* auto-dispatched

This is where shingetsu deliberately differs from reference Lua.
The `MetaMethod::Gc` slot exists so the metamethod model is
complete, but the VM **does not run `__gc` on a userdata** when
its last reference is released.  There is no tracing collector
for userdata: cleanup runs through `Drop`, full stop.

!!! note "Difference from reference Lua"

    In reference Lua, a userdata's `__gc` metamethod fires when
    the collector reclaims the value.  In shingetsu, it does
    not.  Implement cleanup in `Drop` (synchronous) or
    `#[lua_metamethod(Close)]` (async, scope-bounded) instead.
    Implementing `#[lua_metamethod(Gc)]` is harmless but the
    method body will not run unless the host invokes it
    explicitly.

For tables, `__gc` *is* auto-dispatched by `GlobalEnv::collect_cycles()`
and drained during `GlobalEnv::dispose()` — but tables are not
userdata, and that path is not the one you reach for when
building host-bound types.

## When userdata is not the right tool

- "Plain data, no methods" — use `LuaRepr`-derived struct.
- "I want a callable, not a value" — [`Function::wrap`](../api/shingetsu/struct.Function.html#method.wrap).
- "I need a singleton with functions and constants" — that is a
  *module*; see [Modules and functions](modules.md).
