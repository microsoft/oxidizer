// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::marker::PhantomData;
use std::time::Duration;

use super::constants::{DEFAULT_BREAK_DURATION, DEFAULT_FAILURE_THRESHOLD, DEFAULT_MIN_THROUGHPUT, DEFAULT_SAMPLING_DURATION};
use super::{
    Breaker, BreakerId, BreakerIdProvider, Engines, HalfOpenMode, HealthMetricsBuilder, OnClosed, OnClosedArgs, OnOpened, OnOpenedArgs,
    OnProbing, OnProbingArgs, RejectedInput, RejectedInputArgs, ShouldRecover,
};
use crate::breaker::engine::probing::ProbesOptions;
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{NotSet, Recovery, RecoveryInfo, ResilienceContext, Set};
use layered::Layer;

/// Builder for configuring circuit breaker resilience middleware.
///
/// This type is created by calling [`Breaker::layer`] and uses the
/// type-state pattern to enforce that required properties are configured before the circuit breaker
/// middleware can be built:
///
/// - [`recovery`][BreakerLayer::recovery]: Required to determine if an output represents a failure
/// - [`rejected_input`][BreakerLayer::rejected_input]: Required to specify the output when the circuit is open and inputs are rejected
///
/// For comprehensive documentation and examples, see the [`breaker` module][crate::breaker] documentation.
///
/// # Type State
///
/// - `S1`: Tracks whether [`recovery`][BreakerLayer::recovery] has been set
/// - `S2`: Tracks whether [`rejected_input`][BreakerLayer::rejected_input] has been set
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
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> BreakerLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: Cow<'static, str>, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            context: context.clone(),
            recovery: None,
            rejected_input: None,
            on_opened: None,
            on_closed: None,
            on_probing: None,
            breaker_id: None,
            enable_if: EnableIf::always(),
            telemetry: context.create_telemetry(name),
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            min_throughput: DEFAULT_MIN_THROUGHPUT,
            sampling_duration: DEFAULT_SAMPLING_DURATION,
            break_duration: DEFAULT_BREAK_DURATION,
            half_open_mode: HalfOpenMode::reliable(None),
            _state: PhantomData,
        }
    }
}

impl<In, Out, E, S1, S2> BreakerLayer<In, Result<Out, E>, S1, S2> {
    /// Sets the error to return when the circuit breaker is open for Result-returning services.
    ///
    /// When the circuit is open, inputs are immediately rejected and this function
    /// is called to generate the error that should be returned to the caller.
    /// The error is automatically wrapped in a `Result::Err`.
    ///
    /// This is a convenience method for Result-returning services that allows you to
    /// provide a meaningful error when the circuit breaker prevents an input from
    /// reaching the underlying service.
    ///
    /// # Arguments
    ///
    /// * `error_producer` - Function that generates the error to return when the circuit is open
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
    /// This function determines whether a specific output represents a failure
    /// by examining the output and returning a [`RecoveryInfo`] classification.
    ///
    /// The function receives the output and [`RecoveryArgs`][crate::breaker::RecoveryArgs]
    /// with context about the circuit breaker state.
    ///
    /// # Arguments
    ///
    /// * `recover_fn` - Function that takes a reference to the output and
    ///   [`RecoveryArgs`][crate::breaker::RecoveryArgs] containing circuit breaker context,
    ///   and returns a [`RecoveryInfo`] decision
    #[must_use]
    pub fn recovery_with(
        mut self,
        recover_fn: impl Fn(&Out, crate::breaker::RecoveryArgs) -> RecoveryInfo + Send + Sync + 'static,
    ) -> BreakerLayer<In, Out, Set, S2> {
        self.recovery = Some(ShouldRecover::new(recover_fn));
        self.into_state::<Set, S2>()
    }

