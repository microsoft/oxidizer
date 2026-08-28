// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Thread`] identifier and its component id types.
//!
//! A [`Thread`] records where a value runs: which runtime owns it, which OS thread it names,
//! and which memory is closest to that OS thread. See
//! [what the ids mean](crate#what-the-ids-mean) for the guarantees each id carries.

use core::hash::{Hash, Hasher};
use core::sync::atomic::{AtomicUsize, Ordering};
#[cfg(any(test, feature = "std"))]
use std::thread::ThreadId;

/// Hands the next identity to each [`Owner`] created in this process.
///
/// Pointer-width so the counter does not wrap in practice; a wrap would hand out a live
/// identity twice.
static NEXT_OWNER: AtomicUsize = AtomicUsize::new(0);

/// An identifier for the runtime that owns a [`Thread`].
///
/// Every new owner is unique, so two runtimes alive at the same time never share one. That
/// is what lets a value notice it has crossed from one runtime into another and release
/// anything the previous one owned.
///
/// An owner also reports the smallest number of threads its runtime runs, which a value can
/// use to pre-size per-thread state before it has seen any of those threads. The runtime may
/// run more; one that spawns threads entirely on demand promises none and reports `0`.
///
/// Two owners are the same runtime exactly when their identities match; the thread count
/// plays no part in equality or hashing.
///
/// # Examples
///
/// ```
/// use thread_aware_core::Owner;
///
/// let pool = Owner::new(4);
/// assert_eq!(pool.min_threads(), 4);
///
/// let elastic = Owner::new(0);
/// assert_eq!(elastic.min_threads(), 0);
///
/// // Every owner is distinct, however it was built.
/// assert_ne!(pool, Owner::new(4));
/// assert_ne!(elastic, Owner::new(0));
/// ```
#[derive(Copy, Clone, Debug)]
pub struct Owner {
    id: usize,
    min_threads: usize,
}

impl PartialEq for Owner {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Owner {}

impl Hash for Owner {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Owner {
    /// Creates an owner for a runtime that runs at least `min_threads` threads.
    ///
    /// The owner is unique: no other owner in this process compares equal to it.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_core::Owner;
    ///
    /// let owner = Owner::new(2);
    ///
    /// assert_eq!(owner.min_threads(), 2);
    /// ```
    #[must_use]
    pub fn new(min_threads: usize) -> Self {
        Self {
            id: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            min_threads,
        }
    }

