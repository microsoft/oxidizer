// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt::Debug;

use http_extensions::HttpResponse;
use layered::{Intercept, InterceptLayer};
use performables::arc::Arc;
use thread_aware::ThreadAware;

use crate::handlers::{Logging, LoggingLayer, Metrics, MetricsLayer};
use crate::pipeline::PipelineContext;
use crate::resilience::breaker::{HttpBreaker, HttpBreakerLayer, HttpBreakerLayerExt};
use crate::resilience::hedging::{HttpHedging, HttpHedgingLayer, HttpHedgingLayerExt};
use crate::resilience::retry::{HttpRetry, HttpRetryLayer, HttpRetryLayerExt};
use crate::resilience::timeout::{HttpTimeout, HttpTimeoutLayer, HttpTimeoutLayerExt};

const ATTEMPT_TIMEOUT_NAME: &str = "standard.attempt_timeout";
/// Ten seconds allows network variance and leaves time for retries in the total request budget.
const ATTEMPT_TIMEOUT_DURATION: std::time::Duration = std::time::Duration::from_secs(10);

const TOTAL_TIMEOUT_NAME: &str = "standard.total_timeout";
/// Thirty seconds permits the initial attempt and retries without an unlimited request duration.
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
/// The standard pipeline includes timeouts, retries, logging, and metrics.
///
/// [`HttpClientBuilder::standard_pipeline`][crate::HttpClientBuilder::standard_pipeline] uses this
/// type to change the default pipeline. Methods such as [`retry`][Self::retry] and
/// [`attempt_timeout`][Self::attempt_timeout] receive the current layer and return the changed
/// layer.
///
/// # Handlers
///
/// The handler order is outermost to innermost:
///
/// 1. Total metrics ([`total_metrics`][Self::total_metrics]) records pipeline execution through an
///    [`HttpResponse`] or terminal error, including retries and hedged attempts. It uses the
///    `http.client.request.total_duration` instrument. Later body reads are not included.
/// 2. Total timeout ([`total_timeout`][Self::total_timeout]) limits pipeline execution through an
///    [`HttpResponse`] or terminal error. It does not limit later body streaming, buffering, or
///    lazy decompression.
/// 3. Retry ([`retry`][Self::retry]) or hedging ([`hedging`][Self::hedging]) handles transient
///    failures. [`recovery_mode`][Self::recovery_mode] selects the layer.
/// 4. Breaker ([`breaker`][Self::breaker]) rejects requests when the service is likely to fail.
/// 5. Attempt timeout ([`attempt_timeout`][Self::attempt_timeout]) limits each attempt.
/// 6. Attempt intercept ([`attempt_intercept`][Self::attempt_intercept]) runs for each attempt.
/// 7. Logging ([`attempt_logs`][Self::attempt_logs]) records requests and responses.
/// 8. Metrics ([`attempt_metrics`][Self::attempt_metrics]) records per-attempt HTTP metrics.
/// 9. Dispatch ([`crate::handlers::Dispatch`]) sends the request to the transport.
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
    pub(crate) fn new(context: &PipelineContext) -> Self {
        let options = context.resilience_context();
        let clock = context.clock();
        let meter = context.meter();
        let router = context.router();

        Self {
            total_metrics: Metrics::layer().clock(clock).meter(meter.clone()).report_total_duration(true),
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
            attempt_logs: Logging::layer().redaction_engine(context.redaction_engine()).clock(clock),
            attempt_metrics: Metrics::layer().clock(clock).meter(meter.clone()),
            recovery_mode: RecoveryMode::default(),
        }
    }

    /// Changes the timeout for pipeline execution through an [`HttpResponse`] or terminal error.
    ///
    /// The timeout includes retries and hedged attempts. It does not include later body streaming,
    /// buffering, or lazy decompression.
    ///
    /// # Defaults
    ///
    /// - Timeout: 30 seconds.
    /// - Name: `standard.total_timeout`.
    #[must_use]
    pub fn total_timeout(mut self, configure: impl FnOnce(HttpTimeoutLayer) -> HttpTimeoutLayer) -> Self {
        self.total_timeout = configure(self.total_timeout);
        self
    }

    /// Changes the retry layer.
    ///
    /// This layer is active when [`recovery_mode`][Self::recovery_mode] is set to
    /// [`RecoveryMode::Retry`] (the default).
    ///
    /// # Defaults
    ///
    /// - Maximum retries: 3, for 4 total attempts.
    /// - Retry delay: 10 milliseconds. A `Retry-After` header can change the delay.
    /// - Retry policy: 5xx status codes, timeouts, and 429 Too Many Requests.
    /// - Backoff: exponential with jitter.
    /// - Retryable requests: safe HTTP methods (GET, HEAD, OPTIONS, TRACE) with cloneable bodies.
    /// - Name: `standard.retry`.
    #[must_use]
    pub fn retry(mut self, configure: impl FnOnce(HttpRetryLayer) -> HttpRetryLayer) -> Self {
        self.retry = configure(self.retry);
        self
    }

    /// Changes the hedging layer.
    ///
    /// Hedging sends concurrent requests and returns the first response that the recovery policy
    /// accepts. It is active when [`recovery_mode`][Self::recovery_mode] is
    /// [`RecoveryMode::Hedging`].
    ///
    /// # Defaults
    ///
    /// - Cloning: safe HTTP methods only (GET, HEAD, OPTIONS, TRACE).
    /// - Transient failures: 5xx, 429, and timeouts.
    /// - Name: `standard.hedging`.
    #[must_use]
    pub fn hedging(mut self, configure: impl FnOnce(HttpHedgingLayer) -> HttpHedgingLayer) -> Self {
        self.hedging = configure(self.hedging);
        self
    }

    /// Changes the circuit breaker layer.
    ///
    /// The breaker runs between recovery and the attempt timeout. It rejects requests when a
    /// service is likely to fail.
    ///
    /// An open circuit returns `HttpError::unavailable`. The error contains the original request,
    /// so an outer retry layer can restore it.
    ///
    /// # Defaults
    ///
    /// - Failures: 5xx, 429, and timeouts.
    /// - Open-circuit error: `HttpError::unavailable`.
    /// - Name: `standard.breaker`.
    #[must_use]
    pub fn breaker(mut self, configure: impl FnOnce(HttpBreakerLayer) -> HttpBreakerLayer) -> Self {
        self.breaker = configure(self.breaker);
        self
    }

    /// Changes the timeout for each request attempt.
    ///
    /// # Defaults
    ///
    /// - Timeout: 10 seconds.
    /// - Name: `standard.attempt_timeout`.
    #[must_use]
    pub fn attempt_timeout(mut self, configure: impl FnOnce(HttpTimeoutLayer) -> HttpTimeoutLayer) -> Self {
        self.attempt_timeout = configure(self.attempt_timeout);
        self
    }

    /// Changes the interception layer for each request attempt.
    ///
    /// Use it for logging, metrics, or other behavior that must run for retries and the initial
    /// attempt.
    ///
    /// # Defaults
    ///
    /// The layer does nothing until it is changed.
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

    /// Changes the logging layer for each request attempt.
    #[must_use]
    pub fn attempt_logs(mut self, configure: impl FnOnce(LoggingLayer) -> LoggingLayer) -> Self {
        self.attempt_logs = configure(self.attempt_logs);
        self
    }

    /// Changes the layer that records per-attempt HTTP metrics.
    #[must_use]
    pub fn attempt_metrics(mut self, configure: impl FnOnce(MetricsLayer) -> MetricsLayer) -> Self {
        self.attempt_metrics = configure(self.attempt_metrics);
        self
    }

    /// Changes the total metrics layer.
    ///
    /// This outermost layer measures the logical request, including retries and hedged attempts.
    /// It records `http.client.request.total_duration`. [`attempt_metrics`][Self::attempt_metrics]
    /// records each attempt.
    #[must_use]
    pub fn total_metrics(mut self, configure: impl FnOnce(MetricsLayer) -> MetricsLayer) -> Self {
        self.total_metrics = configure(self.total_metrics);
        self
    }

    /// Selects the recovery strategy for the standard pipeline.
    ///
    /// Select [`RecoveryMode::Retry`] for sequential retries. Select [`RecoveryMode::Hedging`] for
    /// concurrent hedged requests.
    ///
    /// # Defaults
    ///
    /// [`RecoveryMode::Retry`] uses sequential retries with exponential backoff.
    #[must_use]
    pub fn recovery_mode(mut self, mode: RecoveryMode) -> Self {
        self.recovery_mode = mode;
        self
    }
}

