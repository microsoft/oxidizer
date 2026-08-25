// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Location`] identifier and its component id types.
//!
//! A [`Location`] locates an execution context by three independent coordinates, each its
//! own domain type so they can evolve independently:
//!
//! - [`Topology`] — the runtime that produced the location.
//! - [`Core`] — the logical processor on the physical machine.
//! - [`MemoryRegion`] — the memory region on the physical machine, such as a NUMA node.
//!
//! [`Core`] and [`MemoryRegion`] describe hardware and are therefore shared by every
//! topology on the machine; [`Topology`] distinguishes the runtimes that use that hardware.
//! The guarantees these ids carry are documented in the crate-level
//! [coordinate space](crate#coordinate-space) section.
//!
//! The id types wrap a `u16` and are built with `From`.

/// Identifies the runtime that produced a [`Location`].
///
/// Runtimes are expected to give each concurrently live instance a distinct topology, so
/// that two runtimes in the same process stay distinguishable even when they run on the
/// same hardware. Nothing enforces this — [`Topology::from`] accepts any `u16` — and the
/// consequence of a collision is not merely degraded locality: a value carrying
/// runtime-bound state that crosses into a second runtime with the same topology concludes
/// it is still at home and keeps using resources owned by the first runtime. Runtimes that
/// share a
/// process own this uniqueness between them.
///
/// A topology does not scope [`Core`] or [`MemoryRegion`]; it lets an implementation tell
/// whether it is still inside the runtime whose resources it holds.
///
/// See the [coordinate space](crate#coordinate-space) notes for what the wrapped value does
/// and does not promise.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Topology(u16);

/// A logical processor on the physical machine.
///
/// Values name hardware rather than indexing a worker list, so state keyed on [`Core`] alone
/// can be shared between runtimes — provided every runtime in the process derives the value
/// from the same physical numbering. This crate cannot check that; see the
/// [coordinate space](crate#coordinate-space) notes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Core(u16);

/// A memory region on the physical machine, such as a NUMA node.
///
/// Like [`Core`], values are hardware coordinates shared by every topology on the machine,
/// so region-keyed state can be shared across runtimes. See the
/// [coordinate space](crate#coordinate-space) notes.
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

/// Identifies where an execution context runs: its topology, core and memory region.
///
/// A `Location` is produced by a runtime, typically once per worker at startup, and handed
/// to [`ThreadAware::relocate`](crate::ThreadAware::relocate) to describe where a value came
/// from and where it now lives. Implementations read the coordinates they care about — a
/// per-core cache keys on [`core`](Self::core), a memory pool on
/// [`memory_region`](Self::memory_region) — and ignore the rest.
///
/// `Location` is cheap to clone but deliberately not `Copy`. It is
/// passed by reference to [`relocate`](crate::ThreadAware::relocate), so consumers rarely
/// clone it.
///
/// Equality covers all three coordinates, so locations from different runtimes never compare
/// equal even when they describe the same physical core. Implementations that care only
/// about hardware should therefore compare [`core`](Self::core) or
/// [`memory_region`](Self::memory_region) directly rather than whole locations. See the
/// [coordinate space](crate#coordinate-space) notes for the guarantees the ids carry.
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
/// // ...but the hardware coordinate is shared, so per-core state can be too.
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
    /// Runtimes call this when they enumerate their workers. Tests can call it directly to
    /// synthesize locations without standing up a runtime.
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
    /// Compare topologies to detect that a value has crossed between runtimes. Such a move
    /// must stay sound, but runtime-bound resources generally cannot survive it.
    #[must_use]
    pub const fn topology(&self) -> Topology {
        self.topology
    }

    /// Returns the core.
    ///
    /// Use this to partition state per core so that cores do not contend for it. Because it
    /// is a hardware coordinate, the partitioning holds across topologies.
    #[must_use]
    pub const fn core(&self) -> Core {
        self.core
    }

    /// Returns the memory region.
    ///
    /// Use this to partition state whose cost is dominated by memory locality rather than
    /// by cross-core sharing; cores in the same region can share it cheaply.
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
