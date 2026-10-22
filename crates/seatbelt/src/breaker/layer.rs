// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use layered::Layer;

use super::constants::{DEFAULT_BREAK_DURATION, DEFAULT_FAILURE_THRESHOLD, DEFAULT_MIN_THROUGHPUT, DEFAULT_SAMPLING_DURATION};
use super::*;
use crate::breaker::engine::probing::ProbesOptions;
use crate::typestates::{NotSet, Set};
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{Recovery, RecoveryInfo, ResilienceContext, TelemetryString};

/// Configures circuit breaker middleware.
///
/// [`Breaker::layer`] creates this builder.
/// Its type state makes these properties required:
///
/// - [`recovery`][BreakerLayer::recovery] decides whether output is a failure.
/// - [`rejected_input`][BreakerLayer::rejected_input] provides output when an open circuit rejects input.
///
/// See the [`breaker` module][crate::breaker] for examples.
///
/// # Type state
///
/// - `S1` records whether [`recovery`][BreakerLayer::recovery] is set.
/// - `S2` records whether [`rejected_input`][BreakerLayer::rejected_input] is set.
#[derive(Debug)]
pub struct BreakerLayer<In, Out, S1 = Set, S2 = Set> {
    context: ResilienceContext<In, Out>,
    recovery: Option<ShouldRecover<Out>>,
    rejected_input: Option<RejectedInput<In, Out>>,
    on_opened: Option<OnOpened<Out>>,
    on_closed: Option<OnClosed<Out>>,
    on_probing: Option<OnProbing<In>>,
    breaker_id: Option<BreakerIdProvider<In>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    failure_threshold: f32,
    min_throughput: u32,
    sampling_duration: Duration,
    break_duration: Duration,
    half_open_mode: HalfOpenMode,
    abandoned_policy: AbandonedPolicy,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> BreakerLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: TelemetryString, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            context: context.clone(),
            recovery: None,
            rejected_input: None,
            on_opened: None,
            on_closed: None,
            on_probing: None,
            breaker_id: None,
            enable_if: EnableIf::default(),
            telemetry: context.create_telemetry(name),
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            min_throughput: DEFAULT_MIN_THROUGHPUT,
            sampling_duration: DEFAULT_SAMPLING_DURATION,
            break_duration: DEFAULT_BREAK_DURATION,
            half_open_mode: HalfOpenMode::progressive(None),
            abandoned_policy: AbandonedPolicy::default(),
            _state: PhantomData,
        }
    }
}

impl<In, Out, E, S1, S2> BreakerLayer<In, Result<Out, E>, S1, S2> {
    /// Sets the error to return from a `Result` service while the circuit is open.
    ///
    /// When the circuit is open, the `error_producer` function receives rejected input.
    /// The middleware wraps its error in `Result::Err`.
    ///
    /// Use this method to describe why the input did not reach the service.
    #[must_use]
    pub fn rejected_input_error(
        self,
        error_producer: impl Fn(In, RejectedInputArgs) -> E + Send + Sync + 'static,
    ) -> BreakerLayer<In, Result<Out, E>, S1, Set> {
        self.into_state::<Set, S2>()
            .rejected_input(move |input, args| Err(error_producer(input, args)))
            .into_state()
    }
}

impl<In, Out, S1, S2> BreakerLayer<In, Out, S1, S2> {
    /// Sets the recovery classification function.
    ///
    /// The function receives the output and [`RecoveryArgs`][crate::breaker::RecoveryArgs].
    /// It returns the [`RecoveryInfo`] classification for the output.
    #[must_use]
    pub fn recovery_with(
        mut self,
        recover_fn: impl Fn(&Out, crate::breaker::RecoveryArgs) -> RecoveryInfo + Send + Sync + 'static,
    ) -> BreakerLayer<In, Out, Set, S2> {
        self.recovery = Some(ShouldRecover::new(recover_fn));
        self.into_state::<Set, S2>()
    }

    /// Uses [`Recovery`] to classify output.
    ///
    /// This method is equivalent to [`recovery_with`][BreakerLayer::recovery_with] with
    /// `|output, _args| output.recovery()`.
    ///
    /// # Type requirements
    ///
    /// The output type `Out` must implement [`Recovery`].
    #[must_use]
    pub fn recovery(self) -> BreakerLayer<In, Out, Set, S2>
    where
        Out: Recovery,
    {
        self.recovery_with(|out, _args| out.recovery())
    }

