// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use layered::Layer;

use crate::retry::backoff::BackoffOptions;
use crate::retry::constants::DEFAULT_RETRY_ATTEMPTS;
use crate::retry::*;
use crate::typestates::{NotSet, Set};
use crate::utils::{EnableIf, TelemetryHelper};
use crate::{Recovery, RecoveryInfo, ResilienceContext, TelemetryString};

/// Configures retry middleware.
///
/// [`Retry::layer`](crate::retry::Retry::layer) creates this builder.
/// Its type state makes these properties required:
///
/// - [`clone_input`][RetryLayer::clone_input] provides input for retry attempts.
/// - [`recovery`][RetryLayer::recovery] decides whether output permits a retry.
///
/// See the [retry module][crate::retry] for examples.
///
/// # Type state
///
/// - `S1` records whether [`clone_input`][RetryLayer::clone_input] is set.
/// - `S2` records whether [`recovery`][RetryLayer::recovery] is set.
#[derive(Debug)]
pub struct RetryLayer<In, Out, S1 = Set, S2 = Set> {
    context: ResilienceContext<In, Out>,
    max_attempts: u32,
    backoff: BackoffOptions,
    clone_input: Option<CloneInput<In>>,
    should_recover: Option<ShouldRecover<Out>>,
    on_retry: Option<OnRetry<Out>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    restore_input: Option<RestoreInput<In, Out>>,
    handle_unavailable: bool,
    _state: PhantomData<fn(In, S1, S2) -> Out>,
}

impl<In, Out> RetryLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: TelemetryString, context: &ResilienceContext<In, Out>) -> Self {
        Self {
            context: context.clone(),
            max_attempts: DEFAULT_RETRY_ATTEMPTS.saturating_add(1),
            backoff: BackoffOptions::default(),
            clone_input: None,
            should_recover: None,
            on_retry: None,
            enable_if: EnableIf::default(),
            telemetry: context.create_telemetry(name),
            restore_input: None,
            handle_unavailable: false,
            _state: PhantomData,
        }
    }
}

