// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Internal generic [`HyperHandler`] driving hyper-util's `legacy::Client`.
//!
//! Implements [`Service<HttpRequest>`]. Type-erased into
//! [`HyperTransport`](crate::HyperTransport) by
//! [`HyperTransportBuilder::build`](crate::HyperTransportBuilder::build).

use std::error::Error;
use std::fmt::{self, Display};
use std::pin::Pin;

use bytesbuf::BytesView;
use fetch_options::ConnectionInfo;
use fetch_tls::TlsBackend;
use futures::TryFutureExt;
use http::Extensions;
use http_body_util::BodyExt;
use http_extensions::timeout::BodyTimeout;
use http_extensions::{HttpBody, HttpBodyOptions, HttpError, HttpRequest, HttpResponse, Result};
use hyper_util::client::legacy::connect::{CaptureConnection, capture_connection};
use hyper_util::client::legacy::{self, Client};
use layered::Service;
use opentelemetry::metrics::Meter;

use crate::builder::HyperTransportBuilder;
use crate::connection::client_connector::ClientConnector;
use crate::connection::connect::Connect;
use crate::connection::hyper_connector_adapter::HyperConnectorAdapter;
use crate::connection::io::HyperIo;
use crate::connection::tracked_stream::TrackedStream;
use crate::error_labels::LABEL_REQUEST_HYPER;
use crate::recoverability::detect_recoverability;
use crate::tls::TlsConnector;

/// The fully-wrapped connector chain handed to `hyper`'s [`Client`].
type WrappedConnector<C, S> =
    HyperConnectorAdapter<ClientConnector<TlsConnector<C, S>, Pin<Box<dyn HyperIo>>>, TrackedStream<Pin<Box<dyn HyperIo>>>>;

/// A Hyper-backed request handler, parameterized by the user-supplied
/// connector and stream types. Public consumers see only the
/// type-erased [`HyperTransport`](crate::HyperTransport).
pub(crate) struct HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    client: Client<WrappedConnector<C, S>, HttpBody>,
    body_builder: http_extensions::HttpBodyBuilder,
}

impl<C, S> fmt::Debug for HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish_non_exhaustive()
    }
}

impl<C, S> Service<HttpRequest> for HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    type Out = Result<HttpResponse>;

    fn execute(&self, mut input: HttpRequest) -> impl Future<Output = Result<HttpResponse>> + Send {
        let captured = capture_connection::<HttpBody>(&mut input);

        let body_options = input
            .extensions()
            .get::<BodyTimeout>()
            .map(|v| HttpBodyOptions::default().timeout(v.duration()))
            .unwrap_or_default();

        self.client
            .request(input)
            .map_err(create_http_error_from_hyper_util)
            .map_ok(move |res| {
                let (parts, body) = res.into_parts();

                let body = body
                    .map_frame(|f| f.map_data(BytesView::from))
                    .map_err(create_http_error_from_hyper);

                handle_poisoning(&captured, &parts.extensions);

                HttpResponse::from_parts(parts, self.body_builder.body(body, &body_options))
            })
    }
}

/// Assembles a [`HyperHandler`] from a configured [`HyperTransportBuilder`].
pub(crate) fn build_hyper_handler<C, S>(
    builder: HyperTransportBuilder<C, S>,
    tls: TlsBackend,
    body_builder: http_extensions::HttpBodyBuilder,
    meter: &Meter,
) -> HyperHandler<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    let HyperTransportBuilder {
        connector,
        clock,
        options,
        pool_index,
        hyper_builder,
        ..
    } = builder;

    let tls_connector = TlsConnector::new(tls, connector, options.request_filter, &options.supported_http_versions);

    let inner = ClientConnector::new(
        tls_connector,
        clock,
        options.connect_timeout,
        options.supported_http_versions,
        meter,
        pool_index,
        options.connection_pool.connection_lifetime,
    );

    HyperHandler {
        client: hyper_builder.build(HyperConnectorAdapter::new(inner)),
        body_builder,
    }
}

fn create_http_error_from_hyper_util(error: legacy::Error) -> HttpError {
    let recovery = detect_recoverability(&error);
    HttpError::other(HyperError::Legacy(error), recovery, LABEL_REQUEST_HYPER)
}

fn create_http_error_from_hyper(error: hyper::Error) -> HttpError {
    let recovery = detect_recoverability(&error);
    HttpError::other(HyperError::Hyper(error), recovery, LABEL_REQUEST_HYPER)
}

#[derive(Debug)]
enum HyperError {
    Legacy(legacy::Error),
    Hyper(hyper::Error),
}

impl Error for HyperError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Legacy(e) => Some(e),
            Self::Hyper(e) => Some(e),
        }
    }
}

impl Display for HyperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Legacy(error) => write!(f, "{error}")?,
            Self::Hyper(error) => write!(f, "{error}")?,
        }

        let mut current: Option<&(dyn Error + 'static)> = self.source();
        while let Some(source) = current {
            write!(f, "\ncaused by: {source}")?;
            current = source.source();
        }

        Ok(())
    }
}

