// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZero;

/// The number of processors to use for the runtime.
///
/// This can be set to `Auto` to use the default number of processors,
/// or `Manual` to specify a specific number of processors.
/// The `All` variant is used to specify that all processors should be used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProcessorCount {
    /// Use the default number of processors. Right now this is equivalent to using
    /// all processors, but this default may change in the future.
    #[default]
    Auto,
    /// Use a specific number of processors.
    Manual(NonZero<usize>),
    /// Use all processors.
    All,
}

/// Resource quota configuration for the runtime.
///
/// # Examples
///
/// ```rust
/// use oxidizer_rt::ResourceQuota;
/// use oxidizer_rt::RuntimeBuilder;
/// use oxidizer_rt::BasicThreadState;
///
/// RuntimeBuilder::new()
/// // Configure the runtime to run on 6 processors.
/// .with_resource_quota(ResourceQuota::new().with_num_processors(6))
/// .build()
/// .expect("Failed to create runtime")
/// .run(async move |cx: BasicThreadState| {
///    println!("Hello, world!");
/// });
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ResourceQuota {
    /// The number of processors to use for the runtime.
    num_processors: ProcessorCount,
}

impl ResourceQuota {
    /// Creates a new `ResourceQuota` that aims to strike a balance between performance and resource usage.
    /// By default, it will use 4 processors.
    ///
    /// This behaviour is due to change in the future, as we will be using the system's capabilities
    /// to determine the optimal number of processors to use.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            num_processors: ProcessorCount::Auto,
        }
    }

    /// Configures the number of processors to use for the runtime.
    ///
    /// If the system has less than this number of processors,
    /// the `RuntimeBuilder` will panic on `build()`.
    ///
    /// # Panics
    ///
    /// This function will panic if the number of processors is zero.
    #[must_use]
    #[expect(clippy::unused_self, reason = "for future expansion")]
    pub const fn with_num_processors(self, count: usize) -> Self {
        let count = NonZero::new(count).expect("count must be non-zero");

        Self {
            num_processors: ProcessorCount::Manual(count),
        }
    }

    /// Configures the runtime to use all processors.
    #[must_use]
    #[expect(clippy::unused_self, reason = "for future expansion")]
    pub const fn with_all_processors(self) -> Self {
        Self {
            num_processors: ProcessorCount::All,
        }
    }
}

impl Default for ResourceQuota {
    fn default() -> Self {
        Self::new()
    }
}

/// A set of processors that can be used to pin threads to specific processors.
#[derive(Debug, Clone)]
pub struct ProcessorSet {
    inner: many_cpus::ProcessorSet,
}

impl ProcessorSet {
    pub(crate) const fn new(inner: many_cpus::ProcessorSet) -> Self {
        Self { inner }
    }

    pub(crate) fn from_processor(processor: many_cpus::Processor) -> Self {
        Self {
            inner: many_cpus::ProcessorSet::from_processor(processor),
        }
    }

    /// Modifies the affinity of the current thread to execute only on the processors in this processor set.
    pub fn pin_current_thread_to(&self) {
        self.inner.pin_current_thread_to();
    }

    /// The number of processors in this set.
    pub(crate) fn count(&self) -> usize {
        self.inner.processors().len()
    }

    /// Returns an iterator over the processors in this set.
    pub(crate) fn to_processors(&self) -> impl Iterator<Item = many_cpus::Processor> {
        self.inner.processors().into_iter().cloned()
    }
}

// TODO: Once we introduce more resource quota options, we should
// consider changing this to return a `Result<ProcessorSet, _>`
// to handle errors more gracefully.

///
/// # Panics
///
/// TODO: Document panics.
#[must_use]
pub fn processor_set_from_config(config: &ResourceQuota) -> ProcessorSet {
    let builder = many_cpus::ProcessorSet::builder();

    let set = match config.num_processors {
        ProcessorCount::Manual(count) => builder.take(count),
        ProcessorCount::All | ProcessorCount::Auto => builder.take_all(),
    }
    .expect("Not enough processors available"); // Right now this is fine, considering `RuntimeBuilder::build()` does not return a `Result`.

    ProcessorSet::new(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn test_num_processors_panics_on_zero() {
        let _ = ResourceQuota::new().with_num_processors(0);
    }

    #[test]
    fn test_all_processors() {
        let config = ResourceQuota::new().with_all_processors();
        assert_eq!(config.num_processors, ProcessorCount::All);
    }

    #[cfg(not(miri))] // ProcessorSet requires talking to the real OS, which Miri cannot do.
    #[test]
    fn processor_set_count() {
        fn inner_test(count: usize) {
            let config = ResourceQuota::new().with_num_processors(count);
            let processor_set = processor_set_from_config(&config);
            assert_eq!(processor_set.count(), count);
        }

        inner_test(1);
        inner_test(2);
    }

    #[cfg(not(miri))] // ProcessorSet requires talking to the real OS, which Miri cannot do.
    #[test]
    fn processor_set_pin_current_thread() {
        let config = ResourceQuota::new().with_num_processors(1);
        let processor_set = processor_set_from_config(&config);

        std::thread::spawn(move || {
            processor_set.pin_current_thread_to();

            assert!(many_cpus::HardwareTracker::is_thread_processor_pinned());
        })
        .join()
        .unwrap();
    }
}