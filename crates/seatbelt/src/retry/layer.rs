// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;
use std::marker::PhantomData;
use std::time::Duration;

use crate::Layer;
use crate::retry::backoff::BackoffOptions;
use crate::retry::constants::DEFAULT_RETRY_ATTEMPTS;
use crate::retry::{CloneArgs, CloneInput, OnRetry, OnRetryArgs, RecoveryArgs, RestoreInput, RestoreInputArgs, Retry, ShouldRecover};
use crate::shared::MaxAttempts;
use crate::utils::EnableIf;
use crate::utils::TelemetryHelper;
use crate::{NotSet, PipelineContext, Recovery, RecoveryInfo, Set, retry::Backoff};

/// Builder for configuring retry resilience middleware.
///
/// This type is created by calling [`Retry::layer`](crate::retry::Retry::layer) and uses the
/// type-state pattern to enforce that required properties are configured before the retry middleware can be built:
///
/// - [`clone_input`][RetryLayer::clone_input]: Required to specify how to clone inputs for retry attempts
/// - [`recovery`][RetryLayer::recovery]: Required to determine if an output should trigger a retry
///
/// For comprehensive examples, see the [retry module][crate::retry] documentation.
#[derive(Debug)]
pub struct RetryLayer<In, Out, CloneInputState = Set, RecoveryState = Set> {
    context: PipelineContext<In, Out>,
    max_attempts: MaxAttempts,
    backoff: BackoffOptions,
    clone_input: Option<CloneInput<In>>,
    should_recover: Option<ShouldRecover<Out>>,
    on_retry: Option<OnRetry<Out>>,
    enable_if: EnableIf<In>,
    telemetry: TelemetryHelper,
    restore_input: Option<RestoreInput<In, Out>>,
    handle_unavailable: bool,
    _state: PhantomData<fn(In, CloneInputState, RecoveryState) -> Out>,
}

impl<In, Out> RetryLayer<In, Out, NotSet, NotSet> {
    #[must_use]
    pub(crate) fn new(name: Cow<'static, str>, context: &PipelineContext<In, Out>) -> Self {
        Self {
            context: context.clone(),
            max_attempts: MaxAttempts::Finite(DEFAULT_RETRY_ATTEMPTS.saturating_add(1)),
            backoff: BackoffOptions::default(),
            clone_input: None,
            should_recover: None,
            on_retry: None,
            enable_if: EnableIf::always(),
            telemetry: context.create_telemetry(name),
            restore_input: None,
            handle_unavailable: false,
            _state: PhantomData,
        }
    }
}

