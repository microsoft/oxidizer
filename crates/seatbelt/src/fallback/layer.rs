// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::Arc;

use layered::Layer;

use crate::fallback::*;
use crate::typestates::{NotSet, Set};
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{ResilienceContext, TelemetryString};

/// Configures fallback middleware.
///
/// [`Fallback::layer`](crate::fallback::Fallback::layer) creates this builder.
/// Its type state makes these properties required:
///
/// - [`should_fallback`][FallbackLayer::should_fallback] decides whether to replace output.
/// - [`fallback`][FallbackLayer::fallback] or [`fallback_async`][FallbackLayer::fallback_async]:
///   creates the replacement output.
///
/// See the [fallback module][crate::fallback] for examples.
///
/// # Type state
///
/// - `S1` records whether [`should_fallback`][FallbackLayer::should_fallback] is set.
/// - `S2` records whether [`fallback`][FallbackLayer::fallback] is set.
#[derive(Debug)]
pub struct FallbackLayer<In, Out, S1 = Set, S2 = Set> {
    should_fallback: Option<ShouldFallback<Out>>,
    fallback_action: Option<FallbackAction<Out>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> FallbackLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: TelemetryString, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            should_fallback: None,
            fallback_action: None,
            enable_if: EnableIf::default(),
            telemetry: context.create_telemetry(name),
            _state: PhantomData,
        }
    }
}

impl<In, Out, S1, S2> FallbackLayer<In, Out, S1, S2> {
    /// Sets the predicate that decides whether to use a fallback.
    ///
    /// The predicate receives output from the inner service.
    /// Return `true` to replace the output.
    /// This call replaces the previous predicate.
    #[must_use]
    pub fn should_fallback(mut self, predicate: impl Fn(&Out) -> bool + Send + Sync + 'static) -> FallbackLayer<In, Out, Set, S2> {
        self.should_fallback = Some(ShouldFallback::new(predicate));
        self.into_state::<Set, S2>()
    }

    /// Enables fallback middleware when a condition is true.
    ///
    /// If the condition is false, the middleware returns the inner service output.
    /// The `is_enabled` function receives the input.
    ///
    /// Default: Enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::custom(is_enabled);
        self
    }

    /// Enables or disables the fallback middleware.
    ///
    /// If disabled, requests run without fallback middleware.
    /// This call replaces the previous condition.
    #[must_use]
    fn enable(mut self, enabled: bool) -> Self {
        self.enable_if = EnableIf::new(enabled);
        self
    }

    /// Enables fallback middleware for all requests.
    ///
    /// This call replaces the previous condition.
    /// Fallback middleware is enabled by default.
    #[must_use]
    pub fn enable_always(self) -> Self {
        self.enable(true)
    }

    /// Disables fallback middleware for all requests.
    ///
    /// This call replaces the previous condition.
    #[must_use]
    pub fn disable(self) -> Self {
        self.enable(false)
    }
}

impl<In, Out: Send + 'static, S1, S2> FallbackLayer<In, Out, S1, S2> {
    /// Sets a synchronous fallback action.
    ///
    /// The action receives invalid output and [`FallbackActionArgs`].
    /// It returns replacement output.
    /// This call replaces the previous fallback action.
    #[must_use]
    pub fn fallback(mut self, action: impl Fn(Out, FallbackActionArgs) -> Out + Send + Sync + 'static) -> FallbackLayer<In, Out, S1, Set> {
        self.fallback_action = Some(FallbackAction::new_sync(action));
        self.into_state::<S1, Set>()
    }

    /// Sets a fixed fallback value that is cloned on every invocation.
    ///
    /// Use this method when the replacement output is always the same.
    /// It discards the invalid output and clones `value`.
    ///
    /// This call replaces the previous fallback action.
    #[must_use]
    pub fn fallback_output(self, value: Out) -> FallbackLayer<In, Out, S1, Set>
    where
        Out: Clone + Sync,
    {
        self.fallback(move |_, _| value.clone())
    }

    /// Sets an asynchronous fallback action.
    ///
    /// The action receives invalid output and [`FallbackActionArgs`].
    /// Its future returns replacement output.
    /// This call replaces the previous fallback action.
    #[must_use]
    pub fn fallback_async<F, Fut>(mut self, action: F) -> FallbackLayer<In, Out, S1, Set>
    where
        F: Fn(Out, FallbackActionArgs) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Out> + Send + 'static,
    {
        self.fallback_action = Some(FallbackAction::new_async(action));
        self.into_state::<S1, Set>()
    }
}

