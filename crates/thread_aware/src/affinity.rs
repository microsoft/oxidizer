// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Identifiers for threads and NUMA regions.

#![allow(clippy::allow_attributes, reason = "Needed for conditional compilation")]

/// A `Affinity` represents a specific binding to a processor and memory region.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Affinity {
    processor_index: u16,
    memory_region_index: u16,

    processor_count: u16,
    memory_region_count: u16,
}

impl Affinity {
    // You should probably use the `ThreadRegistry` or `pinned_affinities` to create these.
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

/// Create memory affinities manually when not using the `ThreadRegistry`.
///
/// # Parameters
///
/// * `counts`: A slice of `usize` representing the number of processors in each memory region.
///
/// # Panics
///
/// If there are more than `u16::MAX` processors or memory regions.
#[must_use]
#[expect(clippy::needless_range_loop, reason = "clearer in this case")]
pub fn pinned_affinities(counts: &[usize]) -> Vec<Affinity> {
    let numa_count = counts.len();
    let core_count = counts.iter().sum();
    let mut affinities = Vec::with_capacity(core_count);
    let mut processor_index = 0;

    for numa_index in 0..numa_count {
        for _ in 0..counts[numa_index] {
            affinities.push(Affinity::new(
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

#[cfg(test)]
mod tests {
    use super::Affinity;

    #[test]
    fn test_memory_affinity() {
        let affinity = Affinity::new(2, 1, 4, 2);

        assert_eq!(affinity.processor_index(), 2);
        assert_eq!(affinity.processor_count(), 4);

        assert_eq!(affinity.memory_region_index(), 1);
        assert_eq!(affinity.memory_region_count(), 2);
    }
}
