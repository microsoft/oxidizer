// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Dependencies and implementation details used by macro expansions.

pub use criterion;
pub use gungraun;

pub use crate::allocation::begin as begin_allocation_measurement;
pub use crate::allocator::Allocator;
pub use crate::gungraun_perf::run as run_gungraun_perf;
pub use crate::perf::begin as begin_perf_measurement;
pub use crate::probe::{ProbeBenchmark, ProbeGroup, run_probe_groups};
pub use crate::runner::{EngineSet, run};
pub use crate::vtune::begin as begin_vtune_measurement;

#[must_use]
pub fn default_criterion() -> criterion::Criterion {
    criterion::Criterion::default()
}
