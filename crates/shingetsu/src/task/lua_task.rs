//! `LuaTask` userdata and the typed return shapes for its
//! result-collecting methods (`:pawait`, `:try_result`) and for
//! `task.select`.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::sync::Mutex;
use crate::types::LuaType;
use crate::{valuevec, Bytes, IntoLua, LuaTyped, Ud, Value, Variadic, VmError};

use super::runtime_error::LuaRuntimeError;
use super::{registry, TaskResult, TaskState};

// ---------------------------------------------------------------------------
// Typed return shapes for `:pawait()`, `:try_result()`, and `task.select`
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
    pub(crate) fn from_finished(r: &TaskResult) -> Self {
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
/// still running, otherwise the same shape as `AwaitResult`.
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

/// Return shape for `task.select(tasks)`: a 1-based index into the
/// input array followed by the same `(true, ...)` / `(false, err)`
/// pair shape as `Task:pawait()`.
#[derive(crate::IntoLuaMulti)]
pub(crate) enum SelectResult {
    Success(i64, TrueLit, Variadic),
    Failure(i64, FalseLit, Ud<LuaRuntimeError>),
    Cancelled(i64, FalseLit, Bytes),
    Aborted(i64, FalseLit, Bytes),
}

impl SelectResult {
    pub(crate) fn from_winner(index: usize, r: &TaskResult) -> Self {
        let i = index as i64 + 1;
        match r {
            TaskResult::Success(vs) => SelectResult::Success(i, TrueLit, Variadic(vs.clone())),
            TaskResult::Failure(err) => {
                SelectResult::Failure(i, FalseLit, Ud(LuaRuntimeError::new(err.clone())))
            }
            TaskResult::Cancelled => SelectResult::Cancelled(i, FalseLit, "task cancelled".into()),
            TaskResult::Aborted => SelectResult::Aborted(i, FalseLit, "task aborted".into()),
        }
    }
}

// ---------------------------------------------------------------------------
// LuaTask userdata
// ---------------------------------------------------------------------------

/// Userdata returned by `task.spawn`.  Holds the join handle for
/// the spawned tokio task plus the shared private `TaskState`.
pub struct LuaTask {
    pub(crate) state: Arc<TaskState>,
    pub(crate) join_handle: Mutex<Option<JoinHandle<()>>>,
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
    async fn cancel(self: Arc<Self>) -> Result<(), VmError> {
        self.state.consumed.store(true, Ordering::SeqCst);
        self.state.cancel.notify_one();
        let _ = self.state.wait().await;
        Ok(())
    }

    /// Abort the task immediately.  `<close>` / `__close` handlers
    /// do **not** run.  Resolves once the underlying tokio task
    /// has been dropped.
    #[lua_method]
    async fn abort(self: Arc<Self>) -> Result<(), VmError> {
        self.state.consumed.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join_handle.lock().take() {
            handle.abort();
        }
        let _ = self.state.wait().await;
        Ok(())
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
