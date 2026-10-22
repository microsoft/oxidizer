// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::marker::PhantomData;
use std::time::Duration;

use layered::Layer;

use crate::hedging::args::*;
use crate::hedging::callbacks::*;
use crate::hedging::config::HedgingConfig;
use crate::hedging::constants::{DEFAULT_HEDGING_DELAY, DEFAULT_MAX_HEDGED_ATTEMPTS};
use crate::hedging::service::{Hedging, HedgingShared};
use crate::typestates::{NotSet, Set};
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{Recovery, RecoveryInfo, ResilienceContext};

/// Configures hedging middleware.
///
/// [`Hedging::layer`](crate::hedging::Hedging::layer) creates this builder.
/// Its type state makes these properties required:
///
/// - [`clone_input`][HedgingLayer::clone_input] provides input for hedged attempts.
/// - [`recovery`][HedgingLayer::recovery] classifies output.
///
/// See the [hedging module][crate::hedging] for examples.
///
/// # Type state
///
/// - `S1` records whether [`clone_input`][HedgingLayer::clone_input] is set.
/// - `S2` records whether [`recovery`][HedgingLayer::recovery] is set.
#[derive(Debug)]
pub struct HedgingLayer<In, Out, S1 = Set, S2 = Set> {
    context: ResilienceContext<In, Out>,
    max_hedged_attempts: u8,
    delay_fn: DelayFn<In>,
    clone_input: Option<CloneInput<In>>,
    should_recover: Option<ShouldRecover<Out>>,
    on_execute: Option<OnExecute<In>>,
    handle_unavailable: bool,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> HedgingLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: Cow<'static, str>, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            context: context.clone(),
            max_hedged_attempts: DEFAULT_MAX_HEDGED_ATTEMPTS,
            delay_fn: DelayFn::new(|_input, _args| DEFAULT_HEDGING_DELAY),
            clone_input: None,
            should_recover: None,
            on_execute: None,
            handle_unavailable: false,
            enable_if: EnableIf::default(),
            telemetry: context.create_telemetry(name),
            _state: PhantomData,
        }
    }
}

impl<In, Out, S1, S2> HedgingLayer<In, Out, S1, S2> {
    /// Sets the maximum number of additional hedged attempts.
    ///
    /// This value does not include the original call.
    /// For example, a value of 2 permits three total attempts.
    ///
    /// Default: 1 hedged attempt (2 total)
    #[must_use]
    pub fn max_hedged_attempts(mut self, count: u8) -> Self {
        self.max_hedged_attempts = count;
        self
    }

    /// Sets a fixed delay between hedging attempts.
    ///
    /// The original request starts immediately.
    /// Each later attempt starts after another `delay`.
    ///
    /// The delay controls when attempts start:
    ///
    /// - [`Duration::ZERO`]: All attempts start together.
    ///   Use this only when the downstream service has enough capacity.
    ///
    /// - A delay from 200 milliseconds to 2 seconds gives the original request time to finish.
    ///
    /// - [`Duration::MAX`]: A new attempt starts only after a recoverable result.
    ///   This permits only one in-flight request.
    ///
    /// Use [`hedging_delay_with`][HedgingLayer::hedging_delay_with] for calculated delays.
    ///
    /// Default: 500 milliseconds
    #[must_use]
    pub fn hedging_delay(mut self, delay: Duration) -> Self {
        self.delay_fn = DelayFn::new(move |_input, _args| delay);
        self
    }

    /// Sets a function that calculates the delay for each hedged attempt.
    ///
    /// The function receives the input and [`HedgingDelayArgs`].
    /// It returns the delay before the attempt.
    ///
    /// Default: 500 milliseconds
    #[must_use]
    pub fn hedging_delay_with(mut self, delay_fn: impl Fn(&In, HedgingDelayArgs) -> Duration + Send + Sync + 'static) -> Self {
        self.delay_fn = DelayFn::new(delay_fn);
        self
    }

    /// Applies all settings from a [`HedgingConfig`] to this layer.
    ///
    /// Use this method to apply configuration from an external source.
    #[must_use]
    pub fn config(self, config: &HedgingConfig) -> Self {
        self.hedging_delay(config.hedging_delay)
            .max_hedged_attempts(config.max_hedged_attempts)
            .handle_unavailable(config.handle_unavailable)
            .enable(config.enabled)
    }

    fn enable(mut self, enabled: bool) -> Self {
        self.enable_if = EnableIf::new(enabled);
        self
    }

    /// Sets the input cloning function for hedged attempts.
    ///
    /// The middleware calls this function before each attempt.
    /// It receives the input and [`CloneArgs`].
    /// Return `Some(cloned_input)` to start the attempt.
    /// Return `None` to skip it.
    ///
    /// You can change the input between attempts.
    /// The final attempt consumes the original input with these changes.
    #[must_use]
    pub fn clone_input_with(
        mut self,
        clone_fn: impl Fn(&mut In, CloneArgs) -> Option<In> + Send + Sync + 'static,
    ) -> HedgingLayer<In, Out, Set, S2> {
        self.clone_input = Some(CloneInput::new(clone_fn));
        self.into_state::<Set, S2>()
    }

