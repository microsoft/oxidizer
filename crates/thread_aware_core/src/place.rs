// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Place`] identifier and its component id types.
//!
//! A [`Place`] records where something runs: the runtime, the thread, and the memory closest
//! to that thread. See [what the ids mean](crate#what-the-ids-mean) for the guarantees each
//! id carries.

#[cfg(any(feature = "std", test))]
use std::thread::ThreadId;

/// An identifier for the runtime that produced a [`Place`].
///
/// Runtimes that run at the same time are expected to use distinct origins, so that a value
/// can detect that it has crossed from one into another and release anything the previous
/// one owned. Nothing enforces this, and a collision is worse than a slowdown: a value
/// moving into a second runtime that chose the same origin concludes that it is still at
/// home and continues to use resources owned by the first.
///
/// # Examples
///
/// ```
/// use thread_aware_core::Origin;
///
/// let origin = Origin::from(1);
///
/// assert_eq!(origin, Origin::from(1));
/// assert_ne!(origin, Origin::from(2));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Origin(u16);

impl From<u16> for Origin {
    fn from(value: u16) -> Self {
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
/// # Examples
///
/// ```
/// use thread_aware_core::NumaNode;
///
/// let node = NumaNode::from(0);
///
/// assert_eq!(node, NumaNode::from(0));
/// assert_ne!(node, NumaNode::from(1));
/// ```
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NumaNode(u16);

impl From<u16> for NumaNode {
    fn from(value: u16) -> Self {
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
/// `Place` is cheap to clone but deliberately not `Copy`, and is passed by reference to
/// [`relocate`](crate::ThreadAware::relocate), so cloning is rarely necessary.
///
/// Two places are equal only if all their ids match. Code concerned only with memory
/// locality compares [`numa_node`](Self::numa_node) rather than whole places, since threads
/// that share a NUMA node still have different thread ids.
///
/// # Without `std`
///
/// The thread id is `std::thread::ThreadId`, so `new` and `thread` require the `std`
/// feature. Without it a `Place` cannot be constructed at all, and only
/// [`origin`](Self::origin) and [`numa_node`](Self::numa_node) can be read. That is the
/// intended split: a `no_std` library implements [`ThreadAware`](crate::ThreadAware) and
/// reads whatever it is given, while the runtime that drives relocation requires `std`
/// regardless.
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
    /// Creates a place from its origin, thread, and NUMA ids.
    ///
    /// Runtimes call this as they set up their workers. Tests may call it directly to
    /// construct places without starting a runtime. A thread id is obtained from
    /// [`thread::current`](std::thread::current).
    ///
    /// Requires the `std` feature.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    ///
    /// use thread_aware_core::{NumaNode, Origin, Place};
    ///
    /// let place = Place::new(Origin::from(1), thread::current().id(), NumaNode::from(0));
    ///
    /// assert_eq!(place.origin(), Origin::from(1));
    /// ```
    #[cfg(any(feature = "std", test))]
    #[must_use]
    pub const fn new(origin: Origin, thread: ThreadId, numa_node: NumaNode) -> Self {
        Self { origin, thread, numa_node }
    }

    /// Returns the identifier of the runtime that produced this place.
    ///
    /// Comparing origins detects that a value has moved between runtimes, provided the
    /// runtimes involved chose distinct origins. Such a move remains sound, but resources
    /// owned by the previous runtime usually cannot follow.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    ///
    /// use thread_aware_core::{NumaNode, Origin, Place};
    ///
    /// let place = Place::new(Origin::from(7), thread::current().id(), NumaNode::from(0));
    ///
    /// assert_eq!(place.origin(), Origin::from(7));
    /// ```
    #[must_use]
    pub const fn origin(&self) -> Origin {
        self.origin
    }

    /// Returns the identifier of the thread this place refers to.
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
    /// use thread_aware_core::{NumaNode, Origin, Place};
    ///
    /// let here = thread::current().id();
    /// let place = Place::new(Origin::from(1), here, NumaNode::from(0));
    ///
    /// assert_eq!(place.thread(), here);
    /// ```
    #[cfg(any(feature = "std", test))]
    #[must_use]
    pub const fn thread(&self) -> ThreadId {
        self.thread
    }

    /// Returns the identifier of the memory nearest to this place.
    ///
    /// This is the id to use when what matters is which memory is nearby rather than which
    /// thread is running, since threads on the same node share that memory cheaply.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::thread;
    ///
    /// use thread_aware_core::{NumaNode, Origin, Place};
    ///
    /// let place = Place::new(Origin::from(1), thread::current().id(), NumaNode::from(3));
    ///
    /// assert_eq!(place.numa_node(), NumaNode::from(3));
    /// ```
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
