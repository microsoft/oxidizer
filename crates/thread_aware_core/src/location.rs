// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Location`] identifier and its component id types.
//!
//! A [`Location`] says where something runs, using three ids: [`Topology`] (the runtime),
//! [`Core`] (the processor) and [`MemoryRegion`] (the nearby memory, such as a NUMA node).
//! The first tells runtimes apart; the other two name hardware, so every runtime on the
//! machine sees the same values. See [what the ids mean](crate#what-the-ids-mean) for what
//! they promise.
//!
//! The id types wrap a `u16` and are built with `From`.

/// Identifies the runtime that produced a [`Location`].
///
/// Runtimes running at the same time should each use their own topology, so they can be told
/// apart even when they share hardware. Nothing enforces this, since [`Topology::from`]
/// accepts any `u16`, and a collision is worse than a slowdown: a value moving into a second
/// runtime with the same topology thinks it is still at home and carries on using resources
/// owned by the first.
///
/// A topology does not change what [`Core`] or [`MemoryRegion`] mean. It only tells you
/// whether you are still inside the runtime that gave you your resources.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Topology(u16);

/// A processor on the machine.
///
/// This names hardware rather than a slot in a worker list, so state keyed on [`Core`] alone
/// can be shared between runtimes, as long as they all number processors the same way. This
/// crate cannot check that; see [what the ids mean](crate#what-the-ids-mean).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Core(u16);

/// A memory region on the machine, such as a NUMA node.
///
/// Like [`Core`], this names hardware, so every runtime on the machine sees the same value
/// and region-keyed state can be shared between them.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MemoryRegion(u16);

impl From<u16> for Topology {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<u16> for Core {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

impl From<u16> for MemoryRegion {
    fn from(value: u16) -> Self {
        Self(value)
    }
}

/// Says where something runs: its topology, core and memory region.
///
/// A runtime builds these, usually once per worker at startup, and passes them to
/// [`ThreadAware::relocate`](crate::ThreadAware::relocate). Use the parts you care about: a
/// per-core cache only needs [`core`](Self::core), a memory pool only needs
/// [`memory_region`](Self::memory_region).
///
/// `Location` is cheap to clone but deliberately not `Copy`, and is passed by reference to
/// [`relocate`](crate::ThreadAware::relocate), so you rarely need to clone it.
///
/// Two locations are equal only if all three ids match, so locations from different runtimes
/// never compare equal even on the same core. If you only care about hardware, compare
/// [`core`](Self::core) or [`memory_region`](Self::memory_region) instead of whole
/// locations.
///
/// # Examples
///
/// ```
/// use thread_aware_core::{Core, Location, MemoryRegion, Topology};
///
/// let location = Location::new(Topology::from(1), Core::from(3), MemoryRegion::from(1));
///
/// assert_eq!(location.core(), Core::from(3));
///
/// // A second runtime on the same core produces a different location...
/// let other = Location::new(Topology::from(2), Core::from(3), MemoryRegion::from(1));
/// assert_ne!(location, other);
///
/// // ...but the core id is shared, so per-core state can be too.
/// assert_eq!(location.core(), other.core());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    topology: Topology,
    core: Core,
    memory_region: MemoryRegion,
}

impl Location {
    /// Creates a location from its topology, core and memory region.
    ///
    /// Runtimes call this as they set up their workers. Tests can call it directly to make
    /// locations without starting a runtime.
    #[must_use]
    pub const fn new(topology: Topology, core: Core, memory_region: MemoryRegion) -> Self {
        Self {
            topology,
            core,
            memory_region,
        }
    }

    /// Returns the topology that produced this location.
    ///
    /// Compare topologies to spot that a value has moved between runtimes. That move has to
    /// stay sound, but resources tied to the old runtime usually cannot come along.
    #[must_use]
    pub const fn topology(&self) -> Topology {
        self.topology
    }

    /// Returns the core.
    ///
    /// Use this to split state per core so cores do not contend for it. Because it names
    /// hardware, that split still holds across runtimes.
    #[must_use]
    pub const fn core(&self) -> Core {
        self.core
    }

    /// Returns the memory region.
    ///
    /// Use this when what matters is which memory is nearby rather than which core is
    /// running. Cores in the same region can share that state cheaply.
    #[must_use]
    pub const fn memory_region(&self) -> MemoryRegion {
        self.memory_region
    }
}

#[cfg(test)]
mod tests {
    use super::{Core, Location, MemoryRegion, Topology};

    #[test]
    fn exposes_components() {
        let location = Location::new(Topology::from(1), Core::from(3), MemoryRegion::from(2));

        assert_eq!(location.topology(), Topology::from(1));
        assert_eq!(location.core(), Core::from(3));
        assert_eq!(location.memory_region(), MemoryRegion::from(2));
    }

    #[test]
    fn different_topology_compares_unequal() {
        let first = Location::new(Topology::from(0), Core::from(0), MemoryRegion::from(0));
        let second = Location::new(Topology::from(1), Core::from(0), MemoryRegion::from(0));

        assert_ne!(first, second);
    }

    #[test]
    fn clone_preserves_components() {
        let location = Location::new(Topology::from(5), Core::from(7), MemoryRegion::from(9));
        let cloned = location.clone();

        assert_eq!(cloned, location);
    }
}
