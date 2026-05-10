# Synchronization, Sharing, and Reload

Design spec for shingetsu's task-library synchronization primitives, the
shared-object registry, and the userdata lifetime machinery that supports them.

## Goals

- Async-aware coordination primitives usable from the `task` library.
- Stable identity for named primitives across host-driven configuration
  reloads (kumomta's primary use case).
- Cross-VM safety: there can be many VMs, each with many tasks; primitives
  shared by name must coordinate correctly across all of them.
- Easy to use, hard to misuse: classic condvar footguns must not be expressible.

## Non-goals (for now)

- Iterating the shared registry from Lua.
- Cross-process / cross-host coordination (use external systems for that).
- Arbitrary user-defined sync primitives in the standard library (the
  registry mechanism is open to embedders, but the stdlib ships a fixed set).
- `task.barrier`, `semaphore.acquire(n)` for n>1 — deferred.

## Concurrency model assumptions

- One host process, one shared-object registry.
- Many `Vm`s in the host, each running its own task scheduler.
- A single task is cooperatively scheduled within its VM; `await` points are
  the only places a task yields.
- Primitives obtained via the named registry may be touched from tasks
  running in different VMs (and thus different OS threads).

### Lock-type discipline

- **Lua-facing primitive state** (the actual mutex/rwlock/semaphore that
  Lua code holds across operations, including arbitrary `await` points
  inside Lua-callable natives) uses `tokio::sync::{Mutex, RwLock,
  Semaphore, Notify}`. Their guards are designed to span suspension; a
  Lua method may yield while logically holding the lock.
- **Internal short critical sections** in the host implementation (the
  guard wrapper's `Option<InnerHandle>` swap, the registry map, any
  helper bookkeeping where we definitely do not await while holding)
  use `shingetsu::sync::{Mutex, RwLock}`. Those guards are `!Send` by
  construction, so accidentally holding one across an `.await` fails to
  compile.
- **Never** use `parking_lot::Mutex` / `RwLock` directly anywhere in
  this subsystem. Either we are sure no `.await` happens (use
  `shingetsu::sync` and let the type system prove it) or we are not
  sure (use `tokio::sync`).

## The shared registry

A `GlobalEnv` extension. Pattern follows `GlobalEnv::extension_or_init`
with its own dedicated namespace.

- **Default: a process-global `LazyLock<SharedRegistry>`.** `GlobalEnv`
  captures a reference to it on construction, so an embedder that does
  nothing special still has a working registry. This means the sync
  surface is unconditionally installed by the task library and Lua
  scripts can always use named primitives without host setup.
- **Override:** an embedder may install a different `SharedRegistry`
  on a specific `GlobalEnv` via the `extension_or_init` slot. Used for:
  - Test isolation (each test fixture installs a fresh registry to
    avoid cross-test pollution from accumulated named entries).
  - Per-tenant isolation in multi-tenant hosts (each tenant's VMs share
    a tenant-scoped registry rather than the process global).
  - Custom backing stores (e.g. an embedder that wants to hook every
    registration for observability).
- Internally: a single map keyed by name, values are
  `Arc<dyn Any + Send + Sync>`. Constructors do `get_or_create::<T>(name, factory)`,
  downcasting on hit. Internal map locking uses
  `shingetsu::sync::Mutex` (no awaits while holding it).
- Type mismatch on a registered name is an error at construction time.
  The diagnostic names both the existing type and the requested type.
- The registry is opaque to Lua: no listing, no removal, no
  introspection. A host may clear or rebuild its registry between
  application lifecycles, but reload alone does not touch it.
- Long-lived names accumulate forever in the registry — this is the
  contract (survives reload). Tests creating many unique-named
  primitives should install their own registry to avoid bloat in the
  process-global; anonymous primitives (no name) do not touch the
  registry at all and are the recommended test default.
- Sandboxing scripts to prevent access to sync primitives is a separate
  concern (library selection / capability filtering at install time),
  not registry presence.

## Constructor convention

Every primitive follows the same shape:

- `task.foo()` — anonymous, never touches the registry.
- `task.foo(name)` — `get_or_create` against the registry. Same userdata
  type, same methods.
- Bind once at module scope: `local CACHE = task.mutex("config-cache")`.
- Acquire/use as needed: `local g = CACHE:lock()` (lifetime managed per
  the userdata-lifetime section below).

Naming uses a flat namespace under `task` (no `task.channel.bounded`-style
nesting) for docgen and discoverability.

## Userdata lifetime: prompt slot-clear, Rust `Drop` for cleanup

Goal: lock guards, channel permits, and similar resources release when
their owning local goes out of scope, without requiring the user to write
`<close>` and without a separate lint.

### Background: what the VM does today

- `Value::Userdata` is `Arc<dyn Userdata + Send + Sync>` — Drop fires
  when the last clone is released.
- Frame registers are a fixed-capacity `Box<[Value]>` allocated at
  `max_stack_size`. Slots are not cleared at scope exit for ordinary
  (non-`<close>`) locals; they are reused if the compiler picks the slot
  again, otherwise the Arc lives until function return.
- On function return the box is sent to a recycle pool. Pool entries
  retain their contents until reacquired (and then zeroed at acquire
  time), so live Arcs in pooled boxes have indeterminate drop latency.
- `<close>` locals are released explicitly via the `CloseVar` opcode,
  which the compiler emits at scope exits in reverse declaration order.

Net effect: a guard held in a `do/end` block today does not release at
end-of-block, and even at function return there can be pool-induced
latency before Drop fires. This is a real correctness issue, not just a
style concern.

### Approach

Lean on Rust `Drop` plus the existing Arc representation:

- Each guard's Rust type implements `Drop` to release its underlying
  resource (mutex, semaphore permit, etc.).
- We do *not* introduce `__close` metamethods for guards. We do *not*
  alter Lua-level `<close>` semantics (those remain available for
  user-written cleanup).
- We close the gap between scope exit and Drop by ensuring the in-frame
  Arc clone is released promptly:
  1. **Compiler:** at scope exit, emit slot-clear for each local going
     out of scope, in reverse declaration order. Peephole-elide when
     the slot is provably reused on the next instruction.
  2. **Runtime:** slot-clear is a trivial `frame.set(slot, Value::Nil)`.
     `write_reg` already skips Drop for primitives, so the cost for
     non-heap values is zero.
  3. **Function return / pool:** clear populated slots before returning
     the register box to the pool (rather than at acquire time). Net
     work is the same; the shift makes Drop fire at return.
- Open upvalues that point into a slot being cleared must be closed
  first; the existing `CloseUpvalues` machinery handles this and the
  compiler must pair the two correctly.

### Why this works for guards that escape

If a guard is returned, stored in a table, or captured by a closure,
those operations created additional Arc clones. Clearing the local's
slot drops only the in-frame clone; the others keep the lock held.
Drop fires when the last clone goes — exactly Rust semantics, no
convention required of the user.

### Explicit early release

Lua method dispatch cannot consume `self`. To support releasing a guard
before its scope ends, every releasable guard wraps its inner handle:

```rust
struct MutexGuard {
    inner: shingetsu::sync::Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>,
}
```

- The outer `shingetsu::sync::Mutex` is the brief swap lock; we never
  await while holding it, and the `!Send` guard makes that a compile
  error if we ever try.
- The inner handle (`tokio::sync::OwnedMutexGuard` or equivalent) is
  the actual resource. Holding it across an `.await` is fine and
  expected — that is the whole point of using the tokio primitive.
- `:unlock()` (mutex/rwlock guards) / `:release()` (semaphore permit)
  takes the inner handle, dropping it (which releases the resource).
- Calling the release method on an already-released guard is a hard
  error ("guard has already been released"), not a silent no-op.
- `Drop` on the wrapper performs the same `take()`-and-drop, so
  explicit release followed by scope-exit Drop is safe — the second
  release sees `None` and does nothing.
- Arc clones share the inner state. Releasing through any clone
  releases for all of them; subsequent calls through any clone error.
  This is the right behaviour: the resource is genuinely gone.

Cost: one tiny `shingetsu::sync::Mutex<Option<H>>` per guard. Negligible.

### What about Lua `<close>` and `__close`?

They remain available as Lua-level features for user-defined cleanup
(file handles, custom resources). They are simply not the mechanism
shingetsu's sync guards use. A user who wants `<close>` semantics on a
guard can still write `local g <close> = M:lock()` — it will work, but
it is redundant given the prompt-slot-clear behaviour above.

### Items to verify before locking in

- Micro-benchmark: cost of universal slot-clear at scope exit on a
  guard-heavy workload.
- Cost shift of clearing-on-return vs clearing-on-acquire (expected
  symmetric, possibly better cache behaviour at return).
- Compiler correctness for slot-clear paired with `CloseUpvalues` when
  a slot has been captured by a closure.
- Cancellation-during-acquire: a task awaiting `:lock()` that is
  cancelled has no guard yet — nothing to drop. The await future's own
  Drop must clean up the registration. Standard async hygiene; spec it
  per-primitive.

## Cancellation

All `wait*` operations integrate with the task scheduler's cancellation
mechanism. Cleanup of any registered waiter state on cancel is the
primitive's responsibility, not the user's. This is a guarantee, not an
incidental property.

## Snapshot discipline for shared values

Any value that crosses a VM boundary through a shared primitive (channel
payload, watch state, etc.) must be snapshottable using the existing
snapshot machinery (same as memoize). This is enforced at the send/set
site, with a clear diagnostic for non-snapshottable values.

## Primitive surface

### `task.mutex(name?)`

Async, cross-thread mutual exclusion.

- `:lock()` returns a guard userdata. Lock held until the last Arc
  clone of the guard is dropped (typically: end of acquiring scope).
- Guard `:unlock()` releases the lock early; double-release is an error.
- No `:wait`/`:notify` — use `task.notify` or `task.watch`.

### `task.rwlock(name?)`

- `:read()` returns a read guard.
- `:write()` returns a write guard.
- Both guards have `:unlock()` for explicit early release; double-release
  is an error.
- `:downgrade()` (write→read) and `:try_upgrade()` (read→write) are
  deferred unless a concrete use case appears.
- Fairness follows tokio's `RwLock` (write-preferring to avoid writer
  starvation under sustained read load). Document the exact behaviour
  alongside the API.

