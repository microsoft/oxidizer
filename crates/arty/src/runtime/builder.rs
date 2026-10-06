// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use observed::Sink;
use thread_aware::ThreadBuilder;
use tick::runtime::InactiveClock;

use crate::runtime::bootstrap;
use crate::runtime::config::{BlockingPoolPolicy, CpuPolicy, RuntimeConfig};
use crate::runtime::error::Error;
use crate::runtime::handle::Runtime;

/// Configures workers, clocks, and telemetry before starting a runtime.
///
/// Obtain a builder from [`Runtime::builder`], change the settings you need,
/// then call [`build`](Self::build) to start the workers. Setters replace earlier
/// values for the same setting; configuring a builder does not start threads.
///
/// The defaults are [`CpuPolicy::auto`], 2 MiB async-worker stacks,
/// a shared blocking pool, a real-time clock, and a no-op telemetry sink.
///
/// # Examples
///
/// ```
/// use arty::runtime::{BlockingPoolPolicy, CpuPolicy, Runtime};
///
/// let runtime = Runtime::builder()
///     .cpu_policy(CpuPolicy::at_most(4))
///     .blocking_pool_policy(BlockingPoolPolicy::shared(4))
///     .build()?;
/// assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct RuntimeBuilder {
    processor_config: RuntimeConfig,
    clock: InactiveClock,
    sink: Sink,
}

impl RuntimeBuilder {
    /// Sets the processor policy for async workers.
    ///
    /// The runtime starts one async worker per selected processor.
    /// The default is [`CpuPolicy::auto`]. This does not set blocking-pool
    /// limits; use [`blocking_pool_policy`](Self::blocking_pool_policy) for those.
    /// A zero count is rejected by [`build`](Self::build), not by this setter.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{CpuPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().cpu_policy(CpuPolicy::at_most(4));
    /// ```
    #[must_use]
    pub const fn cpu_policy(mut self, count: CpuPolicy) -> Self {
        self.processor_config.cpu_policy = count;
        self
    }

    /// Sets each async worker thread's stack size in bytes.
    ///
    /// The default is 2 MiB. A larger value of the `RUST_MIN_STACK` environment
    /// variable takes precedence. This setting does not change blocking-pool
    /// thread stacks.
    ///
    /// # Panics
    ///
    /// Panics if `size` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let builder = Runtime::builder().stack_size(4 * 1024 * 1024);
    /// ```
    #[must_use]
    pub const fn stack_size(mut self, size: usize) -> Self {
        assert!(size > 0, "stack size must be greater than zero");
        self.processor_config.stack_size = size;
        self
    }

    /// Sets whether async workers share their blocking-task pool.
    ///
    /// The default shares one pool across async workers. Use
    /// [`BlockingPoolPolicy::shared`] to set a runtime-wide thread limit, or
    /// [`BlockingPoolPolicy::isolated`] to give each worker its own pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{BlockingPoolPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().blocking_pool_policy(BlockingPoolPolicy::shared(4));
    /// ```
    #[must_use]
    pub const fn blocking_pool_policy(mut self, policy: BlockingPoolPolicy) -> Self {
        self.processor_config.blocking_pool_policy = policy;
        self
    }

    /// Sets the clock used by runtime workers.
    ///
    /// The default follows real time. Pass an [`InactiveClock`] or, with
    /// `test-util` enabled, a `ClockControl` to choose a different time source.
    /// Workers activate their own clocks and drive their timers.
    ///
    /// # Examples
    ///
    /// Enable `test-util` in dev-dependencies to test sequential delays without
    /// waiting for real time:
    ///
    /// ```test_harness
    /// # #[cfg(all(feature = "macros", feature = "rt", feature = "test-util"))]
    /// #[arty::test(builder = arty::runtime::Runtime::builder().clock(
    ///     arty::time::ClockControl::new().auto_advance_timers(true)
    /// ))]
    /// async fn sequential_delay(cx: arty::task::Builtins) {
    ///     let duration = std::time::Duration::from_secs(30);
    ///     let watch = cx.clock().stopwatch();
    ///     cx.clock().delay(duration).await;
    ///     assert_eq!(watch.elapsed(), duration);
    /// }
    /// ```
    ///
    /// Automatic timer advancement occurs eagerly on timer registration or time
    /// reads. Use manual advancement when concurrent timers' relative order
    /// matters; see the [time guide](crate::documentation::time).
    #[must_use]
    pub fn clock(mut self, clock: impl Into<InactiveClock>) -> Self {
        self.clock = clock.into();
        self
    }

