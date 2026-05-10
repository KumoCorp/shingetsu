//! `task.rwlock` — async, cross-thread reader-writer lock.

use std::sync::Arc;

use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock as AsyncRwLock};

use crate::sync::Mutex;
use crate::{valuevec, Ud, Value, Variadic, VmError};

/// Async, cross-thread reader-writer lock exposed to Lua as
/// `task.rwlock()`.
///
/// Wraps a `tokio::sync::RwLock<()>`.  Fairness follows tokio's
/// implementation: writers are preferred to avoid writer starvation
/// under sustained read load.  Both read and write guards may be held
/// across `await` points.
pub struct LuaRwLock {
    inner: Arc<AsyncRwLock<()>>,
}

impl Default for LuaRwLock {
    fn default() -> Self {
        Self {
            inner: Arc::new(AsyncRwLock::new(())),
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "RwLock", index_fallback = "nil")]
impl LuaRwLock {
    /// Acquire a shared read guard, awaiting if a writer holds the
    /// lock.  Multiple read guards can coexist.
    #[lua_method]
    async fn read(self: Arc<Self>) -> Result<Ud<LuaRwLockReadGuard>, VmError> {
        let permit = self.inner.clone().read_owned().await;
        Ok(Ud(Arc::new(LuaRwLockReadGuard {
            inner: Mutex::new(Some(permit)),
        })))
    }

    /// Try to acquire a shared read guard without awaiting.  Returns
    /// `nil` if a writer holds the lock.
    #[lua_method]
    fn try_read(self: Arc<Self>) -> Option<Ud<LuaRwLockReadGuard>> {
        let permit = self.inner.clone().try_read_owned().ok()?;
        Some(Ud(Arc::new(LuaRwLockReadGuard {
            inner: Mutex::new(Some(permit)),
        })))
    }

    /// Acquire an exclusive write guard, awaiting if any reader or
    /// writer holds the lock.
    #[lua_method]
    async fn write(self: Arc<Self>) -> Result<Ud<LuaRwLockWriteGuard>, VmError> {
        let permit = self.inner.clone().write_owned().await;
        Ok(Ud(Arc::new(LuaRwLockWriteGuard {
            inner: Mutex::new(Some(permit)),
        })))
    }

    /// Try to acquire an exclusive write guard without awaiting.
    /// Returns `nil` if any reader or writer holds the lock.
    #[lua_method]
    fn try_write(self: Arc<Self>) -> Option<Ud<LuaRwLockWriteGuard>> {
        let permit = self.inner.clone().try_write_owned().ok()?;
        Some(Ud(Arc::new(LuaRwLockWriteGuard {
            inner: Mutex::new(Some(permit)),
        })))
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        Variadic(valuevec![Value::string("RwLock")])
    }
}

/// Read guard returned by `LuaRwLock:read()` / `:try_read()`.  See
/// `LuaMutexGuard` for the wrapper rationale.
pub struct LuaRwLockReadGuard {
    inner: Mutex<Option<OwnedRwLockReadGuard<()>>>,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "RwLockReadGuard", index_fallback = "nil")]
impl LuaRwLockReadGuard {
    /// Release the read lock early.  Calling on an already-released
    /// guard raises an error.
    #[lua_method]
    fn unlock(self: Arc<Self>) -> Result<(), VmError> {
        let mut g = self.inner.lock();
        if g.is_none() {
            return Err(VmError::LuaError {
                display: "rwlock read guard has already been released".to_owned(),
                value: Value::string("rwlock read guard has already been released"),
            });
        }
        *g = None;
        Ok(())
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = if self.inner.lock().is_some() {
            "RwLockReadGuard (held)"
        } else {
            "RwLockReadGuard (released)"
        };
        Variadic(valuevec![Value::string(s)])
    }
}

/// Write guard returned by `LuaRwLock:write()` / `:try_write()`.
pub struct LuaRwLockWriteGuard {
    inner: Mutex<Option<OwnedRwLockWriteGuard<()>>>,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "RwLockWriteGuard", index_fallback = "nil")]
impl LuaRwLockWriteGuard {
    /// Release the write lock early.  Calling on an already-released
    /// guard raises an error.
    #[lua_method]
    fn unlock(self: Arc<Self>) -> Result<(), VmError> {
        let mut g = self.inner.lock();
        if g.is_none() {
            return Err(VmError::LuaError {
                display: "rwlock write guard has already been released".to_owned(),
                value: Value::string("rwlock write guard has already been released"),
            });
        }
        *g = None;
        Ok(())
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = if self.inner.lock().is_some() {
            "RwLockWriteGuard (held)"
        } else {
            "RwLockWriteGuard (released)"
        };
        Variadic(valuevec![Value::string(s)])
    }
}