### `task.semaphore(permits, name?)`

- `:acquire()` returns a permit guard (single permit only for now).
- Permit `:release()` releases the permit early; double-release is an
  error.
- `permits` arg is required at construction; for named form, the first
  caller's `permits` value is authoritative; later callers passing a
  different value get a diagnostic.

### `task.notify(name?)`

Edge-triggered wake. Register-before-check ordering hidden inside the API
so the canonical lost-wakeup bug is not expressible.

- `:notify_one()` — wakes the longest-waiting waiter (FIFO).
- `:notify_last()` — wakes the most-recently-arrived waiter (LIFO).
- `:notify_all()` — wakes every current waiter.
- `:wait_until(predicate)` — register interest, check predicate, await if
  false, recheck on wake, loop. The predicate is the only correct way to
  use this API.
- `:wait_notified()` — lower-level "await any wake" escape hatch.

### `task.watch(initial, name?)`

State cell with change notification. The 80%-case replacement for the
classic mutex+condvar pairing.

- `initial` is either a snapshottable value or a zero-arg function
  returning a snapshottable value. The function form lets the caller
  defer expensive initialization to the actual first-create case (named
  lookups that hit an existing entry never invoke the function).
- For the named form, the function is invoked at most once across the
  process lifetime for a given name (whichever caller wins the
  get-or-create race).
