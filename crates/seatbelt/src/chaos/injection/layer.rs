// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::Arc;

use layered::Layer;

use crate::chaos::injection::*;
use crate::rnd::Rnd;
use crate::typestates::{NotSet, Set};
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{ResilienceContext, TelemetryString};

/// Configures output injection middleware.
///
/// [`Injection::layer`](crate::chaos::injection::Injection::layer) creates this builder.
/// Its type state makes these properties required:
///
/// - [`rate`][InjectionLayer::rate] or [`rate_with`][InjectionLayer::rate_with]:
///   sets the injection rate.
/// - [`output_with`][InjectionLayer::output_with], [`output`][InjectionLayer::output],
///   [`output_error_with`][InjectionLayer::output_error_with], or
///   [`output_error`][InjectionLayer::output_error]:
///   creates injected output.
///
/// See the [injection module][crate::chaos::injection] for examples.
///
/// # Type state
///
/// - `S1` records whether the injection rate is set.
/// - `S2` records whether the output function is set.
#[derive(Debug)]
pub struct InjectionLayer<In, Out, S1 = Set, S2 = Set> {
    rate: Option<InjectionRate<In>>,
    injection_output: Option<InjectionOutput<In, Out>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    pub(crate) rnd: Rnd,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> InjectionLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: TelemetryString, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            rate: None,
            injection_output: None,
            enable_if: EnableIf::default(),
            telemetry: context.create_telemetry(name),
            rnd: Rnd::default(),
            _state: PhantomData,
        }
    }
}

impl<In, Out, S1, S2> InjectionLayer<In, Out, S1, S2> {
    /// Sets a callback that calculates the injection rate for each request.
    ///
    /// The function receives the input and [`InjectionRateArgs`].
    /// It returns a value in `[0.0, 1.0]`.
    /// A value of `0.0` never injects output.
    /// A value of `1.0` always injects output.
    /// Values outside the range are clamped.
    ///
    /// The rate can depend on the request or external state.
    #[must_use]
    pub fn rate_with(
        mut self,
        rate_fn: impl Fn(&In, InjectionRateArgs) -> f64 + Send + Sync + 'static,
    ) -> InjectionLayer<In, Out, Set, S2> {
        self.rate = Some(InjectionRate::new(rate_fn));
        self.into_state::<Set, S2>()
    }

    /// Sets the rate for injected output.
    ///
    /// The value is clamped to `[0.0, 1.0]`.
    #[must_use]
    pub fn rate(self, rate: f64) -> InjectionLayer<In, Out, Set, S2> {
        let clamped = rate.clamp(0.0, 1.0);
        self.rate_with(move |_, _| clamped)
    }

    /// Applies an [`InjectionConfig`] to this layer.
    ///
    /// This sets the [`rate`][InjectionLayer::rate] and
    /// [`enable`][InjectionLayer::enable_always] / [`disable`][InjectionLayer::disable]
    /// properties from the configuration.
    #[must_use]
    pub fn config(self, config: &InjectionConfig) -> InjectionLayer<In, Out, Set, S2> {
        self.rate(config.rate).enable(config.enabled)
    }

    /// Enables injection middleware when a condition is true.
    ///
    /// If the condition is false, the middleware calls the inner service.
    /// The `is_enabled` function receives the input.
    ///
    /// Default: Enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::custom(is_enabled);
        self
    }

    /// Enables or disables the injection middleware.
    ///
    /// If disabled, requests run without injection.
    /// This call replaces the previous condition.
    #[must_use]
    fn enable(mut self, enabled: bool) -> Self {
        self.enable_if = EnableIf::new(enabled);
        self
    }

    /// Enables injection middleware for all requests.
    ///
    /// This call replaces the previous condition.
    /// Injection middleware is enabled by default.
    #[must_use]
    pub fn enable_always(self) -> Self {
        self.enable(true)
    }

    /// Disables injection middleware for all requests.
    ///
    /// This call replaces the previous condition.
    #[must_use]
    pub fn disable(self) -> Self {
        self.enable(false)
    }
}

impl<In: Send + 'static, Out: Send + 'static, S1, S2> InjectionLayer<In, Out, S1, S2> {
    /// Sets a function that creates injected output.
    ///
    /// The function receives consumed input and [`InjectionOutputArgs`].
    /// It returns output without calling the inner service.
    /// This call replaces the previous output function.
    #[must_use]
    pub fn output_with(
        mut self,
        output_fn: impl Fn(In, InjectionOutputArgs) -> Out + Send + Sync + 'static,
    ) -> InjectionLayer<In, Out, S1, Set> {
        self.injection_output = Some(InjectionOutput::new(output_fn));
        self.into_state::<S1, Set>()
    }