    /// Sets the output to return when the circuit breaker is open.
    ///
    /// When the circuit is open, `rejected_fn` receives rejected input.
    /// It returns an error or fallback value without calling the service.
    #[must_use]
    pub fn rejected_input(
        mut self,
        rejected_fn: impl Fn(In, RejectedInputArgs) -> Out + Send + Sync + 'static,
    ) -> BreakerLayer<In, Out, S1, Set> {
        self.rejected_input = Some(RejectedInput::new(rejected_fn));
        self.into_state::<S1, Set>()
    }

    /// Sets the failure threshold for the circuit breaker.
    ///
    /// The circuit opens when the failure rate reaches `threshold`.
    /// The value must be between 0.0 and 1.0.
    /// A value of 0.1 means 1 failure in 10 executions.
    /// Values outside this range are clamped.
    ///
    /// Default: 0.1
    #[must_use]
    pub fn failure_threshold(mut self, threshold: f32) -> Self {
        self.failure_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Sets the minimum throughput required before the circuit breaker can open.
    ///
    /// The circuit can open only after it records `throughput` executions
    /// during the configured sampling duration.
    /// This prevents a small sample from opening the circuit.
    ///
    /// Default: 100 executions
    #[must_use]
    pub fn min_throughput(mut self, throughput: u32) -> Self {
        self.min_throughput = throughput;
        self
    }

    /// Sets the sampling duration for calculating failure rates.
    ///
    /// The breaker calculates the failure rate over this time window.
    ///
    /// Default: 30 seconds
    ///
    /// Values below 1 second become 1 second.
    #[must_use]
    pub fn sampling_duration(mut self, duration: Duration) -> Self {
        self.sampling_duration = duration;
        self
    }

    /// Sets how long the circuit stays open.
    ///
    /// After this duration, the circuit becomes half-open and tests the service.
    ///
    /// Default: 5 seconds
    #[must_use]
    pub fn break_duration(mut self, duration: Duration) -> Self {
        self.break_duration = duration;
        self
    }

    /// Applies all settings from a [`BreakerConfig`] to this layer.
    ///
    /// Use this method to apply configuration from an external source.
    /// You do not need to call each builder method.
    #[must_use]
    pub fn config(self, config: &BreakerConfig) -> Self {
        self.failure_threshold(config.failure_threshold)
            .min_throughput(config.min_throughput)
            .sampling_duration(config.sampling_duration)
            .break_duration(config.break_duration)
            .half_open_mode(config.half_open_mode.clone())
            .abandoned_policy(config.abandoned_policy.clone())
            .enable(config.enabled)
    }

    /// Sets a callback that runs when the circuit opens.
    ///
    /// The callback receives optional output and [`OnOpenedArgs`].
    ///
    /// Output is `None` when an abandoned execution opens the circuit.
    /// An abandoned execution does not produce output.
    ///
    /// Default: No callback
    #[must_use]
    pub fn on_opened(mut self, callback: impl Fn(Option<&Out>, OnOpenedArgs) + Send + Sync + 'static) -> Self {
        self.on_opened = Some(OnOpened::new(callback));
        self
    }

    /// Sets a callback that runs when the circuit closes.
    ///
    /// The callback receives the probe output that closed the circuit and [`OnClosedArgs`].
    ///
    /// Default: No callback
    #[must_use]
    pub fn on_closed(mut self, callback: impl Fn(&Out, OnClosedArgs) + Send + Sync + 'static) -> Self {
        self.on_closed = Some(OnClosed::new(callback));
        self
    }

    /// Sets a callback that runs before a probe.
    ///
    /// The callback receives probe input and [`OnProbingArgs`].
    ///
    /// Default: No callback
    #[must_use]
    pub fn on_probing(mut self, callback: impl Fn(&mut In, OnProbingArgs) + Send + Sync + 'static) -> Self {
        self.on_probing = Some(OnProbing::new(callback));
        self
    }

    /// Sets the breaker ID provider function.
    ///
    /// Each unique [`BreakerId`] has separate circuit state.
    /// For HTTP requests, derive an ID from the scheme, host, and port.
    /// The `id_provider` function receives the input and returns its ID.
    ///
    /// Default: All input shares one circuit.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use seatbelt::breaker::{BreakerLayer, BreakerId};
    /// // Define the parts of an HTTP request that identify a backend.
    /// struct HttpRequest {
    ///     scheme: String,
    ///     host: String,
    ///     port: u16,
    ///     path: String,
    /// }
    /// # fn example(breaker_layer: BreakerLayer<HttpRequest, ()>) {
    /// // Configure circuit breaker with a breaker ID based on scheme, host and port.
    /// let layer = breaker_layer.breaker_id(|request: &HttpRequest| {
    ///     let id = format!("{}://{}:{}", request.scheme, request.host, request.port);
    ///     BreakerId::from(id)
    /// });
    ///
    /// // Each endpoint now has a separate circuit.
    /// # }
    /// ```
    ///
    /// # Telemetry
    ///
    /// Logs and metrics include the values used to create breaker IDs.
    /// Do not use authentication tokens, personal data, or confidential data.
    #[must_use]
    pub fn breaker_id(mut self, id_provider: impl Fn(&In) -> BreakerId + Send + Sync + 'static) -> Self {
        self.breaker_id = Some(BreakerIdProvider::new(id_provider));
        self
    }

    /// Sets the half-open mode for the circuit breaker.
    ///
    /// This mode controls how a half-open circuit tests the service.
    ///
    /// Default: [`HalfOpenMode::progressive`]
    #[must_use]
    pub fn half_open_mode(mut self, mode: HalfOpenMode) -> Self {
        self.half_open_mode = mode;
        self
    }

    /// Sets how abandoned executions affect circuit health.
    ///
    /// An execution is abandoned when its future is dropped before it finishes.
    /// Telemetry always includes it in throughput.
    /// This policy controls its effect on the health decision.
    ///
    /// Default: [`AbandonedPolicy::rate_threshold`] with `1.0`
    #[must_use]
    pub fn abandoned_policy(mut self, policy: AbandonedPolicy) -> Self {
        self.abandoned_policy = policy;
        self
    }

    /// Enables circuit breaker middleware when a condition is true.
    ///
    /// If the condition is false, input reaches the service without circuit breaker protection.
    /// The `is_enabled` function receives the input.
    /// This call replaces the previous condition.
    ///
    /// Default: Enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::custom(is_enabled);
        self
    }

