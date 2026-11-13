// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::num::NonZeroUsize;

use thread_aware::ThreadBuilder;
use tick::runtime::InactiveClock;

use crate::runtime::bootstrap;
use crate::runtime::config::{BlockingPoolPolicy, MonitorPolicy, RuntimeConfig, RuntimeName, WorkersPolicy};
use crate::runtime::error::Error;
use crate::runtime::handle::Runtime;
use crate::runtime::hook::{Hook, HookFn};

/// Configures workers and clocks before starting a runtime.
///
/// Obtain a builder from [`Runtime::builder`]. Change its configuration, then
/// call [`build`](Self::build) to start the workers.
/// Policy methods replace any earlier value for the same configuration.
/// Hook methods append callbacks in registration order.
/// Configuring a builder does not start threads.
///
/// See the [runtime defaults](crate::runtime#defaults) for the initial
/// configuration.
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
    config: RuntimeConfig,
    clock: InactiveClock,
}

impl RuntimeBuilder {
    /// Sets the runtime name used in worker thread names.
    ///
    /// Async threads use `<name>-async-<index>`, with indices starting at zero.
    /// Shared blocking pools use `<name>-blocking-<thread index>`.
    /// Per-worker pools use `<name>-blocking-<async worker index>-<thread index>`.
    /// The default name is `arty`.
    ///
    /// The builder copies the name. Empty names are accepted.
    /// [`build`](Self::build) rejects names that contain a null character (`\0`).
    /// Operating systems can truncate thread names in diagnostic tools.
    #[must_use]
    pub fn name(mut self, name: impl AsRef<str>) -> Self {
        self.config.name = RuntimeName::new(name);
        self
    }

    /// Registers a runtime hook with worker-local callback data.
    ///
    /// Arty clones the hook data for each worker and relocates it before a
    /// callback can run. Registering another hook of the same type appends it
    /// after earlier callbacks.
    ///
    /// Arty does not catch a panic from the callback. You must make sure that
    /// the callback completes without a panic.
    ///
    /// # Examples
    ///
    /// ```
    /// use arty::runtime::Runtime;
    /// use arty::runtime::hook::Panic;
    ///
    /// let builder = Runtime::builder().on(Panic::IGNORE);
    /// ```
    #[must_use]
    pub fn on<H, D>(mut self, hook: impl Into<HookFn<H, D>>) -> Self
    where
        H: Hook,
        D: thread_aware::ThreadAware + Clone + 'static,
    {
        self.config.hook_registrations.push(crate::runtime::hook::registration(hook.into()));
        self
    }

    /// Sets the runtime's monitoring configuration.
    ///
    /// The default is [`MonitorPolicy::enabled`]. The `monitor` Cargo feature
    /// enables the Seismograph dependencies that monitoring needs.
    /// This choice does not start a monitor or add task measurements.
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
        self.config.monitor_options = policy.into();
        self
    }

    /// Sets how the runtime chooses its number of async workers.
    ///
    /// The runtime assigns each async worker to one available processor.
    /// A fixed worker count can assign multiple worker threads to one processor.
    /// The default is [`WorkersPolicy::default`]. This does not set blocking-pool
    /// limits. Use [`blocking_pool`](Self::blocking_pool) for those limits.
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
        self.config.workers_policy = count;
        self
    }

    /// Sets each async worker thread's stack size in bytes.
    ///
    /// The default is 2 MiB. A larger value of the `RUST_MIN_STACK` environment
    /// variable overrides the configured size.
    /// This configuration does not change blocking-pool thread stacks.
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
        self.config.stack_size = NonZeroUsize::new(size).expect("size was asserted above to be nonzero");
        self
    }

    /// Sets whether async workers share their pool for blocking functions.
    ///
    /// The default shares one pool across async workers. Use
    /// [`BlockingPoolPolicy::shared`] for one runtime-wide pool, or
    /// [`BlockingPoolPolicy::per_worker`] to give each worker its own pool.
    /// Chain [`BlockingPoolPolicy::max`] to set the thread limit.
    /// A zero limit for either pool type is rejected by [`build`](Self::build),
    /// not by this setter.
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
        self.config.blocking_pool_policy = policy;
        self
    }

    /// Sets the clock used by runtime workers.
    ///
    /// The default follows real time. Pass an [`InactiveClock`] or, with
    /// `test-util` enabled, a `ClockControl` to choose a different time source.
    /// Workers wake tasks when their timers expire.
    /// With controlled time, use `ClockControl` to advance time and wake those tasks.
    ///
    /// # Examples
    ///
    /// Enable `test-util` in dev-dependencies to test delays that run one after another without
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
    /// You can advance controlled time yourself or enable automatic advancement.
    /// See the [`time`](crate::time) module.
    #[must_use]
    pub fn clock(mut self, clock: impl Into<InactiveClock>) -> Self {
        self.clock = clock.into();
        self
    }

    /// Starts the configured workers and returns the runtime that owns them.
    ///
    /// Blocks the calling thread until workers are ready to receive tasks.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if the worker count or blocking-pool limit is zero,
    /// or if the runtime name contains a null character (`\0`).
    ///
    /// # Panics
    ///
    /// Panics if starting or preparing worker threads fails.
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
        bootstrap::build(self.config, &self.clock, &ThreadBuilder::default())
    }
}

