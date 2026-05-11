//! `task.watch` — state cell with change notification.

use std::sync::Arc;

use crate::{valuevec, CallContext, Function, SnapshotValue, Value, Variadic, VmError};

/// State cell with change notification, exposed to Lua as `task.watch()`.
///
/// Wraps a `tokio::sync::watch::Sender<SnapshotValue>`.  Values are
/// snapshotted at `:set` time and rebuilt fresh per consumer at
/// `:get` / `:wait_change` / `:wait_for`, so consumers cannot alias
/// each other or mutate the producer's state.
pub struct LuaWatch {
    sender: Arc<tokio::sync::watch::Sender<SnapshotValue>>,
}

impl LuaWatch {
    pub(crate) fn new(initial: SnapshotValue) -> Self {
        let (sender, _) = tokio::sync::watch::channel(initial);
        Self {
            sender: Arc::new(sender),
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "Watch", index_fallback = "nil")]
impl LuaWatch {
    /// Get a lazy view of the current value.  For `Map` and `Vec`
    /// captures, this returns a read-only userdata proxy that
    /// rebuilds values on the fly as they are accessed; pass through
    /// `task.materialize` to obtain a fresh mutable Lua table.
    #[lua_method]
    fn get(self: Arc<Self>, ctx: CallContext) -> Result<Value, VmError> {
        let snap = self.sender.borrow().clone();
        snap.rebuild_lazy(&ctx.global)
    }

    /// Publish a new value.  The value is snapshot-validated during
    /// argument extraction; any non-snapshottable input (function,
    /// opted-out userdata, cyclic table, table with non-int/string
    /// keys) is rejected with an error.  All current and future
    /// waiters are notified of the change.
    #[lua_method]
    fn set(self: Arc<Self>, value: SnapshotValue) {
        // `send_replace` updates the stored value unconditionally,
        // even when there are no current receivers.  Plain `send`
        // returns the value in `SendError` and leaves the stored
        // value untouched in that case.
        self.sender.send_replace(value);
    }

    /// Await the next change to the watch's value, returning a
    /// lazy view of the new value.  Edge-triggered: only changes
    /// that occur after this call begins are observed.  Use
    /// `:wait_for(predicate)` for race-free condition waiting.
    #[lua_method]
    async fn wait_change(self: Arc<Self>, ctx: CallContext) -> Result<Value, VmError> {
        let mut rx = self.sender.subscribe();
        rx.mark_unchanged();
        if rx.changed().await.is_err() {
            return Err(VmError::LuaError {
                display: "watch sender dropped".to_owned(),
                value: Value::string("watch sender dropped"),
            });
        }
        let snap = rx.borrow().clone();
        snap.rebuild_lazy(&ctx.global)
    }

    /// Await until `predicate(current_value)` returns truthy and
    /// return that value.  Re-checks on every change; uses
    /// `borrow_and_update` so each iteration awaits the *next*
    /// change rather than re-firing on the just-checked version.
    #[lua_method]
    async fn wait_for(
        self: Arc<Self>,
        ctx: CallContext,
        predicate: Function,
    ) -> Result<Value, VmError> {
        let mut rx = self.sender.subscribe();
        loop {
            let snap = rx.borrow_and_update().clone();
            let val = snap.rebuild_lazy(&ctx.global)?;
            let results = ctx
                .call_function(predicate.clone(), valuevec![val.clone()])
                .await
                .map_err(|re| re.error)?;
            if results.first().map(|v| v.is_truthy()).unwrap_or(false) {
                return Ok(val);
            }
            if rx.changed().await.is_err() {
                return Err(VmError::LuaError {
                    display: "watch sender dropped".to_owned(),
                    value: Value::string("watch sender dropped"),
                });
            }
        }
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string("Watch")])
    }
}

/// Compute the initial value for `task.watch` from a `Value` that
/// is either a snapshottable value or a zero-arg function returning
/// one.
pub(crate) async fn compute_initial(
    ctx: &CallContext,
    initial: &Value,
) -> Result<SnapshotValue, VmError> {
    use shingetsu_vm::convert::FromLua;
    let value = match initial {
        Value::Function(f) => {
            let results = ctx
                .call_function(f.clone(), valuevec![])
                .await
                .map_err(|re| re.error)?;
            results.into_iter().next().unwrap_or(Value::Nil)
        }
        v => v.clone(),
    };
    SnapshotValue::from_lua(value)
}
