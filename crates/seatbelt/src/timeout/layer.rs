// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::marker::PhantomData;
use std::time::Duration;

use crate::timeout::{
    OnTimeout, OnTimeoutArgs, Timeout, TimeoutOutput as TimeoutOutputCallback, TimeoutOutputArgs, TimeoutOverride, TimeoutOverrideArgs,
};
use crate::utils::EnableIf;
use crate::utils::TelemetryHelper;
use crate::{NotSet, ResilienceContext, Set};
use layered::Layer;

/// Builder for configuring timeout resilience middleware.
///
/// This type is created by calling [`Timeout::layer`](crate::timeout::Timeout::layer) and uses the
/// type-state pattern to enforce that required properties are configured before the timeout middleware can be built:
///
/// - [`timeout_output`][TimeoutLayer::timeout_output]: Required to specify how to represent output values when a timeout occurs
/// - [`timeout`][TimeoutLayer::timeout]: Required to set the timeout duration for operations
///
/// For comprehensive examples, see the [timeout module][crate::timeout] documentation.
///
/// # Type State
///
/// - `S1`: Tracks whether [`timeout`][TimeoutLayer::timeout] has been set
/// - `S2`: Tracks whether [`timeout_output`][TimeoutLayer::timeout_output] has been set
#[derive(Debug)]
pub struct TimeoutLayer<In, Out, S1 = Set, S2 = Set> {
    context: ResilienceContext<In, Out>,
    timeout: Option<Duration>,
    timeout_output: Option<TimeoutOutputCallback<Out>>,
    on_timeout: Option<OnTimeout<Out>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    timeout_override: Option<TimeoutOverride<In>>,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> TimeoutLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: Cow<'static, str>, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            timeout: None,
            timeout_output: None,
            on_timeout: None,
            enable_if: EnableIf::always(),
            telemetry: context.create_telemetry(name),
            context: context.clone(),
            timeout_override: None,
            _state: PhantomData,
        }
    }
}

impl<In, Out, E, S1, S2> TimeoutLayer<In, Result<Out, E>, S1, S2> {
    /// Configures the error value to return when a timeout occurs for Result types.
    ///
    /// This is a convenience method for Result types that creates an error value
    /// when a timeout occurs instead of requiring you to specify the full Result.
    /// The error function receives [`TimeoutOutputArgs`] containing timeout context.
    ///
    /// # Arguments
    ///
    /// * `timeout_error` - Function that takes [`TimeoutOutputArgs`] and returns
    ///   the error value to use when a timeout occurs
    pub fn timeout_error(
        self,
        timeout_error: impl Fn(TimeoutOutputArgs) -> E + Send + Sync + 'static,
    ) -> TimeoutLayer<In, Result<Out, E>, S1, Set> {
        self.into_state::<Set, S2>()
            .timeout_output(move |args| Err(timeout_error(args)))
            .into_state()
    }
}

impl<In, Out, S1, S2> TimeoutLayer<In, Out, S1, S2> {
    /// Sets the timeout duration.
    ///
    /// This specifies how long to wait before timing out an operation.
    /// This call replaces any previous timeout value.
    ///
    /// # Arguments
    ///
    /// * `timeout` - The maximum duration to wait for the operation to complete
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> TimeoutLayer<In, Out, Set, S2> {
        self.timeout = Some(timeout);
        self.into_state::<Set, S2>()
    }

    /// Sets the timeout result factory function.
    ///
    /// This function is called when a timeout occurs to create the output value
    /// that will be returned instead of the original operation's result.
    /// This call replaces any previous timeout output handler.
    ///
    /// # Arguments
    ///
    /// * `output` - Function that takes [`TimeoutOutputArgs`] containing timeout
    ///   context and returns the output value to use when a timeout occurs
    #[must_use]
    pub fn timeout_output(mut self, output: impl Fn(TimeoutOutputArgs) -> Out + Send + Sync + 'static) -> TimeoutLayer<In, Out, S1, Set> {
        self.timeout_output = Some(TimeoutOutputCallback::new(output));
        self.into_state::<S1, Set>()
    }

