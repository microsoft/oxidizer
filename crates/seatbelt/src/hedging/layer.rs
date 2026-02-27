// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::marker::PhantomData;

use layered::Layer;

use crate::hedging::args::*;
use crate::hedging::callbacks::*;
use crate::hedging::constants::DEFAULT_MAX_HEDGED_ATTEMPTS;
use crate::hedging::mode::HedgingMode;
use crate::hedging::service::{Hedging, HedgingShared};
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{NotSet, Recovery, RecoveryInfo, ResilienceContext, Set};

/// Builder for configuring hedging resilience middleware.
///
/// This type is created by calling [`Hedging::layer`](crate::hedging::Hedging::layer) and uses the
/// type-state pattern to enforce that required properties are configured before the hedging
/// middleware can be built:
///
/// - [`clone_input`][HedgingLayer::clone_input]: Required to specify how to clone inputs for hedge attempts
/// - [`recovery`][HedgingLayer::recovery]: Required to determine if an output is acceptable
///
/// For comprehensive examples, see the [hedging module][crate::hedging] documentation.
///
/// # Type State
///
/// - `S1`: Tracks whether [`clone_input`][HedgingLayer::clone_input] has been set
/// - `S2`: Tracks whether [`recovery`][HedgingLayer::recovery] has been set
#[derive(Debug)]
pub struct HedgingLayer<In, Out, S1 = Set, S2 = Set> {
    context: ResilienceContext<In, Out>,
    max_hedged_attempts: u32,
    hedging_mode: HedgingMode,
    clone_input: Option<CloneInput<In>>,
    should_recover: Option<ShouldRecover<Out>>,
    on_hedge: Option<OnHedge>,
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
            hedging_mode: HedgingMode::default(),
            clone_input: None,
            should_recover: None,
            on_hedge: None,
            handle_unavailable: false,
            enable_if: EnableIf::always(),
            telemetry: context.create_telemetry(name),
            _state: PhantomData,
        }
    }
}

impl<In, Out, S1, S2> HedgingLayer<In, Out, S1, S2> {
    /// Sets the maximum number of additional hedged attempts.
    ///
    /// This specifies the maximum number of hedged requests in addition to the original call.
    /// For example, if `max_hedged_attempts` is 2, the operation will be attempted up to
    /// 3 times total (1 original `+` 2 hedges).
    ///
    /// **Default**: 1 hedged attempt (2 total)
    #[must_use]
    pub fn max_hedged_attempts(mut self, count: u32) -> Self {
        self.max_hedged_attempts = count;
        self
    }

    /// Sets the hedging mode that controls timing of hedged requests.
    ///
    /// - [`HedgingMode::immediate()`]: Launches all hedges at once
    /// - [`HedgingMode::delay(duration)`][HedgingMode::delay]: Fixed delay between each hedge
    /// - [`HedgingMode::dynamic(fn)`][HedgingMode::dynamic]: Per-attempt delay via callback
    ///
    /// **Default**: [`HedgingMode::delay(2s)`][HedgingMode::delay]
    #[must_use]
    pub fn hedging_mode(mut self, mode: HedgingMode) -> Self {
        self.hedging_mode = mode;
        self
    }

    /// Sets the input cloning function for hedged attempts.
    ///
    /// Called before each attempt to produce a fresh input value. The `clone_fn`
    /// receives a mutable reference to the input and [`CloneArgs`] containing
    /// context about the attempt. Return `Some(cloned_input)` to proceed, or `None`
    /// to skip that hedge attempt.
    #[must_use]
    pub fn clone_input_with(
        mut self,
        clone_fn: impl Fn(&mut In, CloneArgs) -> Option<In> + Send + Sync + 'static,
    ) -> HedgingLayer<In, Out, Set, S2> {
        self.clone_input = Some(CloneInput::new(clone_fn));
        self.into_state::<Set, S2>()
    }

    /// Automatically sets the input cloning function for types that implement [`Clone`].
    ///
    /// This is equivalent to calling [`clone_input_with`][HedgingLayer::clone_input_with] with
    /// `|input, _args| Some(input.clone())`.
    ///
    /// # Type Requirements
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
    /// This function determines whether a specific output is acceptable by examining
    /// the output and returning a [`RecoveryInfo`] classification:
    ///
    /// - [`RecoveryInfo::never()`]: The result is acceptable - return it immediately
    /// - [`RecoveryInfo::retry()`]: The result is transient - continue waiting for hedges
    /// - [`RecoveryInfo::unavailable()`]: The service is unavailable - by default returned
    ///   immediately, but treated as transient when [`handle_unavailable(true)`][HedgingLayer::handle_unavailable]
    ///   is configured
    #[must_use]
    pub fn recovery_with(
        mut self,
        recover_fn: impl Fn(&Out, RecoveryArgs) -> RecoveryInfo + Send + Sync + 'static,
    ) -> HedgingLayer<In, Out, S1, Set> {
        self.should_recover = Some(ShouldRecover::new(recover_fn));
        self.into_state::<S1, Set>()
    }

