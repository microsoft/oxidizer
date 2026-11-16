// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Thread`] coordinate and its component identifier types.
//!
//! A [`Thread`] records the runtime, OS thread, and nearest memory for a value. See
//! [what the ids mean](crate#what-the-ids-mean) for each id's guarantees.

use core::hash::{Hash, Hasher};
use core::sync::atomic::{AtomicUsize, Ordering};
#[cfg(any(test, feature = "std"))]
use std::thread::ThreadId;

/// The runtime, OS thread, and nearest memory for a value.
///
/// A runtime creates these coordinates, usually one for each worker. It passes them to
/// [`ThreadAware::relocate`](crate::ThreadAware::relocate). An implementation reads only the
/// coordinate that its state needs. The `destination` coordinate must describe the thread that
/// runs `relocate`, not a remote target.
///
/// # Relation to `std::thread::Thread`
///
/// This `Thread` is a coordinate, not a handle. It owns no operating-system resource.
/// [`std::thread::Thread`] is a separate type. Alias one type if a module needs both.
///
/// `Thread` is `Clone` but not `Copy`. [`relocate`](crate::ThreadAware::relocate) takes it by
/// reference, so cloning is rarely necessary.
///
/// Two values are equal if all their ids match. Compare [`numa_node`](Self::numa_node) if only
/// memory locality matters. Threads on one NUMA node still have different thread ids.
///
/// # Without `std`
///
/// Without the `std` feature, a `Thread` has no `std::thread::ThreadId` and cannot be constructed.
/// It keeps [`owner`](Self::owner) and [`numa_node`](Self::numa_node). Equality and hashing use
/// those two coordinates.
///
/// # Examples
///
/// ```
/// use thread_aware_core::Thread;
///
/// fn moved_between_memory_regions(source: &Thread, destination: &Thread) -> bool {
///     source.numa_node() != destination.numa_node()
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Thread {
    owner: Owner,
    // The `test` gate compiles this field in the crate's test build without listing features.
    // Because `cfg(test)` restores `id`, only `tests/no_std_surface.rs` can test the `no_std`
    // shape as an external dependency.
    #[cfg(any(test, feature = "std"))]
    id: ThreadId,
    numa_node: NumaNode,
}

impl Thread {
    /// Creates a `Thread` from an owner, a thread id, and a NUMA node.
    ///
    /// A thread id is obtained from [`thread::current`](std::thread::current).
    /// Requires the `std` feature.
    #[cfg(any(test, feature = "std"))]
    #[inline]
    #[must_use]
    pub(crate) const fn new(owner: Owner, id: ThreadId, numa_node: NumaNode) -> Self {
        Self { owner, id, numa_node }
    }

    /// Returns the identifier of the runtime that owns this [`Thread`].
    ///
    /// Compare owners to detect a move between runtimes. The move remains sound. The
    /// implementation decides if runtime-specific state remains useful.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_core::Thread;
    ///
    /// fn same_runtime(first: &Thread, second: &Thread) -> bool {
    ///     first.owner() == second.owner()
    /// }
    /// ```
    #[inline]
    #[must_use]
    pub const fn owner(&self) -> &Owner {
        &self.owner
    }

    /// Returns the [`ThreadId`](std::thread::ThreadId) of the OS thread this `Thread` names.
    ///
    /// Distinct live OS threads have distinct ids. Code can use them to key state by OS thread.
    /// The storage determines if that state is contended. The platform's thread identifier has no
    /// defined relationship to [`ThreadId`](std::thread::ThreadId).
    ///
    /// Requires the `std` feature.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_core::Thread;
    ///
    /// fn runs_on_current_thread(thread: &Thread) -> bool {
    ///     thread.id() == std::thread::current().id()
    /// }
    /// ```
    #[cfg(any(test, feature = "std"))]
    #[inline]
    #[must_use]
    pub const fn id(&self) -> ThreadId {
        self.id
    }