fn handle_poisoning(capture: &CaptureConnection, extensions: &Extensions) {
    if let Some(info) = extensions.get::<ConnectionInfo>()
        && info.is_expired()
        && let Some(connected) = capture.connection_metadata().as_ref()
    {
        connected.poison();
        ConnectionInfo::poison(info);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use anyspawn::Spawner;
    use bytes::Bytes;
    use fetch_options::{ConnectionLifetime, PoolIndex, RequestFilter};
    use http::Version;
    use http_body_util::BodyExt as _;
    use http_extensions::{HttpBodyBuilder, HttpRequestBuilder};
    use layered::Service as _;

    use super::*;
    use crate::HyperTransport;
    use crate::testing::{FakeConnector, create_hyper_error, fake_body_builder};

    fn tls() -> TlsBackend {
        native_tls::TlsConnector::new().unwrap().into()
    }

    fn http_response_bytes() -> Bytes {
        Bytes::from_static(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
    }

    fn make_handler(connector: FakeConnector, lifetime: ConnectionLifetime) -> HyperTransport {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let mut options = fetch_options::TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        options.connection_pool.connection_lifetime = lifetime;
        HyperTransportBuilder::new(connector, Spawner::new_tokio(), clock, options)
            .body_builder(HttpBodyBuilder::new_fake())
            .build(tls())
    }

    fn test_request() -> HttpRequest {
        HttpRequestBuilder::new(&fake_body_builder())
            .uri("http://example.com/path")
            .build()
            .unwrap()
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn debug_renders_handler_type() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(http_response_bytes(), clock.clone());
        let mut options = fetch_options::TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        let handler: HyperHandler<FakeConnector, crate::testing::FakeStream> = build_hyper_handler(
            HyperTransportBuilder::new(connector, Spawner::new_tokio(), clock, options),
            tls(),
            HttpBodyBuilder::new_fake(),
            &opentelemetry::global::meter("test"),
        );
        let rendered = format!("{handler:?}");
        assert!(rendered.contains("HyperHandler"), "got: {rendered}");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn malformed_response_yields_hyper_util_error() {
        // The byte stream is not a valid HTTP/1 response, so hyper's client
        // request future fails with a `legacy::Error`, exercising
        // `create_http_error_from_hyper_util`.
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(Bytes::from_static(b"NOT A VALID HTTP RESPONSE"), clock.clone());
        let handler = make_handler(connector, ConnectionLifetime::unlimited());
        let err = handler.execute(test_request()).await.expect_err("expected error");
        assert!(!err.to_string().is_empty());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn http2_only_configures_hyper_correctly() {
        // Builder with HTTP/2-only flips `http2_only(true)` on hyper's builder.
        // Using FakeStream over HTTP/1.1-style data will fail, but we want to
        // simply exercise the build path and request execution.
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(http_response_bytes(), clock.clone());
        let mut options = fetch_options::TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        options.supported_http_versions = vec![Version::HTTP_2];
        let handler = HyperTransportBuilder::new(connector, Spawner::new_tokio(), clock, options)
            .body_builder(HttpBodyBuilder::new_fake())
            .build(tls());
        // Execute to drive the http2 path; we don't care if it fails or not.
        let _ = handler.execute(test_request()).await;
    }

    #[test]
    fn poison_path_no_op_when_no_connection_info() {
        let extensions = Extensions::new();
        let mut req = test_request();
        let capture = capture_connection::<HttpBody>(&mut req);
        // No ConnectionInfo on extensions → handle_poisoning is a no-op.
        handle_poisoning(&capture, &extensions);
    }

    #[test]
    fn poison_path_no_op_when_connection_not_expired() {
        let mut extensions = Extensions::new();
        let info = ConnectionInfo::new(std::time::Instant::now, PoolIndex::new(0), Some(Duration::from_mins(1)));
        extensions.insert(info.clone());

        let mut req = test_request();
        let capture = capture_connection::<HttpBody>(&mut req);
        handle_poisoning(&capture, &extensions);
        assert!(!info.is_poisoned(), "should not be poisoned when not expired");
    }

    #[test]
    fn poison_path_no_op_when_no_capture_metadata() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};

        let mut extensions = Extensions::new();
        let base = std::time::Instant::now();
        let offset = Arc::new(AtomicU64::new(0));
        let clock_offset = Arc::clone(&offset);
        let now = move || base + Duration::from_nanos(clock_offset.load(Ordering::Relaxed));
        let info = ConnectionInfo::new(now, PoolIndex::new(0), Some(Duration::from_secs(1)));
        offset.store(u64::try_from(Duration::from_secs(5).as_nanos()).unwrap(), Ordering::Relaxed);
        assert!(info.is_expired());
        extensions.insert(info.clone());

        let mut req = test_request();
        let capture = capture_connection::<HttpBody>(&mut req);
        // capture.connection_metadata() returns None until hyper populates it.
        handle_poisoning(&capture, &extensions);
        // No metadata available → ConnectionInfo::poison must NOT be called.
        assert!(!info.is_poisoned());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn end_to_end_response_is_returned_with_body() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let connector = FakeConnector::new_success(http_response_bytes(), clock.clone());
        let handler = make_handler(connector, ConnectionLifetime::unlimited());
        let resp = handler.execute(test_request()).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&*body, b"hello");
    }

    #[test]
    fn create_http_error_from_hyper_wraps_with_label() {
        use ohno::Labeled;
        let err = create_http_error_from_hyper(create_hyper_error());
        assert!(!err.to_string().is_empty());
        assert_eq!(err.label().as_str(), "request_hyper");
    }

    #[test]
    fn hyper_error_display_includes_source_chain() {
        let err = create_hyper_error();
        let wrapped = HyperError::Hyper(err);
        let rendered = format!("{wrapped}");
        // HyperError::Hyper always exposes its inner error as a source, and
        // create_hyper_error produces a hyper::Error with at least one source
        // level (an io::Error).
        let src = std::error::Error::source(&wrapped);
        assert!(src.is_some());
        if src.and_then(std::error::Error::source).is_some() {
            assert!(rendered.contains("caused by"), "expected chain in: {rendered}");
        }
    }
}