    /// Automatically sets the recovery classification function for types that implement [`Recovery`].
    ///
    /// This is equivalent to calling [`recovery_with`][HedgingLayer::recovery_with] with
    /// `|output, _args| output.recovery()`.
    ///
    /// # Type Requirements
    ///
    /// The output type `Out` must implement [`Recovery`].
    #[must_use]
    pub fn recovery(self) -> HedgingLayer<In, Out, S1, Set>
    where
        Out: Recovery,
    {
        self.recovery_with(|out, _args| out.recovery())
    }

    /// Configures a callback invoked when a new hedged request is about to be launched.
    ///
    /// This callback is useful for logging, metrics, or other observability purposes.
    /// It does not affect hedging behavior - it is purely for observation.
    ///
    /// **Default**: None
    #[must_use]
    pub fn on_hedge(mut self, hedge_fn: impl Fn(OnHedgeArgs) + Send + Sync + 'static) -> Self {
        self.on_hedge = Some(OnHedge::new(hedge_fn));
        self
    }

    /// Configures whether the hedging middleware should treat unavailable services as
    /// recoverable conditions.
    ///
    /// When enabled, [`RecoveryInfo::unavailable()`] classifications are treated as
    /// recoverable - the hedge will continue waiting for other in-flight requests.
    /// When disabled (default), unavailable responses are treated as acceptable results
    /// and returned immediately.
    ///
    /// **Default**: false (unavailable responses are returned immediately)
    #[must_use]
    pub fn handle_unavailable(mut self, enable: bool) -> Self {
        self.handle_unavailable = enable;
        self
    }

    /// Optionally enables the hedging middleware based on a condition.
    ///
    /// When disabled, requests pass through without hedging.
    ///
    /// **Default**: Always enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::new(is_enabled);
        self
    }

    /// Enables the hedging middleware unconditionally.
    ///
    /// **Note**: This is the default behavior.
    #[must_use]
    pub fn enable_always(mut self) -> Self {
        self.enable_if = EnableIf::always();
        self
    }

    /// Disables the hedging middleware completely.
    ///
    /// All requests will pass through without hedging.
    #[must_use]
    pub fn disable(mut self) -> Self {
        self.enable_if = EnableIf::never();
        self
    }

    fn into_state<T1, T2>(self) -> HedgingLayer<In, Out, T1, T2> {
        HedgingLayer {
            context: self.context,
            max_hedged_attempts: self.max_hedged_attempts,
            hedging_mode: self.hedging_mode,
            clone_input: self.clone_input,
            should_recover: self.should_recover,
            on_hedge: self.on_hedge,
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
            hedging_mode: self.hedging_mode.clone(),
            clone_input: self.clone_input.clone().expect("clone_input must be set in Ready state"),
            should_recover: self.should_recover.clone().expect("should_recover must be set in Ready state"),
            on_hedge: self.on_hedge.clone(),
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
        assert!(!layer.hedging_mode.is_immediate());
        assert!(!layer.handle_unavailable);
        assert!(layer.clone_input.is_none());
        assert!(layer.should_recover.is_none());
        assert!(layer.on_hedge.is_none());
        assert_eq!(layer.telemetry.strategy_name.as_ref(), "test_hedging");
        assert!(layer.enable_if.call(&"test_input".to_string()));
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
            },
        );
        assert_eq!(result, RecoveryInfo::retry());
    }

    #[test]
    fn configuration_methods_work() {
        let layer = create_ready_layer().max_hedged_attempts(3).hedging_mode(HedgingMode::immediate());

        assert_eq!(layer.max_hedged_attempts, 3);
        assert!(layer.hedging_mode.is_immediate());
    }

    #[test]
    fn on_hedge_works() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let called = Arc::new(AtomicU32::new(0));
        let called_clone = Arc::clone(&called);

        let layer = create_ready_layer().on_hedge(move |_args| {
            called_clone.fetch_add(1, Ordering::SeqCst);
        });

        layer.on_hedge.unwrap().call(OnHedgeArgs {
            attempt: Attempt::new(1, false),
        });
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
