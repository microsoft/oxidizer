// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::RedactionEngine;
use http_extensions::HttpBodyBuilder;
use http_extensions::routing::Router;
use opentelemetry::metrics::Meter;
use tick::Clock;

use crate::resilience::HttpResilienceContext;

/// Shared dependencies for a custom or standard request pipeline.
///
/// These methods pass it to their factory closure:
///
/// - [`HttpClientBuilder::custom_pipeline`][crate::HttpClientBuilder::custom_pipeline]
/// - [`HttpClientBuilder::standard_pipeline`][crate::HttpClientBuilder::standard_pipeline]
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PipelineContext {
    meter: Meter,
    resilience_context: HttpResilienceContext,
    redaction_engine: RedactionEngine,
    clock: Clock,
    body_builder: HttpBodyBuilder,
    router: Router,
}

impl PipelineContext {
    pub(crate) fn new(
        resilience_context: HttpResilienceContext,
        meter: &Meter,
        redaction_engine: RedactionEngine,
        body_builder: HttpBodyBuilder,
        clock: Clock,
        router: Router,
    ) -> Self {
        Self {
            resilience_context,
            redaction_engine,
            meter: meter.clone(),
            body_builder,
            clock,
            router,
        }
    }

    /// Returns the clock for time-related operations.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Returns the telemetry meter.
    #[must_use]
    pub const fn meter(&self) -> &Meter {
        &self.meter
    }

    /// Returns the resilience context.
    #[must_use]
    pub fn resilience_context(&self) -> &HttpResilienceContext {
        &self.resilience_context
    }

    /// Returns the redaction engine.
    #[must_use]
    pub const fn redaction_engine(&self) -> &RedactionEngine {
        &self.redaction_engine
    }

    /// Returns the HTTP body builder.
    #[must_use]
    pub const fn body_builder(&self) -> &HttpBodyBuilder {
        &self.body_builder
    }

    /// Returns the request router.
    #[must_use]
    pub const fn router(&self) -> &Router {
        &self.router
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http_extensions::HttpBodyOptions;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::SdkMeterProvider;

    use super::*;

    fn test_context(body_builder: HttpBodyBuilder) -> PipelineContext {
        let clock = Clock::new_frozen();
        let meter = SdkMeterProvider::default().meter("test");

        PipelineContext::new(
            HttpResilienceContext::new(&clock),
            &meter,
            RedactionEngine::default(),
            body_builder,
            clock,
            Router::default(),
        )
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn body_builder_returns_the_configured_builder() {
        let body_builder = HttpBodyBuilder::new_fake().with_options(HttpBodyOptions::default().buffer_limit(4321));
        let context = test_context(body_builder);

        // Return the builder supplied at construction, including its buffer limit.
        assert!(format!("{:?}", context.body_builder()).contains("4321"));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn accessors_return_constructed_dependencies() {
        let context = test_context(HttpBodyBuilder::new_fake());

        // Exercise every accessor. Each one must return without a panic.
        let _ = context.clock();
        let _ = context.meter();
        let _ = context.resilience_context();
        let _ = context.redaction_engine();
        let _ = context.router();
    }
}