    /// Sets a fixed value that is cloned on every injection.
    ///
    /// Use this method when injected output is always the same.
    /// It discards the input and clones `value`.
    ///
    /// This call replaces the previous output function.
    #[must_use]
    pub fn output(self, value: Out) -> InjectionLayer<In, Out, S1, Set>
    where
        Out: Clone + Sync,
    {
        self.output_with(move |_, _| value.clone())
    }
}

impl<In, Ok, Err, S1, S2> InjectionLayer<In, Result<Ok, Err>, S1, S2>
where
    In: Send + 'static,
    Ok: Send + 'static,
    Err: Send + 'static,
{
    /// Sets a function that creates an injected error.
    ///
    /// The function receives consumed input and [`InjectionOutputArgs`].
    /// The middleware wraps its error in [`Err`].
    /// This call replaces the previous output function.
    ///
    /// Use this method when injected output is always an error.
    #[must_use]
    pub fn output_error_with(
        self,
        error_fn: impl Fn(In, InjectionOutputArgs) -> Err + Send + Sync + 'static,
    ) -> InjectionLayer<In, Result<Ok, Err>, S1, Set> {
        self.output_with(move |input, args| Result::Err(error_fn(input, args)))
    }

    /// Sets a fixed error value that is cloned on every injection.
    ///
    /// Use this method when the injected error is always the same.
    /// It discards the input and clones `error`.
    ///
    /// This call replaces the previous output function.
    #[must_use]
    pub fn output_error(self, error: Err) -> InjectionLayer<In, Result<Ok, Err>, S1, Set>
    where
        Err: Clone + Sync,
    {
        self.output_error_with(move |_, _| error.clone())
    }
}

impl<In, Out, S> Layer<S> for InjectionLayer<In, Out, Set, Set> {
    type Service = Injection<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        let shared = InjectionShared {
            rate: self.rate.clone().expect("enforced by the type state pattern"),
            enable_if: self.enable_if.clone(),
            injection_output: self.injection_output.clone().expect("enforced by the type state pattern"),
            rnd: self.rnd.clone(),
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry: self.telemetry.clone(),
        };

        Injection {
            shared: Arc::new(shared),
            inner,
        }
    }
}

impl<In, Out, S1, S2> InjectionLayer<In, Out, S1, S2> {
    fn into_state<T1, T2>(self) -> InjectionLayer<In, Out, T1, T2> {
        InjectionLayer {
            rate: self.rate,
            injection_output: self.injection_output,
            enable_if: self.enable_if,
            telemetry: self.telemetry,
            rnd: self.rnd,
            _state: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "exact float comparisons are intentional in these clamping/boundary tests"
)]
mod tests {
    use std::fmt::Debug;

    use layered::Execute;
    use tick::Clock;

    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_needs_rate_and_output() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, NotSet, NotSet> = InjectionLayer::new("test_injection".into(), &context);

        insta::assert_debug_snapshot!(layer);
    }

    #[test]
    fn rate_ensure_set_correctly() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, Set, NotSet> = InjectionLayer::new("test".into(), &context).rate(0.5);