    /// Configures a callback invoked when a timeout occurs.
    ///
    /// This callback is useful for logging, metrics, or other observability
    /// purposes. It receives the timeout output and [`OnTimeoutArgs`] with
    /// detailed timeout information.
    ///
    /// The callback does not affect timeout behavior - it's purely for observation.
    /// This call replaces any previous callback.
    ///
    /// **Default**: None (no observability by default)
    ///
    /// # Arguments
    ///
    /// * `on_timeout` - Function that takes a reference to the timeout output and
    ///   [`OnTimeoutArgs`] containing timeout context information
    #[must_use]
    pub fn on_timeout(mut self, on_timeout: impl Fn(&Out, OnTimeoutArgs) + Send + Sync + 'static) -> Self {
        self.on_timeout = Some(OnTimeout::new(on_timeout));
        self
    }

    /// Overrides the default timeout on a per-request basis.
    ///
    /// Use this to compute a timeout dynamically from the input. Return `Some(Duration)`
    /// to apply an override, or `None` to fall back to the default timeout configured via
    /// [`timeout`][TimeoutLayer::timeout]. The function receives [`TimeoutOverrideArgs`],
    /// which exposes the default via [`TimeoutOverrideArgs::default_timeout`].
    ///
    /// This call replaces any previous timeout override.
    ///
    /// **Default**: None (uses default timeout for all requests)
    ///
    /// # Arguments
    ///
    /// * `timeout_override` - Function that takes a reference to the input and
    ///   [`TimeoutOverrideArgs`] containing the default timeout, and returns
    ///   an optional override duration
    #[must_use]
    pub fn timeout_override(
        mut self,
        timeout_override: impl Fn(&In, TimeoutOverrideArgs) -> Option<Duration> + Send + Sync + 'static,
    ) -> Self {
        self.timeout_override = Some(TimeoutOverride::new(timeout_override));
        self
    }

    /// Optionally enables the timeout middleware based on a condition.
    ///
    /// When disabled, requests pass through without timeout protection.
    /// This call replaces any previous condition.
    ///
    /// **Default**: Always enabled
    ///
    /// # Arguments
    ///
    /// * `is_enabled` - Function that takes a reference to the input and returns
    ///   `true` if timeout protection should be enabled for this request
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::new(is_enabled);
        self
    }

    /// Enables the timeout middleware unconditionally.
    ///
    /// All requests will have timeout protection applied.
    /// This call replaces any previous condition.
    ///
    /// **Note**: This is the default behavior - timeout is enabled by default.
    #[must_use]
    pub fn enable_always(mut self) -> Self {
        self.enable_if = EnableIf::always();
        self
    }

    /// Disables the timeout middleware completely.
    ///
    /// All requests will pass through without timeout protection.
    /// This call replaces any previous condition.
    ///
    /// **Note**: This overrides the default enabled behavior.
    #[must_use]
    pub fn disable(mut self) -> Self {
        self.enable_if = EnableIf::never();
        self
    }
}

impl<In, Out, S> Layer<S> for TimeoutLayer<In, Out, Set, Set> {
    type Service = Timeout<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        Timeout {
            inner,
            clock: self.context.get_clock().clone(),
            timeout: self.timeout.expect("timeout must be set in Ready state"),
            enable_if: self.enable_if.clone(),
            on_timeout: self.on_timeout.clone(),
            timeout_output: self.timeout_output.clone().expect("timeout_result must be set in Ready state"),
            timeout_override: self.timeout_override.clone(),
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry: self.telemetry.clone(),
        }
    }
}

