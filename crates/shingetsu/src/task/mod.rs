//! Concurrent task library.
//!
//! Spawns Lua functions as independent tasks driven by `tokio::spawn`,
//! exposing a `Task` userdata to Lua for awaiting, cancelling, and
//! introspecting them.  Lifecycle events are surfaced through a
//! [`crate::task::TaskObserver`] trait that hosts can implement to
//! model parent/child task graphs, log per-task summaries, count
//! live tasks for graceful shutdown, etc.  The library itself ships
//! no built-in observer.
//!
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

use crate::error::RuntimeError;
use crate::sync::{Mutex, RwLock};
use crate::{Bytes, CallContext, Function, GlobalEnv, Ud, Value, ValueVec, Variadic, VmError};

mod channel;
mod lua_task;
mod lua_task_set;
mod mutex;
mod notify;
mod oneshot;
mod runtime_error;
mod rwlock;
mod semaphore;
mod sync_common;
mod watch;

pub use channel::{LuaBoundedChannel, LuaUnboundedChannel};
pub use lua_task::LuaTask;
pub use lua_task_set::LuaTaskSet;
pub use mutex::{LuaMutex, LuaMutexGuard};
pub use notify::LuaNotify;
pub use oneshot::{LuaOneshotReceiver, LuaOneshotSender};
pub use runtime_error::{LocationResult, LuaRuntimeError};
pub use rwlock::{LuaRwLock, LuaRwLockReadGuard, LuaRwLockWriteGuard};
pub use semaphore::{LuaSemaphore, LuaSemaphorePermit};
pub use watch::LuaWatch;

use lua_task::SelectResult;
use sync_common::{shared_lookup, warn};
use watch::compute_initial;

tokio::task_local! {
    /// `TaskInfo` of the task currently executing under `task.spawn`.
    /// Set by the spawn wrapper future for every task it runs;
    /// absent on top-level threads and on threads that originated
    /// outside the task module.  Read by `spawn` to populate
    /// [`TaskInfo::parent`].
    static CURRENT_TASK: Arc<TaskInfo>;
}

// ---------------------------------------------------------------------------
// TaskId / TaskInfo / TaskOutcome
// ---------------------------------------------------------------------------

/// Monotonic per-`GlobalEnv` task identifier.  Allocated from the
/// per-env `TaskRegistry` at spawn time; stable for the lifetime of
/// the task and exposed to Lua via `Task:id()`.
//
// `TaskRegistry` is a private extension stored on the `GlobalEnv`
// via `extension_or_init`; it isn't part of the public surface.
pub type TaskId = u64;

/// Static metadata captured at spawn time.
///
/// `Arc<TaskInfo>` is shared between the spawn wrapper future, the
/// `Task` userdata, observer callbacks, and any child task's
/// `parent` field, so a parent's info remains addressable from a
/// child even after the parent task itself has finished.
#[derive(Debug)]
pub struct TaskInfo {
    pub id: TaskId,
    pub name: Option<Bytes>,
    /// Wall-clock time at which the task was spawned.
    pub spawned_at: Instant,
    /// Rendered traceback of the spawn site, captured before the
    /// child task begins executing.
    pub spawn_site: String,
    /// The task that called `task.spawn`, if any.  `None` for
    /// top-level spawns and for spawns originating outside any
    /// `task.spawn`-managed task.  "Parent" here means the
    /// spawning task, not lexical/closure relationships.
    pub parent: Option<Arc<TaskInfo>>,
}

/// How a task finished, passed to [`TaskObserver::on_complete`].
pub enum TaskOutcome<'a> {
    /// Function returned successfully.
    Success {
        results: &'a ValueVec,
        elapsed: Duration,
    },
    /// Function raised an error.
    Failure {
        error: &'a RuntimeError,
        elapsed: Duration,
    },
    /// Task was cancelled gracefully via `Task:cancel()` (runs
    /// `<close>` / `__close` handlers).
    Cancelled { elapsed: Duration },
    /// Task was aborted hard via `Task:abort()` (the underlying
    /// `tokio::JoinHandle::abort`; no `<close>` handlers run).
    Aborted { elapsed: Duration },
}

// ---------------------------------------------------------------------------
// TaskObserver trait
// ---------------------------------------------------------------------------

/// Hook surface for monitoring task lifecycles.  All methods are
/// passed a `&GlobalEnv` so an observer registered against multiple
/// environments can route events accordingly.
///
/// `on_complete` and `on_handle_abandoned` are independent signals.
/// Both can fire for the same task — the most interesting case is a
/// task that completed (success or failure) but whose result was
/// never collected by Lua, indicating the host code dropped the
/// `Task` handle without checking.
#[allow(unused_variables)]
pub trait TaskObserver: Send + Sync + 'static {
    /// Called once just after the task is spawned, before it begins
    /// executing.
    fn on_spawn(&self, env: &GlobalEnv, info: &TaskInfo) {}

    /// Called once when the task's execution finishes, regardless
    /// of outcome.
    fn on_complete(&self, env: &GlobalEnv, info: &TaskInfo, outcome: &TaskOutcome<'_>) {}

    /// Called once if the `Task` userdata handle was dropped without
    /// the result being consumed (no `await`/`pawait`/`join`/
    /// `await_all`/successful `try_result`) and without explicit
    /// `cancel`/`abort`.
    fn on_handle_abandoned(&self, env: &GlobalEnv, info: &TaskInfo) {}
}

// ---------------------------------------------------------------------------
// Per-GlobalEnv registry (extension storage)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) struct TaskRegistry {
    next_id: AtomicU64,
    observers: RwLock<Vec<Arc<dyn TaskObserver>>>,
}

