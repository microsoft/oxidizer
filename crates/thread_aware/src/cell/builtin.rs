// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use crate::affinity::Affinity;
use crate::cell::Strategy;

/// Defines one strategy partition per processor core.
///
/// Affinities with the same processor index map to the same partition. This is the default strategy
/// used by [`Arc`](crate::Arc).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerCore;

impl Strategy for PerCore {
    fn index(affinity: Affinity) -> usize {
        affinity.processor_index()
    }

    fn count(affinity: Affinity) -> NonZero<usize> {
        // A machine always has at least one processor, so the count is never zero.
        NonZero::new(affinity.processor_count()).expect("a machine always reports at least one processor")
    }
}

/// Defines one strategy partition per memory region (NUMA node).
///
/// Affinities with the same memory-region index map to the same partition.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerNuma;

impl Strategy for PerNuma {
    fn index(affinity: Affinity) -> usize {
        affinity.memory_region_index()
    }

    fn count(affinity: Affinity) -> NonZero<usize> {
        // A machine always has at least one memory region, so the count is never zero.
        NonZero::new(affinity.memory_region_count()).expect("a machine always reports at least one memory region")
    }
}

/// Defines one strategy partition for the entire process.
///
/// All affinities map to the same partition.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct PerProcess;

impl Strategy for PerProcess {
    fn index(_affinity: Affinity) -> usize {
        0
    }

    fn count(_affinity: Affinity) -> NonZero<usize> {
        NonZero::<usize>::MIN
    }
}
