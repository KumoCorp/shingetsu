//! `task.bounded_channel` and `task.unbounded_channel` — async
//! mpsc-style channels with snapshot-validated payloads.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tokio::sync::mpsc::error::{TryRecvError, TrySendError};
use tokio::sync::mpsc::{
    channel as bounded_channel_pair, unbounded_channel, Receiver, Sender, UnboundedReceiver,
    UnboundedSender,
};
use tokio::sync::Mutex as AsyncMutex;

use crate::sync::Mutex as SyncMutex;
use crate::{valuevec, CallContext, SnapshotValue, Value, Variadic, VmError};

/// Bounded async channel exposed to Lua as `task.bounded_channel(cap)`.
///
/// Wraps `tokio::sync::mpsc::channel`.  Senders are cheap to clone
/// internally so any number of Lua tasks can `:send` concurrently.
/// The receiver is wrapped in `tokio::sync::Mutex` so that multiple
/// concurrent `:recv` callers are serialized; each value is delivered
/// to exactly one consumer.  Values transit as `SnapshotValue` so
/// they can safely cross VM boundaries.
///
/// `last_requested` records the most recent capacity value passed
/// to the constructor for this entry, used to suppress duplicate
/// capacity-mismatch warnings from a busy reload path that
/// repeatedly asks for the same (already-warned) value.
pub struct LuaBoundedChannel {
    sender: SyncMutex<Option<Sender<SnapshotValue>>>,
    receiver: AsyncMutex<Receiver<SnapshotValue>>,
    pub(crate) capacity: usize,
    pub(crate) last_requested: AtomicUsize,
}

impl LuaBoundedChannel {
    pub(crate) fn new(capacity: usize) -> Self {
        let (sender, receiver) = bounded_channel_pair(capacity);
        Self {
            sender: SyncMutex::new(Some(sender)),
            receiver: AsyncMutex::new(receiver),
            capacity,
            last_requested: AtomicUsize::new(capacity),
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "BoundedChannel", index_fallback = "nil")]
impl LuaBoundedChannel {
    /// Send a value, awaiting if the channel is full.  Raises if
    /// the channel has been closed.  The value is snapshotted before
    /// being placed in the channel; non-snapshottable inputs are
    /// rejected with an error during argument extraction.
    #[lua_method]
    async fn send(self: Arc<Self>, value: SnapshotValue) -> Result<(), VmError> {
        let sender = self.sender.lock().clone();
        let Some(sender) = sender else {
            return Err(VmError::LuaError {
                display: "channel is closed".to_owned(),
                value: Value::string("channel is closed"),
            });
        };
        sender.send(value).await.map_err(|_| VmError::LuaError {
            display: "channel is closed".to_owned(),
            value: Value::string("channel is closed"),
        })
    }

    /// Try to send a value without awaiting.  Returns `true` on
    /// success or `false` if the channel is full.  Raises if the
    /// channel has been closed.
    #[lua_method]
    fn try_send(self: Arc<Self>, value: SnapshotValue) -> Result<bool, VmError> {
        let sender = self.sender.lock().clone();
        let Some(sender) = sender else {
            return Err(VmError::LuaError {
                display: "channel is closed".to_owned(),
                value: Value::string("channel is closed"),
            });
        };
        match sender.try_send(value) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Closed(_)) => Err(VmError::LuaError {
                display: "channel is closed".to_owned(),
                value: Value::string("channel is closed"),
            }),
        }
    }

    /// Receive a value, awaiting until one is available.  Returns
    /// `nil` once the channel has been closed and drained.  For
    /// `Map` / `Vec` payloads the returned value is a read-only
    /// snapshot proxy; pass through `task.materialize` for a
    /// mutable Lua table copy.
    #[lua_method]
    async fn recv(self: Arc<Self>, ctx: CallContext) -> Result<Value, VmError> {
        let mut rx = self.receiver.lock().await;
        match rx.recv().await {
            Some(snap) => snap.rebuild_lazy(&ctx.global),
            None => Ok(Value::Nil),
        }
    }

    /// Try to receive a value without awaiting.  Returns the value
    /// on success, or `nil` if the channel is empty or closed.
    #[lua_method]
    fn try_recv(self: Arc<Self>, ctx: CallContext) -> Result<Value, VmError> {
        let mut rx = match self.receiver.try_lock() {
            Ok(g) => g,
            Err(_) => return Ok(Value::Nil),
        };
        match rx.try_recv() {
            Ok(snap) => snap.rebuild_lazy(&ctx.global),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => Ok(Value::Nil),
        }
    }

    /// Close the channel.  Subsequent `:send` / `:try_send` calls
    /// fail; pending and future `:recv` calls drain remaining
    /// values and then return `nil`.  Idempotent.
    ///
    /// Implemented by dropping the held `Sender`.  When the
    /// underlying tokio channel sees its last `Sender` go away it
    /// wakes any parked receiver, which then drains buffered
    /// values and returns `None`.  This path deliberately does
    /// **not** acquire the receiver's async mutex: doing so would
    /// deadlock against an in-flight `recv` that holds the lock
    /// across `rx.recv().await`.
    #[lua_method]
    fn close(self: Arc<Self>) {
        let _drop_sender = self.sender.lock().take();
    }

    /// Returns `true` once the channel has been closed (no further
    /// sends will succeed).
    #[lua_method]
    fn is_closed(self: Arc<Self>) -> bool {
        self.sender.lock().is_none()
    }

    /// Configured capacity at construction.
    #[lua_method]
    fn capacity(self: Arc<Self>) -> i64 {
        self.capacity as i64
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = format!("BoundedChannel (capacity={})", self.capacity);
        Variadic(valuevec![Value::string(s)])
    }
}