    /// Automatically sets the recovery classification function for types that implement [`Recovery`].
    ///
    /// This is a convenience method that uses the [`Recovery`] trait to determine
    /// whether an output represents a failure. For types that implement [`Recovery`],
    /// this provides a simple way to enable circuit breaker behavior without manually
    /// implementing a recovery classification function.
    ///
    /// This is equivalent to calling [`recovery_with`][BreakerLayer::recovery_with] with
    /// `|output, _args| output.recovery()`.
    ///
    /// # Type Requirements
    ///
    /// This method is only available when the output type `Out` implements [`Recovery`].
    #[must_use]
    pub fn recovery(self) -> BreakerLayer<In, Out, Set, S2>
    where
        Out: Recovery,
    {
        self.recovery_with(|out, _args| out.recovery())
    }

    /// Sets the output to return when the circuit breaker is open.
    ///
    /// When the circuit is open, inputs are immediately rejected and this function
    /// is called to generate the output that should be returned to the caller.
    ///
    /// This allows you to provide a meaningful error message or fallback value
    /// when the circuit breaker prevents an input from reaching the underlying service.
    ///
    /// # Arguments
    ///
    /// * `rejected_fn` - Function that generates the output to return when the circuit is open
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
    /// The circuit breaker will open when the failure rate exceeds this threshold
    /// over the sampling duration. The value should be between 0.0 and 1.0, where
    /// 0.1 represents a `10%` failure threshold. Values greater than 1.0 will be clamped to 1.0.
    ///
    /// **Default**: 0.1 (`10%` failure rate)
    ///
    /// # Arguments
    ///
    /// * `threshold` - The failure threshold (0.0 to 1.0, values `>` 1.0 are clamped)
    #[must_use]
    pub fn failure_threshold(mut self, threshold: f32) -> Self {
        self.failure_threshold = threshold.min(1.0);
        self
    }

    /// Sets the minimum throughput required before the circuit breaker can open.
    ///
    /// The circuit breaker will only consider opening if at least this many executions
    /// have been processed during the sampling duration. This prevents the circuit
    /// from opening due to a small number of failures when overall traffic is low.
    ///
    /// **Default**: 100 executions
    ///
    /// # Arguments
    ///
    /// * `throughput` - The minimum number of executions required
    #[must_use]
    pub fn min_throughput(mut self, throughput: u32) -> Self {
        self.min_throughput = throughput;
        self
    }

    /// Sets the sampling duration for calculating failure rates.
    ///
    /// The circuit breaker calculates failure rates over this time window.
    /// Only executions within this duration are considered when determining
    /// whether the failure rate exceeds the threshold.
    ///
    /// **Default**: 30 seconds
    ///
    /// > **Note**: The sampling duration cannot be lower than 1 second. If value is less
    /// > than 1 second, it will be clamped to 1 second.
    ///
    /// # Arguments
    ///
    /// * `duration` - The time window for sampling failures
    #[must_use]
    pub fn sampling_duration(mut self, duration: Duration) -> Self {
        self.sampling_duration = duration;
        self
    }

    /// Sets the break duration for how long the circuit stays open.
    ///
    /// When the circuit breaker opens due to failures, it will remain open
    /// for this duration before transitioning to half-open state to test
    /// if the underlying service has recovered.
    ///
    /// **Default**: 5 seconds
    ///
    /// # Arguments
    ///
    /// * `duration` - How long the circuit stays open after breaking
    #[must_use]
    pub fn break_duration(mut self, duration: Duration) -> Self {
        self.break_duration = duration;
        self
    }

    /// Sets the callback to be invoked when the circuit breaker opens.
    ///
    /// This callback is called whenever the circuit breaker transitions from
    /// closed to open state due to exceeding the failure threshold.
    ///
    /// **Default**: No callback
    ///
    /// # Arguments
    ///
    /// * `callback` - Function that takes a reference to the output and
    ///   [`OnOpenedArgs`] containing circuit breaker context
    #[must_use]
    pub fn on_opened(mut self, callback: impl Fn(&Out, OnOpenedArgs) + Send + Sync + 'static) -> Self {
        self.on_opened = Some(OnOpened::new(callback));
        self
    }

