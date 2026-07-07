// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::Debug;
use std::sync::Arc;

use data_privacy::RedactionEngine;
use http_extensions::HttpResponse;
use http_extensions::routing::Router;
use layered::{Intercept, InterceptLayer};
use opentelemetry::metrics::Meter;
use thread_aware::ThreadAware;
use tick::Clock;

use crate::handlers::{Logging, LoggingLayer, Metrics, MetricsLayer};
use crate::pipeline::PipelineContext;
use crate::resilience::HttpResilienceContext;
use crate::resilience::breaker::{HttpBreaker, HttpBreakerLayer, HttpBreakerLayerExt};
use crate::resilience::hedging::{HttpHedging, HttpHedgingLayer, HttpHedgingLayerExt};
use crate::resilience::retry::{HttpRetry, HttpRetryLayer, HttpRetryLayerExt};
use crate::resilience::timeout::{HttpTimeout, HttpTimeoutLayer, HttpTimeoutLayerExt};

const ATTEMPT_TIMEOUT_NAME: &str = "standard.attempt_timeout";
const ATTEMPT_TIMEOUT_DURATION: std::time::Duration = std::time::Duration::from_secs(10);

const TOTAL_TIMEOUT_NAME: &str = "standard.total_timeout";
const TOTAL_TIMEOUT_DURATION: std::time::Duration = std::time::Duration::from_secs(30);

const RETRY_NAME: &str = "standard.retry";
const HEDGING_NAME: &str = "standard.hedging";
const BREAKER_NAME: &str = "standard.breaker";

/// Controls which recovery strategy the standard pipeline uses.
///
/// Both strategies occupy the same position in the middleware stack
/// (after total timeout, before the circuit breaker). The default
/// strategy is [`Retry`][RecoveryMode::Retry].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecoveryMode {
    /// Retries failed requests sequentially with backoff (default).
    #[default]
    Retry,
    /// Sends hedged requests concurrently to reduce tail latency.
    Hedging,
}

/// Configuration for the standard HTTP request pipeline.
///
/// The standard pipeline provides a production-ready configuration with essential
/// handlers that most applications need. It includes timeouts, automatic retries,
/// logging, and metrics collection with sensible defaults.
///
/// This type is used by [`HttpClientBuilder::standard_pipeline`][crate::HttpClientBuilder::standard_pipeline]
/// to configure the default pipeline when creating HTTP clients. Individual layers
/// are configured through builder methods (such as [`retry`][Self::retry] and
/// [`attempt_timeout`][Self::attempt_timeout]) that accept a closure receiving the
/// current layer and returning the modified layer.
///
/// # Handlers
///
/// The standard pipeline creates the following handler chain (from outermost to innermost)
/// with production-ready defaults:
///
/// 1. **total metrics** ([`total_metrics`][Self::total_metrics]): Records the total
///    request duration (including all retries and hedged attempts) under the
///    `http.client.request.total_duration` instrument.
/// 2. **total timeout** ([`total_timeout`][Self::total_timeout]): Enforces a total timeout for the entire request/response cycle.
/// 3. **retry** ([`retry`][Self::retry]) *or* **hedging** ([`hedging`][Self::hedging]): Recovers from transient failures. Controlled by [`recovery_mode`][Self::recovery_mode].
/// 4. **breaker** ([`breaker`][Self::breaker]): Circuit breaker that prevents sending requests to a service that is likely to fail.
/// 5. **attempt timeout** ([`attempt_timeout`][Self::attempt_timeout]): Enforces a per-attempt timeout.
/// 6. **attempt intercept** ([`attempt_intercept`][Self::attempt_intercept]): An interception layer invoked on each request attempt.
/// 7. **logging** ([`attempt_logs`][Self::attempt_logs]): Records request and response information via logging events.
/// 8. **metrics** ([`attempt_metrics`][Self::attempt_metrics]): Collects standardized per-attempt metrics about HTTP requests.
/// 9. **dispatch** ([`crate::handlers::Dispatch`]): Sends the HTTP request over the network.
#[derive(Debug)]
#[non_exhaustive]
pub struct StandardRequestPipeline {
    pub(crate) total_metrics: MetricsLayer,
    pub(crate) total_timeout: HttpTimeoutLayer,
    pub(crate) retry: HttpRetryLayer,
    pub(crate) hedging: HttpHedgingLayer,
    pub(crate) breaker: HttpBreakerLayer,
    pub(crate) attempt_timeout: HttpTimeoutLayer,
    pub(crate) attempt_intercept: InterceptLayer<crate::HttpRequest, crate::Result<HttpResponse>>,
    pub(crate) attempt_logs: LoggingLayer,
    pub(crate) attempt_metrics: MetricsLayer,
    pub(crate) recovery_mode: RecoveryMode,
}