    /// Returns the smallest number of threads the owning runtime runs.
    ///
    /// The runtime may run more, so this is a floor for pre-sizing rather than a bound to
    /// index against. `0` means the runtime promises none and spawns on demand.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_core::Owner;
    ///
    /// assert_eq!(Owner::new(8).min_threads(), 8);
    /// assert_eq!(Owner::new(0).min_threads(), 0);
    /// ```
    #[inline]
    #[must_use]
    pub const fn min_threads(&self) -> usize {
        self.min_threads
    }
}

/// An identifier for the memory closest to a thread, usually a NUMA node.
///
/// On a large machine, memory is divided into regions and a thread reaches its own region
/// fastest. Unlike the thread id, this identifier is shared: every thread near the same
/// memory reports the same `NumaNode`, which is what makes it suitable for state that is
/// shared within a region but not across the machine. Sharing between runtimes holds only
/// while they all number the regions identically; see
/// [what the ids mean](crate#what-the-ids-mean).
///
/// Nodes carry no meaning beyond identity, and the width is wide enough that no real
/// machine can exhaust it.
///
/// # Examples
///
/// ```
/// use thread_aware_core::NumaNode;
///
/// let node = NumaNode::new(0);
///
/// assert_eq!(node, NumaNode::new(0));
/// assert_ne!(node, NumaNode::new(1));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NumaNode(u32);

impl NumaNode {
    /// Creates a node identifier from the number the platform reports.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_core::NumaNode;
    ///
    /// assert_ne!(NumaNode::new(0), NumaNode::new(1));
    /// ```
    #[inline]
    #[must_use]
    pub const fn new(node: u32) -> Self {
        Self(node)
    }
}

/// A record of where a value runs: its runtime, its OS thread, and its nearest memory.
///
/// A runtime constructs these, usually once per worker at startup, and passes them to
/// [`ThreadAware::relocate`](crate::ThreadAware::relocate). An implementation reads whichever
/// part it depends on; [what the ids mean](crate#what-the-ids-mean) describes which to
/// choose.
///
/// # Relation to `std::thread::Thread`
///
/// This `Thread` is a coordinate, not a handle: it records where a value runs and owns no
/// operating-system resource. [`std::thread::Thread`] is unrelated, and naming both in one
/// module is `error[E0252]`; alias the standard handle if you need it.
///
/// `Thread` is cheap to clone but deliberately not `Copy`, and is passed by reference to
/// [`relocate`](crate::ThreadAware::relocate), so cloning is rarely necessary.
///
/// Two values are equal only if all their ids match. Code concerned only with memory
/// locality compares [`numa_node`](Self::numa_node) rather than whole `Thread`s, since
/// threads sharing a NUMA node still have different thread ids.
///
/// # Without `std`
///
/// The thread id is `std::thread::ThreadId`, so without the `std` feature a `Thread` cannot
/// be constructed and holds only [`owner`](Self::owner) and [`numa_node`](Self::numa_node).
/// Equality and hashing then compare those two alone.
///
/// # Examples
///
/// ```
/// # fn main() {
/// # #[cfg(feature = "std")] {
/// use std::thread;
///
/// use thread_aware_core::{NumaNode, Owner, Thread};
///
/// let here = thread::current().id();
/// let owner = Owner::new(1);
/// let mine = Thread::new(owner, here, NumaNode::new(1));
///
/// assert_eq!(mine.owner(), owner);
/// assert_eq!(mine.id(), here);
///
/// // The same OS thread under a different runtime is a different `Thread`...
/// let elsewhere = Thread::new(Owner::new(1), here, NumaNode::new(1));
/// assert_ne!(mine, elsewhere);
///
/// // ...but the thread id and its nearest memory are unchanged.
/// assert_eq!(mine.numa_node(), elsewhere.numa_node());
/// # }
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Thread {
    owner: Owner,
    // `test` is part of the gate so the crate's own test build compiles this field without
    // enumerating features (see docs/optional-deps-in-test-builds.md). The cost is that no
    // unit test can observe the `no_std` shape: `cfg(test)` puts `id` back, so a test gated
    // on `not(feature = "std")` still sees three fields and would assert the wrong thing.
    // `tests/no_std_surface.rs` covers that shape from outside the test build instead.
    #[cfg(any(test, feature = "std"))]
    id: ThreadId,
    numa_node: NumaNode,
}

impl Thread {
    /// Creates a `Thread` from an owner, a thread id, and a NUMA node.
    ///
    /// Runtimes call this as they set up their workers. Tests may call it directly to
    /// construct values without starting a runtime. A thread id is obtained from
    /// [`thread::current`](std::thread::current).
    ///
    /// Requires the `std` feature.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    ///
    /// use thread_aware_core::{NumaNode, Owner, Thread};
    ///
    /// let owner = Owner::new(1);
    /// let mine = Thread::new(owner, thread::current().id(), NumaNode::new(0));
    ///
    /// assert_eq!(mine.owner(), owner);
    /// ```
    #[cfg(any(test, feature = "std"))]
    #[inline]
    #[must_use]
    pub const fn new(owner: Owner, id: ThreadId, numa_node: NumaNode) -> Self {
        Self { owner, id, numa_node }
    }

    /// Returns the identifier of the runtime that owns this [`Thread`].
    ///
    /// Comparing owners detects that a value has moved between runtimes. Such a move remains
    /// sound, but resources owned by the previous runtime usually cannot follow.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// use std::thread;
    ///
    /// use thread_aware_core::{NumaNode, Owner, Thread};
    ///
    /// let owner = Owner::new(7);
    /// let mine = Thread::new(owner, thread::current().id(), NumaNode::new(0));
    ///
    /// assert_eq!(mine.owner(), owner);
    /// # }
    /// # }
    /// ```
    #[inline]
    #[must_use]
    pub const fn owner(&self) -> Owner {
        self.owner
    }

    /// Returns the [`ThreadId`](std::thread::ThreadId) of the OS thread this `Thread` names.
    ///
    /// Distinct live OS threads have distinct ids, so this partitions state by OS thread
    /// without keys colliding. Whether that state is contended depends on the storage around
    /// it. A [`ThreadId`](std::thread::ThreadId) has no defined relationship to whatever
    /// identifier the platform assigns.
    ///
    /// Requires the `std` feature.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    ///
    /// use thread_aware_core::{NumaNode, Owner, Thread};
    ///
    /// let here = thread::current().id();
    /// let mine = Thread::new(Owner::new(1), here, NumaNode::new(0));
    ///
    /// assert_eq!(mine.id(), here);
    /// ```
    #[cfg(any(test, feature = "std"))]
    #[inline]
    #[must_use]
    pub const fn id(&self) -> ThreadId {
        self.id
    }