type ConfigurePipeline = dyn Fn(StandardRequestPipeline, PipelineContext) -> StandardRequestPipeline + Send + Sync;

#[derive(Clone, ThreadAware)]
pub(crate) struct ConfigureStandardPipeline(#[thread_aware(skip)] Arc<ConfigurePipeline>);

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
        let func: Box<ConfigurePipeline> = Box::new(func);
        Self(func.into())
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

    pub(crate) fn create(self, context: PipelineContext) -> StandardRequestPipeline {
        let pipeline = StandardRequestPipeline::new(&context);
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

    use data_privacy::RedactionEngine;
    use http_extensions::HttpBodyBuilder;
    use http_extensions::routing::Router;
    use opentelemetry::metrics::{Meter, MeterProvider};
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use tick::Clock;

    use super::*;
    use crate::resilience::HttpResilienceContext;

    fn test_meter() -> Meter {
        SdkMeterProvider::default().meter("test")
    }

    fn test_context() -> PipelineContext {
        let clock = Clock::new_frozen();
        PipelineContext::new(
            HttpResilienceContext::new(&clock),
            &test_meter(),
            RedactionEngine::default(),
            HttpBodyBuilder::new_fake(),
            clock,
            Router::default(),
        )
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_new_with_clock_creates_pipeline() {
        let pipeline = StandardRequestPipeline::new(&test_context());

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
        let pipeline = StandardRequestPipeline::new(&test_context());

        let debug_str = format!("{pipeline:?}");
        assert!(debug_str.contains("StandardRequestPipeline"));
        assert!(debug_str.contains("total_metrics"));
        assert!(debug_str.contains("total_timeout"));
        assert!(debug_str.contains("retry"));
        assert!(debug_str.contains("hedging"));
        assert!(debug_str.contains("breaker"));
        assert!(debug_str.contains("attempt_timeout"));

        // The total_metrics layer must actually be configured to record the
        // total-duration instrument, not merely be present.
        let total_metrics_debug = format!("{:?}", pipeline.total_metrics);
        assert!(
            total_metrics_debug.contains("report_total_duration: true"),
            "total_metrics must record the total-duration instrument, got: {total_metrics_debug}"
        );
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_multiple_pipelines_are_independent() {
        let pipeline1 = StandardRequestPipeline::new(&test_context());
        let pipeline2 = StandardRequestPipeline::new(&test_context());

        // Format two separately created pipelines.
        let debug1 = format!("{pipeline1:?}");
        let debug2 = format!("{pipeline2:?}");

        // Both pipelines contain the standard structure.
        assert!(debug1.contains("StandardRequestPipeline"));
        assert!(debug2.contains("StandardRequestPipeline"));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_default_recovery_mode_is_retry() {
        let pipeline = StandardRequestPipeline::new(&test_context());

        assert_eq!(pipeline.recovery_mode, RecoveryMode::Retry);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_recovery_mode_can_be_set_to_hedging() {
        let pipeline = StandardRequestPipeline::new(&test_context()).recovery_mode(RecoveryMode::Hedging);

        assert_eq!(pipeline.recovery_mode, RecoveryMode::Hedging);
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn test_attempt_layer_configure_closures_are_invoked() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let invocations = Arc::new(AtomicUsize::new(0));

        let intercept_flag = Arc::clone(&invocations);
        let logs_flag = Arc::clone(&invocations);
        let metrics_flag = Arc::clone(&invocations);
        let total_metrics_flag = Arc::clone(&invocations);

        let context = test_context();
        let _pipeline = StandardRequestPipeline::new(&context)
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
