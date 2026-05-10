//! Process-wide registry of named, typed, shareable values.
//!
//! The registry is the mechanism that lets named synchronization
//! primitives (and other host-shared resources) survive a Lua
//! configuration reload: the registry holds a strong reference to
//! every registered entry, and `get_or_create` returns the same
//! `Arc` to any caller that asks for the same name.
//!
//! # Default vs override
//!
//! [`global_shared_registry`] returns a process-wide
//! [`LazyLock`]-backed instance.  [`crate::GlobalEnv`] uses it as
//! its default backing store, so an embedder that does nothing
//! special still has a working registry.
//!
//! Embedders that need isolation (per-tenant scoping, test
//! fixtures, custom backing stores) install their own
//! [`SharedRegistry`] on a specific [`crate::GlobalEnv`] via
//! `install_shared_registry`.

use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, LazyLock};

use tokio::sync::Notify;

use crate::byte_string::Bytes;
use crate::sync::Mutex;

struct Entry {
    type_id: TypeId,
    type_name: &'static str,
    value: Arc<dyn Any + Send + Sync>,
}

/// Named map of `Arc<dyn Any + Send + Sync>` values.
///
/// `get_or_create` is the only mutating entry point.  Lookups
/// against an existing name require the requested type to match
/// the type the entry was originally created with; a mismatch is
/// reported as [`SharedRegistryError::TypeMismatch`] rather than
/// silently producing a different value.
pub struct SharedRegistry {
    entries: Mutex<HashMap<Bytes, Entry>>,
    /// Per-name in-flight markers used by [`Self::get_or_create_async`]
    /// to serialize concurrent async factory invocations.  Second and
    /// later concurrent callers for the same name await the marker's
    /// `Notify` rather than re-invoking the factory.
    in_flight: Mutex<HashMap<Bytes, Arc<Notify>>>,
}

impl SharedRegistry {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Look up `name`; create with `factory` if absent.
    ///
    /// On a hit, the existing `Arc<T>` is returned.  If the
    /// existing entry was registered under a different type, this
    /// returns [`SharedRegistryError::TypeMismatch`] with both
    /// the existing and requested type names, and `factory` is
    /// not invoked.
    ///
    /// On a miss, `factory` is invoked exactly once and the
    /// resulting value is stored.  Concurrent callers racing to
    /// create the same name see exactly one factory invocation;
    /// losers receive the winner's `Arc`.
    pub fn get_or_create<T, F>(
        &self,
        name: impl Into<Bytes>,
        factory: F,
    ) -> Result<Arc<T>, SharedRegistryError>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> T,
    {
        let name = name.into();
        let mut guard = self.entries.lock();
        if let Some(existing) = guard.get(&name) {
            if existing.type_id == TypeId::of::<T>() {
                return Ok(existing
                    .value
                    .clone()
                    .downcast::<T>()
                    .expect("TypeId match guarantees downcast succeeds"));
            }
            return Err(SharedRegistryError::TypeMismatch {
                name,
                existing_type: existing.type_name,
                requested_type: type_name::<T>(),
            });
        }
        let value: Arc<T> = Arc::new(factory());
        let any: Arc<dyn Any + Send + Sync> = value.clone();
        guard.insert(
            name,
            Entry {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                value: any,
            },
        );
        Ok(value)
    }

    /// Look up `name` and return the existing entry if any, without
    /// creating one.  Returns `Ok(None)` when no entry exists,
    /// `Ok(Some(arc))` on a type-matching hit, and
    /// [`SharedRegistryError::TypeMismatch`] when an entry exists
    /// under a different type.
    pub fn get<T: Any + Send + Sync>(
        &self,
        name: &Bytes,
    ) -> Result<Option<Arc<T>>, SharedRegistryError> {
        let guard = self.entries.lock();
        match guard.get(name) {
            None => Ok(None),
            Some(existing) if existing.type_id == TypeId::of::<T>() => Ok(Some(
                existing
                    .value
                    .clone()
                    .downcast::<T>()
                    .expect("TypeId match guarantees downcast succeeds"),
            )),
            Some(existing) => Err(SharedRegistryError::TypeMismatch {
                name: name.clone(),
                existing_type: existing.type_name,
                requested_type: type_name::<T>(),
            }),
        }
    }

