// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt::Debug;

use data_privacy::{RedactedToString, RedactionEngine};
use http::uri::Authority;
use http_extensions::UriTemplateLabel;
use layered::{Layer, Service};
use ohno::ErrorExt;
use tick::Clock;
use tracing::{Level, event};

use crate::error_labels::collect_error_labels;
use crate::{HttpRequest, HttpResponse, RequestExt, RequestHandler, Result};

/// Logs HTTP requests and responses with timing information.
///
/// Wraps any `RequestHandler` to add logging. It tracks how long requests take
/// and logs details using tracing events. Successful responses appear at DEBUG level,
/// while errors show up at WARN level.
///
/// Emits these events:
///
/// - `http.response.complete`: When a response comes back successfully.
/// - `http.response.error`: When something goes wrong during the request.
#[derive(Debug)]
pub struct Logging<T> {
    inner: T,
    clock: Clock,
    redaction_engine: RedactionEngine,
}

impl Logging<()> {
    /// Creates a new logging handler layer with the provided clock.
    #[must_use]
    pub fn layer(clock: &Clock, redaction_engine: &RedactionEngine) -> LoggingLayer {
        LoggingLayer {
            clock: clock.clone(),
            redaction_engine: redaction_engine.clone(),
        }
    }
}

/// [`Layer`] that wraps a handler with request/response logging.
#[derive(Debug)]
pub struct LoggingLayer {
    clock: Clock,
    redaction_engine: RedactionEngine,
}

impl<S> Layer<S> for LoggingLayer {
    type Service = Logging<S>;

    /// Creates a new layer that wraps the given service with logging.
    ///
    /// This layer will log requests and responses using the provided clock for timing.
    fn layer(&self, inner: S) -> Self::Service {
        Logging {
            inner,
            clock: self.clock.clone(),
            redaction_engine: self.redaction_engine.clone(),
        }
    }
}

impl<T: RequestHandler> Service<HttpRequest> for Logging<T> {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        let watch = self.clock.stopwatch();
        let url = input.uri().clone();
        let method = input.method().clone();
        let template = input.uri_template_label().map(UriTemplateLabel::into_cow);
        let redacted_path_and_query = redacted_path_and_query(&input, &self.redaction_engine);

        async move {
            match self.inner.execute(input).await {
                Ok(response) => {
                    event!(
                        name: "http.response.complete",
                        Level::DEBUG,
                        http.request.method = method.as_str(),
                        server.address = url.authority().map(Authority::as_str),
                        server.port = url.port_u16(),
                        http.response.status_code = response.status().as_u16(),
                        network.protocol.version = ?response.version(),
                        url.scheme = url.scheme_str(),
                        url.path.template = template.as_deref(),
                        url.path.redacted = redacted_path_and_query,
                        http.client.request.duration = watch.elapsed().as_secs_f32(),
                        "HTTP response received successfully",
                    );

                    Ok(response)
                }
                Err(err) => {
                    event!(
                        name: "http.response.error",
                        Level::WARN,
                        http.request.method = method.as_str(),
                        server.address = url.authority().map(Authority::as_str),
                        server.port = url.port_u16(),
                        error.type = %collect_error_labels(&err),
                        exception.message = err.message(),
                        url.scheme = url.scheme_str(),
                        url.path.template = template.as_deref(),
                        url.path.redacted = redacted_path_and_query,
                        http.client.request.duration = watch.elapsed().as_secs_f32(),
                        "HTTP response failed",
                    );

                    Err(err)
                }
            }
        }
    }
}

fn redacted_path_and_query(request: &HttpRequest, engine: &RedactionEngine) -> Option<String> {
    match request.path_and_query() {
        Some(path_and_query) => Some(path_and_query.to_redacted_string(engine)),
        None => request
            .uri()
            .path_and_query()
            .map(|v| templated_uri::PathAndQuery::from(v.clone()).to_redacted_string(engine)),
    }
}

#[cfg(test)]
mod tests {
    use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
    use http::{Request, StatusCode};
    use http_extensions::{FakeHandler, HttpBodyBuilder, HttpRequestBuilder};
    use templated_uri::Uri;

    use super::*;
    use crate::handlers::Dispatch;

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn execute_logs_and_returns_successful_response() {
        let clock = Clock::new_frozen();
        let layer = Logging::layer(&clock, &RedactionEngine::default());
        let handler = layer.layer(Dispatch::new_fake(FakeHandler::from(StatusCode::OK)));

        let request = HttpRequestBuilder::new_fake()
            .uri("https://example.com/path?query=value")
            .build()
            .unwrap();

        let response = handler.execute(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn execute_logs_and_propagates_inner_error() {
        let clock = Clock::new_frozen();
        let layer = Logging::layer(&clock, &RedactionEngine::default());
        let handler = layer.layer(Dispatch::new_fake(FakeHandler::never_completes()));

        // Request without scheme/authority triggers a validation error in Dispatch.
        let request = Request::get(http::Uri::from_static("/no-authority"))
            .body(HttpBodyBuilder::new_fake().empty())
            .unwrap();

        let error = handler.execute(request).await.unwrap_err();
        assert_eq!(collect_error_labels(&error), "uri_origin_missing");
    }

    #[test]
    fn redacted_path_and_query_when_templated_path_and_query_attached() {
        let engine = RedactionEngine::builder()
            .add_class_redactor(Uri::DATA_CLASS, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
            .build();

        let request = HttpRequestBuilder::new_fake()
            .uri("https://example.com/path?query=value")
            .build()
            .unwrap();

        let redacted = redacted_path_and_query(&request, &engine);
        assert_eq!(redacted, Some("/path?query=value".to_string()));
    }

    #[test]
    fn redacted_path_and_query_when_templated_path_and_query_not_attached() {
        let engine = RedactionEngine::builder()
            .add_class_redactor(Uri::DATA_CLASS, SimpleRedactor::with_mode(SimpleRedactorMode::Passthrough))
            .build();

        let request = Request::builder()
            .uri("https://example.com/path?query=value")
            .body(HttpBodyBuilder::new_fake().text("abc"))
            .unwrap();

        let redacted = redacted_path_and_query(&request, &engine);
        assert_eq!(redacted, Some("/path?query=value".to_string()));
    }
}
