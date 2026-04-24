//! Path-keyed slot cache backing the resolver.
//!
//! Keys are full `Vec<TypeId>` *paths*. A classical resolution caches its
//! value at the single-element path `[TypeId::of::<O>()]`, so behavior in the
//! absence of overrides is observationally identical to the older
//! `TypeId`-keyed map.
//!
//! Path-keyed storage is the substrate for scoped dependency overrides: a
//! `provide()` chain pre-allocates an empty slot at every prefix path along
//! the chain (and a filled slot at the leaf). Resolution then performs a
//! *longest suffix* lookup over cached paths whose last element matches the
//! type being resolved.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A shared, write-once container for a resolved value.
///
/// Slot identity is by `Arc`: branched / multi-`provide()` registrations that
/// resolve to the same path get the same `Slot`, so filling it once is
/// observable to all paths through it.
pub(crate) type Slot = Arc<OnceLock<Arc<dyn Any + Send + Sync>>>;

/// A thread-safe path-keyed slot cache.
///
/// Once a slot is created at a given path it is never removed; once filled it
/// is never overwritten. New entries (slots) may still be inserted at fresh
/// paths over the cache's lifetime.
pub(crate) struct PathCache {
    inner: Mutex<CacheState>,
}

#[derive(Default)]
struct CacheState {
    slots: HashMap<Vec<TypeId>, Slot>,
    /// Index from the *last* `TypeId` of each cached path to the list of paths
    /// ending in that `TypeId`. Used to bound longest-suffix searches.
    by_last: HashMap<TypeId, Vec<Vec<TypeId>>>,
}

impl PathCache {
    /// Creates an empty path cache.
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(CacheState::default()),
        }
    }

    /// Returns the value cached at the single-element path `[TypeId::of::<O>()]`,
    /// downcast to `O`. Used by `Resolver::try_get` and as the fast path for
    /// classical (override-free) resolution.
    pub(crate) fn try_get<O: Send + Sync + 'static>(&self, path: &[TypeId]) -> Option<Arc<O>> {
        let guard = self.inner.lock().expect("PathCache mutex poisoned");
        let slot = Arc::clone(guard.slots.get(path)?);
        drop(guard);
        let any = Arc::clone(slot.get()?);
        any.downcast::<O>().ok()
    }

    /// Inserts a filled slot at `path` if no slot exists, or fills an existing
    /// empty slot. Returns a handle to the stored value.
    ///
    /// Used for `Resolver::insert` (single-element path) and for classical
    /// placement of newly-constructed values.
    pub(crate) fn get_or_insert<O: Send + Sync + 'static>(&self, path: Vec<TypeId>, value: O) -> Arc<O> {
        let slot = self.get_or_create_slot(path);
        // `OnceLock::set` ignores the value if already filled; in either case
        // we read back the stored value.
        let new_arc: Arc<dyn Any + Send + Sync> = Arc::new(value);
        let _ = slot.set(new_arc);
        let stored = Arc::clone(slot.get().expect("just set or already set"));
        stored
            .downcast::<O>()
            .expect("path key type must match stored value type by construction")
    }

    /// Returns (or lazily creates) the slot at `path`. The slot may be empty.
    pub(crate) fn get_or_create_slot(&self, path: Vec<TypeId>) -> Slot {
        let mut guard = self.inner.lock().expect("PathCache mutex poisoned");
        if let Some(existing) = guard.slots.get(&path) {
            return Arc::clone(existing);
        }
        let slot: Slot = Arc::new(OnceLock::new());
        let last = *path.last().expect("path must be non-empty");
        guard.by_last.entry(last).or_default().push(path.clone());
        guard.slots.insert(path, Arc::clone(&slot));
        slot
    }

    /// Finds the longest cached path that (a) ends in `target` and (b) is a
    /// suffix of `key`. Returns the slot at that path along with its length.
    ///
    /// `key` must end in `target`.
    pub(crate) fn find_best_slot(&self, key: &[TypeId], target: TypeId) -> Option<(Slot, usize)> {
        debug_assert_eq!(key.last().copied(), Some(target));
        let guard = self.inner.lock().expect("PathCache mutex poisoned");
        let candidates = guard.by_last.get(&target)?;
        let mut best: Option<(&Vec<TypeId>, usize)> = None;
        for cand in candidates {
            if cand.len() > key.len() {
                continue;
            }
            // Suffix check: cand must equal the trailing slice of key.
            let start = key.len() - cand.len();
            if &key[start..] == cand.as_slice()
                && best.is_none_or(|(_, best_len)| cand.len() > best_len)
            {
                best = Some((cand, cand.len()));
            }
        }
        let (path, len) = best?;
        let slot = Arc::clone(guard.slots.get(path).expect("indexed path must exist"));
        Some((slot, len))
    }
}

impl std::fmt::Debug for PathCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.inner.lock().map(|g| g.slots.len()).unwrap_or(0);
        f.debug_struct("PathCache").field("entries", &len).finish()
    }
}
