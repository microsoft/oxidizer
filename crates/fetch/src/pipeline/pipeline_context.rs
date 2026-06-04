// Copyright (c) Microsoft Corporation.

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