- For the named form, even when an existing entry is returned, the passed
  `initial` is type-checked: a non-snapshottable value is a diagnostic.
  Practical implementation: if a value is passed, snapshot-validate it
  and discard; if a function is passed, no validation possible until call.
- `:get()` returns a snapshot of the current value.
- `:set(v)` publishes a new snapshottable value (atomic swap).
- `:wait_change()` returns the next value after the caller's last-seen
  version.
- `:wait_for(predicate)` returns the first value for which predicate is true.

Versioning guarantees: a waiter cannot miss a change that was published
between its previous `get`/`wait_*` and its next call.

#### Open: `Snapshottable` as a type-checker concept

Ideally the type checker could express `snapshottable | fn() ->
snapshottable` for the `initial` parameter (and equivalent constraints on
`set`, channel `send`, etc.). This is more of a trait predicate than a
concrete type, so feasibility within the existing type checker is open.
If representable, it would catch a large class of "oops, I tried to share
a non-snapshottable" bugs at compile time. Treat as an aspiration; the
first implementation enforces at runtime.

### `task.bounded_channel(capacity, name?)`

- `:send(v)` — backpressures (awaits) when full; `v` must be snapshottable.
- `:try_send(v)` — non-blocking; returns success/full.
- `:recv()` — awaits a value; returns nil on close.
- `:try_recv()` — non-blocking; returns value, empty, or closed.
- `:close()` — wakes all senders/receivers; subsequent sends error,
  receives drain then return nil.