impl<In, Out, S1, S2> RetryLayer<In, Out, S1, S2> {
    /// Sets the maximum number of retry attempts.
    ///
    /// This value does not include the original call.
    /// For example, a value of 3 permits four total calls.
    ///
    /// Default: 3 retry attempts
    #[must_use]
    pub fn max_retry_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts.saturating_add(1);
        self
    }

    /// Sets the backoff strategy for retry delays.
    ///
    /// - [`Backoff::Constant`]: Uses the same delay before each retry.
    /// - [`Backoff::Linear`]: Increases the delay by `base_delay × attempt`.
    /// - [`Backoff::Exponential`]: Increases the delay by `base_delay × 2^attempt`.
    ///
    /// Default: [`Backoff::Exponential`]
    #[must_use]
    pub fn backoff(mut self, backoff_type: Backoff) -> Self {
        self.backoff.backoff_type = backoff_type;
        self
    }

    /// Sets the base delay used for backoff calculations.
    ///
    /// The strategy uses this value as follows:
    /// - Constant: The delay before each retry.
    /// - Linear: The first delay. Later delays use `base_delay × attempt_number`.
    /// - Exponential: The first delay. Later delays grow exponentially.
    ///
    /// Default: 10 milliseconds for service-to-service calls
    ///
    /// For client-to-service calls, use 1 to 2 seconds.
    #[must_use]
    pub fn base_delay(mut self, delay: Duration) -> Self {
        self.backoff.base_delay = delay;
        self
    }

    /// Sets the maximum delay between retries.
    ///
    /// The middleware limits each calculated delay to this value.
    /// If you do not set it, the backoff calculation does not have a limit.
    ///
    /// Default: `None`
    #[must_use]
    pub fn max_delay(mut self, max_delay: Duration) -> Self {
        self.backoff.max_delay = Some(max_delay);
        self
    }

    /// Enables or disables jitter.
    ///
    /// Jitter adds random variation to retry delays.
    /// It prevents many clients from retrying at the same time.
    ///
    /// Default: `true`
    #[must_use]
    pub fn use_jitter(mut self, use_jitter: bool) -> Self {
        self.backoff.use_jitter = use_jitter;
        self
    }

    /// Applies all settings from a [`RetryConfig`] to this layer.
    ///
    /// Use this method to apply configuration from an external source.
    /// You do not need to call each builder method.
    #[must_use]
    pub fn config(self, config: &RetryConfig) -> Self {
        self.backoff(config.backoff_type)
            .base_delay(config.base_delay)
            .use_jitter(config.use_jitter)
            .max_retry_attempts(config.max_retry_attempts)
            .max_delay_optional(config.max_delay)
            .handle_unavailable(config.handle_unavailable)
            .enable(config.enabled)
    }
    fn max_delay_optional(mut self, max_delay: Option<Duration>) -> Self {
        self.backoff.max_delay = max_delay;
        self
    }

    /// Sets the input cloning function.
    ///
    /// The middleware calls this function before each retry.
    /// The inner service consumes its input, so each retry needs new input.
    /// The function receives the input and [`CloneArgs`].
    /// Return `Some(cloned_input)` to retry.
    /// Return `None` to stop and return the last output.
    #[must_use]
    pub fn clone_input_with(
        mut self,
        clone_fn: impl Fn(&mut In, CloneArgs) -> Option<In> + Send + Sync + 'static,
    ) -> RetryLayer<In, Out, Set, S2> {
        self.clone_input = Some(CloneInput::new(clone_fn));
        self.into_state::<Set, S2>()
    }

    /// Uses [`Clone`] to create input for retries.
    ///
    /// This method is equivalent to [`clone_input_with`][RetryLayer::clone_input_with] with
    /// `|input, _args| Some(input.clone())`.
    ///
    /// # Type requirements
    ///
    /// The input type `In` must implement [`Clone`].
    #[must_use]
    pub fn clone_input(self) -> RetryLayer<In, Out, Set, S2>
    where
        In: Clone,
    {
        self.clone_input_with(|input, _args| Some(input.clone()))
    }

    /// Sets the recovery classification function.
    ///
    /// The function receives the output and [`RecoveryArgs`].
    /// It returns the [`RecoveryInfo`] classification for the output.
    #[must_use]
    pub fn recovery_with(
        mut self,
        recover_fn: impl Fn(&Out, RecoveryArgs) -> RecoveryInfo + Send + Sync + 'static,
    ) -> RetryLayer<In, Out, S1, Set> {
        self.should_recover = Some(ShouldRecover::new(recover_fn));
        self.into_state::<S1, Set>()
    }

    /// Uses [`Recovery`] to classify output.
    ///
    /// This method is equivalent to [`recovery`][RetryLayer::recovery] with
    /// `|output, _args| output.recovery()`.
    ///
    /// # Type requirements
    ///
    /// The output type `Out` must implement [`Recovery`].
    #[must_use]
    pub fn recovery(self) -> RetryLayer<In, Out, S1, Set>
    where
        Out: Recovery,
    {
        self.recovery_with(|out, _args| out.recovery())
    }

    /// Sets a callback that runs before each retry.
    ///
    /// Use this callback for logs, metrics, or other observations.
    /// It receives the output that caused the retry and [`OnRetryArgs`].
    ///
    /// The callback does not change retry behavior.
    ///
    /// Default: `None`
    #[must_use]
    pub fn on_retry(mut self, retry_fn: impl Fn(&Out, OnRetryArgs) + Send + Sync + 'static) -> Self {
        self.on_retry = Some(OnRetry::new(retry_fn));
        self
    }

    /// Enables retry middleware when a condition is true.
    ///
    /// If the condition is false, the request runs without retries.
    /// The `is_enabled` function receives the input.
    /// This call replaces the previous condition.
    ///
    /// Default: Enabled
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::custom(is_enabled);
        self
    }

    /// Enables or disables the retry middleware.
    ///
    /// If disabled, requests run without retries.
    /// This call replaces the previous condition.
    #[must_use]
    fn enable(mut self, enabled: bool) -> Self {
        self.enable_if = EnableIf::new(enabled);
        self
    }

    /// Enables retry middleware for all requests.
    ///
    /// This call replaces the previous condition.
    /// Retry middleware is enabled by default.
    #[must_use]
    pub fn enable_always(self) -> Self {
        self.enable(true)
    }

    /// Disables retry middleware for all requests.
    ///
    /// Requests run without retries.
    /// This call replaces the previous condition.
    #[must_use]
    pub fn disable(self) -> Self {
        self.enable(false)
    }

    /// Controls retries for unavailable services.
    ///
    /// If enabled, [`RecoveryInfo::unavailable`] permits a retry.
    /// If disabled, the middleware returns unavailable output without a retry.
    ///
    /// Use this option when retries can select another resource or endpoint.
    ///
    /// Set `enable` to `true` to retry unavailable output.
    /// Set it to `false` to return unavailable output.
    ///
    /// Default: `false`
    ///
    /// # Example
    ///
    /// ```rust
    /// # use seatbelt::Attempt;
    /// # use seatbelt::retry::Retry;
    /// # use seatbelt::{RecoveryInfo, ResilienceContext};
    /// # use tick::Clock;
    /// # fn example() {
    /// # let context = ResilienceContext::<String, Result<String, String>>::new(Clock::new_frozen());
    /// // Use multiple endpoints to route around an unavailable service.
    /// let layer = Retry::layer("multi_endpoint_retry", &context)
    ///     .clone_input_with(|input: &mut String, args| {
    ///         let mut input = input.clone();
    ///         update_endpoint(&mut input, args.attempt()); // Select another endpoint.
    ///         Some(input)
    ///     })
    ///     .recovery_with(|result, _args| match result {
    ///         Ok(_) => RecoveryInfo::never(),
    ///         Err(msg) if msg.contains("service unavailable") => RecoveryInfo::unavailable(),
    ///         Err(_) => RecoveryInfo::retry(),
    ///     })
    ///     .handle_unavailable(true); // Try another endpoint when one is unavailable.
    /// # }
    /// # fn update_endpoint(_input : &mut String, _attempt: Attempt)  {}
    /// ```
    #[must_use]
    pub fn handle_unavailable(mut self, enable: bool) -> Self {
        self.handle_unavailable = enable;
        self
    }

    /// Sets the input restoration function.
    ///
    /// The middleware calls this function when input cloning returns `None`.
    /// The function receives the output from the failed attempt and [`RestoreInputArgs`].
    /// It can extract or rebuild input for the next retry.
    ///
    /// Use this function when a failed operation returns the original input.
    /// For example, an HTTP client can reject a request before it sends the request.
    ///
    /// The restore function returns:
    /// - `Some(Input)` to retry with the restored input.
    /// - `None` to stop and return the output.
    ///
    /// This supports input that is expensive or impossible to clone.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::ops::ControlFlow;
    /// # use seatbelt::retry::{Retry, RestoreInputArgs};
    /// # use seatbelt::{RecoveryInfo, ResilienceContext};
    /// # use tick::Clock;
    /// # fn example() {
    /// # let clock = Clock::new_frozen();
    /// # let context = ResilienceContext::new(&clock);
    /// #[derive(Clone)]
    /// struct HttpRequest {
    ///     url: String,
    ///     body: Vec<u8>,
    /// }
    ///
    /// enum HttpResult {
    ///     Success(String),
    ///     ConnectionError { original_request: HttpRequest },
    ///     ServerError(u16),
    /// }
    ///
    /// let layer = Retry::layer("http_retry", &context)
    ///     .clone_input_with(|_request, _args| None) // Do not clone expensive request bodies.
    ///     .restore_input(|result: &mut HttpResult, _args| {
    ///         match result {
    ///             // Extract the original request from the error.
    ///             HttpResult::ConnectionError { original_request } => {
    ///                 let request = original_request.clone();
    ///                 *result = HttpResult::ServerError(0);
    ///                 Some(request)
    ///             }
    ///             _ => None,
    ///         }
    ///     })
    ///     .recovery_with(|result, _args| match result {
    ///         HttpResult::ConnectionError { .. } => RecoveryInfo::retry(),
    ///         _ => RecoveryInfo::never(),
    ///     });
    /// # }
    /// ```
    #[must_use]
    pub fn restore_input(mut self, restore_fn: impl Fn(&mut Out, RestoreInputArgs) -> Option<In> + Send + Sync + 'static) -> Self {
        self.restore_input = Some(RestoreInput::new(restore_fn));
        self
    }
}

