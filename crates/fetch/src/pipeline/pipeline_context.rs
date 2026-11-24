// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use http_extensions::HttpBodyBuilder;
use http_extensions::routing::Router;
use observed::Sink;
use opentelemetry::metrics::Meter;
use tick::Clock;

use crate::error_labels::collect_error_labels;
use crate::resilience::HttpResilienceContext;
use crate::telemetry::{Client, ClientAttributes, Telemetry, TelemetryLayer};

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
    client_attributes: ClientAttributes,
    clock: Clock,
    sink: Sink,
    body_builder: HttpBodyBuilder,
    router: Router,
}

impl PipelineContext {
    pub(crate) fn new(
        resilience_context: HttpResilienceContext,
        meter: &Meter,
        client_attributes: ClientAttributes,
        body_builder: HttpBodyBuilder,
        clock: Clock,
        sink: Sink,
        router: Router,
    ) -> Self {
        Self {
            resilience_context,
            client_attributes,
            meter: meter.clone(),
            body_builder,
            clock,
            sink,
            router,
        }
    }

    /// Returns the clock for time-related operations.
    #[must_use]
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// Returns the `observed` sink that the deps of the client supplied.
    #[must_use]
    pub const fn sink(&self) -> &Sink {
        &self.sink
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

    /// Returns a telemetry layer that emits the request events of this client.
    ///
    /// Each event has the fields `http.client.name`, `fetch.runtime` and `fetch.transport`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use fetch::HttpClientBuilder;
    /// # use layered::Stack;
    /// # fn configure(builder: HttpClientBuilder) -> HttpClientBuilder {
    /// builder.custom_pipeline(|dispatch, ctx| (ctx.client_telemetry(), dispatch).into_service())
    /// # }
    /// ```
    #[must_use]
    pub fn client_telemetry(&self) -> TelemetryLayer<Client> {
        self.client_attributes.apply(
            Telemetry::client(self.sink.clone())
                .clock(&self.clock)
                .error_labels_with(collect_error_labels),
        )
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

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        use opentelemetry::metrics::MeterProvider as _;

        let clock = Clock::new_frozen();
        Self::new(
            HttpResilienceContext::new(&clock),
            &opentelemetry_sdk::metrics::SdkMeterProvider::default().meter("test"),
            ClientAttributes::new(
                crate::constants::FAKE_RUNTIME_NAME.into(),
                crate::constants::FAKE_TRANSPORT_NAME.into(),
                crate::constants::DEFAULT_HTTP_CLIENT_NAME.into(),
            ),
            HttpBodyBuilder::new_fake(),
            clock,
            Sink::noop(),
            Router::default(),
        )
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http_extensions::HttpBodyOptions;

    use super::*;

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn body_builder_returns_the_configured_builder() {
        let mut context = PipelineContext::for_test();
        context.body_builder = HttpBodyBuilder::new_fake().with_options(HttpBodyOptions::default().buffer_limit(4321));

        // Return the builder supplied at construction, including its buffer limit.
        assert!(format!("{:?}", context.body_builder()).contains("4321"));
    }

    #[cfg_attr(miri, ignore)] // SdkMeterProvider uses operations unsupported by Miri.
    #[test]
    fn accessors_return_constructed_dependencies() {
        let context = PipelineContext::for_test();

        // Exercise every accessor. Each one must return without a panic.
        let _ = context.clock();
        let _ = context.sink();
        let _ = context.meter();
        let _ = context.resilience_context();
        let _ = context.client_telemetry();
        let _ = context.router();
    }
}
