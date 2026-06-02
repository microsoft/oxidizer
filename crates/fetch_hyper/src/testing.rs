// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! In-process fakes used by this crate's own tests.

use std::fmt;
use std::io::Error as IoError;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bytes::Bytes;
use http_extensions::{HttpBody, HttpBodyBuilder, HttpError, HttpRequest, HttpRequestBuilder};
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use seatbelt::RecoveryInfo;
use templated_uri::BaseUri;
use tick::Clock;

use crate::error_labels::LABEL_CONNECT;

/// A stream that panics on every operation.
///
/// Useful for verifying that wrappers (e.g. `Box<dyn HyperIo>`,
/// `TrackedStream`) forward each method to the inner stream — the panic
/// message is the method name.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct PanickingStream;

impl Read for PanickingStream {
    fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        panic!("poll_read");
    }
}

impl Write for PanickingStream {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<std::io::Result<usize>> {
        panic!("poll_write");
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        panic!("poll_flush");
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        panic!("poll_shutdown");
    }

    fn is_write_vectored(&self) -> bool {
        panic!("is_write_vectored");
    }

    fn poll_write_vectored(self: Pin<&mut Self>, _cx: &mut Context<'_>, _bufs: &[std::io::IoSlice<'_>]) -> Poll<std::io::Result<usize>> {
        panic!("poll_write_vectored");
    }
}

impl Connection for PanickingStream {
    fn connected(&self) -> Connected {
        panic!("connected");
    }
}

/// A fake stream that returns a single canned response (or error) once any
/// data has been written to it.
///
/// The first [`poll_read`](Read::poll_read) parks the caller until the first
/// [`poll_write`](Write::poll_write) (the request). The canned response is
/// then delivered in one read, and `EOF` is returned afterwards.
pub struct FakeStream {
    result: Option<std::result::Result<Bytes, TestError>>,
    state: Arc<Mutex<FakeStreamState>>,
}

#[derive(Debug)]
struct FakeStreamState {
    request_received: bool,
    read_waker: Option<Waker>,
}

impl fmt::Debug for FakeStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>()).finish_non_exhaustive()
    }
}

impl Read for FakeStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, mut buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        let mut state = self.state.lock().unwrap();

        if !state.request_received {
            state.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        state.read_waker = None;
        drop(state);

        self.as_mut().result.take().map_or(Poll::Ready(Ok(())), |res| match res {
            Ok(bytes) => {
                buf.put_slice(&bytes);
                Poll::Ready(Ok(()))
            }
            Err(error) => Poll::Ready(Err(error.into_io_error())),
        })
    }
}

impl Write for FakeStream {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        let mut state = self.state.lock().unwrap();

        let was_waiting = !state.request_received;
        state.request_received = true;

