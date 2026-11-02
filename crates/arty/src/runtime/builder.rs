// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use thread_aware::ThreadBuilder;
use tick::runtime::InactiveClock;

use crate::runtime::bootstrap;
use crate::runtime::config::{BlockingPoolPolicy, MonitorPolicy, PanicPolicy, RuntimeConfig, WorkersPolicy};
use crate::runtime::error::Error;
use crate::runtime::handle::Runtime;

/// Configures workers and clocks before starting a runtime.
///
/// Obtain a builder from [`Runtime::builder`], change the settings you need,
/// then call [`build`](Self::build) to start the workers. Setters replace earlier
/// values for the same setting; configuring a builder does not start threads.
///
/// The defaults are [`WorkersPolicy::default`], 2 MiB async-worker stacks,
/// a shared blocking pool, a real-time clock, and [`MonitorPolicy::enabled`].
/// A contained async task panic starts
/// coordinated shutdown by default.
///
/// # Examples
///
/// ```
/// use arty::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};
///
/// let runtime = Runtime::builder()
///     .workers(WorkersPolicy::at_most(4))
///     .blocking_pool(BlockingPoolPolicy::shared().max(4))
///     .build()?;
/// assert_eq!(runtime.scheduler().block_on(async |_| 42)?, 42);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct RuntimeBuilder {
    processor_config: RuntimeConfig,
    clock: InactiveClock,
}

impl RuntimeBuilder {
    /// Sets how the runtime responds to a contained task or callback panic.
    ///
    /// The default is [`PanicPolicy::shutdown`]. Use
    /// [`PanicPolicy::ignore`] to keep the runtime available after a panic.
    /// The panicking task still fails through its join handle under either
    /// policy. The policy also applies to blocking callbacks.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{PanicPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().panic(PanicPolicy::ignore());
    /// ```
    #[must_use]
    pub const fn panic(mut self, policy: PanicPolicy) -> Self {
        self.processor_config.panic_policy = policy;
        self
    }

    /// Sets whether this runtime is prepared for monitoring.
    ///
    /// The default is [`MonitorPolicy::enabled`]. The `monitor` Cargo feature
    /// enables the Seismograph dependencies that monitoring needs. This stage
    /// only stores and propagates the policy; it does not start a monitor,
    /// instrument tasks, or change runtime scheduling.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{MonitorPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().monitor(MonitorPolicy::enabled());
    /// ```
    #[must_use]
    pub fn monitor(mut self, policy: MonitorPolicy) -> Self {
        self.processor_config.monitor_options = policy.into();
        self
    }

    /// Sets the worker-count policy.
    ///
    /// The runtime assigns each async worker to one available processor.
    /// Exact policies can assign multiple worker threads to one processor.
    /// The default is [`WorkersPolicy::default`]. This does not set blocking-pool
    /// limits; use [`blocking_pool`](Self::blocking_pool) for those.
    /// A zero count is rejected by [`build`](Self::build), not by this setter.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{Runtime, WorkersPolicy};
    ///
    /// let builder = Runtime::builder().workers(WorkersPolicy::at_most(4));
    /// ```
    #[must_use]
    pub const fn workers(mut self, count: WorkersPolicy) -> Self {
        self.processor_config.workers_policy = count;
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
        self.processor_config.stack_size = NonZeroUsize::new(size).expect("size was asserted above to be nonzero");
        self
    }

    /// Sets whether async workers share their blocking-task pool.
    ///
    /// The default shares one pool across async workers. Use
    /// [`BlockingPoolPolicy::shared`] for one runtime-wide pool, or
    /// [`BlockingPoolPolicy::per_worker`] to give each worker its own pool.
    /// Chain [`BlockingPoolPolicy::max`] to set the thread limit.
    /// A zero shared-pool limit is rejected by [`build`](Self::build), not by
    /// this setter.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::{BlockingPoolPolicy, Runtime};
    ///
    /// let builder = Runtime::builder().blocking_pool(BlockingPoolPolicy::shared().max(4));
    /// ```
    #[must_use]
    pub const fn blocking_pool(mut self, policy: BlockingPoolPolicy) -> Self {
        self.processor_config.blocking_pool_policy = policy;
        self
    }

    /// Sets the clock used by runtime workers.
    ///
    /// The default follows real time. Pass an [`InactiveClock`] or, with
    /// `test-util` enabled, a `ClockControl` to choose a different time source.
    /// Workers drive system timers. A `ClockControl` owner advances controlled
    /// time and wakes due timers.
    ///
    /// # Examples
    ///
    /// Enable `test-util` in dev-dependencies to test sequential delays without
    /// waiting for real time:
    ///
    /// ```
    /// # #[cfg(all(feature = "rt", feature = "test-util"))]
    /// # fn sequential_delay() -> Result<(), Box<dyn std::error::Error>> {
    /// let control = arty::time::ClockControl::new().auto_advance_timers(true);
    /// let runtime = arty::runtime::Runtime::builder().clock(control).build()?;
    /// runtime.scheduler().block_on(async |cx| {
    ///     let duration = std::time::Duration::from_secs(30);
    ///     let watch = cx.clock().stopwatch();
    ///     cx.clock().delay(duration).await;
    ///     assert_eq!(watch.elapsed(), duration);
    /// })?;
    /// runtime.stop()?;
    /// # Ok(())
    /// # }
    /// # #[cfg(all(feature = "rt", feature = "test-util"))] sequential_delay()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// Automatic timer advancement occurs eagerly on timer registration or
    /// explicit controlled-time reads. Use manual advancement when concurrent
    /// timers' relative order matters; see the [time guide](crate::documentation::time).
    #[must_use]
    pub fn clock(mut self, clock: impl Into<InactiveClock>) -> Self {
        self.clock = clock.into();
        self
    }