        assert!(layer.rate.is_some());
    }

    #[test]
    fn rate_clamps_below_zero() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, Set, NotSet> = InjectionLayer::new("test".into(), &context).rate(-0.1);
        let rate = layer.rate.unwrap().call(&"test".to_string(), InjectionRateArgs {});
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn rate_clamps_above_one() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, Set, NotSet> = InjectionLayer::new("test".into(), &context).rate(1.1);
        let rate = layer.rate.unwrap().call(&"test".to_string(), InjectionRateArgs {});
        assert_eq!(rate, 1.0);
    }

    #[test]
    fn rate_boundary_zero_ok() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, Set, NotSet> = InjectionLayer::new("test".into(), &context).rate(0.0);
        let rate = layer.rate.unwrap().call(&"test".to_string(), InjectionRateArgs {});
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn rate_boundary_one_ok() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, Set, NotSet> = InjectionLayer::new("test".into(), &context).rate(1.0);
        let rate = layer.rate.unwrap().call(&"test".to_string(), InjectionRateArgs {});
        assert_eq!(rate, 1.0);
    }

    #[test]
    fn output_with_ensure_set_correctly() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, NotSet, Set> =
            InjectionLayer::new("test".into(), &context).output_with(|_input, _args| "injected".to_string());

        assert!(layer.injection_output.is_some());
    }

    #[test]
    fn output_ensure_set_correctly() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, NotSet, Set> = InjectionLayer::new("test".into(), &context).output("fixed".to_string());

        assert!(layer.injection_output.is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn config_applies_all_settings() {
        let context = create_test_context();
        let config = InjectionConfig {
            enabled: false,
            rate: 0.75,
        };
        let layer: InjectionLayer<_, _, Set, NotSet> = InjectionLayer::new("test".into(), &context).config(&config);

        insta::assert_debug_snapshot!(layer);
    }

    #[test]
    fn enable_if_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer().enable_if(|input| matches!(input.as_ref(), "enable"));

        assert!(layer.enable_if.call(&"enable".to_string()));
        assert!(!layer.enable_if.call(&"disable".to_string()));
    }

    #[test]
    fn disable_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer().disable();

        assert!(!layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn enable_always_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer().disable().enable_always();

        assert!(layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn rate_when_ready_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer().rate(0.99);

        let rate = layer.rate.unwrap().call(&"test".to_string(), InjectionRateArgs {});
        assert_eq!(rate, 0.99);
    }

    #[test]
    fn output_error_with_ensure_set_correctly() {
        let context = create_test_context_result();
        let layer: InjectionLayer<_, _, NotSet, Set> =
            InjectionLayer::new("test".into(), &context).output_error_with(|_input, _args| "injected_error".to_string());

        assert!(layer.injection_output.is_some());
    }

    #[test]
    fn output_error_ensure_set_correctly() {
        let context = create_test_context_result();
        let layer: InjectionLayer<_, _, NotSet, Set> = InjectionLayer::new("test".into(), &context).output_error("fixed_error".to_string());

        assert!(layer.injection_output.is_some());
    }

    #[test]
    fn output_error_with_when_ready_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer_result().output_error_with(|_, _| "new_error".to_string());

        assert!(layer.injection_output.is_some());
    }

    #[test]
    fn output_error_when_ready_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer_result().output_error("fixed_error".to_string());

        assert!(layer.injection_output.is_some());
    }

    #[test]
    fn output_with_when_ready_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer().output_with(|_, _| "new".to_string());

        assert!(layer.injection_output.is_some());
    }

    #[test]
    fn rate_with_ensure_set_correctly() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, Set, NotSet> = InjectionLayer::new("test".into(), &context).rate_with(|_input, _args| 0.42);

        assert!(layer.rate.is_some());
    }

    #[test]
    fn rate_with_receives_input() {
        let context = create_test_context();
        let layer: InjectionLayer<_, _, Set, NotSet> =
            InjectionLayer::new("test".into(), &context).rate_with(|input, _args| if input.starts_with("high") { 1.0 } else { 0.0 });

        let rate_fn = layer.rate.unwrap();
        assert_eq!(rate_fn.call(&"high_priority".to_string(), InjectionRateArgs {}), 1.0);
        assert_eq!(rate_fn.call(&"low_priority".to_string(), InjectionRateArgs {}), 0.0);
    }

    #[test]
    fn rate_with_when_ready_ok() {
        let layer: InjectionLayer<_, _, Set, Set> = create_ready_layer().rate_with(|_, _| 0.75);

        assert!(layer.rate.is_some());
    }

    #[test]
    fn layer_ok() {
        let _layered = create_ready_layer().layer(Execute::new(|input: String| async move { input }));
    }

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(InjectionLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(InjectionLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(InjectionLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(InjectionLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> ResilienceContext<String, String> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer() -> InjectionLayer<String, String, Set, Set> {
        InjectionLayer::new("test".into(), &create_test_context())
            .rate(0.5)
            .output_with(|_input, _args| "injected_value".to_string())
    }

    fn create_test_context_result() -> ResilienceContext<String, Result<String, String>> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer_result() -> InjectionLayer<String, Result<String, String>, Set, Set> {
        InjectionLayer::new("test".into(), &create_test_context_result())
            .rate(0.5)
            .output_error_with(|_input, _args| "injected_error".to_string())
    }
}