#[allow(dead_code)]
impl TaskRegistry {
    fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            observers: RwLock::new(Vec::new()),
        }
    }

    pub(crate) fn alloc_id(&self) -> TaskId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn snapshot_observers(&self) -> Vec<Arc<dyn TaskObserver>> {
        self.observers.read().clone()
    }
}

pub(crate) fn registry(env: &GlobalEnv) -> Arc<TaskRegistry> {
    env.extension_or_init(TaskRegistry::new)
}

/// Register a [`TaskObserver`] against `env`.  Observers are stored
/// per-`GlobalEnv` and survive re-registration of the `task` module.
/// Multiple observers may be installed; they are notified in
/// registration order.
pub fn add_observer(env: &GlobalEnv, obs: Arc<dyn TaskObserver>) {
    registry(env).observers.write().push(obs);
}

/// Remove a previously registered observer (matched by `Arc::ptr_eq`).
/// Returns `true` if a matching observer was found and removed.
pub fn remove_observer(env: &GlobalEnv, obs: &Arc<dyn TaskObserver>) -> bool {
    let reg = registry(env);
    let mut guard = reg.observers.write();
    let before = guard.len();
    guard.retain(|o| !Arc::ptr_eq(o, obs));
    guard.len() != before
}

/// Remove all observers registered against `env`.
pub fn clear_observers(env: &GlobalEnv) {
    registry(env).observers.write().clear();
}

// ---------------------------------------------------------------------------
// LuaTask userdata + supporting state
// ---------------------------------------------------------------------------

/// Internal record of a finished task.  Wrapped in an `Arc` so
/// multiple awaiters can read the same outcome without cloning
/// the underlying values.
pub(crate) enum TaskResult {
    Success(ValueVec),
    Failure(Arc<RuntimeError>),
    Cancelled,
    Aborted,
}

impl TaskResult {
    fn as_outcome(&self, elapsed: Duration) -> TaskOutcome<'_> {
        match self {
            TaskResult::Success(values) => TaskOutcome::Success {
                results: values,
                elapsed,
            },
            TaskResult::Failure(err) => TaskOutcome::Failure {
                error: err,
                elapsed,
            },
            TaskResult::Cancelled => TaskOutcome::Cancelled { elapsed },
            TaskResult::Aborted => TaskOutcome::Aborted { elapsed },
        }
    }
}

/// State shared between the `LuaTask` userdata and the spawned
/// wrapper future.
pub(crate) struct TaskState {
    pub(crate) env: GlobalEnv,
    pub(crate) info: Arc<TaskInfo>,
    /// Notified by `Task:cancel()` to request graceful cancellation.
    pub(crate) cancel: Notify,
    /// Notified by the wrapper future (or the abort drop guard)
    /// once `result` has been written.
    completed: Notify,
    pub(crate) result: Mutex<Option<Arc<TaskResult>>>,
    /// Set by methods that collect a result or explicitly
    /// cancel/abort, suppressing the `on_handle_abandoned`
    /// observer firing in `LuaTask::drop`.
    pub(crate) consumed: AtomicBool,
}

impl TaskState {
    /// Publish the task's final outcome.  No-op if a result was
    /// already published (e.g. `AbortGuard::drop` racing with a
    /// completed wrapper future).
    fn publish(&self, outcome: Arc<TaskResult>) {
        let mut guard = self.result.lock();
        if guard.is_some() {
            return;
        }
        *guard = Some(outcome);
        drop(guard);
        self.completed.notify_waiters();
    }

    /// Block until the task publishes a result, then clone the
    /// shared `Arc<TaskResult>` out for the caller.
    pub(crate) async fn wait(&self) -> Arc<TaskResult> {
        loop {
            // Register the waiter before checking the slot to avoid
            // missing a notification published in between.
            let waiter = self.completed.notified();
            if let Some(r) = self.result.lock().clone() {
                return r;
            }
            waiter.await;
        }
    }
}

/// RAII guard that surfaces hard-abort outcomes.  The wrapper
/// future installs one at the top of its body; on normal exit it
/// is disarmed via [`AbortGuard::disarm`].  If the future is
/// dropped before reaching `disarm` (i.e. `JoinHandle::abort` was
/// called), `Drop` publishes `TaskResult::Aborted` and fires the
/// `on_complete` observer with [`TaskOutcome::Aborted`].
struct AbortGuard {
    state: Option<Arc<TaskState>>,
    started: Instant,
}

impl AbortGuard {
    fn disarm(&mut self) {
        self.state = None;
    }
}

