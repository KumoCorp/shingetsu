//! `task.oneshot` — single-shot async value handoff.

use std::sync::Arc;

use crate::sync::Mutex;
use crate::{valuevec, Value, Variadic, VmError};

/// Sender half of a oneshot channel created by `task.oneshot()`.
///
/// Wraps `tokio::sync::oneshot::Sender<Value>`.  `Value` (not
/// [`crate::SnapshotValue`]) is safe here because oneshot is
/// anonymous-only — there is no shared registry, so the channel
/// cannot escape its creating [`crate::GlobalEnv`] and the producer's
/// tables can be shared with the consumer by `Arc` clone.
///
/// The sender is consumed by `:send` or `:close`; subsequent calls
/// raise.
pub struct LuaOneshotSender {
    pub(crate) inner: Mutex<Option<tokio::sync::oneshot::Sender<Value>>>,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "OneshotSender", index_fallback = "nil")]
impl LuaOneshotSender {
    /// Send `value` to the receiver.  Consumes the sender.  Raises
    /// if the sender has already been consumed (by a prior `:send`
    /// or `:close`), or if the receiver was already dropped.
    #[lua_method]
    fn send(self: Arc<Self>, value: Value) -> Result<(), VmError> {
        let tx = self.inner.lock().take().ok_or_else(|| VmError::LuaError {
            display: "oneshot sender has already been consumed".to_owned(),
            value: Value::string("oneshot sender has already been consumed"),
        })?;
        tx.send(value).map_err(|_| VmError::LuaError {
            display: "oneshot receiver was dropped before send".to_owned(),
            value: Value::string("oneshot receiver was dropped before send"),
        })
    }

    /// Close the sender without delivering a value, waking the
    /// receiver with `nil`.  Idempotent: subsequent `:close` calls
    /// are no-ops.  Calling `:send` after `:close` raises.
    #[lua_method]
    fn close(self: Arc<Self>) {
        self.inner.lock().take();
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = if self.inner.lock().is_some() {
            "OneshotSender (live)"
        } else {
            "OneshotSender (consumed)"
        };
        Variadic(valuevec![Value::string(s)])
    }
}

/// Receiver half of a oneshot channel created by `task.oneshot()`.
///
/// Consumed by the first `:recv`; subsequent calls raise.  Returns
/// `nil` if the sender was dropped or `:close`d without sending.
pub struct LuaOneshotReceiver {
    pub(crate) inner: Mutex<Option<tokio::sync::oneshot::Receiver<Value>>>,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "OneshotReceiver", index_fallback = "nil")]
impl LuaOneshotReceiver {
    /// Await the value from the paired sender.  Returns the value
    /// on `:send`, or `nil` if the sender was dropped or `:close`d
    /// without sending.  Consumes the receiver; subsequent `:recv`
    /// calls raise.
    #[lua_method]
    async fn recv(self: Arc<Self>) -> Result<Value, VmError> {
        let rx = self.inner.lock().take().ok_or_else(|| VmError::LuaError {
            display: "oneshot receiver has already been consumed".to_owned(),
            value: Value::string("oneshot receiver has already been consumed"),
        })?;
        match rx.await {
            Ok(v) => Ok(v),
            // Sender was dropped or closed without sending.
            Err(_) => Ok(Value::Nil),
        }
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = if self.inner.lock().is_some() {
            "OneshotReceiver (live)"
        } else {
            "OneshotReceiver (consumed)"
        };
        Variadic(valuevec![Value::string(s)])
    }
}
