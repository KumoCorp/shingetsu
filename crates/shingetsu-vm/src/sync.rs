//! Lock primitives whose guards are deliberately `!Send`.
//!
//! [`Mutex`] and [`RwLock`] wrap the corresponding `parking_lot`
//! types and forward to their fast paths.  The only difference is
//! a zero-sized marker on each guard that opts the guard out of
//! [`Send`].
//!
//! # Why
//!
//! Holding a synchronous lock across an `.await` keeps the lock for
//! as long as the future takes to resolve, blocking every other
//! task waiting for it and pinning the executor thread.  Bare
//! `parking_lot::MutexGuard` is `Send`, so this mistake compiles
//! cleanly and only surfaces under load.
//!
//! Shingetsu's async natives are stored as
//! `BoxFuture<'static, ... + Send>` (see
//! [`NativeCall::Async`](crate::function::NativeCall::Async)).  When
//! a native or userdata method holds a guard from this module
//! across an `.await`, the resulting future is `!Send`, the
//! coercion to `BoxFuture<... + Send>` fails, and the compiler
//! points at the offending site.
//!
//! Use these in preference to `parking_lot::Mutex` / `RwLock`
//! anywhere a held guard might cross an `.await` boundary, even
//! indirectly.  For state that genuinely needs to stay locked
//! across `.await`, reach for `tokio::sync::Mutex` / `RwLock`
//! instead — those guards are designed to span suspension.
//!
//! # Behaviour
//!
//! Identical to `parking_lot` underneath: no poisoning, fast
//! uncontended path, the same memory layout for the lock itself.
//! The wrapper guards forward [`Deref`] and [`DerefMut`]; the
//! lock cooperates with [`Default`] and [`Debug`] when `T` does.

use parking_lot;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

// `*const ()` is `!Send + !Sync`; using it as a `PhantomData` makes
// the enclosing guard inherit those negative auto-traits.  We
// deliberately drop `Sync` along with `Send`: holding a `&Guard`
// across threads is rare in practice, and the simpler marker keeps
// the surface area minimal.
type NotSendSync = PhantomData<*const ()>;

/// A mutual-exclusion lock whose guard is `!Send`.
///
/// Drop-in for `parking_lot::Mutex` for everything except holding
/// a guard across an `.await`, which is exactly the case this
/// type exists to forbid at compile time.
///
/// ```compile_fail
/// use shingetsu_vm::sync::Mutex;
///
/// fn require_send<T: Send>(_: T) {}
///
/// async fn holds_guard_across_await() {
///     let m = Mutex::new(0i32);
///     let g = m.lock();
///     tokio::task::yield_now().await;
///     let _ = *g;
/// }
///
/// fn check() {
///     // Fails to compile: the future is not `Send` because `g`
///     // is held across the await.
///     require_send(holds_guard_across_await());
/// }
/// ```
pub struct Mutex<T: ?Sized>(parking_lot::Mutex<T>);

impl<T> Mutex<T> {
    /// Construct a new mutex, exactly as `parking_lot::Mutex::new`.
    #[inline]
    pub const fn new(value: T) -> Self {
        Self(parking_lot::Mutex::new(value))
    }
}

impl<T: ?Sized> Mutex<T> {
    /// Acquire the lock, blocking until it is available.
    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        MutexGuard {
            inner: self.0.lock(),
            _not_send: PhantomData,
        }
    }

    /// Try to acquire the lock without blocking.  Returns `None`
    /// if the lock is currently held.
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        self.0.try_lock().map(|inner| MutexGuard {
            inner,
            _not_send: PhantomData,
        })
    }

    /// Get a mutable reference to the inner value.  Does not need
    /// to acquire the lock — exclusive access is statically known.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}

impl<T> Mutex<T> {
    /// Consume the mutex and return the inner value.
    #[inline]
    pub fn into_inner(self) -> T {
        self.0.into_inner()
    }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Self(parking_lot::Mutex::new(T::default()))
    }
}

