//! `task.semaphore` — async, cross-thread counting semaphore.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore as AsyncSemaphore};

use crate::sync::Mutex;
use crate::{valuevec, Ud, Value, Variadic, VmError};

/// Counting semaphore exposed to Lua as `task.semaphore(permits)`.
///
/// Wraps a `tokio::sync::Semaphore`.  `permits` is the maximum number
/// of permits available; `:acquire()` awaits a permit and returns a
/// guard whose drop (or explicit `:release()`) returns the permit.
///
/// The configured maximum is tracked separately from tokio's
/// `available_permits()` (which fluctuates with held guards) so that a
/// named lookup can compare its requested count against the entry's
/// current configuration.  `last_requested` records the most recent
/// permit value passed to the constructor for this entry, used to
/// suppress duplicate shrink warnings from a busy reload path that
/// repeatedly asks for the same (already-warned) value.
pub struct LuaSemaphore {
    inner: Arc<AsyncSemaphore>,
    permits: AtomicUsize,
    pub(crate) last_requested: AtomicUsize,
}

impl LuaSemaphore {
    pub(crate) fn new(permits: usize) -> Self {
        Self {
            inner: Arc::new(AsyncSemaphore::new(permits)),
            permits: AtomicUsize::new(permits),
            last_requested: AtomicUsize::new(permits),
        }
    }

    pub(crate) fn configured_permits(&self) -> usize {
        self.permits.load(Ordering::Acquire)
    }

    /// Try to grow the configured permit count to `requested`.
    /// Returns `true` and calls `add_permits` if `requested` exceeds
    /// the current configuration; returns `false` otherwise (caller is
    /// already at or above `requested`).  Concurrent grow attempts
    /// race via CAS so the total grow is the maximum requested.
    pub(crate) fn try_grow_to(&self, requested: usize) -> bool {
        loop {
            let current = self.permits.load(Ordering::Acquire);
            if requested <= current {
                return false;
            }
            match self.permits.compare_exchange_weak(
                current,
                requested,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.inner.add_permits(requested - current);
                    return true;
                }
                Err(_) => continue,
            }
        }
    }
}

#[shingetsu_derive::userdata(crate = "crate", rename = "Semaphore", index_fallback = "nil")]
impl LuaSemaphore {
    /// Acquire a single permit, awaiting if none are available.
    /// Raises if the underlying semaphore has been closed.
    #[lua_method]
    async fn acquire(self: Arc<Self>) -> Result<Ud<LuaSemaphorePermit>, VmError> {
        let permit = self.inner.clone().acquire_owned().await.map_err(|e| {
            let msg = format!("semaphore closed: {e}");
            VmError::LuaError {
                display: msg.clone(),
                value: Value::string(msg),
            }
        })?;
        Ok(Ud(Arc::new(LuaSemaphorePermit {
            inner: Mutex::new(Some(permit)),
        })))
    }

    /// Try to acquire a permit without awaiting.  Returns the permit
    /// guard or `nil` if no permits are available.
    #[lua_method]
    fn try_acquire(self: Arc<Self>) -> Option<Ud<LuaSemaphorePermit>> {
        let permit = self.inner.clone().try_acquire_owned().ok()?;
        Some(Ud(Arc::new(LuaSemaphorePermit {
            inner: Mutex::new(Some(permit)),
        })))
    }

    /// Total permits currently configured.  May increase over the
    /// process lifetime if a later named lookup requested a higher
    /// count (configuration reload "grow" path); never decreases.
    #[lua_method]
    fn permits(self: Arc<Self>) -> i64 {
        self.configured_permits() as i64
    }

    /// Permits currently available (not held by any guard).
    #[lua_method]
    fn available(self: Arc<Self>) -> i64 {
        self.inner.available_permits() as i64
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = format!(
            "Semaphore (permits={}, available={})",
            self.configured_permits(),
            self.inner.available_permits(),
        );
        Variadic(valuevec![Value::string(s)])
    }
}

/// Permit guard returned by `LuaSemaphore:acquire()` /
/// `:try_acquire()`.
pub struct LuaSemaphorePermit {
    inner: Mutex<Option<OwnedSemaphorePermit>>,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "SemaphorePermit", index_fallback = "nil")]
impl LuaSemaphorePermit {
    /// Release the permit early.  Calling on an already-released
    /// permit raises an error.
    #[lua_method]
    fn release(self: Arc<Self>) -> Result<(), VmError> {
        let mut g = self.inner.lock();
        if g.is_none() {
            return Err(VmError::LuaError {
                display: "semaphore permit has already been released".to_owned(),
                value: Value::string("semaphore permit has already been released"),
            });
        }
        *g = None;
        Ok(())
    }

    #[lua_metamethod(ToString)]
    fn tostring(self: Arc<Self>) -> Variadic {
        let s = if self.inner.lock().is_some() {
            "SemaphorePermit (held)"
        } else {
            "SemaphorePermit (released)"
        };
        Variadic(valuevec![Value::string(s)])
    }
}
