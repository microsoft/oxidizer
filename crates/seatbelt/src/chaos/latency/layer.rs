// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use layered::Layer;

use crate::chaos::latency::*;
use crate::rnd::Rnd;
use crate::typestates::{NotSet, Set};
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{ResilienceContext, TelemetryString};

/// Builder for configuring chaos latency middleware.
///
/// This type is created by calling [`Latency::layer`](crate::chaos::latency::Latency::layer)
/// and uses the type-state pattern to enforce that required properties are configured
/// before the layer can be built:
///
/// - [`rate`][LatencyLayer::rate] or [`rate_with`][LatencyLayer::rate_with]:
///   Required probability of latency injection
/// - [`latency`][LatencyLayer::latency], [`latency_with`][LatencyLayer::latency_with], or
///   [`latency_range`][LatencyLayer::latency_range]:
///   Required latency duration
///
/// For comprehensive examples, see the [latency module][crate::chaos::latency] documentation.
///
/// # Type State
///
/// - `S1`: Tracks whether [`rate`][LatencyLayer::rate] or [`rate_with`][LatencyLayer::rate_with] has been set
/// - `S2`: Tracks whether [`latency`][LatencyLayer::latency] or [`latency_with`][LatencyLayer::latency_with] has been set
#[derive(Debug)]
pub struct LatencyLayer<In, Out, S1 = Set, S2 = Set> {
    context: ResilienceContext<In, Out>,
    rate: Option<LatencyRate<In>>,
    latency_duration: Option<LatencyDuration<In>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    pub(crate) rnd: Rnd,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> LatencyLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: TelemetryString, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            context: context.clone(),
            rate: None,
            latency_duration: None,
            enable_if: EnableIf::default(),
            telemetry: context.create_telemetry(name),
            rnd: Rnd::default(),
            _state: PhantomData,
        }
    }
}

impl<In, Out, S1, S2> LatencyLayer<In, Out, S1, S2> {
    /// Sets a callback that dynamically computes the injection rate for each
    /// request.
    ///
    /// The `rate_fn` receives a reference to the input and
    /// [`LatencyRateArgs`], and returns a probability in `[0.0, 1.0]` where
    /// `0.0` means never inject latency and `1.0` means always inject. The
    /// returned value is clamped to this range.
    ///
    /// This allows the injection rate to vary per request based on request
    /// properties or external state.
    #[must_use]
    pub fn rate_with(mut self, rate_fn: impl Fn(&In, LatencyRateArgs) -> f64 + Send + Sync + 'static) -> LatencyLayer<In, Out, Set, S2> {
        self.rate = Some(LatencyRate::new(rate_fn));
        self.into_state::<Set, S2>()
    }

    /// Sets the probability of injecting latency before calling the inner
    /// service.
    ///
    /// The `rate` is clamped to the range `[0.0, 1.0]` where `0.0` means never
    /// inject and `1.0` means always inject.
    #[must_use]
    pub fn rate(self, rate: f64) -> LatencyLayer<In, Out, Set, S2> {
        let clamped = rate.clamp(0.0, 1.0);
        self.rate_with(move |_, _| clamped)
    }

    /// Sets a callback that dynamically computes the latency duration for each
    /// request.
    ///
    /// The `latency_fn` receives a reference to the input and
    /// [`LatencyDurationArgs`], and returns the [`Duration`] to delay before
    /// forwarding the request to the inner service.
    #[must_use]
    pub fn latency_with(
        mut self,
        latency_fn: impl Fn(&In, LatencyDurationArgs) -> Duration + Send + Sync + 'static,
    ) -> LatencyLayer<In, Out, S1, Set> {
        self.latency_duration = Some(LatencyDuration::new(latency_fn));
        self.into_state::<S1, Set>()
    }

    /// Sets a fixed latency duration to inject before calling the inner
    /// service.
    #[must_use]
    pub fn latency(self, duration: Duration) -> LatencyLayer<In, Out, S1, Set> {
        self.latency_with(move |_, _| duration)
    }

    /// Sets a random latency duration chosen uniformly from the given range
    /// before calling the inner service.
    ///
    /// On each injection, a duration is picked uniformly at random from
    /// `[range.start, range.end)`.
    ///
    /// # Panics
    ///
    /// Panics if `range.start >= range.end`.
    #[must_use]
    pub fn latency_range(mut self, range: Range<Duration>) -> LatencyLayer<In, Out, S1, Set> {
        assert!(
            range.start < range.end,
            "latency_range requires start < end, got {start:?}..{end:?}",
            start = range.start,
            end = range.end
        );
        let rnd = self.rnd.clone();
        self.latency_duration = Some(LatencyDuration::new(move |_, _| {
            let span = range.end - range.start;
            range.start + span.mul_f64(rnd.next_f64())
        }));
        self.into_state::<S1, Set>()
    }

