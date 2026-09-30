// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::Sink;
use thread_aware::ThreadBuilder;
use tick::runtime::InactiveClock;

use crate::runtime::bootstrap;
use crate::runtime::config::{BlockingPoolPolicy, ProcessorCount, RuntimeConfig};
use crate::runtime::error::Error;
use crate::runtime::handle::Runtime;

/// Configures workers, clocks, and telemetry before starting a runtime.
///
/// By default, the runtime uses [`ProcessorCount::auto`], 2 MiB worker stacks,
/// isolated blocking worker pools, and a noop telemetry sink.
///
/// See the [documentation guides](crate#documentation) for the
/// resource trade-offs between processor and blocking-pool policies.
///
/// # Examples
///
/// ```
/// use std::num::NonZeroUsize;
///
/// use arty::runtime::{Error, ProcessorCount, Runtime};
///
/// let runtime = Runtime::builder()
///     .processor_count(ProcessorCount::at_most(NonZeroUsize::new(4).unwrap()))
///     .build()?;
/// runtime.run(async |_| {});
/// # Ok::<(), Error>(())
/// ```
#[derive(Debug)]
pub struct RuntimeBuilder {
    processor_config: RuntimeConfig,
    clock: InactiveClock,
    sink: Sink,
}

impl RuntimeBuilder {
    /// Selects how many processors the runtime uses, with one asynchronous worker per processor.
    ///
    /// This replaces any earlier processor-count setting.
    /// The default is [`ProcessorCount::auto`].
    #[must_use]
    pub const fn processor_count(mut self, count: ProcessorCount) -> Self {
        self.processor_config.num_processors = count;
        self
    }

    /// Sets each asynchronous worker thread's stack size in bytes.
    ///
    /// This does not configure blocking-task pools.
    ///
    /// A larger value of the `RUST_MIN_STACK` environment variable takes precedence.
    /// The default is 2 MiB.
    ///
    /// # Panics
    ///
    /// Panics if `size` is zero.
    #[must_use]
    pub const fn stack_size(mut self, size: usize) -> Self {
        assert!(size > 0, "stack size must be greater than zero");
        self.processor_config.stack_size = size;
        self
    }

    /// Selects how blocking tasks share worker pools.
    ///
    /// The default is [`BlockingPoolPolicy::isolated`].
    #[must_use]
    pub const fn blocking_pool_policy(mut self, policy: BlockingPoolPolicy) -> Self {
        self.processor_config.blocking_pool_policy = policy;
        self
    }

    /// Sets the clock used by runtime workers.
    ///
    /// An [`InactiveClock`] is cloned to each asynchronous worker and activated
    /// there. The runtime drives the resulting worker clocks.
    ///
    /// # Examples
    ///
    /// Enable `test-util` in dev-dependencies to use `arty::time::ClockControl`.
    /// Automatic timer advancement is useful for sequential delays; it is not
    /// idle-runtime advancement or a simulation of concurrent deadline ordering.
    /// See the [documentation guides](crate#documentation) for manual time control.
    ///
    /// ```
    /// # fn main() {
    /// # #[cfg(all(feature = "rt", feature = "test-util"))] {
    /// # (|| {
    /// use std::time::Duration;
    ///
    /// use arty::runtime::Runtime;
    /// use arty::time::ClockControl;
    ///
    /// let control = ClockControl::new().auto_advance_timers(true);
    /// let runtime = Runtime::builder().clock(control).build()?;
    /// runtime.run(async |cx| {
    ///     let watch = cx.clock().stopwatch();
    ///     cx.clock().delay(Duration::from_secs(30)).await;
    ///     assert_eq!(watch.elapsed(), Duration::from_secs(30));
    /// });
    ///
    /// # Ok::<(), arty::runtime::Error>(())
    /// # })().unwrap();
    /// # }
    /// # }
    /// ```
    #[must_use]
    pub fn clock(mut self, clock: impl Into<InactiveClock>) -> Self {
        self.clock = clock.into();
        self
    }

    /// Sets the [`Sink`] for automatic enrichment propagation.
    ///
    /// When a sink is configured, every asynchronous task spawned via the runtime automatically
    /// inherits the enrichment context that was active at the spawn site. This removes the
    /// need for manual [`Sink::transfer_context`] / `.attach()` calls.
    #[must_use]
    pub fn sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    /// Builds and starts a new instance of the Arty runtime.
    ///
    /// # Errors
    ///
    /// Returns an opaque [`Error`] when the processor selection cannot be satisfied.
    ///
    /// # Panics
    ///
    /// Panics if worker-thread creation or worker initialization fails.
    pub fn build(self) -> Result<Runtime, Error> {
        bootstrap::build(self.processor_config, &self.clock, self.sink, &ThreadBuilder::default())
    }
}