    /// Async-aware get-or-create.  Use when `factory` itself needs
    /// to await (e.g. invokes a Lua callback).  Concurrent callers
    /// for the same `name` are serialized: the first to arrive runs
    /// the factory, others await its completion and then see the
    /// resulting entry.
    ///
    /// The factory error type `E` is generic so callers can
    /// surface arbitrary error info (e.g. `VmError`).  Registry
    /// errors (currently only `TypeMismatch`) are wrapped as
    /// [`AsyncCreateError::Registry`]; factory errors as
    /// [`AsyncCreateError::Factory`].
    ///
    /// Race / failure semantics:
    ///
    /// - The factory runs at most once per name across the lifetime
    ///   of the registry, even under concurrent invocation.
    /// - If the factory returns `Err`, the in-flight marker is
    ///   cleared and waiters wake up to retry the operation; the
    ///   first retry that succeeds creates the entry.
    /// - If the factory's future is dropped (cancellation), the
    ///   in-flight marker is cleared via the drop guard so future
    ///   callers can proceed; nothing is inserted.
    /// - On hit (entry already exists), the factory is not invoked.
    pub async fn get_or_create_async<T, E, F, Fut>(
        &self,
        name: Bytes,
        factory: F,
    ) -> Result<Arc<T>, AsyncCreateError<E>>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        // The factory is FnOnce; we may need to give up our turn (if
        // we lose the in-flight race and have to await), so wrap it
        // in an Option for take().  But because callers expect to
        // pass `FnOnce`, this loop is only entered once with the
        // factory available; we either run it or another caller did.
        let mut factory = Some(factory);
        loop {
            // Fast path: already filled.
            if let Some(arc) = self.get::<T>(&name).map_err(AsyncCreateError::Registry)? {
                return Ok(arc);
            }

            // Reserve in-flight slot or join an existing one.
            let reservation = {
                let mut in_flight = self.in_flight.lock();
                if let Some(existing) = in_flight.get(&name) {
                    Reservation::Wait(existing.clone())
                } else {
                    let notify = Arc::new(Notify::new());
                    in_flight.insert(name.clone(), notify.clone());
                    Reservation::Won(notify)
                }
            };

            match reservation {
                Reservation::Wait(notify) => {
                    notify.notified().await;
                    // After waking, retry: the winning caller may
                    // have inserted, or may have failed (in which
                    // case we get our turn).
                    continue;
                }
                Reservation::Won(_notify) => {
                    // Drop guard ensures we clear the in-flight
                    // marker and notify waiters even on panic /
                    // cancellation.
                    let _guard = InFlightGuard {
                        name: name.clone(),
                        registry: self,
                    };
                    let factory = factory.take().expect(
                        "factory available on the winning iteration; loop \
                         only re-enters via Wait branch which doesn't \
                         consume it",
                    );
                    let value = factory().await.map_err(AsyncCreateError::Factory)?;
                    let arc: Arc<T> = Arc::new(value);
                    let any: Arc<dyn Any + Send + Sync> = arc.clone();
                    self.entries.lock().insert(
                        name,
                        Entry {
                            type_id: TypeId::of::<T>(),
                            type_name: type_name::<T>(),
                            value: any,
                        },
                    );
                    // _guard drop notifies waiters who will then
                    // observe the inserted entry on their fast path.
                    return Ok(arc);
                }
            }
        }
    }

    /// Number of entries currently stored.  Intended for tests
    /// and diagnostics; not exposed to Lua.
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }
}

/// Error returned by [`SharedRegistry::get_or_create_async`].
///
/// Distinguishes registry-level errors (currently only the
/// type-mismatch case) from errors propagated up from the user's
/// async factory.
#[derive(Debug)]
pub enum AsyncCreateError<E> {
    Registry(SharedRegistryError),
    Factory(E),
}

impl<E: fmt::Display> fmt::Display for AsyncCreateError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(e) => fmt::Display::fmt(e, f),
            Self::Factory(e) => fmt::Display::fmt(e, f),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for AsyncCreateError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Registry(e) => Some(e),
            Self::Factory(e) => Some(e),
        }
    }
}

enum Reservation {
    Won(Arc<Notify>),
    Wait(Arc<Notify>),
}

struct InFlightGuard<'a> {
    name: Bytes,
    registry: &'a SharedRegistry,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        let notify = self.registry.in_flight.lock().remove(&self.name);
        if let Some(notify) = notify {
            notify.notify_waiters();
        }
    }
}

impl Default for SharedRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SharedRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedRegistry")
            .field("len", &self.len())
            .finish()
    }
}

/// Errors returned by [`SharedRegistry::get_or_create`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedRegistryError {
    /// `name` exists but was registered with a different type.
    TypeMismatch {
        name: Bytes,
        existing_type: &'static str,
        requested_type: &'static str,
    },
}

impl fmt::Display for SharedRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SharedRegistryError::TypeMismatch {
                name,
                existing_type,
                requested_type,
            } => {
                write!(
                    f,
                    "shared registry entry {name:?} already exists with type {existing_type}, \
                     cannot reuse as {requested_type}",
                )
            }
        }
    }
}

impl std::error::Error for SharedRegistryError {}

static GLOBAL_REGISTRY: LazyLock<Arc<SharedRegistry>> =
    LazyLock::new(|| Arc::new(SharedRegistry::new()));