    /// Enables or disables the circuit breaker middleware.
    ///
    /// If disabled, input reaches the service without circuit breaker protection.
    /// This call replaces the previous condition.
    #[must_use]
    fn enable(mut self, enabled: bool) -> Self {
        self.enable_if = EnableIf::new(enabled);
        self
    }

    /// Enables circuit breaker middleware for all input.
    ///
    /// This call replaces the previous condition.
    /// Circuit breaker middleware is enabled by default.
    #[must_use]
    pub fn enable_always(self) -> Self {
        self.enable(true)
    }

    /// Disables circuit breaker middleware for all input.
    ///
    /// Input reaches the service without circuit breaker protection.
    /// This call replaces the previous condition.
    #[must_use]
    pub fn disable(self) -> Self {
        self.enable(false)
    }
}

impl<In, Out, S> Layer<S> for BreakerLayer<In, Out, Set, Set> {
    type Service = Breaker<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        let shared = BreakerShared {
            clock: self.context.get_clock().clone(),
            recovery: self.recovery.clone().expect("enforced by the type state pattern"),
            rejected_input: self.rejected_input.clone().expect("enforced by the type state pattern"),
            enable_if: self.enable_if.clone(),
            engines: self.engines(),
            on_opened: self.on_opened.clone(),
            on_closed: self.on_closed.clone(),
            on_probing: self.on_probing.clone(),
            id_provider: self.breaker_id.clone(),
        };

        Breaker {
            shared: Arc::new(shared),
            inner,
        }
    }
}

impl<In, Out, S1, S2> BreakerLayer<In, Out, S1, S2> {
    fn probes_options(&self) -> ProbesOptions {
        self.half_open_mode
            // we will use break duration as the sampling duration for probes
            .to_options(self.break_duration, self.failure_threshold, &self.abandoned_policy)
    }

    fn engines(&self) -> Engines {
        Engines::new(
            super::engine::EngineOptions {
                break_duration: self.break_duration,
                health_metrics_builder: HealthMetricsBuilder::new(
                    self.sampling_duration,
                    self.failure_threshold,
                    self.min_throughput,
                    self.abandoned_policy.clone(),
                ),
                probes: self.probes_options(),
            },
            self.context.get_clock().clone(),
            self.telemetry.clone(),
        )
    }

