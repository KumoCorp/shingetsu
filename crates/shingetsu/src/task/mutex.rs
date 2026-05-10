//! `task.mutex` — async, cross-thread mutual exclusion.

use std::sync::Arc;

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::sync::Mutex;
use crate::{valuevec, Ud, Value, Variadic, VmError};

/// Async, cross-thread mutual exclusion exposed to Lua as `task.mutex()`.
///
/// Wraps a `tokio::sync::Mutex<()>` (the `()` payload makes it a pure
/// signaling primitive; the value being protected lives in Lua and is
/// referenced from inside the critical section).  The guard returned by
/// `:lock()` may be held across `await` points in Lua-callable natives
/// because it wraps an `OwnedMutexGuard`, which is designed for that.
///
/// Identity for named mutexes is established by storing
/// `Arc<LuaMutex>` in the [`crate::SharedRegistry`]: every
/// `task.mutex("foo")` call returns the same `Arc`, so the same
/// underlying lock survives configuration reload.
pub struct LuaMutex {
    pub(crate) inner: Arc<AsyncMutex<()>>,
}

impl Default for LuaMutex {
    fn default() -> Self {
        Self {
            inner: Arc::new(AsyncMutex::new(())),
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "Mutex", index_fallback = "nil")]
impl LuaMutex {
    /// Acquire the lock, awaiting if it is currently held.  Returns
    /// a guard userdata; the lock is released when the guard's last
    /// reference is dropped (typically at end of the scope holding
    /// the local) or via `guard:unlock()`.
    #[lua_method]
    async fn lock(self: Arc<Self>) -> Result<Ud<LuaMutexGuard>, VmError> {
        let permit = self.inner.clone().lock_owned().await;
        Ok(Ud(Arc::new(LuaMutexGuard {
            inner: Mutex::new(Some(permit)),
        })))
    }

    /// Try to acquire the lock without awaiting.  Returns the guard
    /// on success or `nil` if the lock is currently held.
    #[lua_method]
    fn try_lock(self: Arc<Self>) -> Option<Ud<LuaMutexGuard>> {
        let permit = self.inner.clone().try_lock_owned().ok()?;
        Some(Ud(Arc::new(LuaMutexGuard {
            inner: Mutex::new(Some(permit)),
        })))
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string("Mutex")])
    }
}

/// Guard returned by `LuaMutex:lock()` / `:try_lock()`.
///
/// The inner `Option<OwnedMutexGuard<()>>` lets us release the lock
/// either via Rust `Drop` (when the last `Arc` clone of the guard
/// goes away) or via the explicit `:unlock()` method.  The outer
/// `shingetsu::sync::Mutex` is the brief swap lock; we never await
/// while holding it, and the `!Send` guard makes that a compile
/// error if we ever try.
pub struct LuaMutexGuard {
    inner: Mutex<Option<OwnedMutexGuard<()>>>,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "MutexGuard", index_fallback = "nil")]
impl LuaMutexGuard {
    /// Release the lock early, before the guard would otherwise be
    /// dropped at scope exit.  Calling on an already-released guard
    /// raises an error.
    #[lua_method]
    fn unlock(self: Arc<Self>) -> Result<(), VmError> {
        let mut g = self.inner.lock();
        if g.is_none() {
            return Err(VmError::LuaError {
                display: "mutex guard has already been released".to_owned(),
                value: Value::string("mutex guard has already been released"),
            });
        }
        *g = None;
        Ok(())
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = if self.inner.lock().is_some() {
            "MutexGuard (held)"
        } else {
            "MutexGuard (released)"
        };
        Variadic(valuevec![Value::string(s)])
    }
}