impl<In, Out, S1, S2> TimeoutLayer<In, Out, S1, S2> {
    fn into_state<T1, T2>(self) -> TimeoutLayer<In, Out, T1, T2> {
        TimeoutLayer {
            timeout: self.timeout,
            enable_if: self.enable_if,
            timeout_output: self.timeout_output,
            on_timeout: self.on_timeout,
            telemetry: self.telemetry,
            context: self.context,
            timeout_override: self.timeout_override,
            _state: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use layered::Execute;
    use tick::Clock;

    use super::*;

    #[test]
    fn new_needs_timeout_output() {
        let context = create_test_context();
        let layer: TimeoutLayer<_, _, NotSet, NotSet> = TimeoutLayer::new("test_timeout".into(), &context);

        assert!(layer.timeout.is_none());
        assert!(layer.timeout_output.is_none());
        assert!(layer.on_timeout.is_none());
        assert!(layer.timeout_override.is_none());
        assert_eq!(layer.telemetry.strategy_name.as_ref(), "test_timeout");
        assert!(layer.enable_if.call(&"test_input".to_string()));
    }

    #[test]
    fn timeout_output_ensure_set_correctly() {
        let context = create_test_context();
        let layer = TimeoutLayer::new("test".into(), &context);

        let layer: TimeoutLayer<_, _, NotSet, Set> = layer.timeout_output(|args| format!("timeout: {}", args.timeout().as_millis()));
        let result = layer.timeout_output.unwrap().call(TimeoutOutputArgs {
            timeout: Duration::from_millis(3),
        });

        assert_eq!(result, "timeout: 3");
    }

    #[test]
    fn timeout_error_ensure_set_correctly() {
        let context = create_test_context_result();
        let layer = TimeoutLayer::new("test".into(), &context);

        let layer: TimeoutLayer<_, _, NotSet, Set> = layer.timeout_error(|args| format!("timeout: {}", args.timeout().as_millis()));
        let result = layer
            .timeout_output
            .unwrap()
            .call(TimeoutOutputArgs {
                timeout: Duration::from_millis(3),
            })
            .unwrap_err();

        assert_eq!(result, "timeout: 3");
    }

    #[test]
    fn timeout_ensure_set_correctly() {
        let layer: TimeoutLayer<_, _, Set, Set> = TimeoutLayer::new("test".into(), &create_test_context())
            .timeout_output(|_args| "timeout: ".to_string())
            .timeout(Duration::from_millis(3));

        assert_eq!(layer.timeout.unwrap(), Duration::from_millis(3));
    }

    #[test]
    fn on_timeout_ok() {
        let called = Arc::new(AtomicBool::new(false));
        let called_clone = Arc::clone(&called);

        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer().on_timeout(move |_output, _args| {
            called_clone.store(true, Ordering::SeqCst);
        });

        layer.on_timeout.unwrap().call(
            &"output".to_string(),
            OnTimeoutArgs {
                timeout: Duration::from_millis(3),
            },
        );

        assert!(called.load(Ordering::SeqCst));
    }

    #[test]
    fn timeout_override_ok() {
        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer().timeout_override(|_input, _args| Some(Duration::from_secs(3)));

        let result = layer.timeout_override.unwrap().call(
            &"a".to_string(),
            TimeoutOverrideArgs {
                default_timeout: Duration::from_millis(3),
            },
        );

        assert_eq!(result, Some(Duration::from_secs(3)));
    }

    #[test]
    fn enable_if_ok() {
        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer().enable_if(|input| matches!(input.as_ref(), "enable"));

        assert!(layer.enable_if.call(&"enable".to_string()));
        assert!(!layer.enable_if.call(&"disable".to_string()));
    }

    #[test]
    fn disable_ok() {
        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer().disable();

        assert!(!layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn enable_ok() {
        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer().disable().enable_always();

        assert!(layer.enable_if.call(&"whatever".to_string()));
    }

    #[test]
    fn timeout_when_ready_ok() {
        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer().timeout(Duration::from_secs(123));

        assert_eq!(layer.timeout.unwrap(), Duration::from_secs(123));
    }

    #[test]
    fn timeout_output_when_ready_ok() {
        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer().timeout_output(|_args| "some new value".to_string());
        assert!(layer.timeout_output.is_some());
        let result = layer.timeout_output.unwrap().call(TimeoutOutputArgs {
            timeout: Duration::from_secs(123),
        });

        assert_eq!(result, "some new value");
    }

    #[test]
    fn timeout_error_when_ready_ok() {
        let layer: TimeoutLayer<_, _, Set, Set> = create_ready_layer_with_result().timeout_error(|_args| "some error value".to_string());
        assert!(layer.timeout_output.is_some());
        let result = layer
            .timeout_output
            .unwrap()
            .call(TimeoutOutputArgs {
                timeout: Duration::from_secs(123),
            })
            .unwrap_err();

        assert_eq!(result, "some error value");
    }

    #[test]
    fn layer_ok() {
        let _layered = create_ready_layer().layer(Execute::new(|input: String| async move { input }));
    }

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(TimeoutLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(TimeoutLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(TimeoutLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(TimeoutLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> ResilienceContext<String, String> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_test_context_result() -> ResilienceContext<String, Result<String, String>> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer() -> TimeoutLayer<String, String, Set, Set> {
        TimeoutLayer::new("test".into(), &create_test_context())
            .timeout_output(|_args| "timeout: ".to_string())
            .timeout(Duration::from_millis(3))
    }

    fn create_ready_layer_with_result() -> TimeoutLayer<String, Result<String, String>, Set, Set> {
        TimeoutLayer::new("test".into(), &create_test_context_result())
            .timeout_error(|_args| "timeout: ".to_string())
            .timeout(Duration::from_millis(3))
    }
}
