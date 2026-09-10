// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Dependencies and implementation details used by macro expansions.

pub use alloc_tracker::Allocator;
pub use criterion;
pub use gungraun;

pub use crate::allocation::begin as begin_allocation_measurement;
pub use crate::perf::begin as begin_perf_measurement;
pub use crate::runner::{EngineSet, run};

#[must_use]
pub fn default_criterion() -> criterion::Criterion {
    criterion::Criterion::default()
}
