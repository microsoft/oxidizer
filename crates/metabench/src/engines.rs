// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use bitflags::bitflags;

use crate::Mode;

bitflags! {
    /// Benchmark engines that may execute a registered workload.
    ///
    /// # Example
    ///
    /// ```
    /// use metabench::Engines;
    ///
    /// let deterministic = Engines::GUNGRAUN | Engines::ALLOCATIONS;
    /// assert!(deterministic.contains(Engines::GUNGRAUN));
    /// assert!(!deterministic.contains(Engines::PERF));
    /// ```
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct Engines: u8 {
        /// Criterion statistical wall-clock measurement.
        const CRITERION = 1 << 0;
        /// Gungraun synthetic instrumentation using Callgrind.
        const GUNGRAUN = 1 << 1;
        /// Native Linux perf hardware counters.
        const PERF = 1 << 2;
        /// Process-wide allocation tracking.
        const ALLOCATIONS = 1 << 3;
        /// Default engines: Criterion, Gungraun, and allocation tracking.
        const DEFAULT = Self::CRITERION.bits()
            | Self::GUNGRAUN.bits()
            | Self::ALLOCATIONS.bits();
        /// Every benchmark engine.
        const ALL = Self::DEFAULT.bits() | Self::PERF.bits();
    }
}

impl Default for Engines {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Engines {
    pub(crate) const fn contains_mode(self, mode: Mode) -> bool {
        self.contains(match mode {
            Mode::Criterion => Self::CRITERION,
            Mode::Callgrind => Self::GUNGRAUN,
            Mode::Perf => Self::PERF,
            Mode::Allocations => Self::ALLOCATIONS,
        })
    }
}