    /// Sets the callback to be invoked when the circuit breaker closes.
    ///
    /// This callback is called whenever the circuit breaker transitions from
    /// half-open state to closed state after successful recovery.
    ///
    /// **Default**: No callback
    ///
    /// # Arguments
    ///
    /// * `callback` - Function that takes a reference to the output and
    ///   [`OnClosedArgs`] containing circuit breaker context
    #[must_use]
    pub fn on_closed(mut self, callback: impl Fn(&Out, OnClosedArgs) + Send + Sync + 'static) -> Self {
        self.on_closed = Some(OnClosed::new(callback));
        self
    }

    /// Sets the callback to be invoked when the circuit breaker is probing.
    ///
    /// This callback is called when the circuit breaker is in half-open state
    /// and is testing whether the underlying service has recovered.
    ///
    /// **Default**: No callback
    ///
    /// # Arguments
    ///
    /// * `callback` - Function that takes a mutable reference to the input and
    ///   [`OnProbingArgs`] containing circuit breaker context
    #[must_use]
    pub fn on_probing(mut self, callback: impl Fn(&mut In, OnProbingArgs) + Send + Sync + 'static) -> Self {
        self.on_probing = Some(OnProbing::new(callback));
        self
    }

    /// Sets the breaker ID provider function.
    ///
    /// Each unique [`BreakerId`] maintains its own independent circuit breaker state.
    /// A typical use case is HTTP requests where the breaker ID is derived from scheme,
    /// host, and port to isolate failures per backend service.
    ///
    /// **Default**: Single global circuit - all inputs share the same circuit breaker state
    ///
    /// # Arguments
    ///
    /// * `id_provider` - Function that takes a reference to the input and returns
    ///   a [`BreakerId`] identifying the circuit breaker instance to use
    ///
    /// # Example
    ///
    /// ```rust
    /// # use seatbelt::breaker::{BreakerLayer, BreakerId};
    /// // Example HTTP request structure
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
    /// // This ensures that:
    /// // - Inputs targeting https://api.service1.com share one circuit breaker instance
    /// // - Inputs targeting https://api.service2.com:8080 share another circuit breaker instance
    /// // - Inputs targeting http://localhost:3000 share yet another circuit breaker instance
    /// # }
    /// ```
    ///
    /// # Telemetry
    ///
    /// The values used to create breaker IDs are included in telemetry data (logs and metrics)
    /// for observability purposes. **Important**: Ensure that the values from which breaker IDs
    /// are created do not contain any sensitive data such as authentication tokens, personal
    /// identifiable information (PII), or other confidential data.
    #[must_use]
    pub fn breaker_id(mut self, id_provider: impl Fn(&In) -> BreakerId + Send + Sync + 'static) -> Self {
        self.breaker_id = Some(BreakerIdProvider::new(id_provider));
        self
    }

    /// Sets the half-open mode for the circuit breaker.
    ///
    /// This determines how the circuit breaker behaves when transitioning from half-open
    /// to a closed state.
    ///
    /// **Default**: [`HalfOpenMode::reliable`]
    #[must_use]
    pub fn half_open_mode(mut self, mode: HalfOpenMode) -> Self {
        self.half_open_mode = mode;
        self
    }

    /// Optionally enables the circuit breaker middleware based on a condition.
    ///
    /// When disabled, inputs pass through without circuit breaker protection.
    /// This call replaces any previous condition.
    ///
    /// **Default**: Always enabled
    ///
    /// # Arguments
    ///
    /// * `is_enabled` - Function that takes a reference to the input and returns
    ///   `true` if circuit breaker protection should be enabled for this input
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::new(is_enabled);
        self
    }

    /// Enables the circuit breaker middleware unconditionally.
    ///
    /// All inputs will have circuit breaker protection applied.
    /// This call replaces any previous condition.
    ///
    /// **Note**: This is the default behavior - circuit breaker is enabled by default.
    #[must_use]
    pub fn enable_always(mut self) -> Self {
        self.enable_if = EnableIf::always();
        self
    }

    /// Disables the circuit breaker middleware completely.
    ///
    /// All inputs will pass through without circuit breaker protection.
    /// This call replaces any previous condition.
    ///
    /// **Note**: This overrides the default enabled behavior.
    #[must_use]
    pub fn disable(mut self) -> Self {
        self.enable_if = EnableIf::never();
        self
    }
}

