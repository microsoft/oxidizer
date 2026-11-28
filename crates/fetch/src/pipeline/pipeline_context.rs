// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use http_extensions::HttpBodyBuilder;
use http_extensions::routing::Router;
use observed::Sink;
use tick::Clock;

use crate::custom::Transport;
use crate::error_labels::collect_error_labels;
use crate::resilience::HttpResilienceContext;
use crate::telemetry::{Client, FETCH_RUNTIME_ATTRIBUTE, FETCH_TRANSPORT_ATTRIBUTE, HTTP_CLIENT_NAME_ATTRIBUTE, Telemetry, TelemetryLayer};

/// Shared dependencies for a custom or standard request pipeline.
///
/// These methods pass it to their factory closure:
///
/// - [`HttpClientBuilder::custom_pipeline`][crate::HttpClientBuilder::custom_pipeline]
/// - [`HttpClientBuilder::standard_pipeline`][crate::HttpClientBuilder::standard_pipeline]
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PipelineContext {
    resilience_context: HttpResilienceContext,
    runtime_name: Cow<'static, str>,
    transport_name: Cow<'static, str>,
    client_name: Cow<'static, str>,
    clock: Clock,
    sink: Sink,
    body_builder: HttpBodyBuilder,
    router: Router,
}

impl PipelineContext {
    pub(crate) fn new(
        resilience_context: HttpResilienceContext,
        transport: &Transport,
        client_name: Cow<'static, str>,
        body_builder: HttpBodyBuilder,
        router: Router,
    ) -> Self {
        Self {
            resilience_context,
            runtime_name: transport.runtime().clone(),
            transport_name: transport.name().clone(),
            client_name,
            clock: transport.clock().clone(),
            sink: transport.sink().clone(),
            body_builder,
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
        Telemetry::client(self.sink.clone())
            .clock(&self.clock)
            .error_labels_with(collect_error_labels)
            .dimension(FETCH_RUNTIME_ATTRIBUTE, self.runtime_name.clone())
            .dimension(FETCH_TRANSPORT_ATTRIBUTE, self.transport_name.clone())
            .dimension(HTTP_CLIENT_NAME_ATTRIBUTE, self.client_name.clone())
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
        let clock = Clock::new_frozen();
        Self {
            resilience_context: HttpResilienceContext::new(&clock),
            runtime_name: crate::constants::FAKE_RUNTIME_NAME.into(),
            transport_name: crate::constants::FAKE_TRANSPORT_NAME.into(),
            client_name: crate::constants::DEFAULT_HTTP_CLIENT_NAME.into(),
            clock,
            sink: Sink::noop(),
            body_builder: HttpBodyBuilder::new_fake(),
            router: Router::default(),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http_extensions::HttpBodyOptions;

    use super::*;

    #[test]
    fn body_builder_returns_the_configured_builder() {
        let mut context = PipelineContext::for_test();
        context.body_builder = HttpBodyBuilder::new_fake().with_options(HttpBodyOptions::default().buffer_limit(4321));

        // Return the builder supplied at construction, including its buffer limit.
        assert!(format!("{:?}", context.body_builder()).contains("4321"));
    }

    #[test]
    fn accessors_return_constructed_dependencies() {
        let context = PipelineContext::for_test();

        // Exercise every accessor. Each one must return without a panic.
        let _ = context.clock();
        let _ = context.sink();
        let _ = context.resilience_context();
        let _ = context.client_telemetry();
        let _ = context.router();
    }
}