### `task.unbounded_channel(name?)`

Same as bounded minus the backpressure on send. No capacity arg.

### `task.oneshot()`

Single-shot value handoff. **Anonymous-only for now** — a named oneshot
has awkward semantics because either end may be dropped on a different
VM, leaving the registry holding a half-consumed pair with no clean
recovery story. Revisit if a concrete use case appears.

- Constructor returns a `(sender, receiver)` pair.
- `sender:send(v)` — logically consumes the sender; subsequent `:send`
  is an error. `v` must be snapshottable.
- `sender:close()` — signal "no value coming" without sending; wakes
  the receiver with nil.
- `receiver:recv()` — awaits the value; returns nil if sender is
  dropped or closed without sending.

## Phasing and checklist

Each phase pauses for review on completion.

### Phase A: registry plumbing

- [x] `SharedRegistry` concrete type with `get_or_create::<T>` and
      type-mismatch diagnostic; internal locking via
      `shingetsu::sync::Mutex`
- [x] Process-global `LazyLock<SharedRegistry>` instance
- [x] `GlobalEnv` defaults its registry slot to the global instance;
      embedders can override via `install_shared_registry`
- [ ] Task-library installs sync surface unconditionally (no gating)
      — deferred to per-primitive phases (C–I)
- [x] Tests: get-or-create round-trip, type-mismatch diagnostic,
      override produces a fresh isolated registry, default-shared
      registry is observable across multiple `GlobalEnv` instances in
      the same process, factory runs at most once under contention,
      install-after-observation is rejected

### Phase B: prompt slot-clear at scope exit

Staged as two independent sub-PRs.

#### B-prep notes

- No new opcode required. `LoadNil { dst }` already does
  `frame.set(dst, Value::Nil)`, which drops the slot's prior value via
  `write_reg`. Slot clear at scope exit is just emitting `LoadNil` per
  local in reverse declaration order.
- Insertion point in the compiler: every site that calls
  `pop_scope_with_debug`. The popped `Locals` already carry their slot
  numbers. Order is `CloseUpvalues` (existing) then per-slot `LoadNil`s
  (new); the upvalue close copies the value into the cell before the
  slot is cleared, preserving captured values.
- Locals already covered by `<close>` (`CloseVar` opcode) must be
  skipped to avoid double-clear; `CloseVar` already nils the slot.
- `emit_close_for_exit(target_depth)` (used by `break` and `return`)
  needs the same treatment so jumps across scopes also clear the
  abandoned slots.
- shingetsu has no tail-call optimization, so frame reuse on return is
  not a concern.
- Debug info: `proto.locals[].end_pc` is set in `pop_scope_with_debug`
  *before* the new clears would be emitted, which is correct — the
  local's logical lifetime ends at the scope close, not at the
  physical clear instructions.
- Goto is currently stubbed; if re-enabled, the goto-crossing logic
  must integrate with the wider clear semantics.

#### Sub-phase B1: compiler scope-exit slot clear

- [x] Emit `LoadNil` for each non-`<close>` local at scope exit, in
      reverse declaration order, after any `CloseUpvalues`.  Emission
      is explicit at sites that benefit (do/end, if/elseif/else,
      repeat-until, generic-for control scope, numeric-for control
      scope) and skipped at loop-body sites where the pop sits before
      the back-jump (while body, numeric-for body, generic-for user
      vars).  At loop-body sites, the next iteration's writes drop the
      previous values via `write_reg`, so per-iteration `LoadNil`s
      would add cost with no observable promptness benefit.