/// Process-wide default [`SharedRegistry`].
///
/// All `GlobalEnv`s that do not have an explicit override installed
/// share this instance, so named primitives created in one
/// `GlobalEnv` are visible to any other in the same process.
pub fn global_shared_registry() -> Arc<SharedRegistry> {
    GLOBAL_REGISTRY.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct Thing(u32);

    #[test]
    fn create_then_get_returns_same_arc() {
        let r = SharedRegistry::new();
        let a = r.get_or_create::<Thing, _>("x", || Thing(7)).unwrap();
        let b = r
            .get_or_create::<Thing, _>("x", || panic!("must not run"))
            .unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        k9::assert_equal!(a.0, 7);
    }

    #[test]
    fn type_mismatch_diagnostic() {
        let r = SharedRegistry::new();
        let _ = r.get_or_create::<Thing, _>("x", || Thing(1)).unwrap();
        let err = r
            .get_or_create::<u64, _>("x", || 0)
            .expect_err("expected TypeMismatch");
        k9::assert_equal!(
            err.to_string(),
            "shared registry entry \"x\" already exists with type \
             shingetsu_vm::shared_registry::tests::Thing, cannot reuse as u64"
        );
    }

    #[test]
    fn factory_runs_at_most_once_under_contention() {
        let r = Arc::new(SharedRegistry::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for _ in 0..16 {
            let r = r.clone();
            let calls = calls.clone();
            handles.push(std::thread::spawn(move || {
                r.get_or_create::<Thing, _>("x", || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Thing(42)
                })
                .unwrap()
            }));
        }
        let arcs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        k9::assert_equal!(calls.load(Ordering::SeqCst), 1);
        let first = &arcs[0];
        for other in &arcs[1..] {
            assert!(Arc::ptr_eq(first, other));
        }
    }

    #[test]
    fn global_default_is_shared() {
        let a = global_shared_registry();
        let b = global_shared_registry();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn get_returns_none_when_absent() {
        let r = SharedRegistry::new();
        let result = r.get::<Thing>(&"missing".into()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_returns_existing() {
        let r = SharedRegistry::new();
        let created = r.get_or_create::<Thing, _>("x", || Thing(7)).unwrap();
        let fetched = r.get::<Thing>(&"x".into()).unwrap().unwrap();
        assert!(Arc::ptr_eq(&created, &fetched));
    }

    #[test]
    fn get_returns_type_mismatch() {
        let r = SharedRegistry::new();
        let _ = r.get_or_create::<Thing, _>("x", || Thing(1)).unwrap();
        let err = r.get::<u64>(&"x".into()).expect_err("expected mismatch");
        match err {
            SharedRegistryError::TypeMismatch { .. } => {}
        }
    }

    #[tokio::test]
    async fn get_or_create_async_creates_when_missing() {
        let r = SharedRegistry::new();
        let arc: Arc<Thing> = r
            .get_or_create_async("x".into(), || async { Ok::<_, ()>(Thing(42)) })
            .await
            .unwrap();
        k9::assert_equal!(arc.0, 42);
    }

    #[tokio::test]
    async fn get_or_create_async_returns_existing() {
        let r = SharedRegistry::new();
        let _ = r.get_or_create::<Thing, _>("x", || Thing(7)).unwrap();
        let arc: Arc<Thing> = r
            .get_or_create_async("x".into(), || async {
                panic!("factory must not run on hit");
                #[allow(unreachable_code)]
                Ok::<_, ()>(Thing(0))
            })
            .await
            .unwrap();
        k9::assert_equal!(arc.0, 7);
    }

    #[tokio::test]
    async fn get_or_create_async_factory_runs_at_most_once_under_contention() {
        let r = Arc::new(SharedRegistry::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for _ in 0..16 {
            let r = r.clone();
            let calls = calls.clone();
            handles.push(tokio::spawn(async move {
                r.get_or_create_async::<Thing, (), _, _>("x".into(), || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        // Yield once so concurrent callers definitely
                        // contend on the in-flight marker.
                        tokio::task::yield_now().await;
                        Ok(Thing(99))
                    }
                })
                .await
                .unwrap()
            }));
        }
        let arcs: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();
        k9::assert_equal!(calls.load(Ordering::SeqCst), 1);
        let first = &arcs[0];
        for other in &arcs[1..] {
            assert!(Arc::ptr_eq(first, other));
        }
    }

    #[tokio::test]
    async fn get_or_create_async_factory_error_clears_in_flight() {
        let r = Arc::new(SharedRegistry::new());

        // First attempt fails.
        let err = r
            .get_or_create_async::<Thing, _, _, _>("x".into(), || async {
                Err::<Thing, &'static str>("factory failed")
            })
            .await
            .expect_err("expected error");
        match err {
            AsyncCreateError::Factory("factory failed") => {}
            other => panic!("unexpected error: {other:?}"),
        }

        // Retry must succeed: in-flight slot was cleared by the drop
        // guard.
        let arc: Arc<Thing> = r
            .get_or_create_async("x".into(), || async { Ok::<_, ()>(Thing(5)) })
            .await
            .unwrap();
        k9::assert_equal!(arc.0, 5);
    }

    #[tokio::test]
    async fn get_or_create_async_type_mismatch_with_existing() {
        let r = SharedRegistry::new();
        let _ = r.get_or_create::<Thing, _>("x", || Thing(1)).unwrap();
        let err = r
            .get_or_create_async::<u64, _, _, _>("x".into(), || async { Ok::<_, ()>(0u64) })
            .await
            .expect_err("expected mismatch");
        match err {
            AsyncCreateError::Registry(SharedRegistryError::TypeMismatch { .. }) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
