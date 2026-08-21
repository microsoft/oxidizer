// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use crate::affinity::Affinity;
use crate::cell::Strategy;

/// A strategy that stores data per processor core / thread.
///
/// This strategy uses the processor index and count from the `Affinity` to determine
/// where to store and retrieve data.
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

/// A strategy that stores data per memory region.
///
/// This strategy uses the memory region index and count from the `Affinity` to determine
/// where to store and retrieve data.
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

/// A strategy that stores data per process.
///
/// This strategy does not differentiate between affinities, storing all data in a single slot.
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
