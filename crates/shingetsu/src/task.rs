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
//! This module currently provides only the observer plumbing and a
//! `RuntimeError` userdata wrapper.  The `task.spawn` Lua surface
//! will land in a follow-up step.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;

use crate::diagnostic::{render_runtime_error, RenderStyle};
use crate::error::RuntimeError;
use crate::{valuevec, Bytes, GlobalEnv, Value, ValueVec, Variadic, VmError};

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

/// Register the `task` module's userdata types against `env`.
///
/// At present this only registers the `RuntimeError` userdata type;
/// the `task.*` Lua surface is installed in a follow-up step.
pub fn register(env: &GlobalEnv) -> Result<(), VmError> {
    env.register_userdata_type(LuaRuntimeError::userdata_type());
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