    /// Applies configuration from a [`LatencyConfig`] struct.
    ///
    /// This sets the [`rate`][LatencyLayer::rate],
    /// [`latency`][LatencyLayer::latency] (or
    /// [`latency_range`][LatencyLayer::latency_range] when
    /// [`max_latency`][LatencyConfig::max_latency] is set), and
    /// [`enable`][LatencyLayer::enable] properties from the config.
    ///
    /// This transitions both type-state parameters to [`Set`], so a single
    /// `config()` call is sufficient to produce a buildable layer.
    #[must_use]
    pub fn config(self, config: &LatencyConfig) -> LatencyLayer<In, Out, Set, Set> {
        let with_rate = self.rate(config.rate).enable(config.enabled);
        match config.max_latency {
            Some(max) => with_rate.latency_range(config.latency..max),
            None => with_rate.latency(config.latency),
        }
    }

    /// Optionally enables the latency middleware based on a condition.
    ///
    /// When disabled, the inner service output is returned as-is. The
    /// `is_enabled` function receives a reference to the input and returns
    /// `true` when latency injection should be active for this request.
    ///
    /// **Default**: Always enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::custom(is_enabled);
        self
    }

    /// Enables or disables the latency middleware.
    ///
    /// When disabled, requests pass through without latency injection.
    /// This call replaces any previous condition.
    #[must_use]
    pub fn enable(mut self, enabled: bool) -> Self {
        self.enable_if = EnableIf::new(enabled);
        self
    }
}

impl<In, Out, S> Layer<S> for LatencyLayer<In, Out, Set, Set> {
    type Service = Latency<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        let shared = LatencyShared::new(
            self.context.get_clock().clone(),
            self.rate.clone().expect("enforced by the type state pattern"),
            self.enable_if.clone(),
            self.latency_duration.clone().expect("enforced by the type state pattern"),
            self.rnd.clone(),
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            self.telemetry.clone(),
        );

        Latency {
            shared: Arc::new(shared),
            inner,
        }
    }
}

impl<In, Out, S1, S2> LatencyLayer<In, Out, S1, S2> {
    fn into_state<T1, T2>(self) -> LatencyLayer<In, Out, T1, T2> {
        LatencyLayer {
            context: self.context,
            rate: self.rate,
            latency_duration: self.latency_duration,
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
    fn new_needs_rate_and_latency() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, NotSet, NotSet> = LatencyLayer::new("test_latency".into(), &context);

        insta::assert_debug_snapshot!(layer);
    }

    #[test]
    fn rate_ensure_set_correctly() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, Set, NotSet> = LatencyLayer::new("test".into(), &context).rate(0.5);

        assert!(layer.rate.is_some());
    }

