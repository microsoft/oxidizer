// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::{NonZero, NonZeroUsize};

use many_cpus::ProcessorSet;

use crate::runtime::config::{BlockingPoolPolicy, MonitorOptions, PanicPolicy};
use crate::runtime::error::Error;

/// A worker-count policy for the async runtime.
///
/// The runtime assigns each async worker to one available processor. More than
/// one worker can share a processor when an exact count exceeds the available
/// processor count. Pass a policy to
/// [`RuntimeBuilder::workers`](crate::runtime::RuntimeBuilder::workers).
/// [`Default`] uses one worker. Use [`at_most`](Self::at_most) for
/// an upper bound, [`exactly`](Self::exactly) for a fixed worker count, or
/// [`all`](Self::all) to use all available processors.
///
/// Counts are validated when the runtime is built. Both `at_most(0)` and
/// `exactly(0)` produce a construction error.
///
/// # Examples
///
/// ```
/// use arty::runtime::{Runtime, WorkersPolicy};
///
/// let builder = Runtime::builder().workers(WorkersPolicy::at_most(4));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkersPolicy(WorkersPolicyKind);

impl WorkersPolicy {
    /// Creates a policy requiring exactly `count` workers.
    ///
    /// Each worker is a separate operating-system thread. If `count` exceeds
    /// the available processor count, Arty assigns workers to processors in
    /// repeated processor-ID order. The operating system schedules the workers
    /// that share each processor.
    ///
    /// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build) returns
    /// [`Error`] if `count` is zero. Creating the policy does not validate the
    /// count. Large counts can consume substantial thread and stack resources.
    /// Use [`at_most`](Self::at_most) if the application can work with fewer
    /// workers.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::exactly(4));
    /// ```
    #[must_use]
    pub const fn exactly(count: usize) -> Self {
        Self(WorkersPolicyKind::Exactly(count))
    }

    /// Creates a policy using at most `count` workers.
    ///
    /// Fewer available processors means fewer workers, not a construction error.
    /// A zero count is rejected by
    /// [`RuntimeBuilder::build`](crate::runtime::RuntimeBuilder::build), not when
    /// creating this policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::at_most(4));
    /// ```
    #[must_use]
    pub const fn at_most(count: usize) -> Self {
        Self(WorkersPolicyKind::AtMost(count))
    }

    /// Creates a policy using all available processors.
    ///
    /// This explicitly selects all processors rather than relying on the
    /// default policy.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::all());
    /// ```
    #[must_use]
    pub const fn all() -> Self {
        Self(WorkersPolicyKind::All)
    }

    pub(crate) fn select(&self, available: &ProcessorSet) -> Result<WorkerPlacements, Error> {
        match self.0 {
            WorkersPolicyKind::Exactly(0) | WorkersPolicyKind::AtMost(0) => Err(Error::invalid_worker_count()),
            WorkersPolicyKind::Exactly(count) => {
                let processors = sorted_singletons(available);
                let workers = processors.iter().cycle().take(count).cloned().collect();
                Ok(WorkerPlacements {
                    workers,
                    #[cfg(test)]
                    processors_used: count.min(processors.len()),
                })
            }
            WorkersPolicyKind::AtMost(count) => {
                let count = count.min(available.len());
                let count = NonZero::new(count).expect("zero counts were rejected above and available processor sets are nonempty");
                let selected = available
                    .to_builder()
                    .take(count)
                    .expect("the requested count fits in the available processor set");
                Ok(WorkerPlacements::from_selected(&selected))
            }
            WorkersPolicyKind::All => Ok(WorkerPlacements::from_selected(available)),
        }
    }
}

impl Default for WorkersPolicy {
    fn default() -> Self {
        // Keep implicit runtime construction independent of host processor count.
        // Applications opt into more worker threads when they need them.
        Self::exactly(1)
    }
}

fn sorted_singletons(processors: &ProcessorSet) -> Vec<ProcessorSet> {
    let mut processors: Vec<_> = processors.decompose().into_iter().collect();
    processors.sort_by_key(|processor| processor.processors().first().id());
    processors
}

#[derive(Debug)]
pub(crate) struct WorkerPlacements {
    workers: Vec<ProcessorSet>,
    #[cfg(test)]
    processors_used: usize,
}

