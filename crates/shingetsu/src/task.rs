//! Concurrent task library.
//!
//! Spawns Lua functions as independent tasks driven by `tokio::spawn`,
//! exposing a `Task` userdata to Lua for awaiting, cancelling, and
//! introspecting them.  Lifecycle events are surfaced through a
//! [`TaskObserver`] trait that hosts can implement to model
//! parent/child task graphs, log per-task summaries, count live
//! tasks for graceful shutdown, etc.  The library itself ships no
//! built-in observer.
//!
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::convert::LuaTypedMulti;
use crate::diagnostic::{render_runtime_error, RenderStyle};
use crate::error::RuntimeError;
use crate::sync::{Mutex, RwLock};
use crate::types::LuaType;
use crate::{
    valuevec, Bytes, CallContext, FromLua, FromLuaMulti, Function, GlobalEnv, IntoLua, LuaTyped,
    Ud, Value, ValueVec, Variadic, VmError,
};

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
/// per-env [`TaskRegistry`] at spawn time; stable for the lifetime of
/// the task and exposed to Lua via `Task:id()`.
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
// LuaRuntimeError userdata
// ---------------------------------------------------------------------------

/// Userdata wrapper around [`RuntimeError`] exposed to Lua.
///
/// Returned as the second value of `Task:pawait()` and the error
/// value of `Task:try_result()` / `task.select`.  Lets Lua code
/// inspect the structured error rather than receive a flattened
/// string.
pub struct LuaRuntimeError(Arc<RuntimeError>);

/// Return shape for [`LuaRuntimeError::lua_location`]: either a
/// `(source_name, line)` pair or a single `nil` when the error
/// has no associated Lua source location.  The derive expands to
/// `(string, integer) | nil` for the type checker.
#[derive(crate::IntoLuaMulti)]
pub enum LocationResult {
    FileAndLine(Bytes, i64),
    None,
}

impl LuaRuntimeError {
    pub fn new(err: Arc<RuntimeError>) -> Arc<Self> {
        Arc::new(Self(err))
    }

    pub fn inner(&self) -> &RuntimeError {
        &self.0
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "RuntimeError", index_fallback = "nil")]
impl LuaRuntimeError {
    /// The bare error message (no traceback, no source snippets).
    #[lua_method]
    fn message(self: Arc<Self>) -> Bytes {
        self.0.error.to_string().into()
    }

    /// Rendered stack traceback for the error, in the same format
    /// produced by Lua's `debug.traceback`.
    #[lua_method]
    fn traceback(self: Arc<Self>) -> Bytes {
        crate::traceback::render_traceback(&self.0.call_stack, None, 0).into()
    }

    /// Source location of the innermost Lua frame: returns
    /// `(source_name, line)` when the error originated in Lua
    /// code, or `nil` when it was raised outside any Lua frame
    /// (e.g. from a host-only call path).
    #[lua_method]
    fn location(self: Arc<Self>) -> LocationResult {
        match self
            .0
            .call_stack
            .iter()
            .rev()
            .find_map(|f| f.source_location())
        {
            Some(loc) => {
                LocationResult::FileAndLine(loc.source_name.as_str().into(), loc.line as i64)
            }
            None => LocationResult::None,
        }
    }

    /// Array of help-text hints attached to the error, in the order
    /// they were attached.  Empty array if no hints were attached.
    #[lua_method]
    fn hints(self: Arc<Self>) -> Vec<Bytes> {
        self.0
            .hints
            .iter()
            .map(|h| h.message.clone().into())
            .collect()
    }

    /// Render the full annotated diagnostic — the same multi-line
    /// output the CLI prints for an unhandled error, including
    /// source snippets, hints, and the stack trace.
    #[lua_method]
    fn render(self: Arc<Self>) -> Bytes {
        render_runtime_error(&self.0, RenderStyle::Plain).into()
    }