impl RuntimeBuilder {
    #[must_use]
    pub(in crate::runtime) fn new() -> Self {
        Self {
            clock: InactiveClock::default(),
            processor_config: RuntimeConfig::default(),
            sink: Sink::noop(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::num::NonZeroUsize;
    #[cfg(not(miri))]
    use std::sync::Arc;
    use std::time::Duration;

    use tick::ClockControl;

    use super::*;
    #[cfg(not(miri))]
    use crate::runtime::context::Builtins;

    #[test]
    fn custom_clock_ok() {
        let clock_control = ClockControl::new();
        let builder = Runtime::builder().clock(clock_control.clone());

        let (clock, _) = builder.clock.activate();
        let now = clock.system_time();

        clock_control.advance(Duration::from_secs(1));

        assert_eq!(clock.system_time().duration_since(now).unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn resource_limits_preserve_independent_settings() {
        let builder = Runtime::builder()
            .processor_count(ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()))
            .stack_size(1024 * 1024)
            .blocking_pool_policy(BlockingPoolPolicy::shared(1));
        assert_eq!(
            builder.processor_config,
            RuntimeConfig {
                num_processors: ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()),
                stack_size: 1024 * 1024,
                blocking_pool_policy: BlockingPoolPolicy::shared(1),
            }
        );
    }

    #[test]
    fn maximum_processor_selection_replaces_exact_count() {
        let builder = Runtime::builder()
            .processor_count(ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()))
            .processor_count(ProcessorCount::at_most(NonZeroUsize::MIN));
        assert_eq!(builder.processor_config.num_processors, ProcessorCount::at_most(NonZeroUsize::MIN),);
    }

    #[test]
    fn all_processors_selection_replaces_maximum_count() {
        let builder = Runtime::builder()
            .processor_count(ProcessorCount::at_most(NonZeroUsize::MIN))
            .processor_count(ProcessorCount::all());
        assert_eq!(builder.processor_config.num_processors, ProcessorCount::all(),);
    }

    #[test]
    fn automatic_processor_selection_replaces_exact_count() {
        let builder = Runtime::builder()
            .processor_count(ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()))
            .processor_count(ProcessorCount::auto());
        assert_eq!(builder.processor_config.num_processors, ProcessorCount::auto(),);
    }

    #[test]
    fn exact_processor_selection_preserves_other_resource_settings() {
        let builder = Runtime::builder()
            .stack_size(1024 * 1024)
            .blocking_pool_policy(BlockingPoolPolicy::shared(1))
            .processor_count(ProcessorCount::at_most(NonZeroUsize::MIN))
            .processor_count(ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()));
        assert_eq!(
            builder.processor_config,
            RuntimeConfig {
                num_processors: ProcessorCount::exactly(NonZeroUsize::new(2).unwrap()),
                stack_size: 1024 * 1024,
                blocking_pool_policy: BlockingPoolPolicy::shared(1),
            }
        );
    }

    #[test]
    #[should_panic(expected = "greater than zero")]
    fn zero_stack_size_panics() {
        let _ = Runtime::builder().stack_size(0);
    }

    #[cfg(not(miri))] // can't call foreign function `CreateIoCompletionPort` on OS `windows`
    #[test]
    fn emitter_is_available_by_default() {
        let runtime = Runtime::builder().build().expect("Failed to create runtime");

        runtime.run(async move |cx: Builtins| {
            assert!(cx.sink().is_noop());
        });
    }

    #[cfg(not(miri))] // can't call foreign function `CreateIoCompletionPort` on OS `windows`
    #[test]
    fn configured_emitter_is_available_from_builtins() {
        use observed::Sink;
        use observed::metadata::EventDescription;
        use observed::processing::{EventProcessor, EventView};

        struct TestProcessor;

        impl EventProcessor for TestProcessor {
            fn is_interested(&self, _description: &EventDescription) -> bool {
                true
            }

            fn process(&self, _event: &EventView<'_>) {}

            fn flush(&self) -> Result<(), observed::FlushError> {
                Ok(())
            }
        }

        let sink = Sink::new("test", vec![Arc::new(TestProcessor)], tick::SimpleClock::new_frozen());
        let runtime = Runtime::builder().sink(sink).build().expect("Failed to create runtime");

        runtime.run(async move |cx: Builtins| {
            assert!(!cx.sink().is_noop());
            cx.sink().flush().unwrap();
        });
    }
}