impl<In, Out, CloneInputState, RecoveryState> RetryLayer<In, Out, CloneInputState, RecoveryState> {
    /// Sets the maximum number of retry attempts.
    ///
    /// This specifies the maximum number of retry attempts in addition to the original call.
    /// For example, if `max_retry_attempts` is 3, the operation will be attempted up to
    /// 4 times total (1 original `+` 3 retries).
    ///
    /// **Default**: 3 retry attempts
    #[must_use]
    pub fn max_retry_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = MaxAttempts::Finite(max_attempts.saturating_add(1));
        self
    }

    /// Configures infinite retry attempts.
    ///
    /// This setting will cause the operation to be retried indefinitely until it succeeds
    /// or the retry is aborted by other means (e.g., cancellation, timeout).
    ///
    /// **Warning**: Use with caution as this can cause infinite loops if the operation
    /// consistently fails.
    #[must_use]
    pub fn infinite_retry_attempts(mut self) -> Self {
        self.max_attempts = MaxAttempts::Infinite;
        self
    }

    /// Sets the backoff strategy for delay calculation.
    ///
    /// - [`Backoff::Constant`]: Same delay between all retries
    /// - [`Backoff::Linear`]: Linearly increasing delay (`base_delay` `×` attempt)
    /// - [`Backoff::Exponential`]: Exponentially increasing delay (`base_delay × 2^attempt`)
    ///
    /// **Default**: [`Backoff::Exponential`]
    #[must_use]
    pub fn backoff(mut self, backoff_type: Backoff) -> Self {
        self.backoff.backoff_type = backoff_type;
        self
    }

    /// Sets the base delay used for backoff calculations.
    ///
    /// The meaning depends on the backoff strategy:
    /// - **Constant**: The actual delay between retries
    /// - **Linear**: Initial delay; subsequent delays are `base_delay × attempt_number`
    /// - **Exponential**: Initial delay; subsequent delays grow exponentially
    ///
    /// **Default**: 2 seconds
    #[must_use]
    pub fn base_delay(mut self, delay: Duration) -> Self {
        self.backoff.base_delay = delay;
        self
    }

    /// Sets the maximum allowed delay between retries.
    ///
    /// This caps the backoff calculation to prevent excessively long delays.
    /// If not set, delays can grow indefinitely based on the backoff algorithm.
    ///
    /// **Default**: None (no limit)
    #[must_use]
    pub fn max_delay(mut self, max_delay: Duration) -> Self {
        self.backoff.max_delay = Some(max_delay);
        self
    }

    /// Enables or disables jitter to reduce correlation between retries.
    ///
    /// Jitter adds randomization to delay calculations to prevent thundering herd
    /// problems when multiple clients retry simultaneously. This is especially
    /// important in distributed systems to avoid synchronized retry storms.
    ///
    /// **Default**: true (jitter enabled)
    #[must_use]
    pub fn use_jitter(mut self, use_jitter: bool) -> Self {
        self.backoff.use_jitter = use_jitter;
        self
    }

    /// Sets the input cloning function.
    ///
    /// This function is called before retry attempts to clone the input.
    /// Return `Some(cloned_input)` to proceed with retry, or `None` to abort
    /// retry and return the last failed result.
    ///
    /// This is required because Rust's ownership model doesn't allow reusing
    /// the same input value across multiple attempts.
    ///
    /// # Arguments
    ///
    /// * `clone_fn` - Function that takes a reference to the input and [`CloneArgs`]
    ///   containing context about the retry attempt, and returns an optional cloned input
    #[must_use]
    pub fn clone_input_with(
        mut self,
        clone_fn: impl Fn(&mut In, CloneArgs) -> Option<In> + Send + Sync + 'static,
    ) -> RetryLayer<In, Out, Set, RecoveryState> {
        self.clone_input = Some(CloneInput::new(clone_fn));
        self.into_state::<Set, RecoveryState>()
    }

    /// Automatically sets the input cloning function for types that implement [`Clone`].
    ///
    /// This is a convenience method that uses the standard [`Clone`] trait to clone
    /// inputs for retry attempts. For types that implement [`Clone`], this provides
    /// a simple way to enable retries without manually implementing a cloning function.
    ///
    /// This is equivalent to calling [`clone_input_with`][RetryLayer::clone_input_with] with
    /// `|input, _args| Some(input.clone())`.
    ///
    /// # Type Requirements
    ///
    /// This method is only available when the input type `In` implements [`Clone`].
    #[must_use]
    pub fn clone_input(self) -> RetryLayer<In, Out, Set, RecoveryState>
    where
        In: Clone,
    {
        self.clone_input_with(|input, _args| Some(input.clone()))
    }

    /// Sets the recovery classification function.
    ///
    /// This function determines whether a specific output should trigger a retry
    /// by examining the output and returning a [`RecoveryInfo`] classification.
    ///
    /// The function receives the output and [`RecoveryArgs`] with context
    /// about the current attempt.
    ///
    /// # Arguments
    ///
    /// * `recover_fn` - Function that takes a reference to the output and
    ///   [`RecoveryArgs`] containing retry attempt context, and returns
    ///   a [`RecoveryInfo`] decision
    #[must_use]
    pub fn recovery_with(
        mut self,
        recover_fn: impl Fn(&Out, RecoveryArgs) -> RecoveryInfo + Send + Sync + 'static,
    ) -> RetryLayer<In, Out, CloneInputState, Set> {
        self.should_recover = Some(ShouldRecover::new(recover_fn));
        self.into_state::<CloneInputState, Set>()
    }

    /// Automatically sets the recovery classification function for types that implement [`Recovery`].
    ///
    /// This is a convenience method that uses the [`Recovery`] trait to determine
    /// whether an output should trigger a retry. For types that implement [`Recovery`],
    /// this provides a simple way to enable intelligent retry behavior without manually
    /// implementing a recovery classification function.
    ///
    /// This is equivalent to calling [`recovery`][RetryLayer::recovery] with
    /// `|output, _args| output.recovery()`.
    ///
    /// # Type Requirements
    ///
    /// This method is only available when the output type `Out` implements [`Recovery`].
    #[must_use]
    pub fn recovery(self) -> RetryLayer<In, Out, CloneInputState, Set>
    where
        Out: Recovery,
    {
        self.recovery_with(|out, _args| out.recovery())
    }

    /// Configures a callback invoked before each retry attempt.
    ///
    /// This callback is useful for logging, metrics, or other observability
    /// purposes. It receives the output that triggered the retry and
    /// [`OnRetryArgs`] with detailed retry information.
    ///
    /// The callback does not affect retry behavior - it's purely for observation.
    ///
    /// **Default**: None (no observability by default)
    ///
    /// # Arguments
    ///
    /// * `retry_fn` - Function that takes a reference to the output and
    ///   [`OnRetryArgs`] containing retry context information
    #[must_use]
    pub fn on_retry(mut self, retry_fn: impl Fn(&Out, OnRetryArgs) + Send + Sync + 'static) -> Self {
        self.on_retry = Some(OnRetry::new(retry_fn));
        self
    }

    /// Optionally enables the retry middleware based on a condition.
    ///
    /// When disabled, requests pass through without retry protection.
    /// This call replaces any previous condition.
    ///
    /// **Default**: Always enabled
    ///
    /// # Arguments
    ///
    /// * `is_enabled` - Function that takes a reference to the input and returns
    ///   `true` if retry protection should be enabled for this request
    #[must_use]
    pub fn enable_if(mut self, is_enabled: impl Fn(&In) -> bool + Send + Sync + 'static) -> Self {
        self.enable_if = EnableIf::new(is_enabled);
        self
    }

    /// Enables the retry middleware unconditionally.
    ///
    /// All requests will have retry protection applied.
    /// This call replaces any previous condition.
    ///
    /// **Note**: This is the default behavior - retry is enabled by default.
    #[must_use]
    pub fn enable_always(mut self) -> Self {
        self.enable_if = EnableIf::always();
        self
    }

    /// Disables the retry middleware completely.
    ///
    /// All requests will pass through without retry protection.
    /// This call replaces any previous condition.
    ///
    /// **Note**: This overrides the default enabled behavior.
    #[must_use]
    pub fn disable(mut self) -> Self {
        self.enable_if = EnableIf::never();
        self
    }

    /// Configures whether the retry middleware should attempt to recover from unavailable services.
    ///
    /// When enabled, the retry middleware will treat [`RecoveryInfo::unavailable`] classifications
    /// as recoverable conditions and attempt retries. When disabled (default), unavailable services
    /// are treated as non-recoverable and cause immediate failure without retry attempts.
    ///
    /// This is particularly useful when you have access to multiple resources
    /// or service endpoints. When one resource is unavailable, the retry
    /// mechanism can attempt the operation against a different resource in subsequent
    /// attempts, potentially allowing the operation to succeed despite the unavailability.
    ///
    /// **Default**: false (unavailable responses are not retried)
    ///
    /// # Arguments
    ///
    /// * `enable` - `true` to enable unavailable recovery, `false` to treat unavailable responses as permanent failures
    ///
    /// # Example
    ///
    /// ```rust
    /// # use seatbelt::retry::{Retry, Attempt};
    /// # use seatbelt::{RecoveryInfo, PipelineContext};
    /// # use tick::Clock;
    /// # fn example() {
    /// # let context = PipelineContext::<String, Result<String, String>>::new(Clock::new_frozen());
    /// // Service with multiple endpoints that can route around unavailable services
    /// let layer = Retry::layer("multi_endpoint_retry", &context)
    ///     .clone_input_with(|input: &mut String, args| {
    ///         let mut input = input.clone();
    ///         update_endpoint(&mut input, args.attempt()); // Modify input to use a different endpoint
    ///         Some(input)
    ///     })
    ///     .recovery_with(|result, _args| match result {
    ///         Ok(_) => RecoveryInfo::never(),
    ///         Err(msg) if msg.contains("service unavailable") => RecoveryInfo::unavailable(),
    ///         Err(_) => RecoveryInfo::retry(),
    ///     })
    ///     .handle_unavailable(true); // Try different endpoints on unavailable
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
    /// This function is called when the original input could not be cloned for a retry
    /// attempt (i.e., when [`clone_input_with`][RetryLayer::clone_input_with] returns `None`).
    /// The restore function receives the output from the failed attempt and can attempt
    /// to extract and reconstruct the input for the next retry.
    ///
    /// This is particularly useful when a service is unavailable and the input was not actually
    /// consumed by the operation. A common pattern is that error responses contain or reference
    /// the original input that can be extracted for retry. For example, an HTTP request that
    /// is rejected even before sending, because the remote service is known to be down.
    ///
    /// The restore function should return:
    /// - `Some(Input)` to proceed with retry using the restored input
    /// - `None` to abort retry and return the provided output
    ///
    /// This enables retry scenarios where input cloning is expensive or impossible, but
    /// the input can be extracted from error responses or failure contexts.
    ///
    /// # Arguments
    ///
    /// * `restore_fn` - Function that takes the output and [`RestoreInputArgs`] containing
    ///   context about the retry attempt, and returns either a restored input and modified
    ///   output, or just the output to abort retry
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::ops::ControlFlow;
    /// # use seatbelt::retry::{Retry, RestoreInputArgs};
    /// # use seatbelt::{RecoveryInfo, PipelineContext};
    /// # use tick::Clock;
    /// # fn example() {
    /// # let clock = Clock::new_frozen();
    /// # let context = PipelineContext::new(&clock);
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
    ///     .clone_input_with(|_request, _args| None) // Don't clone expensive request bodies
    ///     .restore_input(|result: &mut HttpResult, _args| {
    ///         match result {
    ///             // Extract the original request from the error for retry
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
        Retry {
            inner,
            clock: self.context.get_clock().clone(),
            max_attempts: self.max_attempts,
            backoff: self.backoff.clone().into(),
            clone_input: self.clone_input.clone().expect("clone_input must be set in Ready state"),
            should_recover: self.should_recover.clone().expect("should_recover must be set in Ready state"),
            on_retry: self.on_retry.clone(),
            enable_if: self.enable_if.clone(),
            #[cfg(any(feature = "logs", feature = "metrics", test))]
            telemetry: self.telemetry.clone(),
            restore_input: self.restore_input.clone(),
            handle_unavailable: self.handle_unavailable,
        }
    }
}