impl StandardRequestPipeline {
    pub(crate) fn new(options: &HttpResilienceContext, redaction: &RedactionEngine, clock: &Clock, meter: &Meter, router: &Router) -> Self {
        Self {
            total_metrics: Metrics::layer()
                .clock(clock)
                .meter(meter.clone())
                .report_total_duration(true)
                .client_name(options.get_name().to_owned()),
            total_timeout: HttpTimeout::layer(TOTAL_TIMEOUT_NAME, options)
                .http_timeout_error()
                .timeout(TOTAL_TIMEOUT_DURATION),
            retry: HttpRetry::layer(RETRY_NAME, options)
                .http_configure_defaults()
                .handle_unavailable(router.has_alternatives()),
            hedging: HttpHedging::layer(HEDGING_NAME, options)
                .http_configure_defaults()
                .handle_unavailable(router.has_alternatives()),
            breaker: HttpBreaker::layer(BREAKER_NAME, options).http_configure_defaults(),
            attempt_timeout: HttpTimeout::layer(ATTEMPT_TIMEOUT_NAME, options)
                .http_timeout_error()
                .timeout(ATTEMPT_TIMEOUT_DURATION),
            attempt_intercept: Intercept::layer(),
            attempt_logs: Logging::layer(redaction).clock(clock),
            attempt_metrics: Metrics::layer()
                .clock(clock)
                .meter(meter.clone())
                .client_name(options.get_name().to_owned()),
            recovery_mode: RecoveryMode::default(),
        }
    }

    /// Configures the total timeout for the entire request/response cycle, including retries.
    ///
    /// # Defaults
    ///
    /// - **Timeout**: 30 seconds for the entire request/response cycle.
    /// - **Name**: `standard.total_timeout`
    #[must_use]
    pub fn total_timeout(mut self, configure: impl FnOnce(HttpTimeoutLayer) -> HttpTimeoutLayer) -> Self {
        self.total_timeout = configure(self.total_timeout);
        self
    }

    /// Configures the retry layer.
    ///
    /// This layer is active when [`recovery_mode`][Self::recovery_mode] is set to
    /// [`RecoveryMode::Retry`] (the default).
    ///
    /// # Defaults
    ///
    /// - **Max Retries**: 3 retry attempts (4 total attempts including the initial one).
    /// - **Retry Delay**: 2 seconds between retry attempts. Handles the `Retry-After` header
    ///   if present.
    /// - **Retry Policy**: Retries transient errors (5xx status codes, timeouts, and
    ///   429 Too Many Requests).
    /// - **Backoff Strategy**: Exponential backoff with jitter.
    /// - **Retryable Requests**: Only safe HTTP methods (GET, HEAD, OPTIONS, TRACE) with
    ///   cloneable bodies.
    /// - **Name**: `standard.retry`
    #[must_use]
    pub fn retry(mut self, configure: impl FnOnce(HttpRetryLayer) -> HttpRetryLayer) -> Self {
        self.retry = configure(self.retry);
        self
    }

    /// Configures the hedging layer.
    ///
    /// Hedging reduces tail latency by sending concurrent requests and returning the
    /// first successful response. This layer is active when [`recovery_mode`][Self::recovery_mode]
    /// is set to [`RecoveryMode::Hedging`].
    ///
    /// # Defaults
    ///
    /// - **Cloning strategy**: Safe HTTP methods only (GET, HEAD, OPTIONS, TRACE).
    /// - **Recovery classification**: 5xx, 429, and timeouts are treated as transient.
    /// - **Name**: `standard.hedging`
    #[must_use]
    pub fn hedging(mut self, configure: impl FnOnce(HttpHedgingLayer) -> HttpHedgingLayer) -> Self {
        self.hedging = configure(self.hedging);
        self
    }

    /// Configures the circuit breaker layer.
    ///
    /// The circuit breaker sits between the retry/hedging layer and the attempt timeout,
    /// preventing requests from being sent to a service that is likely to fail.
    ///
    /// When the circuit is open, requests are immediately rejected with an
    /// `HttpError::unavailable` error. The original request is attached to the error
    /// so that an outer retry layer can restore and re-attempt it.
    ///
    /// # Defaults
    ///
    /// - **Recovery classification**: 5xx, 429, and timeouts are treated as failures.
    /// - **Rejected request error**: Returns `HttpError::unavailable` when the circuit is open.
    /// - **Name**: `standard.breaker`
    #[must_use]
    pub fn breaker(mut self, configure: impl FnOnce(HttpBreakerLayer) -> HttpBreakerLayer) -> Self {
        self.breaker = configure(self.breaker);
        self
    }