impl Drop for AbortGuard {
    fn drop(&mut self) {
        let Some(state) = self.state.take() else {
            return;
        };
        let elapsed = self.started.elapsed();
        // Hold the result lock across the check-and-write so we
        // can't race a concurrent publish.  In practice the
        // wrapper future is the only other publisher and it can't
        // run concurrently with this drop (the future is being
        // dropped right now), but taking the lock for the whole
        // decision keeps the invariant explicit and survives
        // future refactors that introduce additional publishers.
        let outcome_to_fire = {
            let mut guard = state.result.lock();
            if guard.is_some() {
                // The wrapper future already published its own
                // outcome before being dropped; nothing for us to
                // do beyond waking any late waiter.
                None
            } else {
                let outcome = Arc::new(TaskResult::Aborted);
                *guard = Some(outcome.clone());
                Some(outcome)
            }
        };
        // Wake waiters outside the critical section.
        state.completed.notify_waiters();
        if let Some(outcome) = outcome_to_fire {
            let view = outcome.as_outcome(elapsed);
            for obs in registry(&state.env).snapshot_observers() {
                obs.on_complete(&state.env, &state.info, &view);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// task module: Lua-visible functions
// ---------------------------------------------------------------------------

/// Drive the inner shingetsu-vm `Task` to completion, intercepting
/// the cancel signal so that `__close` handlers run on graceful
/// cancel.  Publishes the outcome and fires `on_complete`.
async fn run_inner(env: GlobalEnv, state: Arc<TaskState>, func: Function, fn_args: ValueVec) {
    let started = state.info.spawned_at;
    let mut task = Box::pin(crate::Task::new(env.clone(), func, fn_args));
    let mut cancelling = false;

    let raw = loop {
        tokio::select! {
            biased;
            _ = state.cancel.notified(), if !cancelling => {
                task.as_mut().begin_dispose();
                cancelling = true;
            }
            r = task.as_mut() => break r,
        }
    };

    let elapsed = started.elapsed();
    let outcome = Arc::new(if cancelling {
        TaskResult::Cancelled
    } else {
        match raw {
            Ok(values) => TaskResult::Success(values),
            Err(re) => TaskResult::Failure(Arc::new(re)),
        }
    });

    let view = outcome.as_outcome(elapsed);
    for obs in registry(&env).snapshot_observers() {
        obs.on_complete(&env, &state.info, &view);
    }
    state.publish(outcome);
}

/// Argument shape for `task.spawn`, dispatched on whether the
/// first argument is a string or a function.  Trailing args are
/// forwarded to the task body verbatim.
///
/// `#[derive(FromLuaMulti)]` matches the longest-prefix variant
/// first and falls through to the next on a per-field type
/// mismatch, so `task.spawn(f)` matches `NoName` even though
/// `Named`'s arity range overlaps.
#[derive(crate::FromLuaMulti)]
pub enum SpawnArgs {
    Named {
        name: Bytes,
        func: Function,
        args: Variadic,
    },
    NoName {
        func: Function,
        args: Variadic,
    },
}

/// Concurrent tasks plus async-aware synchronization primitives
/// (mutex, rwlock, semaphore, notify, watch, channels, oneshot).
///
/// ## Anonymous vs named primitives
///
/// Each sync-primitive constructor accepts an optional `name`
/// argument:
///
/// ```lua
/// local m = task.mutex()           -- anonymous: local to this caller
/// local m = task.mutex("cache")    -- named: shared via the host registry
/// ```
///
/// **Anonymous** primitives are created fresh on every call and
/// cannot escape the [`GlobalEnv`](../../api/shingetsu/struct.GlobalEnv.html)
/// they were created in.  They are the right default for one-off
/// coordination between tasks within a single VM.
///
/// **Named** primitives are looked up in a process-wide registry.
/// Two callers in any [`GlobalEnv`](../../api/shingetsu/struct.GlobalEnv.html)
/// asking for the same name get the same underlying primitive, so
/// they can coordinate across VMs and across configuration reloads.
/// The registry holds a strong reference for the lifetime of the
/// process, so a named entry survives every script reload.
///
/// ## Reload-friendly reconfiguration
///
/// A configuration reload re-runs constructor calls with potentially
/// different arguments.  Hard-failing on any difference would force
/// a process restart, defeating the point of reload.  Named
/// primitives instead follow this rule:
///
/// - **Tunables that can grow** are grown to match.  For example,
///   calling `task.semaphore(5, "throttle")` after
///   `task.semaphore(3, "throttle")` silently raises the permit
///   count to 5.
/// - **Tunables that cannot grow further or shrink at all** keep
///   the existing configuration and emit a warning.  Tokio's
///   semaphore cannot shrink, and tokio's bounded mpsc capacity is
///   fixed at construction; both fall back to "warn and keep".
/// - **Type mismatch** (e.g. a name first registered as `mutex`
///   later requested as `rwlock`) is a hard error — the calling
///   code cannot function with the wrong primitive type, so
///   reload-loop reasoning does not apply.
///
/// Warnings are emitted at most once per distinct requested value,
/// so a busy reload path repeatedly asking for the same
/// (already-warned) value does not flood the log.
///
/// ## Cross-VM value transport
///
/// Values that cross a [`GlobalEnv`](../../api/shingetsu/struct.GlobalEnv.html)
/// boundary through `task.watch:set` or `task.bounded_channel:send`
/// are deep-copied via shingetsu's snapshot machinery, so consumers
/// in another VM cannot alias the producer's tables.  Functions and
/// userdata that have not opted in to snapshotting are rejected with
/// a clear diagnostic at send/set time.
///
/// `task.oneshot()` is the exception: it is anonymous-only and
/// passes values by reference, since the pair cannot escape its
/// creating env.
///
/// ## Acquiring and releasing locks
///
/// Lock guards (`Mutex:lock()`, `RwLock:read()`, `Semaphore:acquire()`,
/// etc.) are released when the local holding the guard goes out of
/// scope, no `<close>` annotation needed:
///
/// ```lua
/// local lock = task.mutex("cache")
///
/// local function with_cache(fn)
///     local g = lock:lock()
///     return fn()
///     -- g is released here
/// end
/// ```
///
/// Each guard also has a method (`:unlock()`, `:release()`) for
/// releasing the lock before scope exit.  Calling the release
/// method twice raises an error so a stale guard is not silently
/// reused.
#[crate::module(name = "task")]
pub mod task_mod {
    use super::*;

    /// Spawn a Lua function as a concurrent task.
    ///
    /// Two argument shapes are accepted:
    /// - `task.spawn(func, ...)` — spawn an unnamed task.
    /// - `task.spawn(name, func, ...)` — spawn with a string name
    ///   surfaced through `Task:name()` and observer callbacks.
    ///
    /// Trailing arguments are passed to `func` when it begins
    /// executing.  Returns a `Task` userdata you can `:await()`,
    /// `:cancel()`, etc.
    #[function(variadic)]
    async fn spawn(ctx: CallContext, args: SpawnArgs) -> Result<Ud<LuaTask>, VmError> {
        let (name, func, fn_args) = match args {
            SpawnArgs::Named { name, func, args } => (Some(name), func, args.0),
            SpawnArgs::NoName { func, args } => (None, func, args.0),
        };

        let env = ctx.global.clone();
        let reg = registry(&env);
        let id = reg.alloc_id();
        let spawn_site =
            crate::traceback::render_traceback(ctx.call_stack().frames_bottom_up(), None, 0);
        let parent = CURRENT_TASK.try_with(|p| p.clone()).ok();
        let info = Arc::new(TaskInfo {
            id,
            name,
            spawned_at: Instant::now(),
            spawn_site,
            parent,
        });

        let state = Arc::new(TaskState {
            env: env.clone(),
            info: info.clone(),
            cancel: Notify::new(),
            completed: Notify::new(),
            result: Mutex::new(None),
            consumed: AtomicBool::new(false),
        });

        // Fire on_spawn before scheduling so observers see the
        // event before any chance of on_complete arriving.
        for obs in reg.snapshot_observers() {
            obs.on_spawn(&env, &info);
        }

        let started = info.spawned_at;
        let join_handle = tokio::spawn({
            let state = state.clone();
            let info = info.clone();
            let env_inner = env.clone();
            async move {
                let mut guard = AbortGuard {
                    state: Some(state.clone()),
                    started,
                };
                CURRENT_TASK
                    .scope(info, run_inner(env_inner, state, func, fn_args))
                    .await;
                guard.disarm();
            }
        });

        Ok(Ud(Arc::new(LuaTask {
            state,
            join_handle: Mutex::new(Some(join_handle)),
        })))
    }

    /// Wait for all tasks in the array to finish, returning their
    /// results in input order.
    ///
    /// Each element of the returned array is itself a sub-array
    /// containing the corresponding task's full return list,
    /// preserving multi-value returns and distinguishing
    /// no-return tasks (empty sub-array) from one-return-of-nil
    /// tasks (one-element sub-array containing nil).
    ///
    /// Raises on the first task that raised, was cancelled, or
    /// was aborted.  Use `task.taskset` if you want to consume
    /// completions one by one and handle failures individually.
    #[function]
    async fn join(tasks: Vec<Ud<LuaTask>>) -> Result<Vec<ValueVec>, VmError> {
        let mut out: Vec<ValueVec> = Vec::with_capacity(tasks.len());
        for task in &tasks {
            task.0.state.consumed.store(true, Ordering::SeqCst);
        }
        for task in tasks {
            let result = task.0.state.wait().await;
            match &*result {
                TaskResult::Success(vs) => out.push(vs.clone()),
                TaskResult::Failure(err) => {
                    let msg = err.error.to_string();
                    return Err(VmError::LuaError {
                        display: msg.clone(),
                        value: Value::string(msg),
                    });
                }
                TaskResult::Cancelled => {
                    return Err(VmError::LuaError {
                        display: "task cancelled".to_owned(),
                        value: Value::string("task cancelled"),
                    });
                }
                TaskResult::Aborted => {
                    return Err(VmError::LuaError {
                        display: "task aborted".to_owned(),
                        value: Value::string("task aborted"),
                    });
                }
            }
        }
        Ok(out)
    }

    /// Wait for all tasks to finish, discarding their results.
    /// Raises on the first task that raised; useful when the tasks
    /// were spawned for their side effects and you only care that
    /// each completed successfully.
    #[function]
    async fn await_all(tasks: Vec<Ud<LuaTask>>) -> Result<(), VmError> {
        for task in &tasks {
            task.0.state.consumed.store(true, Ordering::SeqCst);
        }
        for task in tasks {
            let result = task.0.state.wait().await;
            match &*result {
                TaskResult::Success(_) => {}
                TaskResult::Failure(err) => {
                    let msg = err.error.to_string();
                    return Err(VmError::LuaError {
                        display: msg.clone(),
                        value: Value::string(msg),
                    });
                }
                TaskResult::Cancelled => {
                    return Err(VmError::LuaError {
                        display: "task cancelled".to_owned(),
                        value: Value::string("task cancelled"),
                    });
                }
                TaskResult::Aborted => {
                    return Err(VmError::LuaError {
                        display: "task aborted".to_owned(),
                        value: Value::string("task aborted"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Wait for the first task in the array to finish.  Returns
    /// `(index, true, ...results)` on success or
    /// `(index, false, err)` on failure / cancel / abort, where
    /// `index` is the 1-based position of the winning task.
    /// Tasks that didn't win are left untouched and may still be
    /// awaited or cancelled.
    #[function]
    async fn select(tasks: Vec<Ud<LuaTask>>) -> Result<SelectResult, VmError> {
        if tasks.is_empty() {
            return Err(VmError::LuaError {
                display: "task.select called with empty task list".to_owned(),
                value: Value::string("task.select called with empty task list"),
            });
        }
        let futures: Vec<_> = tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let state = t.0.state.clone();
                Box::pin(async move {
                    let r = state.wait().await;
                    (i, r)
                })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = (usize, Arc<TaskResult>)> + Send>,
                    >
            })
            .collect();
        let ((winner, result), _, _) = futures::future::select_all(futures).await;
        // Mark only the winner consumed; losers may still be
        // awaited individually or cancelled.
        tasks[winner].0.state.consumed.store(true, Ordering::SeqCst);
        Ok(SelectResult::from_winner(winner, &result))
    }

    /// Yield to the runtime, allowing other tasks to make progress
    /// before resuming.  Useful inside long-running CPU loops to
    /// avoid starving other tasks on the same executor thread.
    // Lua name is `yield`; the Rust ident is `yield_now` because
    // `yield` is a reserved keyword in Rust and the proc-macro path
    // stringifier preserves the `r#` prefix on raw idents.
    #[function(rename = "yield")]
    async fn yield_now() {
        tokio::task::yield_now().await;
    }

    /// Materialize a value into a fresh mutable Lua table.
    ///
    /// `task.watch:get()`, `task.channel:recv()`, and similar return
    /// values as read-only snapshot-table proxies for `Map` / `Vec`
    /// shapes — cheap to receive, lazy on field access.  Pass such
    /// a proxy through `task.materialize` to obtain a fresh, fully
    /// mutable plain Lua table.
    ///
    /// Non-proxy values pass through unchanged: primitives, strings,
    /// plain Lua tables, and unrelated userdata are returned as-is.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local w = task.watch({ a = 1, nested = { b = 2 } })
    /// local snap = w:get()              -- read-only proxy
    /// local copy = task.materialize(snap)
    /// copy.a = 99                       -- mutation works on the copy
    /// assert(copy.a == 99)
    /// assert(w:get().a == 1)            -- original is unchanged
    /// ```
    #[function]
    fn materialize(ctx: CallContext, value: Value) -> Result<Value, VmError> {
        if let Value::Userdata(ud) = &value {
            let ud: Arc<dyn shingetsu_vm::Userdata> = ud.clone();
            if let Ok(m) = ud.clone().downcast_arc::<shingetsu_vm::LuaSnapshotMap>() {
                return m.materialize(&ctx.global);
            }
            if let Ok(v) = ud.downcast_arc::<shingetsu_vm::LuaSnapshotVec>() {
                return v.materialize(&ctx.global);
            }
        }
        Ok(value)
    }

    /// Sleep for `seconds` (a number) before resuming.  Fractional
    /// values are supported.  Cancellation via `Task:cancel()` /
    /// `Task:abort()` interrupts the sleep.
    #[function]
    async fn sleep(seconds: f64) {
        if seconds.is_finite() && seconds > 0.0 {
            tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)).await;
        }
    }

    /// Build a `TaskSet` from an initial array of tasks.  The
    /// set yields each task's completion via `:next()` in the
    /// order tasks finish, regardless of input order.  More tasks
    /// can be added later with `:add()`.
    ///
    /// Iterating with `for task, ok, ...results in set do` works
    /// too — the userdata is itself callable as the iterator.
    #[function]
    fn taskset(tasks: Vec<Ud<LuaTask>>) -> Ud<LuaTaskSet> {
        let set = LuaTaskSet::new();
        for task in tasks {
            set.watch(task.0);
        }
        Ud(set)
    }

    /// Construct a mutex.
    ///
    /// `task.mutex()` returns a fresh anonymous mutex local to the
    /// caller.  `task.mutex(name)` looks up `name` in the
    /// process-shared registry and returns the same mutex on every
    /// subsequent call with that name, so the lock survives
    /// configuration reload and is visible to other VMs in the same
    /// host.  A name previously registered with a different
    /// primitive type raises an error.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Serialise concurrent access to shared state.
    /// local m = task.mutex()
    /// local count = 0
    /// local workers = {}
    /// for i = 1, 4 do
    ///     workers[i] = task.spawn(function()
    ///         local g = m:lock()
    ///         count = count + 1
    ///         -- g is released when the function returns
    ///     end)
    /// end
    /// task.await_all(workers)
    /// assert(count == 4)
    /// ```
    #[function]
    fn mutex(ctx: CallContext, name: Option<Bytes>) -> Result<Ud<LuaMutex>, VmError> {
        let mu = match name {
            Some(name) => shared_lookup::<LuaMutex, _>(
                &ctx.global.shared_registry(),
                "mutex",
                1,
                name,
                LuaMutex::default,
            )?,
            None => Arc::new(LuaMutex::default()),
        };
        Ok(Ud(mu))
    }

    /// Construct a reader-writer lock.
    ///
    /// Argument shape and registry semantics match `task.mutex`:
    /// `task.rwlock()` is anonymous, `task.rwlock(name)` is shared.
    /// Fairness is write-preferring (tokio default) to avoid writer
    /// starvation under sustained read load.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Multiple readers can hold the lock simultaneously; a
    /// -- writer waits until all readers release.
    /// local rw = task.rwlock()
    /// local r1 = rw:read()
    /// local r2 = rw:read()
    /// assert(rw:try_write() == nil)  -- writer is blocked by readers
    /// r1:unlock()
    /// r2:unlock()
    /// local w = rw:write()
    /// assert(w ~= nil)
    /// ```
    #[function]
    fn rwlock(ctx: CallContext, name: Option<Bytes>) -> Result<Ud<LuaRwLock>, VmError> {
        let rw = match name {
            Some(name) => shared_lookup::<LuaRwLock, _>(
                &ctx.global.shared_registry(),
                "rwlock",
                1,
                name,
                LuaRwLock::default,
            )?,
            None => Arc::new(LuaRwLock::default()),
        };
        Ok(Ud(rw))
    }

    /// Construct a oneshot channel: a sender that can deliver one
    /// value, and a receiver that awaits that value.
    ///
    /// Always anonymous — a named oneshot has awkward semantics
    /// because either end may be dropped on a different VM, leaving
    /// the registry holding a half-consumed pair with no clean
    /// recovery story.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Pass a single value from one task to another.
    /// local tx, rx = task.oneshot()
    /// task.spawn(function() tx:send(42) end)
    /// assert(rx:recv() == 42)
    /// ```
    ///
    /// ```lua
    /// -- The receiver wakes with nil if the sender closes
    /// -- without delivering a value.
    /// local tx, rx = task.oneshot()
    /// task.spawn(function() tx:close() end)
    /// assert(rx:recv() == nil)
    /// ```
    #[function]
    fn oneshot() -> (Ud<LuaOneshotSender>, Ud<LuaOneshotReceiver>) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        (
            Ud(Arc::new(LuaOneshotSender {
                inner: Mutex::new(Some(tx)),
            })),
            Ud(Arc::new(LuaOneshotReceiver {
                inner: Mutex::new(Some(rx)),
            })),
        )
    }

    /// Construct a bounded async channel with the given capacity.
    ///
    /// `task.bounded_channel(capacity)` is anonymous;
    /// `task.bounded_channel(capacity, name)` is shared via the
    /// registry.  Capacity is fixed at construction (tokio's mpsc
    /// has no runtime resize); a named lookup that requests a
    /// different capacity keeps the existing channel and emits a
    /// warning, so a configuration reload that touches the capacity
    /// does not break the process.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Producer/consumer: send blocks when full, recv blocks
    /// -- when empty.
    /// local ch = task.bounded_channel(4)
    /// local producer = task.spawn(function()
    ///     for i = 1, 3 do ch:send(i) end
    ///     ch:close()
    /// end)
    /// local total = 0
    /// while true do
    ///     local v = ch:recv()
    ///     if v == nil then break end  -- channel closed and drained
    ///     total = total + v
    /// end
    /// producer:await()
    /// assert(total == 6)
    /// ```
    #[function]
    fn bounded_channel(
        ctx: CallContext,
        capacity: i64,
        name: Option<Bytes>,
    ) -> Result<Ud<LuaBoundedChannel>, VmError> {
        if capacity <= 0 {
            return Err(VmError::ArgError {
                position: 1,
                function: "bounded_channel".to_owned(),
                msg: format!("capacity must be positive, got {capacity}"),
            });
        }
        let capacity = capacity as usize;
        let ch = match name {
            Some(name) => {
                let arc = shared_lookup::<LuaBoundedChannel, _>(
                    &ctx.global.shared_registry(),
                    "bounded_channel",
                    2,
                    name.clone(),
                    || LuaBoundedChannel::new(capacity),
                )?;
                // Track the most recent requested capacity so that
                // a busy reload path repeatedly asking for the same
                // (already-warned) capacity gets a single warning,
                // not a flood.  The swap also races concurrent
                // identical requests down to one warning.
                let prev_requested = arc.last_requested.swap(capacity, Ordering::AcqRel);
                if arc.capacity != capacity && capacity != prev_requested {
                    warn(format_args!(
                        "task.bounded_channel: named entry {:?} already configured with \
                         capacity {}, requested {}; capacity is fixed at construction, \
                         keeping existing",
                        bstr::BStr::new(name.as_ref()),
                        arc.capacity,
                        capacity,
                    ));
                }
                arc
            }
            None => Arc::new(LuaBoundedChannel::new(capacity)),
        };
        Ok(Ud(ch))
    }

    /// Construct an unbounded async channel.
    ///
    /// `task.unbounded_channel()` is anonymous;
    /// `task.unbounded_channel(name)` is shared via the registry.
    /// Without backpressure a fast producer can grow the queue
    /// without bound; reach for `task.bounded_channel` when the
    /// producer cannot afford to outpace the consumer.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Send never awaits; the queue grows as needed.
    /// local ch = task.unbounded_channel()
    /// for i = 1, 10 do ch:send(i) end
    /// ch:close()
    /// local sum = 0
    /// while true do
    ///     local v = ch:recv()
    ///     if v == nil then break end
    ///     sum = sum + v
    /// end
    /// assert(sum == 55)
    /// ```
    #[function]
    fn unbounded_channel(
        ctx: CallContext,
        name: Option<Bytes>,
    ) -> Result<Ud<LuaUnboundedChannel>, VmError> {
        let ch = match name {
            Some(name) => shared_lookup::<LuaUnboundedChannel, _>(
                &ctx.global.shared_registry(),
                "unbounded_channel",
                1,
                name,
                LuaUnboundedChannel::default,
            )?,
            None => Arc::new(LuaUnboundedChannel::default()),
        };
        Ok(Ud(ch))
    }

    /// Construct a state cell with change notification.
    ///
    /// `initial` is either a snapshottable value (snapshot-validated
    /// at construction) or a zero-arg function that returns a
    /// snapshottable value.  The function form lets named-watch
    /// callers defer expensive initialization to the first creation
    /// (named lookups that hit an existing entry never invoke the
    /// function).  For anonymous watches the function is always
    /// invoked since there is no prior entry to reuse.
    ///
    /// Reload-friendly: on a named-lookup hit the existing watch is
    /// returned and `initial` is ignored.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Publish state from one task; observe it from another.
    /// local w = task.watch({ ready = false })
    /// local observer = task.spawn(function()
    ///     return w:wait_for(function(v) return v.ready end).ready
    /// end)
    /// w:set({ ready = true })
    /// assert(observer:await() == true)
    /// ```
    #[function]
    async fn watch(
        ctx: CallContext,
        initial: Value,
        name: Option<Bytes>,
    ) -> Result<Ud<LuaWatch>, VmError> {
        match name {
            None => {
                let snap = compute_initial(&ctx, &initial).await?;
                Ok(Ud(Arc::new(LuaWatch::new(snap))))
            }
            Some(name) => {
                let registry = ctx.global.shared_registry();
                let arc = registry
                    .get_or_create_async::<LuaWatch, VmError, _, _>(name, || async {
                        let snap = compute_initial(&ctx, &initial).await?;
                        Ok(LuaWatch::new(snap))
                    })
                    .await
                    .map_err(|e| match e {
                        shingetsu_vm::AsyncCreateError::Registry(reg) => VmError::ArgError {
                            position: 2,
                            function: "watch".to_owned(),
                            msg: reg.to_string(),
                        },
                        shingetsu_vm::AsyncCreateError::Factory(vm) => vm,
                    })?;
                Ok(Ud(arc))
            }
        }
    }

    /// Construct an edge-triggered notification primitive.
    ///
    /// `task.notify()` returns a fresh anonymous notify; `task.notify(name)`
    /// is shared via the registry.
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- wait_until evaluates the predicate, then if false awaits a
    /// -- notification before re-checking.  The register-before-check
    /// -- ordering means a notification raced against the predicate
    /// -- check is not lost.
    /// local n = task.notify()
    /// local ready = false
    /// local waiter = task.spawn(function()
    ///     n:wait_until(function() return ready end)
    ///     return "woke up"
    /// end)
    /// task.yield()
    /// ready = true
    /// n:notify_one()
    /// assert(waiter:await() == "woke up")
    /// ```
    #[function]
    fn notify(ctx: CallContext, name: Option<Bytes>) -> Result<Ud<LuaNotify>, VmError> {
        let n = match name {
            Some(name) => shared_lookup::<LuaNotify, _>(
                &ctx.global.shared_registry(),
                "notify",
                1,
                name,
                LuaNotify::default,
            )?,
            None => Arc::new(LuaNotify::default()),
        };
        Ok(Ud(n))
    }

    /// Construct a counting semaphore with `permits` initial permits.
    ///
    /// `task.semaphore(permits)` is anonymous; `task.semaphore(permits,
    /// name)` is shared via the registry.  For the named form, a
    /// later call requesting *more* permits silently grows the
    /// existing semaphore via `add_permits`; a request for *fewer*
    /// permits keeps the existing configuration and emits a warning
    /// (tokio's semaphore cannot shrink).
    ///
    /// # Examples
    ///
    /// ```lua
    /// -- Throttle concurrent work to at most 2 in flight.
    /// local sem = task.semaphore(2)
    /// local in_flight = 0
    /// local peak = 0
    /// local workers = {}
    /// for i = 1, 6 do
    ///     workers[i] = task.spawn(function()
    ///         local p = sem:acquire()
    ///         in_flight = in_flight + 1
    ///         if in_flight > peak then peak = in_flight end
    ///         task.yield()
    ///         in_flight = in_flight - 1
    ///     end)
    /// end
    /// task.await_all(workers)
    /// assert(peak <= 2)
    /// ```
    #[function]
    fn semaphore(
        ctx: CallContext,
        permits: i64,
        name: Option<Bytes>,
    ) -> Result<Ud<LuaSemaphore>, VmError> {
        if permits < 0 {
            return Err(VmError::ArgError {
                position: 1,
                function: "semaphore".to_owned(),
                msg: format!("permits must be non-negative, got {permits}"),
            });
        }
        let permits = permits as usize;
        let sem = match name {
            Some(name) => {
                let arc = shared_lookup::<LuaSemaphore, _>(
                    &ctx.global.shared_registry(),
                    "semaphore",
                    2,
                    name.clone(),
                    || LuaSemaphore::new(permits),
                )?;
                let configured = arc.configured_permits();
                // Always update last_requested so the dedup logic below
                // tracks the most recent caller's value.  The swap also
                // races concurrent shrink requests for the same value
                // down to a single warning.
                let prev_requested = arc.last_requested.swap(permits, Ordering::AcqRel);
                if permits > configured {
                    // Grow path: silently add permits so a config
                    // reload that bumps the cap takes effect.
                    arc.try_grow_to(permits);
                } else if permits < configured && permits != prev_requested {
                    // Shrink not supported by tokio's Semaphore.  Warn
                    // once per distinct shrink value and keep the
                    // existing configuration so reload doesn't break
                    // the process.  A busy reload path that repeatedly
                    // asks for the same value gets a single warning.
                    warn(format_args!(
                        "task.semaphore: named entry {:?} already configured with {} permits, \
                         requested {}; cannot shrink, keeping existing",
                        bstr::BStr::new(name.as_ref()),
                        configured,
                        permits,
                    ));
                }
                arc
            }
            None => Arc::new(LuaSemaphore::new(permits)),
        };
        Ok(Ud(sem))
    }
}

/// Register the `task` module's userdata types and Lua-visible
/// functions against `env`.  Call this once per `GlobalEnv`;
/// observers can be added/removed independently via
/// [`add_observer`] / [`remove_observer`] / [`clear_observers`].
pub fn register(env: &GlobalEnv) -> Result<(), VmError> {
    env.register_userdata_type(LuaRuntimeError::userdata_type());
    env.register_userdata_type(LuaTask::userdata_type());
    env.register_userdata_type(LuaTaskSet::userdata_type());
    env.register_userdata_type(LuaMutex::userdata_type());
    env.register_userdata_type(LuaMutexGuard::userdata_type());
    env.register_userdata_type(LuaRwLock::userdata_type());
    env.register_userdata_type(LuaRwLockReadGuard::userdata_type());
    env.register_userdata_type(LuaRwLockWriteGuard::userdata_type());
    env.register_userdata_type(LuaSemaphore::userdata_type());
    env.register_userdata_type(LuaSemaphorePermit::userdata_type());
    env.register_userdata_type(LuaNotify::userdata_type());
    env.register_userdata_type(LuaWatch::userdata_type());
    env.register_userdata_type(LuaBoundedChannel::userdata_type());
    env.register_userdata_type(LuaUnboundedChannel::userdata_type());
    env.register_userdata_type(LuaOneshotSender::userdata_type());
    env.register_userdata_type(LuaOneshotReceiver::userdata_type());
    let table = task_mod::build_module_table(env)?;
    env.set_global("task", Value::Table(table));
    env.register_module_type("task", task_mod::module_type());
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::valuevec;
    use std::sync::atomic::AtomicUsize;

    fn new_env() -> GlobalEnv {
        GlobalEnv::new()
    }

    struct CountingObserver {
        spawns: AtomicUsize,
        completes: AtomicUsize,
        abandons: AtomicUsize,
    }

    impl CountingObserver {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                spawns: AtomicUsize::new(0),
                completes: AtomicUsize::new(0),
                abandons: AtomicUsize::new(0),
            })
        }
    }

    impl TaskObserver for CountingObserver {
        fn on_spawn(&self, _env: &GlobalEnv, _info: &TaskInfo) {
            self.spawns.fetch_add(1, Ordering::Relaxed);
        }
        fn on_complete(&self, _env: &GlobalEnv, _info: &TaskInfo, _outcome: &TaskOutcome<'_>) {
            self.completes.fetch_add(1, Ordering::Relaxed);
        }
        fn on_handle_abandoned(&self, _env: &GlobalEnv, _info: &TaskInfo) {
            self.abandons.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn dummy_info() -> TaskInfo {
        TaskInfo {
            id: 1,
            name: None,
            spawned_at: Instant::now(),
            spawn_site: String::new(),
            parent: None,
        }
    }

    #[test]
    fn registry_allocates_monotonic_ids() {
        let env = new_env();
        let reg = registry(&env);
        k9::assert_equal!(reg.alloc_id(), 1);
        k9::assert_equal!(reg.alloc_id(), 2);
        k9::assert_equal!(reg.alloc_id(), 3);
    }

    #[test]
    fn registry_is_per_env() {
        let env_a = new_env();
        let env_b = new_env();
        let id_a = registry(&env_a).alloc_id();
        let id_b = registry(&env_b).alloc_id();
        // Both envs start at 1 independently.
        k9::assert_equal!(id_a, 1);
        k9::assert_equal!(id_b, 1);
    }

    #[test]
    fn add_and_clear_observers() {
        let env = new_env();
        let obs1 = CountingObserver::new();
        let obs2 = CountingObserver::new();
        add_observer(&env, obs1.clone());
        add_observer(&env, obs2.clone());

        let snapshot = registry(&env).snapshot_observers();
        k9::assert_equal!(snapshot.len(), 2);

        clear_observers(&env);
        let snapshot = registry(&env).snapshot_observers();
        k9::assert_equal!(snapshot.len(), 0);
    }

    #[test]
    fn remove_observer_matches_by_arc_identity() {
        let env = new_env();
        let obs1 = CountingObserver::new();
        let obs2 = CountingObserver::new();
        let obs1_dyn: Arc<dyn TaskObserver> = obs1.clone();
        let obs2_dyn: Arc<dyn TaskObserver> = obs2.clone();
        add_observer(&env, obs1_dyn.clone());
        add_observer(&env, obs2_dyn.clone());

        k9::assert_equal!(remove_observer(&env, &obs1_dyn), true);
        k9::assert_equal!(registry(&env).snapshot_observers().len(), 1);

        // Removing again returns false (already gone).
        k9::assert_equal!(remove_observer(&env, &obs1_dyn), false);

        k9::assert_equal!(remove_observer(&env, &obs2_dyn), true);
        k9::assert_equal!(registry(&env).snapshot_observers().len(), 0);
    }

    #[test]
    fn observer_methods_dispatch_via_arc() {
        // Sanity-check that dyn dispatch through Arc<dyn TaskObserver>
        // reaches the concrete impl.
        let env = new_env();
        let obs = CountingObserver::new();
        let obs_dyn: Arc<dyn TaskObserver> = obs.clone();
        add_observer(&env, obs_dyn);

        let info = dummy_info();
        let observers = registry(&env).snapshot_observers();
        for o in &observers {
            o.on_spawn(&env, &info);
            o.on_complete(
                &env,
                &info,
                &TaskOutcome::Success {
                    results: &valuevec![],
                    elapsed: Duration::from_millis(0),
                },
            );
            o.on_handle_abandoned(&env, &info);
        }

        k9::assert_equal!(obs.spawns.load(Ordering::Relaxed), 1);
        k9::assert_equal!(obs.completes.load(Ordering::Relaxed), 1);
        k9::assert_equal!(obs.abandons.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn parent_chain_is_walkable() {
        let root = Arc::new(TaskInfo {
            id: 1,
            name: Some("root".into()),
            spawned_at: Instant::now(),
            spawn_site: String::new(),
            parent: None,
        });
        let child = Arc::new(TaskInfo {
            id: 2,
            name: Some("child".into()),
            spawned_at: Instant::now(),
            spawn_site: String::new(),
            parent: Some(root.clone()),
        });
        let grandchild = TaskInfo {
            id: 3,
            name: Some("grandchild".into()),
            spawned_at: Instant::now(),
            spawn_site: String::new(),
            parent: Some(child.clone()),
        };

        let mut chain = Vec::new();
        let mut current: Option<&TaskInfo> = Some(&grandchild);
        while let Some(info) = current {
            chain.push(info.id);
            current = info.parent.as_deref();
        }
        k9::assert_equal!(chain, vec![3u64, 2, 1]);
    }
}