    /// Returns the identifier of the memory nearest to this [`Thread`].
    ///
    /// Use this id when nearby memory matters more than the OS thread. Threads on one node have
    /// similar access to that node's memory.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_core::Thread;
    ///
    /// fn same_memory_region(first: &Thread, second: &Thread) -> bool {
    ///     first.numa_node() == second.numa_node()
    /// }
    /// ```
    #[inline]
    #[must_use]
    pub const fn numa_node(&self) -> &NumaNode {
        &self.numa_node
    }
}

/// Provides a new identity for each [`Owner`] in this process.
///
/// The values and their order have no meaning. A counter provides distinct values until it is
/// exhausted. Pointer-width makes exhaustion impractical. Relaxed ordering is enough because the
/// atomic only provides uniqueness. It does not publish or order other state.
static NEXT_OWNER: AtomicUsize = AtomicUsize::new(0);

/// An identifier for the runtime that owns a [`Thread`].
///
/// Each new owner is unique. Two live runtimes do not share an owner. A value can compare owners
/// to detect a move between runtimes.
///
/// Two owners are the same runtime exactly when their identities match.
///
/// An owner names one live runtime. It is `Clone` but not `Copy`. [`Thread::owner`] returns a
/// reference.
///
/// # Examples
///
/// ```
/// use thread_aware_core::Owner;
///
/// fn same_runtime(first: &Owner, second: &Owner) -> bool {
///     first == second
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Owner {
    id: usize,
}

impl PartialEq for Owner {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Owner {}

impl Hash for Owner {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Owner {
    /// Creates an owner for a runtime.
    ///
    /// No other owner in this process compares equal to the new owner.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::from_counter(&NEXT_OWNER)
    }

    fn from_counter(next: &AtomicUsize) -> Self {
        Self {
            // The atomic update makes each identity distinct. Refuse to wrap because a wrapped
            // identity can make a new runtime equal to an earlier runtime.
            id: next
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                .expect("owner identity space exhausted before allocating a unique runtime owner"),
        }
    }
}

#[cfg(test)]
mod gamma_round2_tests {
    use std::panic::catch_unwind;

    use super::*;

    #[test]
    fn owner_identity_exhaustion_panics_without_reusing_counter_values() {
        let next = AtomicUsize::new(usize::MAX);
        for _ in 0..2 {
            catch_unwind(|| Owner::from_counter(&next)).unwrap_err();
            assert_eq!(next.load(Ordering::Relaxed), usize::MAX);
        }
    }

    #[test]
    fn gamma_round2_owner_identity_reserves_exactly_one_counter_value_including_the_last_available_slot() {
        let next = AtomicUsize::new(41);
        assert_eq!(Owner::from_counter(&next).id, 41);
        assert_eq!(Owner::from_counter(&next).id, 42);
        assert_eq!(next.load(Ordering::Relaxed), 43);
        let last = AtomicUsize::new(usize::MAX - 1);
        assert_eq!(Owner::from_counter(&last).id, usize::MAX - 1);
        assert_eq!(last.load(Ordering::Relaxed), usize::MAX);
    }
}

/// An identifier for the memory nearest to a thread, usually a non-uniform memory access
/// (NUMA) node.
///
/// A NUMA node is a hardware region whose processors have similar access to local memory.
/// Threads near the same memory share a `NumaNode`. This makes it useful for state shared within
/// one region. Runtimes can share node-keyed state only if they number regions identically. See
/// [what the ids mean](crate#what-the-ids-mean).
///
/// A node value has no meaning beyond identity. Its width is enough for real hardware.
///
/// # Examples
///
/// ```
/// use thread_aware_core::NumaNode;
///
/// fn same_memory_region(first: &NumaNode, second: &NumaNode) -> bool {
///     first == second
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NumaNode(u32);

impl NumaNode {
    /// Creates a node identifier from the number the platform reports.
    #[inline]
    #[must_use]
    pub(crate) const fn new(node: u32) -> Self {
        Self(node)
    }
}