    /// Starts the configured workers and returns their runtime owner.
    ///
    /// Blocks the calling thread until workers are ready to receive tasks.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the worker count or shared blocking-pool limit is
    /// zero.
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
        bootstrap::build(self.processor_config, &self.clock, &ThreadBuilder::default())
    }
}

impl RuntimeBuilder {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            clock: InactiveClock::new_isolated(),
            processor_config: RuntimeConfig::default(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::time::Duration;

    use tick::ClockControl;

    use super::*;

    #[test]
    fn custom_clock_ok() {
        let clock_control = ClockControl::new();
        let builder = Runtime::builder().clock(clock_control.clone());

        let (clock, _) = builder.clock.activate(std::task::Waker::noop().clone());
        let now = clock.system_time();

        clock_control.advance(Duration::from_secs(1));

        assert_eq!(clock.system_time().duration_since(now).unwrap(), Duration::from_secs(1));
    }

    #[test]
    fn resource_limits_preserve_independent_settings() {
        let builder = Runtime::builder()
            .workers(WorkersPolicy::exactly(2))
            .stack_size(1024 * 1024)
            .blocking_pool(BlockingPoolPolicy::shared().max(1));
        assert_eq!(builder.processor_config.workers_policy, WorkersPolicy::exactly(2));
        assert_eq!(builder.processor_config.stack_size, NonZeroUsize::new(1024 * 1024).unwrap());
        assert_eq!(builder.processor_config.blocking_pool_policy, BlockingPoolPolicy::shared().max(1));
        assert!(builder.processor_config.monitor_options.policy().is_enabled());
        assert!(matches!(
            builder.processor_config.panic_policy.into_executor_policy(),
            arty_executor::PanicPolicy::Shutdown
        ));
    }

    #[test]
    fn maximum_worker_count_replaces_exact_count() {
        let builder = Runtime::builder()
            .workers(WorkersPolicy::exactly(2))
            .workers(WorkersPolicy::at_most(1));
        assert_eq!(builder.processor_config.workers_policy, WorkersPolicy::at_most(1),);
    }

    #[test]
    fn zero_worker_policies_can_be_replaced_before_building() {
        for policy in [WorkersPolicy::exactly(0), WorkersPolicy::at_most(0)] {
            let builder = Runtime::builder().workers(policy);
            assert_eq!(builder.processor_config.workers_policy, policy);

            let builder = builder.workers(WorkersPolicy::at_most(1));
            assert_eq!(builder.processor_config.workers_policy, WorkersPolicy::at_most(1));
        }
    }

    #[test]
    fn all_workers_policy_replaces_maximum_count() {
        let builder = Runtime::builder().workers(WorkersPolicy::at_most(1)).workers(WorkersPolicy::all());
        assert_eq!(builder.processor_config.workers_policy, WorkersPolicy::all(),);
    }

    #[test]
    fn automatic_worker_selection_replaces_exact_count() {
        let builder = Runtime::builder()
            .workers(WorkersPolicy::exactly(2))
            .workers(WorkersPolicy::default());
        assert_eq!(builder.processor_config.workers_policy, WorkersPolicy::default());
    }

    #[test]
    fn exact_processor_selection_preserves_other_resource_settings() {
        let builder = Runtime::builder()
            .stack_size(1024 * 1024)
            .blocking_pool(BlockingPoolPolicy::shared().max(1))
            .workers(WorkersPolicy::at_most(1))
            .workers(WorkersPolicy::exactly(2));
        assert_eq!(builder.processor_config.workers_policy, WorkersPolicy::exactly(2));
        assert_eq!(builder.processor_config.stack_size, NonZeroUsize::new(1024 * 1024).unwrap());
        assert_eq!(builder.processor_config.blocking_pool_policy, BlockingPoolPolicy::shared().max(1));
        assert!(builder.processor_config.monitor_options.policy().is_enabled());
    }

    #[test]
    fn monitor_policy_replaces_the_default() {
        let builder = Runtime::builder().monitor(MonitorPolicy::disabled());

        assert!(!builder.processor_config.monitor_options.policy().is_enabled());
    }

    #[test]
    fn panic_replaces_the_default() {
        let builder = Runtime::builder().panic(PanicPolicy::ignore());

        assert!(matches!(
            builder.processor_config.panic_policy.into_executor_policy(),
            arty_executor::PanicPolicy::Ignore
        ));
    }

    #[test]
    fn build_and_stop_inside_futures_executor() {
        futures::executor::block_on(async {
            let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();

            let (value, scheduler) = runtime
                .scheduler()
                .spawn_anywhere((), |cx, ()| async move { (42, cx.scheduler().clone()) })
                .await
                .unwrap();
            assert_eq!(value, 42);

            runtime.stop().unwrap();
            assert!(scheduler.spawn(async |_| ()).await.unwrap_err().is_shutdown());
        });
    }
}