/// Unbounded async channel exposed to Lua as `task.unbounded_channel()`.
///
/// Same shape as `LuaBoundedChannel` but `:send` never awaits.
/// Without backpressure, a fast producer can grow the channel
/// queue without bound; reach for the bounded variant when the
/// producer cannot afford to outpace the consumer indefinitely.
///
/// `sender` is held inside an `Option` for the same reason as
/// [`LuaBoundedChannel`]: `close` takes the sender out so the
/// underlying tokio channel notices the last sender went away and
/// wakes any parked receiver, without needing the receiver's
/// async mutex.
pub struct LuaUnboundedChannel {
    sender: SyncMutex<Option<UnboundedSender<SnapshotValue>>>,
    receiver: AsyncMutex<UnboundedReceiver<SnapshotValue>>,
}

impl Default for LuaUnboundedChannel {
    fn default() -> Self {
        let (sender, receiver) = unbounded_channel();
        Self {
            sender: SyncMutex::new(Some(sender)),
            receiver: AsyncMutex::new(receiver),
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "UnboundedChannel", index_fallback = "nil")]
impl LuaUnboundedChannel {
    /// Send a value.  Never awaits; raises if the channel is closed.
    #[lua_method]
    fn send(self: Arc<Self>, value: SnapshotValue) -> Result<(), VmError> {
        let sender = self.sender.lock().clone();
        let Some(sender) = sender else {
            return Err(VmError::LuaError {
                display: "channel is closed".to_owned(),
                value: Value::string("channel is closed"),
            });
        };
        sender.send(value).map_err(|_| VmError::LuaError {
            display: "channel is closed".to_owned(),
            value: Value::string("channel is closed"),
        })
    }

    /// Receive a value, awaiting until one is available.  Returns
    /// `nil` once the channel has been closed and drained.  For
    /// `Map` / `Vec` payloads the returned value is a read-only
    /// snapshot proxy; pass through `task.materialize` for a
    /// mutable Lua table copy.
    #[lua_method]
    async fn recv(self: Arc<Self>, ctx: CallContext) -> Result<Value, VmError> {
        let mut rx = self.receiver.lock().await;
        match rx.recv().await {
            Some(snap) => snap.rebuild_lazy(&ctx.global),
            None => Ok(Value::Nil),
        }
    }

    /// Try to receive a value without awaiting.  Returns the value
    /// on success, or `nil` if the channel is empty or closed.
    #[lua_method]
    fn try_recv(self: Arc<Self>, ctx: CallContext) -> Result<Value, VmError> {
        let mut rx = match self.receiver.try_lock() {
            Ok(g) => g,
            Err(_) => return Ok(Value::Nil),
        };
        match rx.try_recv() {
            Ok(snap) => snap.rebuild_lazy(&ctx.global),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => Ok(Value::Nil),
        }
    }

    /// Close the channel.  See `LuaBoundedChannel:close`.
    #[lua_method]
    fn close(self: Arc<Self>) {
        let _drop_sender = self.sender.lock().take();
    }

    /// Returns `true` once the channel has been closed.
    #[lua_method]
    fn is_closed(self: Arc<Self>) -> bool {
        self.sender.lock().is_none()
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string("UnboundedChannel")])
    }
}