    /// Sets the telemetry sink for runtime events and task enrichment.
    ///
    /// The default is [`Sink::noop`]. Async tasks inherit enrichment
    /// active on the configured sink at submission, including their completion
    /// events. Retrieve the sink inside a task with
    /// [`Builtins::sink`](crate::task::Builtins::sink).
    ///
    /// Configured event processors must not panic. Runtime events are emitted
    /// synchronously; Arty does not provide telemetry-panic recovery.
    ///
    /// # Examples
    ///
    /// Pass a sink configured by the application:
    ///
    /// ```
    /// use arty::runtime::{Runtime, RuntimeBuilder};
    ///
    /// fn app_builder(sink: observed::Sink) -> RuntimeBuilder {
    ///     Runtime::builder().sink(sink)
    /// }
    /// ```
    #[must_use]
    pub fn sink(mut self, sink: Sink) -> Self {
        self.sink = sink;
        self
    }

    /// Starts the configured workers and returns their runtime owner.
    ///
    /// Blocks the calling thread until workers are ready to receive tasks.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the processor count is zero or the policy cannot be
    /// satisfied, such as an [`exactly`](CpuPolicy::exactly) request exceeding
    /// available processors.
    ///
    /// # Panics
    ///
    /// Panics if worker-thread creation or worker initialization fails.
    /// These startup failures are not configuration errors returned by this method.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    ///
    /// let runtime = Runtime::builder().build()?;
    /// assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
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
    #[cfg(not(miri))]
    use std::sync::Arc;
    use std::time::Duration;

    use tick::ClockControl;

    use super::*;
    #[cfg(not(miri))]
    use crate::task::Builtins;

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
            .cpu_policy(CpuPolicy::exactly(2))
            .stack_size(1024 * 1024)
            .blocking_pool_policy(BlockingPoolPolicy::shared(1));
        assert_eq!(
            builder.processor_config,
            RuntimeConfig {
                cpu_policy: CpuPolicy::exactly(2),
                stack_size: 1024 * 1024,
                blocking_pool_policy: BlockingPoolPolicy::shared(1),
            }
        );
    }

    #[test]
    fn maximum_processor_selection_replaces_exact_count() {
        let builder = Runtime::builder()
            .cpu_policy(CpuPolicy::exactly(2))
            .cpu_policy(CpuPolicy::at_most(1));
        assert_eq!(builder.processor_config.cpu_policy, CpuPolicy::at_most(1),);
    }

    #[test]
    fn zero_cpu_policys_can_be_replaced_before_building() {
        for policy in [CpuPolicy::exactly(0), CpuPolicy::at_most(0)] {
            let builder = Runtime::builder().cpu_policy(policy);
            assert_eq!(builder.processor_config.cpu_policy, policy);

            let builder = builder.cpu_policy(CpuPolicy::at_most(1));
            assert_eq!(builder.processor_config.cpu_policy, CpuPolicy::at_most(1));
        }
    }

    #[test]
    fn all_processors_selection_replaces_maximum_count() {
        let builder = Runtime::builder().cpu_policy(CpuPolicy::at_most(1)).cpu_policy(CpuPolicy::all());
        assert_eq!(builder.processor_config.cpu_policy, CpuPolicy::all(),);
    }

    #[test]
    fn automatic_processor_selection_replaces_exact_count() {
        let builder = Runtime::builder().cpu_policy(CpuPolicy::exactly(2)).cpu_policy(CpuPolicy::auto());
        assert_eq!(builder.processor_config.cpu_policy, CpuPolicy::auto(),);
    }

    #[test]
    fn exact_processor_selection_preserves_other_resource_settings() {
        let builder = Runtime::builder()
            .stack_size(1024 * 1024)
            .blocking_pool_policy(BlockingPoolPolicy::shared(1))
            .cpu_policy(CpuPolicy::at_most(1))
            .cpu_policy(CpuPolicy::exactly(2));
        assert_eq!(
            builder.processor_config,
            RuntimeConfig {
                cpu_policy: CpuPolicy::exactly(2),
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

    #[test]
    fn build_and_stop_inside_futures_executor() {
        testing_aids::execute_or_terminate_process(|| {
            futures::executor::block_on(async {
                let runtime = Runtime::builder().cpu_policy(CpuPolicy::exactly(1)).build().unwrap();
                assert!(runtime.shared_state.iter().all(|state| state.get().is_some()));

                let (value, scheduler) = runtime
                    .scheduler()
                    .spawn_anywhere((), |cx, ()| async move { (42, cx.scheduler().clone()) })
                    .await
                    .unwrap();
                assert_eq!(value, 42);

                runtime.stop().unwrap();
                assert!(scheduler.spawn(async |_| ()).await.unwrap_err().is_shutdown());
            });
        });
    }

    #[cfg(not(miri))] // can't call foreign function `CreateIoCompletionPort` on OS `windows`
    #[test]
    fn emitter_is_available_by_default() {
        let runtime = Runtime::builder().build().expect("Failed to create runtime");

        runtime
            .scheduler()
            .block_on(async move |cx: Builtins| {
                assert!(cx.sink().is_noop());
            })
            .unwrap();
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

        runtime
            .scheduler()
            .block_on(async move |cx: Builtins| {
                assert!(!cx.sink().is_noop());
                cx.sink().flush().unwrap();
            })
            .unwrap();
    }
}