impl<In, Res, Error, CloneInputState, RecoveryState> RetryLayer<In, Result<Res, Error>, CloneInputState, RecoveryState> {
    /// Sets a specialized input restoration callback that operates only on error cases.
    ///
    /// This is a convenience method for working with `Result<Res, Error>` outputs, where you
    /// only want to restore input when an error occurs. The callback receives a mutable reference
    /// to the error and can extract the original input from it, while potentially modifying the
    /// error for the next attempt.
    ///
    /// This method is particularly useful when:
    /// - Your service returns `Result<T, E>` where the error type contains recoverable request data
    /// - You want to extract and restore input only from error cases, not successful responses
    /// - You need to modify the error (e.g., to remove sensitive data) before the next retry
    ///
    /// # Parameters
    ///
    /// * `restore_fn` - A function that takes a mutable reference to the error and restoration
    ///   arguments, returning `Some(input)` if the input can be restored from the error, or
    ///   `None` if restoration is not possible or desired.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use tick::Clock;
    /// # use seatbelt::retry::*;
    /// # use seatbelt::{RecoveryInfo, PipelineContext};
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
    /// # let context = PipelineContext::<HttpRequest, Result<HttpResponse, HttpError>>::new(&clock);
    /// type HttpResult = Result<HttpResponse, HttpError>;
    ///
    /// let layer = Retry::layer("http_retry", &context).restore_input_from_error(
    ///     |error: &mut HttpError, _args| {
    ///         // Only restore input from connection errors that contain the original request
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

impl<In, Out, CloneInputState, RecoveryState> RetryLayer<In, Out, CloneInputState, RecoveryState> {
    fn into_state<C, S>(self) -> RetryLayer<In, Out, C, S> {
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use layered::Execute;
    use tick::Clock;

    use super::*;
    use crate::retry::Attempt;
    use crate::testing::RecoverableType;

    #[test]
    fn new_creates_correct_initial_state() {
        let context = create_test_context();
        let layer: RetryLayer<_, _, NotSet, NotSet> = RetryLayer::new("test_retry".into(), &context);

        assert_eq!(layer.max_attempts, MaxAttempts::Finite(4)); // 3 retries + 1 original = 4 total
        assert!(matches!(layer.backoff.backoff_type, Backoff::Exponential));
        assert_eq!(layer.backoff.base_delay, Duration::from_secs(2));
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
        let context = PipelineContext::<RecoverableType, RecoverableType>::new(Clock::new_frozen());
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

        assert_eq!(layer.max_attempts, MaxAttempts::Finite(6));
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
    fn infinite_retry_attempts_sets_correctly() {
        let context = create_test_context();
        let layer = RetryLayer::new("test".into(), &context).infinite_retry_attempts();
        assert_eq!(layer.max_attempts, MaxAttempts::Infinite);
    }

    #[test]
    fn restore_input_from_error_sets_correctly() {
        let context: PipelineContext<String, Result<String, String>> = PipelineContext::new(Clock::new_frozen()).name("test");
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

    #[test]
    fn static_assertions() {
        static_assertions::assert_impl_all!(RetryLayer<String, String, Set, Set>: Layer<String>);
        static_assertions::assert_not_impl_all!(RetryLayer<String, String, Set, NotSet>: Layer<String>);
        static_assertions::assert_not_impl_all!(RetryLayer<String, String, NotSet, Set>: Layer<String>);
        static_assertions::assert_impl_all!(RetryLayer<String, String, Set, Set>: Debug);
    }

    fn create_test_context() -> PipelineContext<String, String> {
        PipelineContext::new(Clock::new_frozen()).name("test_pipeline")
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