impl<T: fmt::Debug> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// RAII guard for [`Mutex`].  Releases the lock on drop.
///
/// Deliberately `!Send`: holding this across an `.await` produces
/// a `!Send` future, which fails to satisfy the `Send` bound on
/// shingetsu's async-native storage.
pub struct MutexGuard<'a, T: ?Sized> {
    inner: parking_lot::MutexGuard<'a, T>,
    _not_send: NotSendSync,
}

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.inner, f)
    }
}

/// A reader-writer lock whose read and write guards are `!Send`.
///
/// Drop-in for `parking_lot::RwLock`.  See [`Mutex`] for the
/// rationale and a `compile_fail` example.
pub struct RwLock<T: ?Sized>(parking_lot::RwLock<T>);

impl<T> RwLock<T> {
    /// Construct a new lock, exactly as `parking_lot::RwLock::new`.
    #[inline]
    pub const fn new(value: T) -> Self {
        Self(parking_lot::RwLock::new(value))
    }
}

impl<T: ?Sized> RwLock<T> {
    /// Acquire a shared read lock, blocking until available.
    #[inline]
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        RwLockReadGuard {
            inner: self.0.read(),
            _not_send: PhantomData,
        }
    }

    /// Try to acquire a shared read lock without blocking.
    #[inline]
    pub fn try_read(&self) -> Option<RwLockReadGuard<'_, T>> {
        self.0.try_read().map(|inner| RwLockReadGuard {
            inner,
            _not_send: PhantomData,
        })
    }

    /// Acquire an exclusive write lock, blocking until available.
    #[inline]
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        RwLockWriteGuard {
            inner: self.0.write(),
            _not_send: PhantomData,
        }
    }

    /// Try to acquire an exclusive write lock without blocking.
    #[inline]
    pub fn try_write(&self) -> Option<RwLockWriteGuard<'_, T>> {
        self.0.try_write().map(|inner| RwLockWriteGuard {
            inner,
            _not_send: PhantomData,
        })
    }

    /// Get a mutable reference to the inner value without locking.
    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.0.get_mut()
    }
}

impl<T> RwLock<T> {
    /// Consume the lock and return the inner value.
    #[inline]
    pub fn into_inner(self) -> T {
        self.0.into_inner()
    }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> Self {
        Self(parking_lot::RwLock::new(T::default()))
    }
}

impl<T: fmt::Debug> fmt::Debug for RwLock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// RAII guard for [`RwLock::read`].  Releases the read lock on drop.
pub struct RwLockReadGuard<'a, T: ?Sized> {
    inner: parking_lot::RwLockReadGuard<'a, T>,
    _not_send: NotSendSync,
}

impl<T: ?Sized> Deref for RwLockReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLockReadGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.inner, f)
    }
}

/// RAII guard for [`RwLock::write`].  Releases the write lock on drop.
pub struct RwLockWriteGuard<'a, T: ?Sized> {
    inner: parking_lot::RwLockWriteGuard<'a, T>,
    _not_send: NotSendSync,
}

impl<T: ?Sized> Deref for RwLockWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T: ?Sized> DerefMut for RwLockWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for RwLockWriteGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn mutex_is_send_and_sync() {
        assert_send::<Mutex<i32>>();
        assert_sync::<Mutex<i32>>();
    }

    #[test]
    fn rwlock_is_send_and_sync() {
        assert_send::<RwLock<i32>>();
        assert_sync::<RwLock<i32>>();
    }

    #[test]
    fn lock_and_use() {
        let m = Mutex::new(7);
        {
            let mut g = m.lock();
            *g = 9;
        }
        k9::assert_equal!(*m.lock(), 9);
    }

    #[test]
    fn try_lock_returns_none_when_held() {
        let m = Mutex::new(0);
        let _g = m.lock();
        assert!(m.try_lock().is_none());
    }

    #[test]
    fn rwlock_concurrent_reads() {
        let r = RwLock::new(42);
        let g1 = r.read();
        let g2 = r.read();
        k9::assert_equal!(*g1, 42);
        k9::assert_equal!(*g2, 42);
    }

    #[test]
    fn rwlock_write_excludes_read() {
        let r = RwLock::new(0);
        let _w = r.write();
        assert!(r.try_read().is_none());
    }
}