    fn into_state<T1, T2>(self) -> BreakerLayer<In, Out, T1, T2> {
        BreakerLayer {
            context: self.context,
            recovery: self.recovery,
            rejected_input: self.rejected_input,
            on_opened: self.on_opened,
            on_closed: self.on_closed,
            on_probing: self.on_probing,
            breaker_id: self.breaker_id,
            enable_if: self.enable_if,
            telemetry: self.telemetry.clone(),
            failure_threshold: self.failure_threshold,
            min_throughput: self.min_throughput,
            sampling_duration: self.sampling_duration,
            break_duration: self.break_duration,
            half_open_mode: self.half_open_mode,
            abandoned_policy: self.abandoned_policy,
            _state: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[expect(clippy::float_cmp, reason = "simpler tests")]
mod tests {
    use std::fmt::Debug;

    use layered::Execute;
    use tick::Clock;

    use super::*;
    use crate::breaker::engine::probing::ProbeOptions;
    use crate::testing::RecoverableType;

    #[test]
    fn new_creates_correct_initial_state() {
        let context = create_test_context();
        let layer: BreakerLayer<_, _, NotSet, NotSet> = BreakerLayer::new("test_breaker".into(), &context);

        assert!(layer.recovery.is_none());
        assert!(layer.rejected_input.is_none());
        assert_eq!(layer.telemetry.strategy_name.as_ref(), "test_breaker");
        assert!(layer.enable_if.call(&"test_input".to_string()));
        assert_eq!(layer.failure_threshold, 0.1);
        assert_eq!(layer.min_throughput, 100);
        assert_eq!(layer.sampling_duration, Duration::from_secs(30));
    }

    #[test]
    fn recovery_sets_correctly() {
        let context = create_test_context();
        let layer = BreakerLayer::new("test".into(), &context);

        let layer: BreakerLayer<_, _, Set, NotSet> = layer.recovery_with(|output, _args| {
            if output.contains("error") {
                RecoveryInfo::retry()
            } else {
                RecoveryInfo::never()
            }
        });

        let result = layer.recovery.as_ref().unwrap().call(
            &"error message".to_string(),
            RecoveryArgs {
                breaker_id: &BreakerId::default(),
                clock: &Clock::new_frozen(),
            },
        );
        assert_eq!(result, RecoveryInfo::retry());

        let result = layer.recovery.as_ref().unwrap().call(
            &"success".to_string(),
            RecoveryArgs {
                breaker_id: &BreakerId::default(),
                clock: &Clock::new_frozen(),
            },
        );
        assert_eq!(result, RecoveryInfo::never());
    }

    #[test]
    fn recovery_auto_sets_correctly() {
        let context = ResilienceContext::<RecoverableType, RecoverableType>::new(Clock::new_frozen());
        let layer = BreakerLayer::new("test".into(), &context);

        let layer: BreakerLayer<_, _, Set, NotSet> = layer.recovery();

        let result = layer.recovery.as_ref().unwrap().call(
            &RecoverableType::from(RecoveryInfo::retry()),
            RecoveryArgs {
                breaker_id: &BreakerId::default(),
                clock: &Clock::new_frozen(),
            },
        );
        assert_eq!(result, RecoveryInfo::retry());

        let result = layer.recovery.as_ref().unwrap().call(
            &RecoverableType::from(RecoveryInfo::never()),
            RecoveryArgs {
                breaker_id: &BreakerId::default(),
                clock: &Clock::new_frozen(),
            },
        );
        assert_eq!(result, RecoveryInfo::never());
    }

    #[test]
    fn rejected_input_sets_correctly() {
        let context = create_test_context();
        let layer = BreakerLayer::new("test".into(), &context);

        let layer: BreakerLayer<_, _, NotSet, Set> = layer.rejected_input(|_, _| "rejected".to_string());

        let result = layer.rejected_input.as_ref().unwrap().call(
            "test".to_string(),
            RejectedInputArgs {
                breaker_id: &BreakerId::default(),
            },
        );
        assert_eq!(result, "rejected");
    }

    #[test]
    fn rejected_input_error_wraps_in_err() {
        let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(Clock::new_frozen());
        let layer = BreakerLayer::new("test".into(), &context);

        let layer: BreakerLayer<_, _, NotSet, Set> = layer.rejected_input_error(|input, _| format!("rejected: {input}"));

        let result = layer.rejected_input.as_ref().unwrap().call(
            "test_input".to_string(),
            RejectedInputArgs {
                breaker_id: &BreakerId::default(),
            },
        );
        assert_eq!(result, Err("rejected: test_input".to_string()));
    }

    #[test]
    fn enable_disable_conditions_work() {
        let layer = create_ready_layer().enable_if(|input| input.contains("enable"));

        assert!(layer.enable_if.call(&"enable_test".to_string()));
        assert!(!layer.enable_if.call(&"disable_test".to_string()));

        let layer = layer.disable();
        assert!(!layer.enable_if.call(&"anything".to_string()));

        let layer = layer.enable_always();
        assert!(layer.enable_if.call(&"anything".to_string()));
    }

    #[test]
    fn layer_builds_service_when_ready() {
        let layer = create_ready_layer();
        let _service = layer.layer(Execute::new(|input: String| async move { input }));
    }

    #[test]
    fn failure_threshold_sets_correctly() {
        let layer = create_ready_layer();

        // Test setting a valid threshold
        let layer = layer.failure_threshold(0.25);
        assert_eq!(layer.failure_threshold, 0.25);

        // Test clamping values greater than 1.0
        let layer = layer.failure_threshold(1.5);
        assert_eq!(layer.failure_threshold, 1.0);

        // Test edge cases
        let layer = layer.failure_threshold(0.0);
        assert_eq!(layer.failure_threshold, 0.0);

        let layer = layer.failure_threshold(1.0);
        assert_eq!(layer.failure_threshold, 1.0);
    }

    #[test]
    fn min_throughput_sets_correctly() {
        let layer = create_ready_layer();

        // Test setting different throughput values
        let layer = layer.min_throughput(50);
        assert_eq!(layer.min_throughput, 50);

        let layer = layer.min_throughput(1000);
        assert_eq!(layer.min_throughput, 1000);

        let layer = layer.min_throughput(0);
        assert_eq!(layer.min_throughput, 0);
    }

    #[test]
    fn sampling_duration_sets_correctly() {
        let layer = create_ready_layer();

        // Test setting different durations
        let layer = layer.sampling_duration(Duration::from_secs(10));
        assert_eq!(layer.sampling_duration, Duration::from_secs(10));

        let layer = layer.sampling_duration(Duration::from_mins(1));
        assert_eq!(layer.sampling_duration, Duration::from_mins(1));

        let layer = layer.sampling_duration(Duration::from_millis(500));
        assert_eq!(layer.sampling_duration, Duration::from_millis(500));
    }

    #[test]
    fn break_duration_sets_correctly() {
        let layer = create_ready_layer();

        // Test setting different break durations
        let layer = layer.break_duration(Duration::from_secs(5));
        assert_eq!(layer.break_duration, Duration::from_secs(5));

        let layer = layer.break_duration(Duration::from_mins(2));
        assert_eq!(layer.break_duration, Duration::from_mins(2));

        let layer = layer.break_duration(Duration::from_secs(2));
        assert_eq!(layer.break_duration, Duration::from_secs(2));
    }

    #[test]
    fn default_values_are_correct() {
        let context = create_test_context();
        let layer = BreakerLayer::new("test".into(), &context);

        assert_eq!(layer.failure_threshold, DEFAULT_FAILURE_THRESHOLD);
        assert_eq!(layer.min_throughput, DEFAULT_MIN_THROUGHPUT);
        assert_eq!(layer.sampling_duration, DEFAULT_SAMPLING_DURATION);
        assert_eq!(layer.break_duration, DEFAULT_BREAK_DURATION);
        assert_eq!(layer.half_open_mode, HalfOpenMode::progressive(None));
    }

    #[test]
    pub(crate) fn half_open_mode_ok() {
        let layer = create_ready_layer().half_open_mode(HalfOpenMode::quick());
        assert_eq!(layer.half_open_mode, HalfOpenMode::quick());

        let probes = layer
            .break_duration(Duration::from_secs(234))
            .failure_threshold(0.52)
            .half_open_mode(HalfOpenMode::progressive(None))
            .probes_options();

        // Access the last probe. It is the health probe.
        let probe = probes.probes().last().unwrap();

        match probe {
            ProbeOptions::HealthProbe(health_probe) => {
                assert_eq!(health_probe.stage_duration(), Duration::from_secs(234));
                assert_eq!(health_probe.failure_threshold(), 0.52);
            }
            ProbeOptions::SingleProbe { .. } => panic!("Expected HealthProbe"),
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn config_applies_all_settings() {
        let config = BreakerConfig {
            enabled: false,
            failure_threshold: 0.25,
            min_throughput: 50,
            sampling_duration: Duration::from_mins(1),
            break_duration: Duration::from_secs(15),
            half_open_mode: HalfOpenMode::quick(),
            abandoned_policy: AbandonedPolicy::as_failures(),
        };

        let layer = create_ready_layer().config(&config);

        insta::assert_debug_snapshot!(layer);
    }

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(BreakerLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(BreakerLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(BreakerLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(BreakerLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> ResilienceContext<String, String> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer() -> BreakerLayer<String, String, Set, Set> {
        BreakerLayer::new("test".into(), &create_test_context())
            .recovery_with(|output, _args| {
                if output.contains("error") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
            .rejected_input(|_, _| "circuit is open".to_string())
    }
}
