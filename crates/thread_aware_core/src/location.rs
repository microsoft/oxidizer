// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Location`] identifier and its component id types.
//!
//! A [`Location`] locates an execution context by three independent coordinates, each
//! its own domain type so they can evolve independently:
//!
//! - [`Provenance`] — the topology that produced the location. Values minted by
//!   unrelated runtimes carry different provenance, so locations from different
//!   topologies never compare equal and cross-topology relocation stays well defined.
//! - [`Core`] — the logical processor.
//! - [`MemoryRegion`] — the memory region (for example, a NUMA node).
//!
//! The id types wrap a `u16` and are built with `From`. If that range is ever too
//! small, a `From<u32>` (or wider) can be added later without a breaking change.

/// Identity of the topology that produced a [`Location`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Provenance(u16);

/// A logical processor within a topology.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Core(u16);

/// A memory region (for example, a NUMA node) within a topology.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MemoryRegion(u16);

impl From<u16> for Provenance {
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

/// Identifies where an execution context runs: its provenance, core and memory region.
///
/// `Location` is cheap to clone but deliberately not `Copy`, which leaves room to carry
/// richer data (such as a runtime handle) in the future without a breaking change. It
/// is passed by reference to [`ThreadAware::relocate`](crate::ThreadAware::relocate),
/// so consumers rarely clone it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    provenance: Provenance,
    core: Core,
    memory_region: MemoryRegion,
}

impl Location {
    /// Creates a location from its provenance, core and memory region.
    #[must_use]
    pub const fn new(provenance: Provenance, core: Core, memory_region: MemoryRegion) -> Self {
        Self {
            provenance,
            core,
            memory_region,
        }
    }

    /// Returns the provenance of the topology that produced this location.
    #[must_use]
    pub const fn provenance(&self) -> Provenance {
        self.provenance
    }

    /// Returns the core.
    #[must_use]
    pub const fn core(&self) -> Core {
        self.core
    }

    /// Returns the memory region.
    #[must_use]
    pub const fn memory_region(&self) -> MemoryRegion {
        self.memory_region
    }
}

#[cfg(test)]
mod tests {
    use super::{Core, Location, MemoryRegion, Provenance};

    #[test]
    fn exposes_components() {
        let location = Location::new(Provenance::from(1), Core::from(3), MemoryRegion::from(2));

        assert_eq!(location.provenance(), Provenance::from(1));
        assert_eq!(location.core(), Core::from(3));
        assert_eq!(location.memory_region(), MemoryRegion::from(2));
    }

    #[test]
    fn different_provenance_compares_unequal() {
        let first = Location::new(Provenance::from(0), Core::from(0), MemoryRegion::from(0));
        let second = Location::new(Provenance::from(1), Core::from(0), MemoryRegion::from(0));

        assert_ne!(first, second);
    }

    #[test]
    fn clone_preserves_components() {
        let location = Location::new(Provenance::from(5), Core::from(7), MemoryRegion::from(9));
        let cloned = location.clone();

        assert_eq!(cloned, location);
    }
}