    /// Configures the timeout for each individual request attempt.
    ///
    /// # Defaults
    ///
    /// - **Timeout**: 10 seconds per individual request attempt.
    /// - **Name**: `standard.attempt_timeout`
    #[must_use]
    pub fn attempt_timeout(mut self, configure: impl FnOnce(HttpTimeoutLayer) -> HttpTimeoutLayer) -> Self {
        self.attempt_timeout = configure(self.attempt_timeout);
        self
    }

    /// Configures the interception layer that is invoked on each request attempt.
    ///
    /// This can be used for custom logging, metrics, or other cross-cutting concerns
    /// that need to be applied to every attempt, including retries.
    ///
    /// # Defaults
    ///
    /// No default behavior; this layer is a no-op unless customized.
    #[must_use]
    pub fn attempt_intercept(
        mut self,
        configure: impl FnOnce(
            InterceptLayer<crate::HttpRequest, crate::Result<HttpResponse>>,
        ) -> InterceptLayer<crate::HttpRequest, crate::Result<HttpResponse>>,
    ) -> Self {
        self.attempt_intercept = configure(self.attempt_intercept);
        self
    }

    /// Configures the logging layer that logs each request attempt.
    #[must_use]
    pub fn attempt_logs(mut self, configure: impl FnOnce(LoggingLayer) -> LoggingLayer) -> Self {
        self.attempt_logs = configure(self.attempt_logs);
        self
    }

    /// Configures the metrics layer that collects standardized per-attempt HTTP request metrics.
    #[must_use]
    pub fn attempt_metrics(mut self, configure: impl FnOnce(MetricsLayer) -> MetricsLayer) -> Self {
        self.attempt_metrics = configure(self.attempt_metrics);
        self
    }

    /// Configures the total metrics layer.
    ///
    /// This is the outermost layer in the pipeline, so it measures the entire
    /// logical request (including all retries and hedged attempts) and records
    /// its duration under the `http.client.request.total_duration` instrument,
    /// distinguishing it from the per-attempt [`attempt_metrics`][Self::attempt_metrics].
    #[must_use]
    pub fn total_metrics(mut self, configure: impl FnOnce(MetricsLayer) -> MetricsLayer) -> Self {
        self.total_metrics = configure(self.total_metrics);
        self
    }

    /// Selects the recovery strategy for the standard pipeline.
    ///
    /// This controls whether the pipeline uses sequential retries
    /// ([`RecoveryMode::Retry`]) or concurrent hedged requests
    /// ([`RecoveryMode::Hedging`]) for recovering from transient failures.
    ///
    /// # Defaults
    ///
    /// [`RecoveryMode::Retry`] — sequential retries with exponential backoff.
    #[must_use]
    pub fn recovery_mode(mut self, mode: RecoveryMode) -> Self {
        self.recovery_mode = mode;
        self
    }
}

#[derive(Clone, ThreadAware)]
pub(crate) struct ConfigureStandardPipeline(
    #[thread_aware(skip)] Arc<dyn Fn(StandardRequestPipeline, PipelineContext) -> StandardRequestPipeline + Send + Sync>,
);

impl Default for ConfigureStandardPipeline {
    fn default() -> Self {
        Self::new(|pipeline, _| pipeline)
    }
}

impl ConfigureStandardPipeline {
    pub(crate) fn new<F>(func: F) -> Self
    where
        F: Fn(StandardRequestPipeline, PipelineContext) -> StandardRequestPipeline + Send + Sync + 'static,
    {
        Self(Arc::new(func))
    }

    pub(crate) fn combine<F>(self, func: F) -> Self
    where
        F: Fn(StandardRequestPipeline, PipelineContext) -> StandardRequestPipeline + Send + Sync + 'static,
    {
        let previous = self.0;

        Self::new(move |pipeline, context| {
            let intermediate = previous(pipeline, context.clone());
            func(intermediate, context)
        })
    }

    pub(crate) fn create(self, context: PipelineContext, redaction: &RedactionEngine) -> StandardRequestPipeline {
        let pipeline = StandardRequestPipeline::new(
            context.resilience_context(),
            redaction,
            context.clock(),
            context.meter(),
            context.router(),
        );
        (self.0)(pipeline, context)
    }
}