    /// `__tostring`: returns the same string as `:render()`.
    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string(render_runtime_error(
            &self.0,
            RenderStyle::Plain
        ))])
    }
}

// ---------------------------------------------------------------------------
// LuaTask userdata + supporting state
// ---------------------------------------------------------------------------

/// Internal record of a finished task.  Wrapped in an `Arc` so
/// multiple awaiters can read the same outcome without cloning
/// the underlying values.
enum TaskResult {
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
struct TaskState {
    env: GlobalEnv,
    info: Arc<TaskInfo>,
    /// Notified by `Task:cancel()` to request graceful cancellation.
    cancel: Notify,
    /// Notified by the wrapper future (or the abort drop guard)
    /// once `result` has been written.
    completed: Notify,
    result: Mutex<Option<Arc<TaskResult>>>,
    /// Set by methods that collect a result or explicitly
    /// cancel/abort, suppressing the `on_handle_abandoned`
    /// observer firing in `LuaTask::drop`.
    consumed: AtomicBool,
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
    async fn wait(&self) -> Arc<TaskResult> {
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

/// Userdata returned by `task.spawn`.  Holds the join handle for
/// the spawned tokio task plus the shared [`TaskState`].
pub struct LuaTask {
    state: Arc<TaskState>,
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for LuaTask {
    fn drop(&mut self) {
        if self.state.consumed.load(Ordering::SeqCst) {
            return;
        }
        // Userdata was abandoned without the result being collected
        // and without explicit cancel/abort.  Notify observers; the
        // task itself continues running independently (tokio detaches
        // when the JoinHandle drops without abort).
        let env = &self.state.env;
        let info = &self.state.info;
        for obs in registry(env).snapshot_observers() {
            obs.on_handle_abandoned(env, info);
        }
    }
}

// ---------------------------------------------------------------------------
// Typed return shapes for `:pawait()` and `:try_result()`
// ---------------------------------------------------------------------------
//
// `TrueLit` / `FalseLit` carry a `BoolLiteral` Lua type so the type
// checker sees the success arm as `(true, ...)` rather than the less
// precise `(boolean, ...)`.  This mirrors the `ProtectedReturn`
// pattern used in `crates/shingetsu-vm/src/builtins.rs` for
// `pcall` / `xpcall`.

pub(crate) struct TrueLit;
impl IntoLua for TrueLit {
    fn into_lua(self) -> Value {
        Value::Boolean(true)
    }
}
impl LuaTyped for TrueLit {
    fn lua_type() -> LuaType {
        LuaType::BoolLiteral(true)
    }
}

pub(crate) struct FalseLit;
impl IntoLua for FalseLit {
    fn into_lua(self) -> Value {
        Value::Boolean(false)
    }
}
impl LuaTyped for FalseLit {
    fn lua_type() -> LuaType {
        LuaType::BoolLiteral(false)
    }
}

/// Return shape for `Task:pawait()`.  One arm per `TaskResult`
/// variant; the type checker sees a `Union<(true, ...any),
/// (false, RuntimeError), (false, string)>`.
#[derive(crate::IntoLuaMulti)]
pub(crate) enum AwaitResult {
    Success(TrueLit, Variadic),
    Failure(FalseLit, Ud<LuaRuntimeError>),
    Cancelled(FalseLit, Bytes),
    Aborted(FalseLit, Bytes),
}

impl AwaitResult {
    fn from_finished(r: &TaskResult) -> Self {
        match r {
            TaskResult::Success(vs) => AwaitResult::Success(TrueLit, Variadic(vs.clone())),
            TaskResult::Failure(err) => {
                AwaitResult::Failure(FalseLit, Ud(LuaRuntimeError::new(err.clone())))
            }
            TaskResult::Cancelled => AwaitResult::Cancelled(FalseLit, "task cancelled".into()),
            TaskResult::Aborted => AwaitResult::Aborted(FalseLit, "task aborted".into()),
        }
    }
}

/// Return shape for `Task:try_result()`: `nil` while the task is
/// still running, otherwise the same shape as [`AwaitResult`].
#[derive(crate::IntoLuaMulti)]
pub(crate) enum TryResult {
    Pending,
    Success(TrueLit, Variadic),
    Failure(FalseLit, Ud<LuaRuntimeError>),
    Cancelled(FalseLit, Bytes),
    Aborted(FalseLit, Bytes),
}

impl TryResult {
    fn from_snapshot(r: Option<&TaskResult>) -> Self {
        match r {
            None => TryResult::Pending,
            Some(TaskResult::Success(vs)) => TryResult::Success(TrueLit, Variadic(vs.clone())),
            Some(TaskResult::Failure(err)) => {
                TryResult::Failure(FalseLit, Ud(LuaRuntimeError::new(err.clone())))
            }
            Some(TaskResult::Cancelled) => TryResult::Cancelled(FalseLit, "task cancelled".into()),
            Some(TaskResult::Aborted) => TryResult::Aborted(FalseLit, "task aborted".into()),
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "Task", index_fallback = "nil")]
impl LuaTask {
    /// Wait for the task to finish, returning its results.
    ///
    /// Re-raises any runtime error the task produced; for cancelled
    /// or aborted tasks raises `"task cancelled"` / `"task aborted"`.
    /// Use `:pawait()` to inspect failures without raising.
    #[lua_method(rename = "await")]
    async fn await_completion(self: Arc<Self>) -> Result<Variadic, VmError> {
        self.state.consumed.store(true, Ordering::SeqCst);
        let result = self.state.wait().await;
        match &*result {
            TaskResult::Success(vs) => Ok(Variadic(vs.clone())),
            TaskResult::Failure(err) => {
                let msg = err.error.to_string();
                Err(VmError::LuaError {
                    display: msg.clone(),
                    value: Value::string(msg),
                })
            }
            TaskResult::Cancelled => Err(VmError::LuaError {
                display: "task cancelled".to_owned(),
                value: Value::string("task cancelled"),
            }),
            TaskResult::Aborted => Err(VmError::LuaError {
                display: "task aborted".to_owned(),
                value: Value::string("task aborted"),
            }),
        }
    }

    /// Protected await: wait for the task and return
    /// `(true, ...results)` on success or `(false, err)` on
    /// failure.  `err` is a `RuntimeError` userdata for runtime
    /// errors, or a string for cancellation/abort.
    #[lua_method]
    async fn pawait(self: Arc<Self>) -> AwaitResult {
        self.state.consumed.store(true, Ordering::SeqCst);
        let result = self.state.wait().await;
        AwaitResult::from_finished(&result)
    }

    /// Request graceful cancellation.  Drives the task's
    /// `<close>` / `__close` handlers, then resolves once cleanup
    /// completes.  Idempotent.
    #[lua_method]
    async fn cancel(self: Arc<Self>) {
        self.state.consumed.store(true, Ordering::SeqCst);
        self.state.cancel.notify_one();
        let _ = self.state.wait().await;
    }

    /// Abort the task immediately.  `<close>` / `__close` handlers
    /// do **not** run.  Resolves once the underlying tokio task
    /// has been dropped.
    #[lua_method]
    async fn abort(self: Arc<Self>) {
        self.state.consumed.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join_handle.lock().take() {
            handle.abort();
        }
        let _ = self.state.wait().await;
    }

    /// Returns true if the task has finished (success, failure,
    /// cancelled, or aborted).  Does not consume the result.
    #[lua_method]
    fn is_finished(self: Arc<Self>) -> bool {
        self.state.result.lock().is_some()
    }

    /// Non-blocking peek at the result.
    ///
    /// Returns `nil` if the task is still running, otherwise the
    /// same `(true, ...)` / `(false, err)` pair as `:pawait()`.
    #[lua_method]
    fn try_result(self: Arc<Self>) -> TryResult {
        let snapshot = self.state.result.lock().clone();
        if snapshot.is_some() {
            self.state.consumed.store(true, Ordering::SeqCst);
        }
        TryResult::from_snapshot(snapshot.as_deref())
    }

    /// The task's monotonic id, allocated at spawn time.
    #[lua_method]
    fn id(self: Arc<Self>) -> i64 {
        self.state.info.id as i64
    }

    /// The task's name as supplied to `task.spawn`, or `nil` if no
    /// name was provided.
    #[lua_method]
    fn name(self: Arc<Self>) -> Option<Bytes> {
        self.state.info.name.clone()
    }

    /// Rendered traceback of the call site that spawned this task.
    #[lua_method]
    fn spawned_by(self: Arc<Self>) -> Bytes {
        self.state.info.spawn_site.clone().into()
    }

    /// Seconds elapsed since the task was spawned.
    #[lua_method]
    fn elapsed(self: Arc<Self>) -> f64 {
        self.state.info.spawned_at.elapsed().as_secs_f64()
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let state_str = if self.state.result.lock().is_some() {
            "finished"
        } else {
            "running"
        };
        let id = self.state.info.id;
        let s = match &self.state.info.name {
            Some(n) => format!("Task#{id} '{}' ({state_str})", bstr::BStr::new(n)),
            None => format!("Task#{id} ({state_str})"),
        };
        Variadic(valuevec![Value::string(s)])
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

/// Argument shape for `task.spawn`, dispatched on the type of the
/// first argument: a string introduces a named task, a function
/// is the task body.  Trailing args are forwarded to the body.
///
/// Hand-rolled rather than `#[derive(FromLuaMulti)]` because the
/// derive currently has no special-case for [`Variadic`] in a
/// variant's last position (it requires every field to implement
/// `FromLua`, which `Variadic` does not).  The two arms are
/// otherwise structurally identical to what the derive would emit.
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

impl FromLuaMulti for SpawnArgs {
    fn from_lua_multi(values: ValueVec) -> Result<Self, VmError> {
        let mut iter = values.into_iter();
        let first = iter.next().unwrap_or(Value::Nil);
        match first {
            Value::String(name) => {
                let second = iter.next().unwrap_or(Value::Nil);
                let got = second.type_name();
                let func = Function::from_lua(second).map_err(|_| VmError::BadArgument {
                    position: 2,
                    function: String::new(),
                    expected: "function".to_owned(),
                    got: got.to_owned(),
                })?;
                Ok(SpawnArgs::Named {
                    name,
                    func,
                    args: Variadic(iter.collect()),
                })
            }
            Value::Function(func) => Ok(SpawnArgs::NoName {
                func,
                args: Variadic(iter.collect()),
            }),
            other => Err(VmError::BadArgument {
                position: 1,
                function: String::new(),
                expected: "string or function".to_owned(),
                got: other.type_name().to_owned(),
            }),
        }
    }
}

impl LuaTypedMulti for SpawnArgs {
    fn lua_types() -> Vec<LuaType> {
        vec![LuaType::Union(vec![
            LuaType::Tuple(vec![
                Bytes::lua_type(),
                Function::lua_type(),
                Variadic::lua_type(),
            ]),
            LuaType::Tuple(vec![Function::lua_type(), Variadic::lua_type()]),
        ])]
    }
}

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
}

/// Register the `task` module's userdata types and Lua-visible
/// functions against `env`.  Call this once per `GlobalEnv`;
/// observers can be added/removed independently via
/// [`add_observer`] / [`remove_observer`] / [`clear_observers`].
pub fn register(env: &GlobalEnv) -> Result<(), VmError> {
    env.register_userdata_type(LuaRuntimeError::userdata_type());
    env.register_userdata_type(LuaTask::userdata_type());
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