    /// Uses [`Clone`] to create input for hedged attempts.
    ///
    /// This is equivalent to calling [`clone_input_with`][HedgingLayer::clone_input_with] with
    /// `|input, _args| Some(input.clone())`.
    ///
    /// # Type requirements
    ///
    /// The input type `In` must implement [`Clone`].
    #[must_use]
    pub fn clone_input(self) -> HedgingLayer<In, Out, Set, S2>
    where
        In: Clone,
    {
        self.clone_input_with(|input, _args| Some(input.clone()))
    }

    /// Sets the recovery classification function.
    ///
    /// The function returns a [`RecoveryInfo`] classification:
    ///
    /// - [`RecoveryInfo::never()`]: Return the output.
    /// - [`RecoveryInfo::retry()`]: Wait for another attempt.
    /// - [`RecoveryInfo::unavailable()`]: Return the output by default.
    ///   [`handle_unavailable(true)`][HedgingLayer::handle_unavailable] makes it recoverable.
    #[must_use]
    pub fn recovery_with(
        mut self,
        recover_fn: impl Fn(&Out, RecoveryArgs) -> RecoveryInfo + Send + Sync + 'static,
    ) -> HedgingLayer<In, Out, S1, Set> {
        self.should_recover = Some(ShouldRecover::new(recover_fn));
        self.into_state::<S1, Set>()
    }

    /// Uses [`Recovery`] to classify output.
    ///
    /// This is equivalent to calling [`recovery_with`][HedgingLayer::recovery_with] with
    /// `|output, _args| output.recovery()`.
    ///
    /// # Type requirements
    ///
    /// The output type `Out` must implement [`Recovery`].
    #[must_use]
    pub fn recovery(self) -> HedgingLayer<In, Out, S1, Set>
    where
        Out: Recovery,
    {
        self.recovery_with(|out, _args| out.recovery())
    }

    /// Sets a callback that runs before each attempt.
    ///
    /// The callback receives the input and [`OnExecuteArgs`].
    /// Use it for logs, metrics, or input changes.
    ///
    /// Default: `None`
    #[must_use]
    pub fn on_execute(mut self, execute_fn: impl Fn(&mut In, OnExecuteArgs) + Send + Sync + 'static) -> Self {
        self.on_execute = Some(OnExecute::new(execute_fn));
        self
    }

    /// Controls whether unavailable output is recoverable.
    ///
    /// If enabled, the middleware waits for other attempts.
    /// If disabled, it returns unavailable output.
    ///
    /// Default: `false`
    #[must_use]
    pub fn handle_unavailable(mut self, enable: bool) -> Self {
        self.handle_unavailable = enable;
        self
    }

    /// Enables hedging middleware when a condition is true.
    ///
    /// If the condition is false, the request runs without hedging.
    ///
    /// Default: Enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::custom(is_enabled);
        self
    }

    /// Enables hedging middleware for all requests.
    ///
    /// Hedging middleware is enabled by default.
    #[must_use]
    pub fn enable_always(mut self) -> Self {
        self.enable_if = EnableIf::new(true);
        self
    }

    /// Disables hedging middleware for all requests.
    ///
    /// Requests run without hedging.
    #[must_use]
    pub fn disable(mut self) -> Self {
        self.enable_if = EnableIf::new(false);
        self
    }

    fn into_state<T1, T2>(self) -> HedgingLayer<In, Out, T1, T2> {
        HedgingLayer {
            context: self.context,
            max_hedged_attempts: self.max_hedged_attempts,
            delay_fn: self.delay_fn,
            clone_input: self.clone_input,
            should_recover: self.should_recover,
            on_execute: self.on_execute,
            handle_unavailable: self.handle_unavailable,
            enable_if: self.enable_if,
            telemetry: self.telemetry,
            _state: PhantomData,
        }
    }
}

impl<In, Out, S> Layer<S> for HedgingLayer<In, Out, Set, Set> {
    type Service = Hedging<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        let shared = HedgingShared {
            clock: self.context.get_clock().clone(),
            max_hedged_attempts: self.max_hedged_attempts,
            delay_fn: self.delay_fn.clone(),
            clone_input: self.clone_input.clone().expect("clone_input must be set in Ready state"),
            should_recover: self.should_recover.clone().expect("should_recover must be set in Ready state"),
            on_execute: self.on_execute.clone(),
            handle_unavailable: self.handle_unavailable,
            enable_if: self.enable_if.clone(),
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry: self.telemetry.clone(),
        };

