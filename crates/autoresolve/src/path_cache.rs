//! Path-keyed slot cache backing the resolver.
//!
//! Replaces the older `SharedTypeMap` (keyed by `TypeId`) with a map keyed by a
//! full `Vec<TypeId>` *path*. A classical resolution caches its value at the
//! single-element path `[TypeId::of::<O>()]`, so behavior in the absence of
//! overrides is observationally identical to the old `TypeId`-keyed map.
//!
//! Path-keyed storage is the substrate for scoped dependency overrides
//! (introduced in later phases): each registered `provide()` will allocate
//! slots at the prefixes of its consumer chain.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A thread-safe, append-only path-keyed map of shared values.
///
/// Values are stored in `Arc<dyn Any + Send + Sync>`. Lookups return cloned
/// `Arc<O>` handles, so callers do not borrow from the cache and the cache
/// itself contains no `unsafe` code.
pub(crate) struct PathCache {
    inner: Mutex<HashMap<Vec<TypeId>, Arc<dyn Any + Send + Sync>>>,
}

impl PathCache {
    /// Creates an empty path cache.
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Returns a handle to the value cached at `path`, if any, downcast to `O`.
    ///
    /// Returns `None` if no entry exists, or if the entry's stored type does
    /// not match `O` (which would indicate a programming error in the caller).
    pub(crate) fn try_get<O: Send + Sync + 'static>(&self, path: &[TypeId]) -> Option<Arc<O>> {
        let guard = self.inner.lock().expect("PathCache mutex poisoned");
        let any_arc = Arc::clone(guard.get(path)?);
        drop(guard);
        any_arc.downcast::<O>().ok()
    }

    /// Inserts a value at `path` if no entry exists, returning a handle to the
    /// stored value (existing or newly inserted).
    ///
    /// If an entry already exists at `path`, the new value is dropped and a
    /// handle to the existing one is returned. The existing entry's stored
    /// type is assumed to match `O`; if it does not, this is a programming
    /// error and the function panics.
    pub(crate) fn get_or_insert<O: Send + Sync + 'static>(&self, path: Vec<TypeId>, value: O) -> Arc<O> {
        let mut guard = self.inner.lock().expect("PathCache mutex poisoned");
        let entry = guard
            .entry(path)
            .or_insert_with(|| Arc::new(value) as Arc<dyn Any + Send + Sync>);
        Arc::clone(entry)
            .downcast::<O>()
            .expect("path key type must match stored value type by construction")
    }
}

impl std::fmt::Debug for PathCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.inner.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("PathCache").field("entries", &len).finish()
    }
}
