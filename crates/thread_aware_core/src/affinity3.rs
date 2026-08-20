// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Experimental affinity model (iteration 3).
//!
//! This module keeps the concrete, named-field shape of the base
//! [`Affinity`](crate::Affinity) but makes two changes:
//!
//! - **No counts:** the topology size is not part of the value. An affinity identifies
//!   a processor and a memory region; it does not describe how many of either exist.
//!   Dropping the counts also removes the index-versus-count validation, so
//!   construction is infallible.
//! - **Ids, not indices:** the fields are `processor_id`/`memory_region_id`, not
//!   `*_index`. They hold the real identifiers reported by the platform, which may be
//!   sparse (for example, a machine that exposes only processors `1` and `399`) rather
//!   than dense zero-based ordinals. Because they are identifiers, they are not
//!   intended to index into a densely sized array.
//!
//! The fields keep the base type's `u16`, and the accessors return the same `u16` so
//! there is no width mismatch between what is stored and what is read.

/// Identifies a processor and memory region by their real platform ids.
///
/// An `Affinity` is a pair of identifiers, not a pin: it does not bind a thread or
/// change operating-system scheduling. The ids are whatever the producing runtime
/// reports and need not be dense or zero-based.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Affinity {
    processor_id: u16,
    memory_region_id: u16,
}

impl Affinity {
    /// Creates an affinity from a processor id and a memory-region id.
    ///
    /// The ids are the identifiers reported by the platform; they need not be dense or
    /// zero-based, so no range validation is performed.
    #[must_use]
    pub const fn new(processor_id: u16, memory_region_id: u16) -> Self {
        Self {
            processor_id,
            memory_region_id,
        }
    }

    /// Returns the processor id.
    #[must_use]
    pub const fn processor_id(self) -> u16 {
        self.processor_id
    }

    /// Returns the memory-region id.
    #[must_use]
    pub const fn memory_region_id(self) -> u16 {
        self.memory_region_id
    }
}

#[cfg(test)]
mod tests {
    use super::Affinity;

    #[test]
    fn exposes_supplied_ids() {
        let affinity = Affinity::new(2, 1);

        assert_eq!(affinity.processor_id(), 2);
        assert_eq!(affinity.memory_region_id(), 1);
    }

    #[test]
    fn preserves_sparse_ids() {
        // Real platform ids may be sparse; nothing is normalized to a dense range.
        let affinity = Affinity::new(399, 7);

        assert_eq!(affinity.processor_id(), 399);
        assert_eq!(affinity.memory_region_id(), 7);
    }

    #[test]
    fn equality_compares_both_ids() {
        let affinity = Affinity::new(4, 0);

        assert_eq!(affinity, Affinity::new(4, 0));
        assert_ne!(affinity, Affinity::new(4, 1));
        assert_ne!(affinity, Affinity::new(5, 0));
    }
}