    #[test]
    fn rate_clamps_below_zero() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, Set, NotSet> = LatencyLayer::new("test".into(), &context).rate(-0.1);
        let rate = layer.rate.unwrap().call(&"test".to_string(), LatencyRateArgs {});
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn rate_clamps_above_one() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, Set, NotSet> = LatencyLayer::new("test".into(), &context).rate(1.1);
        let rate = layer.rate.unwrap().call(&"test".to_string(), LatencyRateArgs {});
        assert_eq!(rate, 1.0);
    }

    #[test]
    fn rate_boundary_zero_ok() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, Set, NotSet> = LatencyLayer::new("test".into(), &context).rate(0.0);
        let rate = layer.rate.unwrap().call(&"test".to_string(), LatencyRateArgs {});
        assert_eq!(rate, 0.0);
    }

    #[test]
    fn rate_boundary_one_ok() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, Set, NotSet> = LatencyLayer::new("test".into(), &context).rate(1.0);
        let rate = layer.rate.unwrap().call(&"test".to_string(), LatencyRateArgs {});
        assert_eq!(rate, 1.0);
    }

    #[test]
    fn latency_with_ensure_set_correctly() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, NotSet, Set> =
            LatencyLayer::new("test".into(), &context).latency_with(|_input, _args| Duration::from_millis(100));

        assert!(layer.latency_duration.is_some());
    }

    #[test]
    fn latency_ensure_set_correctly() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, NotSet, Set> = LatencyLayer::new("test".into(), &context).latency(Duration::from_millis(100));

        assert!(layer.latency_duration.is_some());
    }

    #[test]
    fn latency_range_ensure_set_correctly() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, NotSet, Set> =
            LatencyLayer::new("test".into(), &context).latency_range(Duration::from_millis(100)..Duration::from_millis(500));

        assert!(layer.latency_duration.is_some());
    }

    #[test]
    #[should_panic(expected = "latency_range requires start < end")]
    fn latency_range_panics_on_invalid_range() {
        let context = create_test_context();
        let _layer: LatencyLayer<_, _, NotSet, Set> =
            LatencyLayer::new("test".into(), &context).latency_range(Duration::from_millis(500)..Duration::from_millis(100));
    }

    #[test]
    #[should_panic(expected = "latency_range requires start < end")]
    fn latency_range_panics_on_equal_bounds() {
        let context = create_test_context();
        let _layer: LatencyLayer<_, _, NotSet, Set> =
            LatencyLayer::new("test".into(), &context).latency_range(Duration::from_millis(100)..Duration::from_millis(100));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn config_applies_all_settings() {
        let context = create_test_context();
        let config = LatencyConfig {
            enabled: false,
            rate: 0.75,
            latency: Duration::from_millis(200),
            max_latency: None,
        };
        let layer: LatencyLayer<_, _, Set, Set> = LatencyLayer::new("test".into(), &context).config(&config);

        insta::assert_debug_snapshot!(layer);
    }

    #[test]
    fn config_with_max_latency_applies_range() {
        let context = create_test_context();
        let config = LatencyConfig {
            enabled: true,
            rate: 0.5,
            latency: Duration::from_millis(100),
            max_latency: Some(Duration::from_millis(500)),
        };
        let layer: LatencyLayer<_, _, Set, Set> = LatencyLayer::new("test".into(), &context).config(&config);

        assert!(layer.rate.is_some());
        assert!(layer.latency_duration.is_some());
    }

    #[test]
    fn enable_if_ok() {
        let layer: LatencyLayer<_, _, Set, Set> = create_ready_layer().enable_if(|input| matches!(input.as_ref(), "enable"));

        assert!(layer.enable_if.call(&"enable".to_string()));
        assert!(!layer.enable_if.call(&"disable".to_string()));
    }

    #[test]
    fn enable_false_ok() {
        let layer: LatencyLayer<_, _, Set, Set> = create_ready_layer().enable(false);

        assert!(!layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn enable_true_ok() {
        let layer: LatencyLayer<_, _, Set, Set> = create_ready_layer().enable(false).enable(true);

        assert!(layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn rate_when_ready_ok() {
        let layer: LatencyLayer<_, _, Set, Set> = create_ready_layer().rate(0.99);

        let rate = layer.rate.unwrap().call(&"test".to_string(), LatencyRateArgs {});
        assert_eq!(rate, 0.99);
    }

    #[test]
    fn rate_with_ensure_set_correctly() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, Set, NotSet> = LatencyLayer::new("test".into(), &context).rate_with(|_input, _args| 0.42);

        assert!(layer.rate.is_some());
    }

    #[test]
    fn rate_with_receives_input() {
        let context = create_test_context();
        let layer: LatencyLayer<_, _, Set, NotSet> =
            LatencyLayer::new("test".into(), &context).rate_with(|input, _args| if input.starts_with("high") { 1.0 } else { 0.0 });

        let rate_fn = layer.rate.unwrap();
        assert_eq!(rate_fn.call(&"high_priority".to_string(), LatencyRateArgs {}), 1.0);
        assert_eq!(rate_fn.call(&"low_priority".to_string(), LatencyRateArgs {}), 0.0);
    }

    #[test]
    fn rate_with_when_ready_ok() {
        let layer: LatencyLayer<_, _, Set, Set> = create_ready_layer().rate_with(|_, _| 0.75);

        assert!(layer.rate.is_some());
    }

    #[test]
    fn latency_with_when_ready_ok() {
        let layer: LatencyLayer<_, _, Set, Set> = create_ready_layer().latency_with(|_, _| Duration::from_millis(50));

        assert!(layer.latency_duration.is_some());
    }

    #[test]
    fn layer_ok() {
        let _layered = create_ready_layer().layer(Execute::new(|input: String| async move { input }));
    }

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(LatencyLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(LatencyLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(LatencyLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(LatencyLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> ResilienceContext<String, String> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer() -> LatencyLayer<String, String, Set, Set> {
        LatencyLayer::new("test".into(), &create_test_context())
            .rate(0.5)
            .latency(Duration::from_millis(100))
    }
}
