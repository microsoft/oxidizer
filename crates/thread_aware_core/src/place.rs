// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Place`] identifier and its component id types.
//!
//! A [`Place`] says where something runs: the runtime, the thread, and the memory closest to
//! it. See [what the ids mean](crate#what-the-ids-mean) for what they promise.

#[cfg(any(feature = "std", test))]
use std::thread::ThreadId;

/// Identifies the runtime that produced a [`Place`].
///
/// Runtimes running at the same time should each use their own origin, so a value can tell
/// that it has crossed from one into another and release anything the old one owned.
/// Nothing enforces this, and a collision is worse than a slowdown. A value moving into a
/// second runtime that picked the same origin thinks it is still at home and carries on
/// using resources owned by the first.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin(u16);

impl From<u16> for Origin {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

/// Identifies the memory closest to a thread, usually a NUMA node.
///
/// On a big machine memory is split into regions and a thread reaches its own region
/// fastest. Unlike the thread id this is shared: every thread near the same memory reports
/// the same `NumaNode`, which is what makes it useful for state you want to share within a
/// region but not across the machine. Sharing across runtimes only works while they all
/// number the regions the same way; see [what the ids mean](crate#what-the-ids-mean).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NumaNode(u16);

impl From<u16> for NumaNode {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

/// Says where something runs: its runtime, its thread and its nearest memory.
///
/// A runtime builds these, usually once per worker at startup, and passes them to
/// [`ThreadAware::relocate`](crate::ThreadAware::relocate). Use the part you care about, and
/// see [what the ids mean](crate#what-the-ids-mean) for which to pick.
///
/// `Place` is cheap to clone but deliberately not `Copy`, and is passed by reference to
/// [`relocate`](crate::ThreadAware::relocate), so you rarely need to clone it.
///
/// Two places are equal only if all their ids match. If you only care about memory locality,
/// compare [`numa_node`](Self::numa_node) rather than whole places, since threads that share
/// a NUMA node still have different thread ids.
///
/// # Without `std`
///
/// The thread id is `std::thread::ThreadId`, so `new` and `thread` need the `std` feature.
/// Without it a `Place` cannot be built at all, and only [`origin`](Self::origin) and
/// [`numa_node`](Self::numa_node) can be read. That is the intended split: a `no_std` library
/// implements [`ThreadAware`](crate::ThreadAware) and reads whatever it is given, while the
/// runtime that drives relocation needs `std` anyway.
///
/// # Examples
///
/// ```
/// use std::thread;
///
/// use thread_aware_core::{NumaNode, Origin, Place};
///
/// let here = thread::current().id();
/// let place = Place::new(Origin::from(1), here, NumaNode::from(1));
///
/// assert_eq!(place.origin(), Origin::from(1));
/// assert_eq!(place.thread(), here);
///
/// // The same thread under a different runtime is a different place...
/// let elsewhere = Place::new(Origin::from(2), here, NumaNode::from(1));
/// assert_ne!(place, elsewhere);
///
/// // ...but the thread and its nearest memory are unchanged.
/// assert_eq!(place.numa_node(), elsewhere.numa_node());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    origin: Origin,
    #[cfg(any(feature = "std", test))]
    thread: ThreadId,
    numa_node: NumaNode,
}

impl Place {
    /// Creates a place from its origin, thread and NUMA ids.
    ///
    /// Runtimes call this as they set up their workers. Tests can call it directly to make
    /// places without starting a runtime. Obtain a thread id with
    /// `std::thread::current().id()`.
    ///
    /// Needs the `std` feature.
    #[cfg(any(feature = "std", test))]
    #[must_use]
    pub const fn new(origin: Origin, thread: ThreadId, numa_node: NumaNode) -> Self {
        Self { origin, thread, numa_node }
    }

    /// Returns the runtime that produced this place.
    ///
    /// Compare origins to spot that a value has moved between runtimes. That move has to
    /// stay sound, but resources owned by the old runtime usually cannot come along.
    #[must_use]
    pub const fn origin(&self) -> Origin {
        self.origin
    }

    /// Returns the thread.
    ///
    /// Use this to keep state per thread so that threads never contend for it.
    ///
    /// Needs the `std` feature.
    #[cfg(any(feature = "std", test))]
    #[must_use]
    pub const fn thread(&self) -> ThreadId {
        self.thread
    }

    /// Returns the nearest memory.
    ///
    /// Use this when what matters is which memory is nearby rather than which thread is
    /// running. Threads on the same node can share that state cheaply.
    #[must_use]
    pub const fn numa_node(&self) -> NumaNode {
        self.numa_node
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::{NumaNode, Origin, Place};

    #[test]
    fn exposes_components() {
        let id = thread::current().id();
        let place = Place::new(Origin::from(1), id, NumaNode::from(2));

        assert_eq!(place.origin(), Origin::from(1));
        assert_eq!(place.thread(), id);
        assert_eq!(place.numa_node(), NumaNode::from(2));
    }

    #[test]
    fn different_origin_compares_unequal() {
        let id = thread::current().id();
        let first = Place::new(Origin::from(0), id, NumaNode::from(0));
        let second = Place::new(Origin::from(1), id, NumaNode::from(0));

        assert_ne!(first, second);
    }

    #[test]
    fn different_numa_compares_unequal() {
        let id = thread::current().id();
        let first = Place::new(Origin::from(0), id, NumaNode::from(0));
        let second = Place::new(Origin::from(0), id, NumaNode::from(1));

        assert_ne!(first, second);
    }

    #[test]
    fn different_thread_compares_unequal() {
        let origin = Origin::from(0);
        let numa_node = NumaNode::from(0);
        let here = Place::new(origin, thread::current().id(), numa_node);
        let there = thread::spawn(move || Place::new(origin, thread::current().id(), numa_node))
            .join()
            .unwrap();

        assert_ne!(here, there);
    }

    #[test]
    fn clone_preserves_components() {
        let place = Place::new(Origin::from(4), thread::current().id(), NumaNode::from(9));
        let cloned = place.clone();

        assert_eq!(cloned, place);
    }
}