- [x] Add `emit_clear_for_exit(target_depth)` for `break` / `continue`
      (return uses recycle-path; B2 handles it)
- [x] Drop-timing test: guard in `do/end` releases at end-of-block
- [x] Drop-timing test: guard captured by closure remains held by the
      closure across scope exit (slot clear does not invalidate the
      captured value)
- [x] Drop-timing test: for-body locals drop via reassignment (the
      contract for loop-body scopes; pinned down to prevent
      regression to per-iteration emission)
- [x] Drop-timing test: drop on `break`
- [x] Drop-timing test: returned value is not dropped at function exit
- [x] Benchmark: `int_loop` and `loop_body_locals` benches in
      `crates/shingetsu/benches/vm_benchmarks.rs`.  Refined B1 is
      within noise of baseline (~78 ms / ~216 ms); naive emission in
      every scope was +14% / +20% before the loop-body skip
- [ ] Defer peephole elision (skip `LoadNil` when next instruction
      writes the same slot) until after benchmarking justifies it

#### Sub-phase B2: runtime recycle-time slot clear

- [x] `recycle_registers` clears every slot to `Value::Nil` before
      pooling.  Decided against threading per-frame high-water-mark
      through the call sites: the simpler all-slots clear has
      negligible cost (slots beyond `reg_count` are already `Nil` so
      the writes are no-ops) and keeps the API unchanged.
- [x] `acquire_registers` skips the per-slot zero-fill; debug-asserts
      the all-Nil invariant.
- [x] Drop-timing test: top-level local of a function is dropped when
      the function returns (B2 contract; would have lingered in the
      pooled box without this change).
- [x] Native-frame path unaffected: `Native` frames hold no register
      box, so the recycle path is never reached for them.
- [x] Bench: `int_loop` and `loop_body_locals` within noise of
      baseline; `int_loop` shows a small improvement from no longer
      zeroing at acquire-time.

### Phase C: mutex

- [ ] `task.mutex` constructor (anon + named)
- [ ] Cross-thread implementation (`parking_lot` + scheduler waker)
- [ ] Guard wrapper: `parking_lot::Mutex<Option<InnerHandle>>` with
      `Drop` and `:unlock()` both performing `take()`-and-drop
- [ ] Double-release diagnostic
- [ ] Cancellation safety on `:lock()` await (registration cleanup on
      future drop)
- [ ] Tests: anon lock/unlock, explicit unlock, double-release error,
      named identity across VMs, contention, cancellation during
      lock-await

### Phase D: rwlock

- [ ] `task.rwlock` constructor
- [ ] Read/write guards with `:unlock()` (same wrapper pattern as mutex)
- [ ] Cancellation safety
- [ ] Tests parallel to Phase C

### Phase E: semaphore

- [ ] `task.semaphore` constructor with required `permits`
- [ ] Permit guard with `:release()` (same wrapper pattern)
- [ ] Diagnostic for permit-count disagreement on named form
- [ ] Tests

### Phase F: notify

- [ ] `task.notify` constructor
- [ ] `notify_one` / `notify_last` / `notify_all`
- [ ] `wait_until(predicate)` with register-before-check
- [ ] `wait_notified()` low-level form
- [ ] Cancellation safety
- [ ] Tests including the classic lost-wakeup scenario (must not be expressible)

### Phase G: watch

- [ ] `task.watch` constructor with snapshot validation on initial
- [ ] `get` / `set` / `wait_change` / `wait_for`
- [ ] Versioning so no waiter misses an interleaved update
- [ ] Snapshot validation on `set`
- [ ] Tests: cross-VM observe-after-reload scenario

### Phase H: channels

- [ ] `task.bounded_channel`
- [ ] `task.unbounded_channel`
- [ ] Snapshot validation on send
- [ ] Close semantics
- [ ] Tests

### Phase I: oneshot

- [ ] `task.oneshot` (anonymous only)
- [ ] Sender: `:send(v)` once; `:close()` for explicit no-value;
      double-send is an error
- [ ] Receiver returns nil on dropped or closed sender
- [ ] Tests

### Phase J: docs and examples

- [ ] User-facing docs page covering registry concept and each primitive
- [ ] Reload-survival example end-to-end
- [ ] Embedding-side guide for installing the registry