impl RuntimeBuilder {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            clock: InactiveClock::new_isolated(),
            config: RuntimeConfig::default(),
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
        assert_eq!(builder.config.workers_policy, WorkersPolicy::exactly(2));
        assert_eq!(builder.config.stack_size, NonZeroUsize::new(1024 * 1024).unwrap());
        assert_eq!(builder.config.blocking_pool_policy, BlockingPoolPolicy::shared().max(1));
        assert!(builder.config.monitor_options.policy().is_enabled());
    }

    #[test]
    fn maximum_worker_count_replaces_exact_count() {
        let builder = Runtime::builder()
            .workers(WorkersPolicy::exactly(2))
            .workers(WorkersPolicy::at_most(1));
        assert_eq!(builder.config.workers_policy, WorkersPolicy::at_most(1),);
    }

    #[test]
    fn zero_worker_policies_can_be_replaced_before_building() {
        for policy in [WorkersPolicy::exactly(0), WorkersPolicy::at_most(0)] {
            let builder = Runtime::builder().workers(policy);
            assert_eq!(builder.config.workers_policy, policy);

            let builder = builder.workers(WorkersPolicy::at_most(1));
            assert_eq!(builder.config.workers_policy, WorkersPolicy::at_most(1));
        }
    }

    #[test]
    fn all_workers_policy_replaces_maximum_count() {
        let builder = Runtime::builder().workers(WorkersPolicy::at_most(1)).workers(WorkersPolicy::all());
        assert_eq!(builder.config.workers_policy, WorkersPolicy::all(),);
    }

    #[test]
    fn automatic_worker_selection_replaces_exact_count() {
        let builder = Runtime::builder()
            .workers(WorkersPolicy::exactly(2))
            .workers(WorkersPolicy::default());
        assert_eq!(builder.config.workers_policy, WorkersPolicy::default());
    }

    #[test]
    fn exact_processor_selection_preserves_other_resource_settings() {
        let builder = Runtime::builder()
            .stack_size(1024 * 1024)
            .blocking_pool(BlockingPoolPolicy::shared().max(1))
            .workers(WorkersPolicy::at_most(1))
            .workers(WorkersPolicy::exactly(2));
        assert_eq!(builder.config.workers_policy, WorkersPolicy::exactly(2));
        assert_eq!(builder.config.stack_size, NonZeroUsize::new(1024 * 1024).unwrap());
        assert_eq!(builder.config.blocking_pool_policy, BlockingPoolPolicy::shared().max(1));
        assert!(builder.config.monitor_options.policy().is_enabled());
    }

    #[test]
    fn monitor_policy_replaces_the_default() {
        let builder = Runtime::builder().monitor(MonitorPolicy::disabled());

        assert!(!builder.config.monitor_options.policy().is_enabled());
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
