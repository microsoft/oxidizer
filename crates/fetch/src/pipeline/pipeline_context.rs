// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use data_privacy::RedactionEngine;
use http_extensions::HttpBodyBuilder;
use http_extensions::routing::Router;
use opentelemetry::metrics::Meter;
use tick::Clock;

use crate::resilience::HttpResilienceContext;

/// Context object provided when configuring a custom or standard request pipeline.
///
/// This context is passed to the factory function in:
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

    /// Returns the clock used for time-related operations.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Returns the meter used for telemetry and metrics.
    #[must_use]
    pub const fn meter(&self) -> &Meter {
        &self.meter
    }

    /// Returns the resilience context used for configuring resilience patterns.
    #[must_use]
    pub fn resilience_context(&self) -> &HttpResilienceContext {
        &self.resilience_context
    }

    /// Returns the redaction engine used for sensitive data handling.
    #[must_use]
    pub const fn redaction_engine(&self) -> &RedactionEngine {
        &self.redaction_engine
    }

    /// Returns the HTTP body builder used for constructing request and response bodies.
    #[must_use]
    pub const fn body_builder(&self) -> &HttpBodyBuilder {
        &self.body_builder
    }

    /// Returns the router used for routing requests.
    #[must_use]
    pub const fn router(&self) -> &Router {
        &self.router
    }
}

#[cfg(test)]
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

        // The accessor must expose the exact builder supplied at construction, including its
        // distinctive buffer limit.
        assert!(format!("{:?}", context.body_builder()).contains("4321"));
    }
}