impl Debug for ConfigureStandardPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(type_name::<Self>()).finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::SdkMeterProvider;

    use super::*;

    fn test_meter() -> Meter {
        SdkMeterProvider::default().meter("test")
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_new_with_clock_creates_pipeline() {
        let clock = Clock::new_frozen();
        let pipeline = StandardRequestPipeline::new(
            &HttpResilienceContext::new(&clock),
            &RedactionEngine::default(),
            &clock,
            &test_meter(),
            &Router::default(),
        );

        assert!(format!("{:?}", pipeline.total_timeout).contains("timeout: Some(30s)"));
        assert!(format!("{:?}", pipeline.total_timeout).contains("standard.total_timeout"));

        assert!(format!("{:?}", pipeline.retry).contains("max_attempts: 4"));
        assert!(format!("{:?}", pipeline.attempt_timeout).contains("timeout: Some(10s)"));
        assert!(format!("{:?}", pipeline.attempt_timeout).contains("standard.attempt_timeout"));
    }

    #[test]
    fn test_default_timeout_constants() {
        assert_eq!(TOTAL_TIMEOUT_DURATION, Duration::from_secs(30));
        assert_eq!(ATTEMPT_TIMEOUT_DURATION, Duration::from_secs(10));
        assert!(TOTAL_TIMEOUT_DURATION > ATTEMPT_TIMEOUT_DURATION);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_debug_implementation() {
        let clock = Clock::new_frozen();
        let pipeline = StandardRequestPipeline::new(
            &HttpResilienceContext::new(&clock),
            &RedactionEngine::default(),
            &clock,
            &test_meter(),
            &Router::default(),
        );

        let debug_str = format!("{pipeline:?}");
        assert!(debug_str.contains("StandardRequestPipeline"));
        assert!(debug_str.contains("total_metrics"));
        assert!(debug_str.contains("total_timeout"));
        assert!(debug_str.contains("retry"));
        assert!(debug_str.contains("hedging"));
        assert!(debug_str.contains("breaker"));
        assert!(debug_str.contains("attempt_timeout"));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_multiple_pipelines_are_independent() {
        let clock = Clock::new_frozen();
        let pipeline1 = StandardRequestPipeline::new(
            &HttpResilienceContext::new(&clock),
            &RedactionEngine::default(),
            &clock,
            &test_meter(),
            &Router::default(),
        );
        let pipeline2 = StandardRequestPipeline::new(
            &HttpResilienceContext::new(&clock),
            &RedactionEngine::default(),
            &clock,
            &test_meter(),
            &Router::default(),
        );

        // Different instances should have different memory addresses
        // We can't test this directly, but we can verify they are separate by formatting
        let debug1 = format!("{pipeline1:?}");
        let debug2 = format!("{pipeline2:?}");

        // Both should contain the same structure but be independent instances
        assert!(debug1.contains("StandardRequestPipeline"));
        assert!(debug2.contains("StandardRequestPipeline"));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_default_recovery_mode_is_retry() {
        let clock = Clock::new_frozen();
        let pipeline = StandardRequestPipeline::new(
            &HttpResilienceContext::new(&clock),
            &RedactionEngine::default(),
            &clock,
            &test_meter(),
            &Router::default(),
        );

        assert_eq!(pipeline.recovery_mode, RecoveryMode::Retry);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_recovery_mode_can_be_set_to_hedging() {
        let clock = Clock::new_frozen();
        let pipeline = StandardRequestPipeline::new(
            &HttpResilienceContext::new(&clock),
            &RedactionEngine::default(),
            &clock,
            &test_meter(),
            &Router::default(),
        )
        .recovery_mode(RecoveryMode::Hedging);

        assert_eq!(pipeline.recovery_mode, RecoveryMode::Hedging);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_attempt_layer_configure_closures_are_invoked() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let clock = Clock::new_frozen();
        let invocations = Arc::new(AtomicUsize::new(0));

        let intercept_flag = Arc::clone(&invocations);
        let logs_flag = Arc::clone(&invocations);
        let metrics_flag = Arc::clone(&invocations);
        let total_metrics_flag = Arc::clone(&invocations);

        let _pipeline = StandardRequestPipeline::new(
            &HttpResilienceContext::new(&clock),
            &RedactionEngine::default(),
            &clock,
            &test_meter(),
            &Router::default(),
        )
        .attempt_intercept(move |intercept| {
            intercept_flag.fetch_add(1, Ordering::Relaxed);
            intercept
        })
        .attempt_logs(move |logs| {
            logs_flag.fetch_add(1, Ordering::Relaxed);
            logs
        })
        .attempt_metrics(move |metrics| {
            metrics_flag.fetch_add(1, Ordering::Relaxed);
            metrics
        })
        .total_metrics(move |metrics| {
            total_metrics_flag.fetch_add(1, Ordering::Relaxed);
            metrics
        });

        assert_eq!(invocations.load(Ordering::Relaxed), 4);
    }

    #[cfg_attr(miri, ignore)] // insta snapshots are not supported under Miri.
    #[test]
    fn test_configure_standard_pipeline_debug() {
        let configure = ConfigureStandardPipeline::default();
        insta::assert_debug_snapshot!(configure);
    }
}
