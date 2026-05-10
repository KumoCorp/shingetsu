//! `LuaTaskSet` userdata — FuturesUnordered-shaped completion stream.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex as AsyncMutex;

use crate::{Bytes, Ud, Variadic};

use super::lua_task::{FalseLit, LuaTask, TrueLit};
use super::runtime_error::LuaRuntimeError;
use super::TaskResult;

/// Return shape for `TaskSet:next()`: the task that completed
/// followed by the same `(true, ...)` / `(false, err)` shape as
/// `Task:pawait()`, or `nil` when the set is empty.
#[derive(crate::IntoLuaMulti)]
pub(crate) enum NextResult {
    Empty,
    Success(Ud<LuaTask>, TrueLit, Variadic),
    Failure(Ud<LuaTask>, FalseLit, Ud<LuaRuntimeError>),
    Cancelled(Ud<LuaTask>, FalseLit, Bytes),
    Aborted(Ud<LuaTask>, FalseLit, Bytes),
}

impl NextResult {
    fn from_completion(task: Arc<LuaTask>, r: &TaskResult) -> Self {
        let ud = Ud(task);
        match r {
            TaskResult::Success(vs) => NextResult::Success(ud, TrueLit, Variadic(vs.clone())),
            TaskResult::Failure(err) => {
                NextResult::Failure(ud, FalseLit, Ud(LuaRuntimeError::new(err.clone())))
            }
            TaskResult::Cancelled => NextResult::Cancelled(ud, FalseLit, "task cancelled".into()),
            TaskResult::Aborted => NextResult::Aborted(ud, FalseLit, "task aborted".into()),
        }
    }
}

/// A set of in-progress tasks that yields each completion exactly
/// once via `:next()`, in the order they finish.  The recommended
/// shape for fan-out workloads where you want to handle each
/// task's result as soon as it's ready, and where new tasks may
/// be added to the set dynamically.
///
/// Implementation: each task added to the set spawns a tiny
/// watcher tokio task that awaits the task's completion notify
/// and forwards `(task, result)` over an unbounded mpsc channel.
/// `:next()` reads from the channel.  This sidesteps the need to
/// hold a `FuturesUnordered` mutex across `.await` points.
pub struct LuaTaskSet {
    tx: UnboundedSender<(Arc<LuaTask>, Arc<TaskResult>)>,
    rx: AsyncMutex<UnboundedReceiver<(Arc<LuaTask>, Arc<TaskResult>)>>,
    /// Number of tasks added but not yet returned by `:next()`.
    /// Used to short-circuit `:next()` when the set is empty
    /// without blocking on the channel.
    in_flight: AtomicUsize,
}

impl LuaTaskSet {
    pub(crate) fn new() -> Arc<Self> {
        let (tx, rx) = unbounded_channel();
        Arc::new(Self {
            tx,
            rx: AsyncMutex::new(rx),
            in_flight: AtomicUsize::new(0),
        })
    }

    /// Shared core for `:next()` and the `__call` metamethod.
    /// Returns `NextResult::Empty` if no tasks are in flight,
    /// otherwise blocks until any task in the set finishes and
    /// returns its outcome.
    async fn next_inner(self: Arc<Self>) -> NextResult {
        if self.in_flight.load(Ordering::SeqCst) == 0 {
            return NextResult::Empty;
        }
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some((task, result)) => {
                self.in_flight.fetch_sub(1, Ordering::SeqCst);
                NextResult::from_completion(task, &result)
            }
            None => NextResult::Empty,
        }
    }

    /// Spawn a watcher that awaits `task`'s completion and pushes
    /// the outcome to the channel.  Marks the task consumed so
    /// `on_handle_abandoned` doesn't fire when the set drops the
    /// last reference — the user has handed it to the set,
    /// committing to receive its result via `:next()`.
    pub(crate) fn watch(&self, task: Arc<LuaTask>) {
        task.state.consumed.store(true, Ordering::SeqCst);
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        let tx = self.tx.clone();
        let task_for_watcher = task.clone();
        tokio::spawn(async move {
            let r = task_for_watcher.state.wait().await;
            // Best-effort send: if the receiver was dropped (set
            // dropped before all completions were consumed) the
            // task itself continues running independently and
            // its observer events still fire normally.
            let _ = tx.send((task_for_watcher, r));
        });
        // Keep `task` alive across the spawn closure construction.
        drop(task);
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "TaskSet", index_fallback = "nil")]
impl LuaTaskSet {
    /// Add a task to the set.  Marks it consumed so the abandoned
    /// observer does not fire — the caller is committing to
    /// collect the result via `:next()`.
    #[lua_method]
    fn add(self: Arc<Self>, task: Ud<LuaTask>) {
        self.watch(task.0);
    }

    /// Block until any task in the set finishes, then return
    /// `(task, true, ...results)` on success or
    /// `(task, false, err)` on failure / cancel / abort.  Returns
    /// `nil` when the set is empty.
    ///
    /// Concurrent callers are serialised on an internal lock; each
    /// call returns a different completion in arrival order.
    #[lua_method]
    async fn next(self: Arc<Self>) -> NextResult {
        self.next_inner().await
    }

    /// `__call` lets the userdata itself act as the iterator in a
    /// generic `for ... in task_set do` loop: each iteration
    /// produces the next completion as `(task, ok, ...results)`,
    /// or terminates when the set is empty.  The state and control
    /// arguments Lua threads through generic-for are ignored
    /// because the iterator state lives entirely on the userdata.
    ///
    /// For-loop binders capture leading results positionally:
    ///
    /// ```lua
    /// for task, ok, val in set do      -- 0/1-return tasks
    /// for task, ok, a, b in set do     -- 2-return tasks
    /// ```
    ///
    /// For variable-arity tasks, prefer `:next()` with explicit
    /// handling.
    #[lua_metamethod(Call)]
    async fn iter(self: Arc<Self>, _args: Variadic) -> NextResult {
        self.next_inner().await
    }

    /// Number of tasks added but not yet returned by `:next()`.
    #[lua_method]
    fn len(self: Arc<Self>) -> i64 {
        self.in_flight.load(Ordering::SeqCst) as i64
    }

    /// Whether the set has no remaining unconsumed tasks.
    #[lua_method]
    fn is_empty(self: Arc<Self>) -> bool {
        self.in_flight.load(Ordering::SeqCst) == 0
    }
}