impl<In, Out, S> Layer<S> for FallbackLayer<In, Out, Set, Set> {
    type Service = Fallback<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        let shared = FallbackShared {
            enable_if: self.enable_if.clone(),
            should_fallback: self.should_fallback.clone().expect("enforced by the type state pattern"),
            fallback_action: self.fallback_action.clone().expect("enforced by the type state pattern"),
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry: self.telemetry.clone(),
        };

        Fallback {
            shared: Arc::new(shared),
            inner,
        }
    }
}

impl<In, Out, S1, S2> FallbackLayer<In, Out, S1, S2> {
    fn into_state<T1, T2>(self) -> FallbackLayer<In, Out, T1, T2> {
        FallbackLayer {
            should_fallback: self.should_fallback,
            fallback_action: self.fallback_action,
            enable_if: self.enable_if,
            telemetry: self.telemetry,
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

    #[test]
    fn new_needs_should_fallback_and_action() {
        let context = create_test_context();
        let layer: FallbackLayer<_, _, NotSet, NotSet> = FallbackLayer::new("test_fallback".into(), &context);

        assert!(layer.should_fallback.is_none());
        assert!(layer.fallback_action.is_none());
        assert_eq!(layer.telemetry.strategy_name.as_ref(), "test_fallback");
        assert!(layer.enable_if.call(&"test_input".to_string()));
    }

    #[test]
    fn should_fallback_ensure_set_correctly() {
        let context = create_test_context();
        let layer: FallbackLayer<_, _, Set, NotSet> =
            FallbackLayer::new("test".into(), &context).should_fallback(|output: &String| output == "bad");

        assert!(layer.should_fallback.as_ref().unwrap().call(&"bad".to_string()));
        assert!(!layer.should_fallback.as_ref().unwrap().call(&"good".to_string()));
    }

    #[test]
    fn fallback_sync_ensure_set_correctly() {
        let context = create_test_context();
        let layer: FallbackLayer<_, _, NotSet, Set> =
            FallbackLayer::new("test".into(), &context).fallback(|_output: String, _args| "replaced".to_string());

        assert!(layer.fallback_action.is_some());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fallback_async_ensure_set_correctly() {
        let context = create_test_context();
        let layer: FallbackLayer<_, _, NotSet, Set> =
            FallbackLayer::new("test".into(), &context).fallback_async(|_output: String, _args| async { "replaced".to_string() });

        let result = layer.fallback_action.unwrap().call("bad".to_string(), FallbackActionArgs {}).await;
        assert_eq!(result, "replaced");
    }

    #[test]
    fn enable_if_ok() {
        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().enable_if(|input| matches!(input.as_ref(), "enable"));

        assert!(layer.enable_if.call(&"enable".to_string()));
        assert!(!layer.enable_if.call(&"disable".to_string()));
    }

    #[test]
    fn disable_ok() {
        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().disable();

        assert!(!layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn enable_ok() {
        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().disable().enable_always();

        assert!(layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn should_fallback_when_ready_ok() {
        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().should_fallback(|output: &String| output == "new_bad");

        assert!(layer.should_fallback.unwrap().call(&"new_bad".to_string()));
    }

    #[test]
    fn fallback_output_ok() {
        let context = create_test_context();
        let layer: FallbackLayer<_, _, NotSet, Set> = FallbackLayer::new("test".into(), &context).fallback_output("fixed".to_string());

        assert!(layer.fallback_action.is_some());
    }

    #[test]
    fn fallback_when_ready_ok() {
        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().fallback(|_, _| "new_fallback".to_string());

        assert!(layer.fallback_action.is_some());
    }

    #[test]
    fn layer_ok() {
        let _layered = create_ready_layer().layer(Execute::new(|input: String| async move { input }));
    }

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(FallbackLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(FallbackLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(FallbackLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(FallbackLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> ResilienceContext<String, String> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer() -> FallbackLayer<String, String, Set, Set> {
        FallbackLayer::new("test".into(), &create_test_context())
            .should_fallback(|output: &String| output == "bad")
            .fallback(|_output: String, _args| "fallback_value".to_string())
    }
}
