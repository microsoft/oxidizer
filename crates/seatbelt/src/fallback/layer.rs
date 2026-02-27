// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::Arc;

use layered::Layer;

use crate::fallback::*;
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{NotSet, Set};

/// Builder for configuring fallback resilience middleware.
///
/// This type is created by calling [`Fallback::layer`](crate::fallback::Fallback::layer)
/// and uses the type-state pattern to enforce that required properties are configured
/// before the layer can be built:
///
/// - [`should_fallback`][FallbackLayer::should_fallback]: Required predicate that decides
///   whether the inner service output needs a replacement
/// - [`fallback`][FallbackLayer::fallback] or [`fallback_async`][FallbackLayer::fallback_async]:
///   Required function that produces the replacement output
///
/// For comprehensive examples, see the [fallback module][crate::fallback] documentation.
///
/// # Type State
///
/// - `S1`: Tracks whether [`should_fallback`][FallbackLayer::should_fallback] has been set
/// - `S2`: Tracks whether [`fallback`][FallbackLayer::fallback] has been set
#[derive(Debug)]
pub struct FallbackLayer<In, Out, S1 = Set, S2 = Set> {
    should_fallback: Option<ShouldFallback<Out>>,
    fallback_action: Option<FallbackAction<Out>>,
    before_fallback: Option<BeforeFallback<Out>>,
    after_fallback: Option<AfterFallback<Out>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> FallbackLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: std::borrow::Cow<'static, str>, context: &crate::ResilienceContext<In, Out>) -> Self {
        Self {
            should_fallback: None,
            fallback_action: None,
            before_fallback: None,
            after_fallback: None,
            enable_if: EnableIf::always(),
            telemetry: context.create_telemetry(name),
            _state: PhantomData,
        }
    }
}

impl<In, Out, S1, S2> FallbackLayer<In, Out, S1, S2> {
    /// Sets the predicate that decides whether the fallback should be invoked.
    ///
    /// The `predicate` receives a reference to the output produced by the inner
    /// service and returns `true` when the output is not considered valid and the
    /// fallback action should produce a replacement. This call replaces any
    /// previous predicate.
    #[must_use]
    pub fn should_fallback(mut self, predicate: impl Fn(&Out) -> bool + Send + Sync + 'static) -> FallbackLayer<In, Out, Set, S2> {
        self.should_fallback = Some(ShouldFallback::new(predicate));
        self.into_state::<Set, S2>()
    }

    /// Configures a callback invoked before the fallback action runs.
    ///
    /// This callback receives a mutable reference to the original (invalid)
    /// output that triggered the fallback and [`BeforeFallbackArgs`] with
    /// additional context. It is useful for logging, capturing, or modifying
    /// the original value before it is consumed by the fallback action.
    ///
    /// This call replaces any previous `before_fallback` callback.
    ///
    /// **Default**: None
    #[must_use]
    pub fn before_fallback(mut self, callback: impl Fn(&mut Out, BeforeFallbackArgs) + Send + Sync + 'static) -> Self {
        self.before_fallback = Some(BeforeFallback::new(callback));
        self
    }

    /// Configures a callback invoked after the fallback action completes.
    ///
    /// This callback receives a mutable reference to the replacement output
    /// produced by the fallback action and [`AfterFallbackArgs`] with additional
    /// context. Because the reference is mutable, the callback can modify the
    /// replacement output before it is returned to the caller.
    ///
    /// This call replaces any previous `after_fallback` callback.
    ///
    /// **Default**: None
    #[must_use]
    pub fn after_fallback(mut self, callback: impl Fn(&mut Out, AfterFallbackArgs) + Send + Sync + 'static) -> Self {
        self.after_fallback = Some(AfterFallback::new(callback));
        self
    }

    /// Optionally enables the fallback middleware based on a condition.
    ///
    /// When disabled, the inner service output is returned as-is regardless of
    /// the [`should_fallback`][FallbackLayer::should_fallback] predicate. The
    /// `is_enabled` function receives a reference to the input and returns
    /// `true` when fallback protection should be active for this request.
    ///
    /// **Default**: Always enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::new(is_enabled);
        self
    }

    /// Enables the fallback middleware unconditionally.
    ///
    /// All requests will have fallback protection applied.
    /// This call replaces any previous condition.
    ///
    /// **Note**: This is the default behavior.
    #[must_use]
    pub fn enable_always(mut self) -> Self {
        self.enable_if = EnableIf::always();
        self
    }

    /// Disables the fallback middleware completely.
    ///
    /// All requests will pass through without fallback protection.
    /// This call replaces any previous condition.
    #[must_use]
    pub fn disable(mut self) -> Self {
        self.enable_if = EnableIf::never();
        self
    }
}