impl WorkerPlacements {
    fn from_selected(processors: &ProcessorSet) -> Self {
        Self {
            workers: sorted_singletons(processors),
            #[cfg(test)]
            processors_used: processors.len(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.workers.len()
    }

    #[cfg(test)]
    pub(crate) const fn processors_used(&self) -> usize {
        self.processors_used
    }
}

impl IntoIterator for WorkerPlacements {
    type Item = ProcessorSet;
    type IntoIter = std::vec::IntoIter<ProcessorSet>;

    fn into_iter(self) -> Self::IntoIter {
        self.workers.into_iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkersPolicyKind {
    Exactly(usize),
    AtMost(usize),
    All,
}

/// Resource settings validated during runtime construction.
#[derive(Debug)]
pub(crate) struct RuntimeConfig {
    pub(crate) workers_policy: WorkersPolicy,
    pub(crate) stack_size: NonZeroUsize,
    pub(crate) blocking_pool_policy: BlockingPoolPolicy,
    pub(crate) monitor_options: MonitorOptions,
    pub(crate) panic_policy: PanicPolicy,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            workers_policy: WorkersPolicy::default(),
            // Match Rust's Tier-1 thread-stack baseline instead of platform-native defaults.
            // The builder can override it; bootstrap also honors a larger RUST_MIN_STACK.
            stack_size: NonZeroUsize::new(2 * 1024 * 1024).expect("the default stack size is nonzero"),
            blocking_pool_policy: BlockingPoolPolicy::default(),
            monitor_options: MonitorOptions::default(),
            panic_policy: PanicPolicy::default(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;

    use super::*;

    #[test]
    fn default_runtime_shares_blocking_pool() {
        assert_eq!(RuntimeConfig::default().blocking_pool_policy, BlockingPoolPolicy::shared());
    }

    #[test]
    fn exact_count_assigns_requested_workers() {
        let available = fake_processors(4);
        let selected = WorkersPolicy::exactly(16).select(&available).unwrap();

        assert_eq!((selected.len(), selected.processors_used()), (16, 4));
    }

    #[test]
    fn exact_count_repeats_processors_in_id_order() {
        let available = fake_processors(4);
        let processor_ids: Vec<_> = WorkersPolicy::exactly(10)
            .select(&available)
            .unwrap()
            .into_iter()
            .map(|processor| processor.processors().first().id())
            .collect();

        assert_eq!(processor_ids, vec![0, 1, 2, 3, 0, 1, 2, 3, 0, 1]);
    }

    #[test]
    fn exact_count_can_use_fewer_than_all_processors() {
        let available = fake_processors(4);
        let selected = WorkersPolicy::exactly(2).select(&available).unwrap();

        assert_eq!((selected.len(), selected.processors_used()), (2, 2));
    }

    #[test]
    fn exact_count_matching_capacity_keeps_one_worker_per_processor() {
        let available = fake_processors(8);
        let exact: Vec<_> = WorkersPolicy::exactly(8)
            .select(&available)
            .unwrap()
            .into_iter()
            .map(|processor| processor.processors().first().id())
            .collect();
        let maximum: Vec<_> = WorkersPolicy::at_most(8)
            .select(&available)
            .unwrap()
            .into_iter()
            .map(|processor| processor.processors().first().id())
            .collect();

        assert_eq!(exact, maximum);
        assert_eq!(exact, (0..8).collect::<Vec<_>>());
    }

    #[test]
    fn maximum_count_clamps_to_available_processors() {
        let available = fake_processors(4);
        let selected = WorkersPolicy::at_most(usize::MAX).select(&available).unwrap();

        assert_eq!((selected.len(), selected.processors_used()), (4, 4));
    }

    #[test]
    fn maximum_count_caps_processor_selection() {
        const COUNT: WorkersPolicy = WorkersPolicy::at_most(1);
        let available = fake_processors(4);
        let selected = COUNT.select(&available).unwrap();

        assert_eq!((selected.len(), selected.processors_used()), (1, 1));
    }

    #[test]
    fn default_count_uses_one_processor() {
        let available = fake_processors(4);
        assert_eq!(WorkersPolicy::default().select(&available).unwrap().len(), 1);
    }

    #[test]
    fn all_processors_uses_all_available_processors() {
        const COUNT: WorkersPolicy = WorkersPolicy::all();
        let available = fake_processors(4);
        assert_eq!(COUNT.select(&available).unwrap().len(), available.len());
    }

    fn fake_processors(count: usize) -> ProcessorSet {
        SystemHardware::fake(HardwareBuilder::from_counts(NonZero::new(count).unwrap(), NonZero::new(1).unwrap())).processors()
    }
}
