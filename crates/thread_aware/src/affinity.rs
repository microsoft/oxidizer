// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Identifiers for threads and NUMA regions.

#![allow(clippy::allow_attributes, reason = "Needed for conditional compilation")]

/// An `MemoryAffinity` can be thought of as a placement in a system.
///
/// It is used to represent a specific context or environment where data can be processed.
/// For example a NUMA node, a thread, a specific CPU core, or a specific memory region.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MemoryAffinity {
    /// An unknown affinity represents no specific binding, like an unpinned thread.
    Unknown,
    /// A pinned affinity represents a specific binding to a processor and memory region.
    Pinned(PinnedAffinity),
}

impl From<PinnedAffinity> for MemoryAffinity {
    fn from(pinned: PinnedAffinity) -> Self {
        Self::Pinned(pinned)
    }
}

impl MemoryAffinity {
    /// Returns an unknown affinity.
    #[must_use]
    pub const fn unknown() -> Self {
        Self::Unknown
    }

    /// Returns `true` if the affinity is unknown.
    #[must_use]
    pub const fn is_unknown(self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// A `PinnedAffinity` represents a specific binding to a processor and memory region.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PinnedAffinity {
    processor_index: u16,
    memory_region_index: u16,

    processor_count: u16,
    memory_region_count: u16,
}

impl PinnedAffinity {
    // You should probably use the `ThreadRegistry` or `create_manual_pinned_affinities` to create these.
    #[allow(dead_code, reason = "Used by feature gated function.")]
    #[must_use]
    pub(crate) const fn new(processor_index: u16, memory_region_index: u16, processor_count: u16, memory_region_count: u16) -> Self {
        Self {
            processor_index,
            memory_region_index,
            processor_count,
            memory_region_count,
        }
    }

    /// Returns the processor index of this affinity.
    #[must_use]
    pub const fn processor_index(self) -> usize {
        self.processor_index as _
    }

    /// Returns the memory region index of this affinity.
    #[must_use]
    pub const fn memory_region_index(self) -> usize {
        self.memory_region_index as _
    }

    /// Returns the processor count of this affinity.
    #[must_use]
    pub const fn processor_count(self) -> usize {
        self.processor_count as _
    }

    /// Returns the number of memory regions of this affinity.
    #[must_use]
    pub const fn memory_region_count(self) -> usize {
        self.memory_region_count as _
    }
}

/// Create pinned affinities manually when not using the `ThreadRegistry`.
///
/// # Parameters
///
/// * `counts`: A slice of usize representing the number of processors in each memory region.
///
/// # Panics
///
/// If there are more than `u16::MAX` processors or memory regions.
#[must_use]
#[expect(clippy::needless_range_loop, reason = "clearer in this case")]
pub fn pinned_affinities(counts: &[usize]) -> Vec<PinnedAffinity> {
    let numa_count = counts.len();
    let core_count = counts.iter().sum();
    let mut affinities = Vec::with_capacity(core_count);
    let mut processor_index = 0;

    for numa_index in 0..numa_count {
        for _ in 0..counts[numa_index] {
            affinities.push(PinnedAffinity::new(
                processor_index.try_into().expect("Too many processors"),
                numa_index.try_into().expect("Too many memory regions"),
                core_count.try_into().expect("Too many processors"),
                numa_count.try_into().expect("Too many memory regions"),
            ));
            processor_index += 1;
        }
    }

    affinities
}

/// Create memory affinities manually when not using the `ThreadRegistry`.
///
/// This is similar to `create_manual_pinned_affinities` but returns `MemoryAffinity` values.
///
/// # Parameters
///
/// * `counts`: A slice of usize representing the number of processors in each memory region.
///
/// # Panics
///
/// If there are more than `u16::MAX` processors or memory regions.
#[must_use]
pub fn memory_affinities(counts: &[usize]) -> Vec<MemoryAffinity> {
    pinned_affinities(counts).into_iter().map(MemoryAffinity::Pinned).collect()
}

#[cfg(test)]
mod tests {
    use super::{MemoryAffinity, PinnedAffinity};

    #[test]
    fn test_pinned_affinity() {
        let affinity = PinnedAffinity::new(2, 1, 4, 2);

        assert_eq!(affinity.processor_index(), 2);
        assert_eq!(affinity.processor_count(), 4);

        assert_eq!(affinity.memory_region_index(), 1);
        assert_eq!(affinity.memory_region_count(), 2);
    }

    #[test]
    fn test_memory_affinity_unknown() {
        let affinity = MemoryAffinity::unknown();
        assert!(affinity.is_unknown());
    }

    #[test]
    fn test_memory_affinity_pinned() {
        let affinity = PinnedAffinity::new(2, 1, 4, 2);
        let affinity = MemoryAffinity::from(affinity);
        assert!(!affinity.is_unknown());
    }
}