impl<In, Out: Send + 'static, S1, S2> FallbackLayer<In, Out, S1, S2> {
    /// Sets a synchronous fallback action.
    ///
    /// The `action` receives the original (invalid) output and returns a
    /// replacement output. This call replaces any previous fallback action.
    #[must_use]
    pub fn fallback(mut self, action: impl Fn(Out) -> Out + Send + Sync + 'static) -> FallbackLayer<In, Out, S1, Set> {
        self.fallback_action = Some(FallbackAction::new_sync(action));
        self.into_state::<S1, Set>()
    }

    /// Sets a fixed fallback value that is cloned on every invocation.
    ///
    /// This is a convenience shorthand for [`fallback`][FallbackLayer::fallback]
    /// when the replacement output is always the same value. The original
    /// (invalid) output is discarded and `value` is cloned in its place.
    ///
    /// This call replaces any previous fallback action.
    #[must_use]
    pub fn fallback_output(self, value: Out) -> FallbackLayer<In, Out, S1, Set>
    where
        Out: Clone + Sync,
    {
        self.fallback(move |_| value.clone())
    }

    /// Sets an asynchronous fallback action.
    ///
    /// The `action` receives the original (invalid) output and returns a future
    /// that resolves to the replacement output. This call replaces any previous
    /// fallback action.
    #[must_use]
    pub fn fallback_async<F, Fut>(mut self, action: F) -> FallbackLayer<In, Out, S1, Set>
    where
        F: Fn(Out) -> Fut + Send + Sync + 'static,
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
            before_fallback: self.before_fallback.clone(),
            after_fallback: self.after_fallback.clone(),
            should_fallback: self.should_fallback.clone().expect("should_fallback must be set in Ready state"),
            fallback_action: self.fallback_action.clone().expect("fallback_action must be set in Ready state"),
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
            before_fallback: self.before_fallback,
            after_fallback: self.after_fallback,
            telemetry: self.telemetry,
            _state: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::sync::atomic::{AtomicBool, Ordering};

    use layered::Execute;
    use tick::Clock;

    use super::*;
    use crate::ResilienceContext;

    #[test]
    fn new_needs_should_fallback_and_action() {
        let context = create_test_context();
        let layer: FallbackLayer<_, _, NotSet, NotSet> = FallbackLayer::new("test_fallback".into(), &context);

        assert!(layer.should_fallback.is_none());
        assert!(layer.fallback_action.is_none());
        assert!(layer.before_fallback.is_none());
        assert!(layer.after_fallback.is_none());
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
            FallbackLayer::new("test".into(), &context).fallback(|_output: String| "replaced".to_string());

        assert!(layer.fallback_action.is_some());
    }

    #[tokio::test]
    async fn fallback_async_ensure_set_correctly() {
        let context = create_test_context();
        let layer: FallbackLayer<_, _, NotSet, Set> =
            FallbackLayer::new("test".into(), &context).fallback_async(|_output: String| async { "replaced".to_string() });

        let result = layer.fallback_action.unwrap().call("bad".to_string()).await;
        assert_eq!(result, "replaced");
    }

    #[test]
    fn before_fallback_ok() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = Arc::clone(&called);

        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().before_fallback(move |_output, _args| {
            called_clone.store(true, Ordering::SeqCst);
        });

        layer
            .before_fallback
            .unwrap()
            .call(&mut "output".to_string(), BeforeFallbackArgs {});

        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn after_fallback_ok() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = Arc::clone(&called);

        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().after_fallback(move |_output, _args| {
            called_clone.store(true, Ordering::SeqCst);
        });

        layer
            .after_fallback
            .unwrap()
            .call(&mut "output".to_string(), AfterFallbackArgs {});

        assert!(called.load(Ordering::SeqCst));
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
        let layer: FallbackLayer<_, _, NotSet, Set> =
            FallbackLayer::new("test".into(), &context).fallback_output("fixed".to_string());

        assert!(layer.fallback_action.is_some());
    }

    #[test]
    fn fallback_when_ready_ok() {
        let layer: FallbackLayer<_, _, Set, Set> = create_ready_layer().fallback(|_| "new_fallback".to_string());

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
            .fallback(|_output: String| "fallback_value".to_string())
    }
}