        if was_waiting && let Some(waker) = state.read_waker.take() {
            waker.wake();
        }

        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Connection for FakeStream {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

/// A connector that returns a [`FakeStream`] (or a synthetic connect error)
/// for every request.
///
/// Implements [`layered::Service<BaseUri>`] (and therefore
/// [`Connect<FakeStream>`](crate::Connect)) so it can be plugged directly
/// into [`HyperTransportBuilder`](crate::HyperTransportBuilder).
#[derive(Debug, Clone)]
pub struct FakeConnector {
    response: Option<std::result::Result<Bytes, TestError>>,
    connect_error: Option<TestError>,
    clock: Clock,
    /// Delay applied before the connect resolves. `Duration::ZERO` by default.
    pub delay: Duration,
}

impl FakeConnector {
    /// Builds a connector whose stream replays `data` as the canned response.
    #[must_use]
    pub fn new_success(data: impl Into<Bytes>, clock: Clock) -> Self {
        Self {
            response: Some(Ok(data.into())),
            connect_error: None,
            clock,
            delay: Duration::ZERO,
        }
    }

    /// Builds a connector whose stream returns `error` from the first read.
    #[must_use]
    pub fn new_failure(error: TestError, clock: Clock) -> Self {
        Self {
            response: Some(Err(error)),
            connect_error: None,
            clock,
            delay: Duration::ZERO,
        }
    }

    /// Builds a connector that fails immediately during connect with `error`.
    #[must_use]
    pub fn new_connect_failure(error: TestError, clock: Clock) -> Self {
        Self {
            response: None,
            connect_error: Some(error),
            clock,
            delay: Duration::ZERO,
        }
    }

    /// Sets the delay applied before the connect resolves.
    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

impl layered::Service<BaseUri> for FakeConnector {
    type Out = http_extensions::Result<FakeStream>;

    fn execute(&self, _input: BaseUri) -> impl Future<Output = Self::Out> + Send {
        let response = self.response.clone();
        let connect_error = self.connect_error.clone();
        let clock = self.clock.clone();
        let delay = self.delay;

        async move {
            clock.delay(delay).await;

            if let Some(error) = connect_error {
                return Err(HttpError::other(error, RecoveryInfo::retry(), LABEL_CONNECT));
            }

            Ok(FakeStream {
                result: response,
                state: Arc::new(Mutex::new(FakeStreamState {
                    request_received: false,
                    read_waker: None,
                })),
            })
        }
    }
}

/// A simple [`Error`](std::error::Error) for use in test assertions.
///
/// Optionally carries an inner cause so error-chain traversal logic can be
/// exercised.
#[derive(Debug, Clone)]
pub struct TestError {
    message: String,
    inner: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl TestError {
    /// Creates a new error with the given display message and no source.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            inner: None,
        }
    }

    /// Sets `inner` as the [`source`](std::error::Error::source) of this error.
    #[must_use]
    pub fn with_inner<E: std::error::Error + Send + Sync + 'static>(mut self, inner: E) -> Self {
        self.inner = Some(Arc::new(inner));
        self
    }

    /// Convenience for wrapping a recoverability annotation as the source.
    #[must_use]
    pub fn with_inner_recoverability(self, recoverability: RecoveryInfo) -> Self {
        self.with_inner(HttpError::other("inner error", recoverability, "other"))
    }

    /// Wraps `self` into a [`std::io::Error`].
    #[must_use]
    pub fn into_io_error(self) -> IoError {
        IoError::other(self)
    }
}

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.as_ref().map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Builds a basic GET request against `http://example.com/some-custom-path`,
/// using a fake [`HttpBodyBuilder`] for the request body.
///
/// # Panics
///
/// Panics if the test request cannot be built. The static URI is valid, so
/// this only fails on programming errors in the request builder itself.
#[must_use]
pub fn create_test_request() -> HttpRequest {
    HttpRequestBuilder::new(&HttpBodyBuilder::new_fake())
        .uri("http://example.com/some-custom-path")
        .build()
        .expect("test request should build")
}

/// Returns a fake [`HttpBodyBuilder`] suitable for tests.
#[must_use]
pub fn fake_body_builder() -> HttpBodyBuilder {
    HttpBodyBuilder::new_fake()
}

/// Returns OpenTelemetry [`KeyValue`]s as `(key, value)` string pairs sorted
/// by key, suitable for deterministic snapshot assertions.
#[must_use]
pub fn sorted_attributes(attrs: &[opentelemetry::KeyValue]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = attrs.iter().map(|kv| (kv.key.to_string(), kv.value.to_string())).collect();
    pairs.sort();
    pairs
}

/// Returns a real [`hyper::Error`] driven by an in-memory stream.
///
/// Drives a [`hyper`] handshake against a stream that fails every read and
/// write, returning the resulting error. Useful for inspecting a real
/// `hyper::Error` without the network.
///
/// # Panics
///
/// Panics if the in-memory handshake unexpectedly succeeds or the connection
/// completes without an error.
#[must_use]
pub fn create_hyper_error() -> hyper::Error {
    use futures::executor::block_on;

    let (_, conn) = block_on(hyper::client::conn::http1::Builder::new().handshake::<_, HttpBody>(FailingStream))
        .expect("handshake should succeed against in-memory stream");

    block_on(conn).expect_err("connection driven against FailingStream must fail")
}

#[derive(Debug)]
struct FailingStream;

impl Read for FailingStream {
    fn poll_read(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: ReadBufCursor<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(IoError::other("FailingStream read error")))
    }
}

impl Write for FailingStream {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(IoError::other("FailingStream write error")))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(IoError::other("FailingStream flush error")))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(IoError::other("FailingStream shutdown error")))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use anyspawn::Spawner;
    use bytes::Bytes;
    use fetch_options::RequestFilter;
    use http_body_util::BodyExt;
    use layered::Service as _;
    use native_tls::TlsConnector;
    use seatbelt::RecoveryInfo;

    use crate::testing::{FakeConnector, TestError, create_test_request, fake_body_builder};
    use crate::{HyperTransportBuilder, TlsBackend};

    fn build_tls() -> TlsBackend {
        TlsBackend::NativeTls(TlsConnector::new().unwrap())
    }

    fn http_1_response() -> Bytes {
        Bytes::from_static(b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!")
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fake_connector_serves_canned_response() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let mut options = fetch_options::TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        let handler = HyperTransportBuilder::new(
            FakeConnector::new_success(http_1_response(), clock.clone()),
            Spawner::new_tokio(),
            clock,
            options,
        )
        .body_builder(fake_body_builder())
        .build(build_tls());

        let response = handler.execute(create_test_request()).await.unwrap();

        assert_eq!(response.status(), 200);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&*body, b"Hello, World!");
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn fake_connector_propagates_connect_failure() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let mut options = fetch_options::TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        options.connect_timeout = Duration::from_secs(5);
        let handler = HyperTransportBuilder::new(
            FakeConnector::new_connect_failure(
                TestError::new("forced connect error").with_inner_recoverability(RecoveryInfo::retry()),
                clock.clone(),
            ),
            Spawner::new_tokio(),
            clock,
            options,
        )
        .body_builder(fake_body_builder())
        .build(build_tls());

        let error = handler
            .execute(create_test_request())
            .await
            .expect_err("connect failure should propagate");

        let rendered = error.to_string();
        assert!(
            rendered.contains("forced connect error"),
            "expected error to mention forced connect error, got: {rendered}"
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn https_only_filter_rejects_http_request() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let handler = HyperTransportBuilder::new(
            FakeConnector::new_success(http_1_response(), clock.clone()),
            Spawner::new_tokio(),
            clock,
            // default RequestFilter is Https only
            fetch_options::TransportOptions::default(),
        )
        .body_builder(fake_body_builder())
        .build(build_tls());

        let error = handler
            .execute(create_test_request())
            .await
            .expect_err("http request should be rejected when only https is allowed");

        assert!(
            error.to_string().to_lowercase().contains("scheme") || error.to_string().to_lowercase().contains("http"),
            "expected scheme/http error, got: {error}"
        );
    }
}
