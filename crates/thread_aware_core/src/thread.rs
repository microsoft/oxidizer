// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Thread`] identifier and its component id types.
//!
//! A [`Thread`] records where a value runs: which runtime owns it, which thread it is on,
//! and which memory is closest to that thread. See
//! [what the ids mean](crate#what-the-ids-mean) for the guarantees each id carries.

#[cfg(any(test, feature = "std"))]
use std::thread::ThreadId;

/// An identifier for the runtime that owns a [`Thread`].
///
/// Runtimes that run at the same time are expected to take distinct owner ids, so that a
/// value can detect that it has crossed from one into another and release anything the
/// previous one owned. Nothing enforces this, and a collision is worse than a slowdown: a
/// value moving into a second runtime that took the same owner id concludes that it is
/// still at home and continues to use resources owned by the first.
///
/// # Examples
///
/// ```
/// use thread_aware_core::Owner;
///
/// let owner = Owner::new(1);
///
/// assert_eq!(owner, Owner::new(1));
/// assert_ne!(owner, Owner::new(2));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Owner(u32);

impl Owner {
    /// Creates an owner from a runtime-assigned number.
    ///
    /// The number carries no meaning beyond telling one runtime apart from another.
    ///
    /// # Examples
    ///
    /// ```
    /// use thread_aware_core::Owner;
    ///
    /// assert_ne!(Owner::new(0), Owner::new(1));
    /// ```
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
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
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
}

/// A record of where a value runs: its runtime, its thread, and its nearest memory.
///
/// A runtime constructs these, usually once per worker at startup, and passes them to
/// [`ThreadAware::relocate`](crate::ThreadAware::relocate). An implementation reads whichever
/// part it depends on; [what the ids mean](crate#what-the-ids-mean) describes which to
/// choose.
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
/// The thread id is `std::thread::ThreadId`, so `new` and [`id`](Self::id) require the `std`
/// feature. Without it a `Thread` cannot be constructed at all, and only
/// [`owner`](Self::owner) and [`numa_node`](Self::numa_node) can be read. That is the
/// intended split: a `no_std` library implements [`ThreadAware`](crate::ThreadAware) and
/// reads whatever it is given, while the runtime that drives relocation requires `std`
/// regardless.
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
/// let mine = Thread::new(Owner::new(1), here, NumaNode::new(1));
///
/// assert_eq!(mine.owner(), Owner::new(1));
/// assert_eq!(mine.id(), here);
///
/// // The same OS thread under a different runtime is a different `Thread`...
/// let elsewhere = Thread::new(Owner::new(2), here, NumaNode::new(1));
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
    /// let mine = Thread::new(Owner::new(1), thread::current().id(), NumaNode::new(0));
    ///
    /// assert_eq!(mine.owner(), Owner::new(1));
    /// ```
    #[cfg(any(test, feature = "std"))]
    #[must_use]
    pub const fn new(owner: Owner, id: ThreadId, numa_node: NumaNode) -> Self {
        Self { owner, id, numa_node }
    }

    /// Returns the identifier of the runtime that owns this [`Thread`].
    ///
    /// Comparing owners detects that a value has moved between runtimes, provided the
    /// runtimes involved took distinct owner ids. Such a move remains sound, but resources
    /// owned by the previous runtime usually cannot follow.
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
    /// let mine = Thread::new(Owner::new(7), thread::current().id(), NumaNode::new(0));
    ///
    /// assert_eq!(mine.owner(), Owner::new(7));
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub const fn owner(&self) -> Owner {
        self.owner
    }

    /// Returns the [`ThreadId`](std::thread::ThreadId) this value refers to.
    ///
    /// Distinct live threads have distinct ids, so this partitions state by thread without
    /// keys colliding. Whether that state is contended depends on the storage around it.
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
    #[must_use]
    pub const fn numa_node(&self) -> NumaNode {
        self.numa_node
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use core::panic::{RefUnwindSafe, UnwindSafe};
    use std::thread;

    use static_assertions::assert_impl_all;

    use super::{NumaNode, Owner, Thread};

    assert_impl_all!(Owner: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(NumaNode: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Thread: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);

    #[test]
    fn exposes_components() {
        let id = thread::current().id();
        let mine = Thread::new(Owner::new(1), id, NumaNode::new(2));

        assert_eq!(mine.owner(), Owner::new(1));
        assert_eq!(mine.id(), id);
        assert_eq!(mine.numa_node(), NumaNode::new(2));
    }

    #[test]
    fn different_owner_compares_unequal() {
        let id = thread::current().id();
        let first = Thread::new(Owner::new(0), id, NumaNode::new(0));
        let second = Thread::new(Owner::new(1), id, NumaNode::new(0));

        assert_ne!(first, second);
    }

    #[test]
    fn different_numa_compares_unequal() {
        let id = thread::current().id();
        let first = Thread::new(Owner::new(0), id, NumaNode::new(0));
        let second = Thread::new(Owner::new(0), id, NumaNode::new(1));

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
    }
}
