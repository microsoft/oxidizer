// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

use many_cpus::ProcessorSet;

use crate::rt::runtime::config::WorkerPoolPolicy;
use crate::rt::runtime::error::BuildError;

/// Selects how many processors the runtime uses, with one asynchronous worker per processor.
///
/// Pass this value to
/// [`RuntimeBuilder::processor_count`](crate::rt::config::RuntimeBuilder::processor_count).
/// The default is [`Self::auto`], which lets the runtime choose the processor count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessorCount(ProcessorCountKind);

impl ProcessorCount {
    /// Uses exactly `count` processors.
    ///
    /// Runtime construction returns [`BuildError`] when fewer processors are available.
    #[must_use]
    pub const fn exactly(count: NonZero<usize>) -> Self {
        Self(ProcessorCountKind::Exactly(count))
    }

    /// Uses at most `count` processors, clamping to those available on the machine.
    #[must_use]
    pub const fn at_most(count: NonZero<usize>) -> Self {
        Self(ProcessorCountKind::AtMost(count))
    }

    /// Uses all available processors.
    #[must_use]
    pub const fn all() -> Self {
        Self(ProcessorCountKind::All)
    }

    /// Lets the runtime choose the processor count (the default).
    ///
    /// The selection policy may evolve. It currently uses all available processors.
    /// Use an explicit policy when the processor count matters to the application.
    #[must_use]
    pub const fn auto() -> Self {
        Self(ProcessorCountKind::Auto)
    }

    pub(crate) fn select(&self, available: &ProcessorSet) -> Result<ProcessorSet, BuildError> {
        let count = match self.0 {
            ProcessorCountKind::Exactly(count) if count.get() > available.len() => {
                return Err(BuildError::insufficient_processors(count.get(), available.len()));
            }
            ProcessorCountKind::Exactly(count) | ProcessorCountKind::AtMost(count) => count.get().min(available.len()),
            ProcessorCountKind::Auto | ProcessorCountKind::All => {
                return Ok(available.to_builder().take_all().expect("available processor sets are nonempty"));
            }
        };
        let count = NonZero::new(count).expect("available processor sets are nonempty");
        Ok(available
            .to_builder()
            .take(count)
            .expect("the requested count fits in the available processor set"))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ProcessorCountKind {
    #[default]
    Auto,
    Exactly(NonZero<usize>),
    AtMost(NonZero<usize>),
    All,
}

/// Validated resource settings, independent of live runtime state.
#[derive(Debug, PartialEq)]
pub(crate) struct RuntimeConfig {
    pub(crate) num_processors: ProcessorCount,
    pub(crate) stack_size: usize,
    pub(crate) worker_pool_policy: WorkerPoolPolicy,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            num_processors: ProcessorCount::default(),
            // Match Rust's Tier-1 thread-stack baseline instead of platform-native defaults.
            // The builder can override it; bootstrap also honors a larger RUST_MIN_STACK.
            stack_size: 2 * 1024 * 1024,
            worker_pool_policy: WorkerPoolPolicy::isolated(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    #[cfg(not(miri))]
    use many_cpus::SystemHardware;

    use super::*;

    #[test]
    fn default_count_is_automatic() {
        const AUTOMATIC: ProcessorCount = ProcessorCount::auto();
        assert_eq!(ProcessorCount::default(), AUTOMATIC);
    }

    #[cfg(not(miri))]
    #[test]
    fn exact_count_selects_requested_processors() {
        const COUNT: ProcessorCount = ProcessorCount::exactly(NonZero::new(1).unwrap());
        let available = SystemHardware::current().processors();
        let selected = COUNT.select(&available).unwrap();
        assert_eq!(selected.len(), 1);
    }

    #[cfg(not(miri))]
    #[test]
    fn exact_count_accepts_all_available_processors() {
        let available = SystemHardware::current().processors().take(NonZero::new(1).unwrap()).unwrap();
        let selected = ProcessorCount::exactly(NonZero::new(available.len()).unwrap())
            .select(&available)
            .unwrap();

        assert_eq!(selected.len(), available.len());
    }

    #[cfg(not(miri))]
    #[test]
    fn unavailable_exact_count_returns_error() {
        let available = SystemHardware::current().processors().take(NonZero::new(1).unwrap()).unwrap();
        ProcessorCount::exactly(NonZero::new(2).unwrap()).select(&available).unwrap_err();
    }

    #[cfg(not(miri))]
    #[test]
    fn maximum_count_clamps_to_available_processors() {
        let available = SystemHardware::current().processors();
        let selected = ProcessorCount::at_most(NonZero::new(usize::MAX).unwrap())
            .select(&available)
            .unwrap();
        assert_eq!(selected.len(), available.len());
    }

    #[cfg(not(miri))]
    #[test]
    fn maximum_count_caps_processor_count() {
        const COUNT: ProcessorCount = ProcessorCount::at_most(NonZero::new(1).unwrap());
        let available = SystemHardware::current().processors();
        let selected = COUNT.select(&available).unwrap();
        assert_eq!(selected.len(), 1);
    }

    #[cfg(not(miri))]
    #[test]
    fn automatic_count_uses_all_available_processors() {
        let available = SystemHardware::current().processors();
        assert_eq!(ProcessorCount::auto().select(&available).unwrap().len(), available.len(),);
    }

    #[cfg(not(miri))]
    #[test]
    fn all_processors_uses_all_available_processors() {
        const COUNT: ProcessorCount = ProcessorCount::all();
        let available = SystemHardware::current().processors();
        assert_eq!(COUNT.select(&available).unwrap().len(), available.len());
    }
}
