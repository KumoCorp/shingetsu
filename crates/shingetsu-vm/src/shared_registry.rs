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
use std::sync::{Arc, LazyLock};

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
}

impl SharedRegistry {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
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

    /// Number of entries currently stored.  Intended for tests
    /// and diagnostics; not exposed to Lua.
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
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
}