        Hedging {
            shared: std::sync::Arc::new(shared),
            inner,
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
    use crate::Attempt;
    use crate::testing::RecoverableType;

    #[test]
    fn new_creates_correct_initial_state() {
        let context = create_test_context();
        let layer: HedgingLayer<_, _, NotSet, NotSet> = HedgingLayer::new("test_hedging".into(), &context);

        assert_eq!(layer.max_hedged_attempts, 1);
        assert!(!layer.handle_unavailable);
        assert!(layer.clone_input.is_none());
        assert!(layer.should_recover.is_none());
        assert!(layer.on_execute.is_none());
        assert_eq!(layer.telemetry.strategy_name.as_ref(), "test_hedging");
        assert!(layer.enable_if.call(&"test_input".to_string()));
        // The default delay is 500 milliseconds.
        let args = HedgingDelayArgs {
            attempt: Attempt::new(1, false),
        };
        assert_eq!(layer.delay_fn.call(&"any".to_string(), args), Duration::from_millis(500));
    }

    #[test]
    fn clone_input_sets_correctly() {
        let context = create_test_context();
        let layer = HedgingLayer::new("test".into(), &context);

        let layer: HedgingLayer<_, _, Set, NotSet> = layer.clone_input_with(|input, _args| Some(input.clone()));

        let result = layer.clone_input.unwrap().call(
            &mut "test".to_string(),
            CloneArgs {
                attempt: Attempt::new(0, false),
            },
        );
        assert_eq!(result, Some("test".to_string()));
    }

    #[test]
    fn recovery_sets_correctly() {
        let context = create_test_context();
        let layer = HedgingLayer::new("test".into(), &context);

        let layer: HedgingLayer<_, _, NotSet, Set> = layer.recovery_with(|output, _args| {
            if output.contains("error") {
                RecoveryInfo::retry()
            } else {
                RecoveryInfo::never()
            }
        });

        let result = layer.should_recover.as_ref().unwrap().call(
            &"error message".to_string(),
            RecoveryArgs {
                clock: context.get_clock(),
                attempt: Attempt::default(),
            },
        );
        assert_eq!(result, RecoveryInfo::retry());
    }

    #[test]
    fn recovery_auto_sets_correctly() {
        let context = ResilienceContext::<RecoverableType, RecoverableType>::new(Clock::new_frozen());
        let layer = HedgingLayer::new("test".into(), &context);

        let layer: HedgingLayer<_, _, NotSet, Set> = layer.recovery();

        let result = layer.should_recover.as_ref().unwrap().call(
            &RecoverableType::from(RecoveryInfo::retry()),
            RecoveryArgs {
                clock: context.get_clock(),
                attempt: Attempt::default(),
            },
        );
        assert_eq!(result, RecoveryInfo::retry());
    }

    #[test]
    fn configuration_methods_work() {
        let layer = create_ready_layer().max_hedged_attempts(3).hedging_delay(Duration::ZERO);

        assert_eq!(layer.max_hedged_attempts, 3);
        let args = HedgingDelayArgs {
            attempt: Attempt::new(1, false),
        };
        assert_eq!(layer.delay_fn.call(&"any".to_string(), args), Duration::ZERO);
    }

    #[test]
    fn hedging_delay_sets_fixed_delay() {
        let layer = create_ready_layer().hedging_delay(Duration::from_millis(500));

        let args = HedgingDelayArgs {
            attempt: Attempt::new(1, false),
        };
        assert_eq!(layer.delay_fn.call(&"any".to_string(), args), Duration::from_millis(500));
    }

    #[test]
    fn on_execute_works() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let called = Arc::new(AtomicU32::new(0));
        let called_clone = Arc::clone(&called);

        let layer = create_ready_layer().on_execute(move |_input, _args| {
            called_clone.fetch_add(1, Ordering::SeqCst);
        });

        layer.on_execute.unwrap().call(
            &mut "test".to_string(),
            OnExecuteArgs {
                attempt: Attempt::new(1, false),
                delay: Duration::from_secs(2),
            },
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);
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
    fn handle_unavailable_defaults_to_false() {
        let layer = create_ready_layer();
        assert!(!layer.handle_unavailable);

        let layer = layer.handle_unavailable(true);
        assert!(layer.handle_unavailable);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn config_applies_all_settings() {
        let config = HedgingConfig {
            enabled: false,
            hedging_delay: Duration::from_secs(2),
            max_hedged_attempts: 4,
            handle_unavailable: true,
        };

        let layer = create_ready_layer().config(&config);

        insta::assert_debug_snapshot!(layer);

        let delay = layer.delay_fn.call(
            &String::default(),
            HedgingDelayArgs {
                attempt: Attempt::default(),
            },
        );
        assert_eq!(delay, Duration::from_secs(2));
    }

    #[test]
    fn layer_builds_service_when_ready() {
        let layer = create_ready_layer();
        let _service = layer.layer(Execute::new(|input: String| async move { input }));
    }

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(HedgingLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(HedgingLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(HedgingLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(HedgingLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> ResilienceContext<String, String> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer() -> HedgingLayer<String, String, Set, Set> {
        HedgingLayer::new("test".into(), &create_test_context())
            .clone_input_with(|input, _args| Some(input.clone()))
            .recovery_with(|output, _args| {
                if output.contains("error") {
                    RecoveryInfo::retry()
                } else {
                    RecoveryInfo::never()
                }
            })
    }
}
