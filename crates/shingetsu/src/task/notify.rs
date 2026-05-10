//! `task.notify` — edge-triggered async notification primitive.

use std::sync::Arc;

use tokio::sync::Notify;

use crate::{valuevec, CallContext, Function, Value, Variadic, VmError};

/// Edge-triggered async notification primitive exposed to Lua as
/// `task.notify()`.  Wraps `tokio::sync::Notify`.
///
/// Two waiting forms are exposed:
///
/// * `:wait_notified()` — await any notification.  Use only when the
///   caller has already established the predicate they're waiting on
///   in a way that's safe against lost wakeups.
/// * `:wait_until(predicate)` — register interest, check predicate,
///   await on miss, recheck on wake, loop.  This is the recommended
///   form because the register-before-check ordering (via tokio's
///   `Notified::enable`) makes the classic lost-wakeup bug not
///   expressible.
pub struct LuaNotify {
    inner: Arc<Notify>,
}

impl Default for LuaNotify {
    fn default() -> Self {
        Self {
            inner: Arc::new(Notify::new()),
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "Notify", index_fallback = "nil")]
impl LuaNotify {
    /// Wake the longest-waiting waiter (FIFO).  If no waiter is
    /// currently registered, the next caller to register a wait will
    /// see the notification (one permit is stored).
    #[lua_method]
    fn notify_one(self: Arc<Self>) {
        self.inner.notify_one();
    }

    /// Wake the most-recently-arrived waiter (LIFO).  Like
    /// `:notify_one`, stores a permit if no waiter is currently
    /// registered.
    #[lua_method]
    fn notify_last(self: Arc<Self>) {
        self.inner.notify_last();
    }

    /// Wake every currently-registered waiter.  Does not store a
    /// permit; a future waiter that registers after this call will
    /// wait for the next notification.
    #[lua_method]
    fn notify_all(self: Arc<Self>) {
        self.inner.notify_waiters();
    }

    /// Await any notification.  Lower-level form: callers who need
    /// lost-wakeup safety should prefer `:wait_until(predicate)`.
    #[lua_method]
    async fn wait_notified(self: Arc<Self>) {
        self.inner.notified().await;
    }

    /// Await until `predicate()` returns truthy.  Registers interest
    /// in the next notification *before* evaluating the predicate, so
    /// a notification raced between predicate evaluation and the next
    /// await is not lost.
    #[lua_method]
    async fn wait_until(
        self: Arc<Self>,
        ctx: CallContext,
        predicate: Function,
    ) -> Result<(), VmError> {
        loop {
            // Pin the notified future and enable it (registers a
            // waiter slot without parking).  Any notification from
            // this point on will satisfy this future.
            let notified = self.inner.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            // Now check the predicate.  If it was already true (or
            // becomes true between our enable and the call), return.
            let results = ctx
                .call_function(predicate.clone(), valuevec![])
                .await
                .map_err(|re| re.error)?;
            let truthy = results.first().map(|v| v.is_truthy()).unwrap_or(false);
            if truthy {
                return Ok(());
            }

            // Predicate is false; await the next notify.  Because
            // `enable` was called before the predicate check, a
            // notification that arrived during the check has already
            // armed this future and the await returns immediately.
            notified.await;
        }
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string("Notify")])
    }
}