    /// Returns the identifier of the memory nearest to this [`Thread`].
    ///
    /// This is the id to use when what matters is which memory is nearby rather than which
    /// thread is running, since threads on the same node share that memory cheaply.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(feature = "std")] {
    /// use std::thread;
    ///
    /// use thread_aware_core::{NumaNode, Owner, Thread};
    ///
    /// let mine = Thread::new(Owner::new(1), thread::current().id(), NumaNode::new(3));
    ///
    /// assert_eq!(mine.numa_node(), NumaNode::new(3));
    /// # }
    /// # }
    /// ```
    #[inline]
    #[must_use]
    pub const fn numa_node(&self) -> NumaNode {
        self.numa_node
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use core::panic::{RefUnwindSafe, UnwindSafe};
    use std::hash::DefaultHasher;
    use std::thread;

    use static_assertions::assert_impl_all;

    use super::{Hash, Hasher, NumaNode, Owner, Thread};

    assert_impl_all!(Owner: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(NumaNode: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Thread: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);

    #[test]
    fn exposes_components() {
        let id = thread::current().id();
        let owner = Owner::new(3);
        let mine = Thread::new(owner, id, NumaNode::new(2));

        assert_eq!(mine.owner(), owner);
        assert_eq!(mine.owner().min_threads(), 3, "the owner survives intact, count included");
        assert_eq!(mine.id(), id);
        assert_eq!(mine.numa_node(), NumaNode::new(2));
    }

    #[test]
    fn different_owner_compares_unequal() {
        let id = thread::current().id();
        let numa_node = NumaNode::new(0);
        let first = Thread::new(Owner::new(1), id, numa_node);
        let second = Thread::new(Owner::new(1), id, numa_node);

        assert_ne!(first, second);
    }

    #[test]
    fn different_numa_compares_unequal() {
        let id = thread::current().id();
        // One owner, so the NUMA node is the only thing that differs.
        let owner = Owner::new(1);
        let first = Thread::new(owner, id, NumaNode::new(0));
        let second = Thread::new(owner, id, NumaNode::new(1));

        assert_ne!(first, second);
    }

    #[test]
    fn different_thread_compares_unequal() {
        let owner = Owner::new(0);
        let numa_node = NumaNode::new(0);
        let here = Thread::new(owner, thread::current().id(), numa_node);
        let there = thread::spawn(move || Thread::new(owner, thread::current().id(), numa_node))
            .join()
            .unwrap();

        assert_ne!(here, there);
    }

    #[test]
    fn clone_preserves_components() {
        let mine = Thread::new(Owner::new(4), thread::current().id(), NumaNode::new(9));
        let cloned = mine.clone();

        assert_eq!(cloned, mine);
        assert_eq!(cloned.owner().min_threads(), mine.owner().min_threads());
        assert_eq!(cloned.numa_node(), mine.numa_node());
    }

    #[test]
    fn owner_identity_is_unique_per_construction() {
        let first = Owner::new(4);
        let second = Owner::new(4);

        assert_ne!(first, second, "each owner takes its own identity");
        assert_eq!(first, first, "an owner equals itself");
        assert_eq!(first, first.clone(), "a copy keeps the same identity");
    }

    #[test]
    fn owner_equality_ignores_the_thread_count() {
        // The public API cannot produce two owners sharing an identity, but `Eq` and `Hash`
        // must still agree if it ever could, so build the pair directly.
        let one = Owner { id: 7, min_threads: 1 };
        let other = Owner { id: 7, min_threads: 99 };

        assert_eq!(one, other, "identity alone decides equality");
        assert_eq!(hash_of(one), hash_of(other), "equal owners must hash equally");
    }

    #[test]
    fn owner_hashes_by_identity() {
        let one = Owner::new(4);
        let other = Owner::new(4);

        // Equal counts, so only the identity tells these apart. Hashing has to carry it, or
        // every owner would land in one bucket.
        assert_ne!(one, other);
        assert_ne!(hash_of(one), hash_of(other), "distinct owners must hash apart");
    }

    #[test]
    fn owner_reports_its_minimum() {
        assert_eq!(Owner::new(8).min_threads(), 8);
        assert_eq!(Owner::new(0).min_threads(), 0, "zero means the runtime promises none");
    }

    #[test]
    fn owner_copy_preserves_the_thread_count() {
        let owner = Owner::new(4);
        let copy = owner;

        assert_eq!(copy.min_threads(), 4, "a copy keeps the count, not just the identity");
    }

    fn hash_of(owner: Owner) -> u64 {
        let mut hasher = DefaultHasher::new();
        owner.hash(&mut hasher);
        hasher.finish()
    }
}