impl<In, Out, S> Layer<S> for BreakerLayer<In, Out, Set, Set> {
    type Service = Breaker<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        Breaker {
            inner,
            clock: self.context.get_clock().clone(),
            recovery: self.recovery.clone().expect("recovery must be set in Ready state"),
            rejected_input: self.rejected_input.clone().expect("rejected_input must be set in Ready state"),
            enable_if: self.enable_if.clone(),
            engines: self.engines(),
            on_opened: self.on_opened.clone(),
            on_closed: self.on_closed.clone(),
            on_probing: self.on_probing.clone(),
            id_provider: self.breaker_id.clone(),
        }
    }
}

impl<In, Out, S1, S2> BreakerLayer<In, Out, S1, S2> {
    fn probes_options(&self) -> ProbesOptions {
        self.half_open_mode
            // we will use break duration as the sampling duration for probes
            .to_options(self.break_duration, self.failure_threshold)
    }

    fn engines(&self) -> Engines {
        Engines::new(
            super::engine::EngineOptions {
                break_duration: self.break_duration,
                health_metrics_builder: HealthMetricsBuilder::new(self.sampling_duration, self.failure_threshold, self.min_throughput),
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
            _state: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use layered::Execute;
    use tick::Clock;

    use super::*;
    use crate::breaker::RecoveryArgs;
    use crate::breaker::engine::probing::ProbeOptions;
    use crate::testing::RecoverableType;

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
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
    #[expect(clippy::float_cmp, reason = "Test")]
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

        let layer = layer.sampling_duration(Duration::from_secs(60));
        assert_eq!(layer.sampling_duration, Duration::from_secs(60));

        let layer = layer.sampling_duration(Duration::from_millis(500));
        assert_eq!(layer.sampling_duration, Duration::from_millis(500));
    }

    #[test]
    fn break_duration_sets_correctly() {
        let layer = create_ready_layer();

        // Test setting different break durations
        let layer = layer.break_duration(Duration::from_secs(5));
        assert_eq!(layer.break_duration, Duration::from_secs(5));

        let layer = layer.break_duration(Duration::from_secs(120));
        assert_eq!(layer.break_duration, Duration::from_secs(120));

        let layer = layer.break_duration(Duration::from_millis(2000));
        assert_eq!(layer.break_duration, Duration::from_millis(2000));
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
    fn default_values_are_correct() {
        let context = create_test_context();
        let layer = BreakerLayer::new("test".into(), &context);

        assert_eq!(layer.failure_threshold, DEFAULT_FAILURE_THRESHOLD);
        assert_eq!(layer.min_throughput, DEFAULT_MIN_THROUGHPUT);
        assert_eq!(layer.sampling_duration, DEFAULT_SAMPLING_DURATION);
        assert_eq!(layer.break_duration, DEFAULT_BREAK_DURATION);
        assert_eq!(layer.half_open_mode, HalfOpenMode::reliable(None));
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "Test")]
    pub fn half_open_mode_ok() {
        let layer = create_ready_layer().half_open_mode(HalfOpenMode::quick());
        assert_eq!(layer.half_open_mode, HalfOpenMode::quick());

        let probes = layer
            .break_duration(Duration::from_secs(234))
            .failure_threshold(0.52)
            .half_open_mode(HalfOpenMode::reliable(None))
            .probes_options();

        // access the last probe which should be the health probe
        let probe = probes.probes().last().unwrap();

        match probe {
            ProbeOptions::HealthProbe(health_probe) => {
                assert_eq!(health_probe.stage_duration(), Duration::from_secs(234));
                assert_eq!(health_probe.failure_threshold(), 0.52);
            }
            ProbeOptions::SingleProbe { .. } => panic!("Expected HealthProbe"),
        }
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
