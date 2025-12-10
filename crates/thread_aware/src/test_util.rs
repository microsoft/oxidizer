// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Helpers mainly used for testing thread-aware types without runtimes.

use crate::{MemoryAffinity, PinnedAffinity};

/// Create pinned affinities for testing purposes or when not using the `ThreadRegistry`.
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
pub fn create_manual_pinned_affinities(counts: &[usize]) -> Vec<PinnedAffinity> {
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

/// Create memory affinities for testing purposes or when not using the `ThreadRegistry`.
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
pub fn create_manual_memory_affinities(counts: &[usize]) -> Vec<MemoryAffinity> {
    create_manual_pinned_affinities(counts)
        .into_iter()
        .map(MemoryAffinity::Pinned)
        .collect()
}