impl<In, Out, S> Layer<S> for RetryLayer<In, Out, Set, Set> {
    type Service = Retry<In, Out, S>;

    fn layer(&self, inner: S) -> Self::Service {
        let shared = RetryShared {
            clock: self.context.get_clock().clone(),
            max_attempts: self.max_attempts,
            backoff: self.backoff.clone().into(),
            clone_input: self.clone_input.clone().expect("enforced by the type state pattern"),
            should_recover: self.should_recover.clone().expect("enforced by the type state pattern"),
            on_retry: self.on_retry.clone(),
            enable_if: self.enable_if.clone(),
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry: self.telemetry.clone(),
            restore_input: self.restore_input.clone(),
            handle_unavailable: self.handle_unavailable,
        };

        Retry {
            shared: Arc::new(shared),
            inner,
        }
    }
}

impl<In, Res, Error, S1, S2> RetryLayer<In, Result<Res, Error>, S1, S2> {
    /// Sets an input restoration callback for `Err` values.
    ///
    /// Use this method with `Result<Res, Error>` output.
    /// The callback receives the error and can extract the original input.
    /// It can also change the error before the next attempt.
    ///
    /// Use this method when:
    /// - The error contains request data that can be recovered.
    /// - Only errors can restore input.
    /// - The callback must change the error before the next retry.
    ///
    /// # Parameters
    ///
    /// * `restore_fn` receives the error and restoration data.
    ///   It returns `Some(input)` when it restores input.
    ///   It returns `None` when it cannot or must not restore input.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use tick::Clock;
    /// # use seatbelt::retry::*;
    /// # use seatbelt::{RecoveryInfo, ResilienceContext};
    /// # #[derive(Clone)]
    /// # struct HttpRequest { url: String, body: Vec<u8> }
    /// # struct HttpResponse { status: u16 }
    /// # enum HttpError {
    /// #     ConnectionError { original_request: HttpRequest },
    /// #     ServerError(u16),
    /// #     AuthError,
    /// # }
    /// # impl HttpError {
    /// #     fn try_restore_request(&mut self) -> Option<HttpRequest> {
    /// #         match self {
    /// #             HttpError::ConnectionError { original_request } => {
    /// #                 Some(original_request.clone())
    /// #             },
    /// #             _ => None,
    /// #         }
    /// #     }
    /// # }
    /// # fn example(clock: Clock) {
    /// # let context = ResilienceContext::<HttpRequest, Result<HttpResponse, HttpError>>::new(&clock);
    /// type HttpResult = Result<HttpResponse, HttpError>;
    ///
    /// let layer = Retry::layer("http_retry", &context).restore_input_from_error(
    ///     |error: &mut HttpError, _args| {
    ///         // Restore input only when a connection error contains the request.
    ///         error.try_restore_request()
    ///     },
    /// );
    /// # }
    /// ```
    #[must_use]
    pub fn restore_input_from_error(self, restore_fn: impl Fn(&mut Error, RestoreInputArgs) -> Option<In> + Send + Sync + 'static) -> Self {
        self.restore_input(move |input, args| match input {
            Ok(_) => None,
            Err(e) => restore_fn(e, args),
        })
    }
}

