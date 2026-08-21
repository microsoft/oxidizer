// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Affinity`] identifier and the [`pinned_affinities`] topology helper.

/// Identifies a processor and memory region in an application's affinity topology.
///
/// Indices are zero-based and counts describe the complete topology known to the
/// runtime that created the value. An `Affinity` is a logical identifier; it does
/// not pin the current thread or change operating-system scheduling.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Affinity {
    processor_index: u16,
    memory_region_index: u16,
    processor_count: u16,
    memory_region_count: u16,
}

impl Affinity {
    /// Creates an affinity from zero-based indices and topology counts.
    ///
    /// Affinities compared or passed to the same thread-aware component should
    /// describe the same topology. Use consistent processor and memory-region
    /// counts when constructing related values.
    ///
    /// # Panics
    ///
    /// Panics if either count is zero or an index is outside its corresponding
    /// count.
    #[must_use]
    pub(crate) const fn new(processor_index: u16, memory_region_index: u16, processor_count: u16, memory_region_count: u16) -> Self {
        assert!(processor_count > 0, "processor count must be nonzero");
        assert!(memory_region_count > 0, "memory region count must be nonzero");
        assert!(
            processor_index < processor_count,
            "processor index must be less than processor count"
        );
        assert!(
            memory_region_index < memory_region_count,
            "memory region index must be less than memory region count"
        );

        Self {
            processor_index,
            memory_region_index,
            processor_count,
            memory_region_count,
        }
    }

    /// Returns the zero-based processor index.
    #[must_use]
    pub const fn processor_index(self) -> usize {
        self.processor_index as _
    }

    /// Returns the zero-based memory-region index.
    #[must_use]
    pub const fn memory_region_index(self) -> usize {
        self.memory_region_index as _
    }

    /// Returns the number of processors in the topology.
    #[must_use]
    pub const fn processor_count(self) -> usize {
        self.processor_count as _
    }

    /// Returns the number of memory regions in the topology.
    #[must_use]
    pub const fn memory_region_count(self) -> usize {
        self.memory_region_count as _
    }
}

/// Creates affinities for a topology described by per-memory-region processor counts.
///
/// `counts` contains the number of processors in each memory region.
///
/// # Panics
///
/// Panics if there are more than `u16::MAX` processors or memory regions.
#[must_use]
#[expect(clippy::needless_range_loop, reason = "clearer in this case")]
pub fn pinned_affinities(counts: &[usize]) -> alloc::vec::Vec<Affinity> {
    let memory_region_count = counts.len();
    let processor_count = counts.iter().sum();
    let mut affinities = alloc::vec::Vec::with_capacity(processor_count);
    let mut processor_index = 0;

    for memory_region_index in 0..memory_region_count {
        for _ in 0..counts[memory_region_index] {
            affinities.push(Affinity::new(
                processor_index.try_into().expect("too many processors"),
                memory_region_index.try_into().expect("too many memory regions"),
                processor_count.try_into().expect("too many processors"),
                memory_region_count.try_into().expect("too many memory regions"),
            ));
            processor_index += 1;
        }
    }

    affinities
}

#[cfg(test)]
mod tests {
    use super::{Affinity, pinned_affinities};

    #[test]
    fn affinity_exposes_topology() {
        let affinity = Affinity::new(2, 0, 4, 2);
        let other_memory_region = Affinity::new(3, 1, 4, 2);

        assert_eq!(affinity.processor_index(), 2);
        assert_eq!(affinity.memory_region_index(), 0);
        assert_eq!(other_memory_region.memory_region_index(), 1);
        assert_eq!(affinity.processor_count(), 4);
        assert_eq!(affinity.memory_region_count(), 2);
    }

    #[test]
    #[should_panic(expected = "processor index must be less than processor count")]
    fn affinity_rejects_out_of_range_processor() {
        let _ = Affinity::new(4, 0, 4, 1);
    }

    #[test]
    fn pinned_affinities_describe_topology() {
        let affinities = pinned_affinities(&[2, 1]);

        assert_eq!(affinities.len(), 3);
        assert_eq!(affinities[0], Affinity::new(0, 0, 3, 2));
        assert_eq!(affinities[1], Affinity::new(1, 0, 3, 2));
        assert_eq!(affinities[2], Affinity::new(2, 1, 3, 2));
    }
}