impl<In, Out, S1, S2> RetryLayer<In, Out, S1, S2> {
    fn into_state<T1, T2>(self) -> RetryLayer<In, Out, T1, T2> {
        RetryLayer {
            context: self.context,
            max_attempts: self.max_attempts,
            backoff: self.backoff,
            clone_input: self.clone_input,
            should_recover: self.should_recover,
            on_retry: self.on_retry,
            enable_if: self.enable_if,
            telemetry: self.telemetry,
            restore_input: self.restore_input,
            handle_unavailable: self.handle_unavailable,
            _state: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::sync::atomic::{AtomicU32, Ordering};

    use layered::Execute;
    use tick::Clock;

    use super::*;
    use crate::testing::RecoverableType;

    #[test]
    fn new_creates_correct_initial_state() {
        let context = create_test_context();
        let layer: RetryLayer<_, _, NotSet, NotSet> = RetryLayer::new("test_retry".into(), &context);

        assert_eq!(layer.max_attempts, 4); // 3 retries + 1 original = 4 total
        assert!(matches!(layer.backoff.backoff_type, Backoff::Exponential));
        assert_eq!(layer.backoff.base_delay, Duration::from_millis(10));
        assert!(layer.backoff.max_delay.is_none());
        assert!(layer.backoff.use_jitter); // Default is true
        assert!(layer.clone_input.is_none());
        assert!(layer.should_recover.is_none());
        assert!(layer.on_retry.is_none());
        assert_eq!(layer.telemetry.strategy_name.as_ref(), "test_retry");
        assert!(layer.enable_if.call(&"test_input".to_string()));
    }

    #[test]
    fn clone_input_sets_correctly() {
        let context = create_test_context();
        let layer = RetryLayer::new("test".into(), &context);

        let layer: RetryLayer<_, _, Set, NotSet> = layer.clone_input_with(|input, _args| Some(input.clone()));

        let result = layer.clone_input.unwrap().call(
            &mut "test".to_string(),
            CloneArgs {
                attempt: Attempt::new(0, false),
                previous_recovery: None,
            },
        );
        assert_eq!(result, Some("test".to_string()));
    }

    #[test]
    fn recovery_sets_correctly() {
        let context = create_test_context();
        let layer = RetryLayer::new("test".into(), &context);

        let layer: RetryLayer<_, _, NotSet, Set> = layer.recovery_with(|output, _args| {
            if output.contains("error") {
                RecoveryInfo::retry()
            } else {
                RecoveryInfo::never()
            }
        });

        let result = layer.should_recover.as_ref().unwrap().call(
            &"error message".to_string(),
            RecoveryArgs {
                attempt: Attempt::new(1, false),
                clock: context.get_clock(),
            },
        );
        assert_eq!(result, RecoveryInfo::retry());

        let result = layer.should_recover.as_ref().unwrap().call(
            &"success".to_string(),
            RecoveryArgs {
                attempt: Attempt::new(1, false),
                clock: context.get_clock(),
            },
        );
        assert_eq!(result, RecoveryInfo::never());
    }

    #[test]
    fn recovery_auto_sets_correctly() {
        let context = ResilienceContext::<RecoverableType, RecoverableType>::new(Clock::new_frozen());
        let layer = RetryLayer::new("test".into(), &context);

        let layer: RetryLayer<_, _, NotSet, Set> = layer.recovery();

        let result = layer.should_recover.as_ref().unwrap().call(
            &RecoverableType::from(RecoveryInfo::retry()),
            RecoveryArgs {
                attempt: Attempt::new(1, false),
                clock: context.get_clock(),
            },
        );
        assert_eq!(result, RecoveryInfo::retry());

        let result = layer.should_recover.as_ref().unwrap().call(
            &RecoverableType::from(RecoveryInfo::never()),
            RecoveryArgs {
                attempt: Attempt::new(1, false),
                clock: context.get_clock(),
            },
        );
        assert_eq!(result, RecoveryInfo::never());
    }

    #[test]
    fn configuration_methods_work() {
        let layer = create_ready_layer()
            .max_retry_attempts(5)
            .backoff(Backoff::Exponential)
            .base_delay(Duration::from_millis(500))
            .max_delay(Duration::from_secs(30))
            .use_jitter(true);

        assert_eq!(layer.max_attempts, 6);
        assert!(matches!(layer.backoff.backoff_type, Backoff::Exponential));
        assert_eq!(layer.backoff.base_delay, Duration::from_millis(500));
        assert_eq!(layer.backoff.max_delay, Some(Duration::from_secs(30)));
        assert!(layer.backoff.use_jitter);
    }

    #[test]
    fn on_retry_works() {
        let called = Arc::new(AtomicU32::new(0));
        let called_clone = Arc::clone(&called);

        let layer = create_ready_layer().on_retry(move |_output, _args| {
            called_clone.fetch_add(1, Ordering::SeqCst);
        });

        layer.on_retry.unwrap().call(
            &"output".to_string(),
            OnRetryArgs {
                retry_delay: Duration::ZERO,
                attempt: Attempt::new(1, false),
                recovery: RecoveryInfo::retry(),
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
    fn layer_builds_service_when_ready() {
        let layer = create_ready_layer();
        let _service = layer.layer(Execute::new(|input: String| async move { input }));
    }

    #[test]
    fn handle_unavailable_sets_correctly() {
        let context = create_test_context();
        let layer = RetryLayer::new("test".into(), &context);

        // Test default value
        assert!(!layer.handle_unavailable);

        // Test enabling outage handling
        let layer = layer.handle_unavailable(true);
        assert!(layer.handle_unavailable);

        // Test disabling outage handling
        let layer = layer.handle_unavailable(false);
        assert!(!layer.handle_unavailable);
    }

    #[test]
    fn restore_input_sets_correctly() {
        let context = create_test_context();
        let layer = RetryLayer::new("test".into(), &context);

        let layer = layer.restore_input(|output: &mut String, _args| {
            (output == "restore_me").then(|| {
                *output = "modified_output".to_string();
                "restored_input".to_string()
            })
        });

        let mut test_output = "restore_me".to_string();
        let result = layer.restore_input.as_ref().unwrap().call(
            &mut test_output,
            RestoreInputArgs {
                attempt: Attempt::new(1, false),
                recovery: RecoveryInfo::retry(),
            },
        );

        match result {
            Some(input) => {
                assert_eq!(input, "restored_input");
                assert_eq!(test_output, "modified_output");
            }
            None => panic!("Expected Some, got None"),
        }

        let mut test_output2 = "no_restore".to_string();
        let result = layer.restore_input.as_ref().unwrap().call(
            &mut test_output2,
            RestoreInputArgs {
                attempt: Attempt::new(1, false),
                recovery: RecoveryInfo::retry(),
            },
        );

        match result {
            None => {
                assert_eq!(test_output2, "no_restore");
            }
            Some(_) => panic!("Expected None, got Some"),
        }
    }

    #[test]
    fn restore_input_from_error_sets_correctly() {
        let context: ResilienceContext<String, Result<String, String>> = ResilienceContext::new(Clock::new_frozen()).name("test");
        let layer = RetryLayer::new("test".into(), &context)
            .restore_input_from_error(|e: &mut String, _| (e == "restore").then(|| std::mem::take(e)));

        let restore = layer.restore_input.as_ref().unwrap();
        let args = || RestoreInputArgs {
            attempt: Attempt::new(1, false),
            recovery: RecoveryInfo::retry(),
        };

        assert_eq!(restore.call(&mut Err("restore".into()), args()), Some("restore".to_string()));
        assert_eq!(restore.call(&mut Err("other".into()), args()), None);
        assert_eq!(restore.call(&mut Ok("success".into()), args()), None);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn config_applies_all_settings() {
        let config = RetryConfig {
            enabled: false,
            backoff_type: Backoff::Linear,
            base_delay: Duration::from_secs(2),
            max_delay: Some(Duration::from_mins(1)),
            use_jitter: false,
            max_retry_attempts: 7,
            handle_unavailable: true,
        };

        let layer = create_ready_layer().config(&config);

        insta::assert_debug_snapshot!(layer);
    }

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(RetryLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(RetryLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(RetryLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(RetryLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> ResilienceContext<String, String> {
        ResilienceContext::new(Clock::new_frozen()).name("test_pipeline")
    }

    fn create_ready_layer() -> RetryLayer<String, String, Set, Set> {
        RetryLayer::new("test".into(), &create_test_context())
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
