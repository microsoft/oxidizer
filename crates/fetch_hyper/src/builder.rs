// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`HyperTransportBuilder`]: the public entry point for assembling a
//! [`HyperTransport`].

use std::fmt;
use std::marker::PhantomData;

use anyspawn::Spawner;
use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch_options::{ConnectionIdleTimeout, ConnectionKeepAlive, ConnectionPoolOptions, Http2Options, PoolIndex, TransportOptions};
use fetch_tls::TlsBackend;
use http::Version;
use http_extensions::{HttpBodyBuilder, HttpRequest, HttpResponse, Result};
use hyper_util::client::legacy;
use layered::{DynamicService, DynamicServiceExt, Service};
use opentelemetry::metrics::Meter;
use tick::Clock;

use crate::HyperIo;
use crate::connection::Connect;
use crate::connection::hyper_handler::build_hyper_handler;

/// A type-erased Hyper request handler.
#[derive(Clone, Debug)]
pub struct HyperTransport {
    service: DynamicService<HttpRequest, Result<HttpResponse>>,
}

impl From<HyperTransport> for DynamicService<HttpRequest, Result<HttpResponse>> {
    fn from(transport: HyperTransport) -> Self {
        transport.service
    }
}

impl HyperTransport {
    pub(crate) fn new(service: DynamicService<HttpRequest, Result<HttpResponse>>) -> Self {
        Self { service }
    }
}

impl Service<HttpRequest> for HyperTransport {
    type Out = Result<HttpResponse>;

    fn execute(&self, input: HttpRequest) -> impl Future<Output = Self::Out> + Send {
        self.service.execute(input)
    }
}

/// Adapter exposing an [`anyspawn::Spawner`] as a [`hyper::rt::Executor`].
#[derive(Clone)]
pub(crate) struct SpawnerExecutor(pub(crate) Spawner);

impl<F> hyper::rt::Executor<F> for SpawnerExecutor
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        // Drop the join handle: hyper expects fire-and-forget execution.
        drop(self.0.spawn(fut));
    }
}

/// Builder for [`HyperTransport`].
///
/// Generic over the user-supplied [`Connect`] service `C` and the stream type
/// `S` it produces.
///
/// Transport-level knobs (request filter, supported HTTP versions, connect
/// timeout, connection pool, keep-alive, HTTP/2 tuning) come from the
/// [`fetch_options::TransportOptions`] passed to [`new`](Self::new). The
/// remaining knobs (body builder, pool index, OpenTelemetry meter) have
/// dedicated setters.
///
/// # Examples
///
/// ```
/// use anyspawn::Spawner;
/// use fetch_hyper::{HyperTransport, HyperTransportBuilder};
/// use fetch_options::TransportOptions;
/// use hyper_util::rt::TokioIo;
/// use layered::Execute;
/// use templated_uri::BaseUri;
/// use tokio::net::TcpStream;
///
/// type MyStream = TokioIo<TcpStream>;
///
/// // Stubbed out: the doctest never dials, so the body is never reached.
/// async fn connect(_uri: BaseUri) -> http_extensions::Result<MyStream> {
///     unreachable!("doc example; never invoked")
/// }
///
/// # async fn run() {
/// // A real `TlsBackend` does expensive store initialization, so it is
/// // stubbed out too; the function below is never actually called.
/// let tls: fetch_tls::TlsBackend = unreachable!("doc example; never invoked");
///
/// let transport: HyperTransport = HyperTransportBuilder::new(
///     Execute::new(connect),
///     Spawner::new_tokio(None),
///     tick::ClockControl::new()
///         .auto_advance_timers(true)
///         .to_clock(),
///     TransportOptions::default(),
/// )
/// .build(tls);
/// # let _ = transport;
/// # }
/// ```
pub struct HyperTransportBuilder<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    pub(crate) connector: C,
    pub(crate) clock: Clock,
    pub(crate) body_builder: Option<HttpBodyBuilder>,
    pub(crate) options: TransportOptions,
    pub(crate) pool_index: PoolIndex,
    pub(crate) meter: Option<Meter>,
    pub(crate) hyper_builder: legacy::Builder,
    pub(crate) _marker: PhantomData<fn() -> S>,
}

impl<C, S> fmt::Debug for HyperTransportBuilder<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("options", &self.options)
            .field("pool_index", &self.pool_index)
            .finish_non_exhaustive()
    }
}

impl<C, S> HyperTransportBuilder<C, S>
where
    C: Connect<S>,
    S: HyperIo + Unpin,
{
    /// Creates a new builder configured by `options`.
    ///
    /// `connector` is any [`Connect`] service. `spawner` drives `hyper`'s
    /// background tasks. `clock` drives connect-timeout and connection-age
    /// accounting and acts as `hyper`'s timer.
    ///
    /// `options` drives every transport-level knob; pass
    /// [`TransportOptions::default`] to accept the defaults. The body builder,
    /// pool index, and OpenTelemetry meter have dedicated setters instead.
    ///
    /// The `TLS` backend is supplied at [`build`](Self::build) time.
    #[must_use]
    pub fn new(connector: C, spawner: Spawner, clock: Clock, mut options: TransportOptions) -> Self {
        // #[gamma::skip(call_result.default, tag = "equivalent", reason = "coerce_options returns unit; default_after still executes its option updates and replaces only () with ()")]
        coerce_options(&mut options);

        let hyper_builder = configure_hyper_builder(spawner, &clock, &options);

        Self {
            connector,
            clock,
            body_builder: None,
            options,
            pool_index: PoolIndex::new(0),
            meter: None,
            hyper_builder,
            _marker: PhantomData,
        }
    }

    /// Sets the [`HttpBodyBuilder`] used to wrap incoming response bodies.
    ///
    /// When not set, [`build`](Self::build) constructs one with a fresh
    /// [`GlobalPool`] and the builder's clock.
    #[must_use]
    pub fn body_builder(mut self, body_builder: HttpBodyBuilder) -> Self {
        self.body_builder = Some(body_builder);
        self
    }

    /// Sets the pool index used to tag connection-level telemetry.
    #[must_use]
    pub fn pool_index(mut self, pool_index: PoolIndex) -> Self {
        self.pool_index = pool_index;
        self
    }

    /// Sets the OpenTelemetry [`Meter`] used to record connection metrics.
    #[must_use]
    pub fn meter(mut self, meter: Meter) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Builds the configured [`HyperTransport`] using the supplied `TLS` backend.
    ///
    /// Requires at least one `TLS` feature (`rustls` or `native-tls`);
    /// otherwise [`TlsBackend`] cannot be constructed.
    #[must_use]
    pub fn build(self, tls: TlsBackend) -> HyperTransport {
        let meter = transport_meter(self.meter.clone(), opentelemetry::global::meter);
        let body_builder = self
            .body_builder
            .clone()
            .unwrap_or_else(|| HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &self.clock));

        HyperTransport::new(build_hyper_handler(self, tls, body_builder, &meter).into_dynamic())
    }
}

fn coerce_options(options: &mut TransportOptions) {
    if options.supported_http_versions.is_empty() {
        options.supported_http_versions = vec![Version::HTTP_11, Version::HTTP_2];
    }
}

/// Builds a `hyper-util` legacy client builder pre-configured from
/// `options`, including the timer, pool sizing, HTTP/2 tuning, keep-alive
/// policy, and HTTP-version preference.
fn configure_hyper_builder(spawner: Spawner, clock: &Clock, options: &TransportOptions) -> legacy::Builder {
    let timer = crate::timer::ClockTimer::new(clock.clone());
    let mut hyper_builder = legacy::Client::builder(SpawnerExecutor(spawner));
    hyper_builder.timer(timer.clone()).pool_timer(timer);

    // #[gamma::skip(call_result.default, tag = "equivalent", reason = "apply_pool_options returns unit; default_after preserves all builder mutations and returns the same ()")]
    apply_pool_options(&mut hyper_builder, &options.connection_pool);
    // #[gamma::skip(call_result.default, tag = "equivalent", reason = "apply_http2_options returns unit; default_after preserves all builder mutations and returns the same ()")]
    apply_http2_options(&mut hyper_builder, &options.http_2);
    // #[gamma::skip(call.replace_with_default, stmt.delete_call, tag = "resource", reason = "Nov 4 SARIF 24f333b1f171 and 1784ebd6412c timed out after removing keep-alive configuration")]
    // #[gamma::skip(call_result.default, tag = "equivalent", reason = "apply_keep_alive returns unit; default_after still applies the keep-alive configuration and returns the same ()")]
    apply_keep_alive(&mut hyper_builder, &options.connection_keep_alive);
    // #[gamma::skip(call_result.default, tag = "equivalent", reason = "apply_http_version_preference returns unit; default_after preserves its builder mutations and returns the same ()")]
    apply_http_version_preference(&mut hyper_builder, &options.supported_http_versions);

    hyper_builder
}

fn transport_meter(meter: Option<Meter>, global: impl FnOnce(&'static str) -> Meter) -> Meter {
    meter.unwrap_or_else(|| global("fetch_hyper"))
}

#[cfg_attr(test, mutants::skip)] // cannot be verified with hyper APIs
fn apply_pool_options(hyper_builder: &mut legacy::Builder, pool: &ConnectionPoolOptions) {
    let pool_idle_timeout = match pool.connection_idle_timeout {
        ConnectionIdleTimeout::Unlimited => None,
        ConnectionIdleTimeout::Limited(timeout) => Some(timeout),
    };

    hyper_builder
        .pool_idle_timeout(pool_idle_timeout)
        .pool_max_idle_per_host(pool.max_connections);
}

#[cfg_attr(test, mutants::skip)] // cannot be verified with hyper APIs
fn apply_http2_options(hyper_builder: &mut legacy::Builder, http_2: &Http2Options) {
    // Order matters: hyper's `http2_adaptive_window(true)` resets the stream window to the
    // spec default, so the explicit size has to be applied first for adaptive tuning to win
    // when both are configured. That precedence is documented on `initial_stream_window_size`.
    hyper_builder
        .http2_initial_max_send_streams(http_2.initial_max_send_streams)
        .http2_initial_stream_window_size(initial_stream_window_size(http_2))
        .http2_adaptive_window(http_2.adaptive_window);
}

/// RFC 9113 limits `SETTINGS_INITIAL_WINDOW_SIZE` to `2^31 - 1`.
///
/// Exceeding this limit is a protocol error that would surface as a connection failure far
/// from the configuration site, so the transport clamps requests at this boundary.
const MAX_HTTP2_STREAM_WINDOW_SIZE: u32 = i32::MAX as u32;

/// Applies the HTTP/2 protocol limit before forwarding the stream window to hyper.
fn initial_stream_window_size(http_2: &Http2Options) -> Option<u32> {
    http_2.initial_stream_window_size.map(|size| size.min(MAX_HTTP2_STREAM_WINDOW_SIZE))
}

#[cfg_attr(test, mutants::skip)] // cannot be verified with hyper APIs
// #[gamma::skip(fn_value.unit, tag = "resource", reason = "Nov 4 SARIF fce5a9bd1283 timed out after replacing keep-alive configuration with unit")]
fn apply_keep_alive(hyper_builder: &mut legacy::Builder, keep_alive: &ConnectionKeepAlive) {
    match *keep_alive {
        ConnectionKeepAlive::Disabled => {
            hyper_builder
                .http2_keep_alive_while_idle(
                    // #[gamma::skip(literal.bool_flip, tag = "equivalent", reason = "the following interval(None) disables KeepAlive creation in hyper's ping::channel; the idle flag is read only when that interval is Some")]
                    false,
                )
                .http2_keep_alive_interval(None);
        }
        ConnectionKeepAlive::ActiveConnections { interval, timeout } => {
            hyper_builder
                .http2_keep_alive_while_idle(false)
                .http2_keep_alive_interval(interval)
                .http2_keep_alive_timeout(timeout);
        }
        ConnectionKeepAlive::ActiveAndIdleConnections { interval, timeout } => {
            // #[gamma::skip(stmt.delete_call, tag = "resource", reason = "Nov 4 SARIF 783ccdbf03a8 timed out after deleting the idle keep-alive configuration chain")]
            hyper_builder
                .http2_keep_alive_while_idle(true)
                .http2_keep_alive_interval(interval)
                .http2_keep_alive_timeout(timeout);
        }
    }
}

#[cfg_attr(test, mutants::skip)] // cannot be verified with hyper APIs
fn apply_http_version_preference(hyper_builder: &mut legacy::Builder, versions: &[Version]) {
    if versions.len() == 1 && versions[0] == Version::HTTP_2 {
        hyper_builder.http2_only(true);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use bytes::Bytes;
    use fetch_options::{ConnectionLifetime, RequestFilter};
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::SdkMeterProvider;

    use super::*;
    use crate::testing::FakeConnector;

    #[test]
    fn gamma_round2_default_meter_scope_and_explicit_meter_short_circuit_are_exact() {
        let provider = SdkMeterProvider::builder().build();
        let expected = provider.meter("controlled-meter");
        let rendered = format!("{expected:?}");
        let actual = transport_meter(None, |scope| {
            assert_eq!(scope, "fetch_hyper");
            expected.clone()
        });
        assert_eq!(format!("{actual:?}"), rendered);
        let actual = transport_meter(Some(expected), |_| panic!("explicit meter must bypass the global provider"));
        assert_eq!(format!("{actual:?}"), rendered);
    }

    fn gamma_round2_raw_builder() -> legacy::Builder {
        legacy::Client::builder(SpawnerExecutor(Spawner::new_tokio(None)))
    }

    #[test]
    fn gamma_round2_pool_settings_match_explicit_hyper_configuration() {
        for timeout in [None, Some(Duration::from_secs(17))] {
            let pool = ConnectionPoolOptions::default().max_connections(3).connection_idle_timeout(timeout);
            let mut actual = gamma_round2_raw_builder();
            let mut expected = actual.clone();
            expected.pool_idle_timeout(timeout).pool_max_idle_per_host(3);
            apply_pool_options(&mut actual, &pool);
            assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        }
    }

    struct GammaH2Io(hyper_util::rt::TokioIo<tokio::io::DuplexStream>);

    impl hyper::rt::Read for GammaH2Io {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: hyper::rt::ReadBufCursor<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
        }
    }

    impl hyper::rt::Write for GammaH2Io {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::pin::Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
        }
        fn poll_flush(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().0).poll_flush(cx)
        }
        fn poll_shutdown(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> {
            std::pin::Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
        }
    }

    impl hyper_util::client::legacy::connect::Connection for GammaH2Io {
        fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
            hyper_util::client::legacy::connect::Connected::new()
        }
    }

    #[derive(Clone)]
    struct GammaH2Connector(std::sync::Arc<std::sync::Mutex<Option<tokio::io::DuplexStream>>>);

    impl tower::Service<http::Uri> for GammaH2Connector {
        type Response = GammaH2Io;
        type Error = std::io::Error;
        type Future = std::future::Ready<std::result::Result<GammaH2Io, std::io::Error>>;

        fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> std::task::Poll<std::result::Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn call(&mut self, _request: http::Uri) -> Self::Future {
            let io = self.0.lock().unwrap().take().unwrap();
            std::future::ready(Ok(GammaH2Io(hyper_util::rt::TokioIo::new(io))))
        }
    }

    #[derive(Clone)]
    struct GammaH2Timer {
        clock: Clock,
        sleeps: futures::channel::mpsc::UnboundedSender<Duration>,
    }

    impl hyper::rt::Timer for GammaH2Timer {
        fn sleep(&self, duration: Duration) -> std::pin::Pin<Box<dyn hyper::rt::Sleep>> {
            self.sleeps.unbounded_send(duration).unwrap();
            hyper::rt::Timer::sleep(&crate::timer::ClockTimer::new(self.clock.clone()), duration)
        }

        fn sleep_until(&self, deadline: std::time::Instant) -> std::pin::Pin<Box<dyn hyper::rt::Sleep>> {
            self.sleep(deadline.saturating_duration_since(self.clock.instant()))
        }

        fn now(&self) -> std::time::Instant {
            self.clock.instant()
        }
    }

    type GammaTaskQueue = std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<std::sync::Arc<GammaManualTask>>>>;

    #[derive(Clone, Default)]
    struct GammaManualExecutor(GammaTaskQueue);

    struct GammaManualTask {
        future: std::sync::Mutex<std::pin::Pin<Box<dyn Future<Output = ()> + Send>>>,
        queue: GammaTaskQueue,
    }

    impl futures::task::ArcWake for GammaManualTask {
        fn wake_by_ref(arc_self: &std::sync::Arc<Self>) {
            arc_self.queue.lock().unwrap().push_back(std::sync::Arc::clone(arc_self));
        }
    }

    impl<F: Future<Output = ()> + Send + 'static> hyper::rt::Executor<F> for GammaManualExecutor {
        fn execute(&self, fut: F) {
            self.0.lock().unwrap().push_back(std::sync::Arc::new(GammaManualTask {
                future: std::sync::Mutex::new(Box::pin(fut)),
                queue: std::sync::Arc::clone(&self.0),
            }));
        }
    }

    impl GammaManualExecutor {
        fn run_until_stalled(&self) {
            for _ in 0..10_000 {
                let Some(task) = self.0.lock().unwrap().pop_front() else { return };
                let waker = futures::task::waker_ref(&task);
                let mut future = task.future.lock().unwrap();
                if future.as_mut().poll(&mut std::task::Context::from_waker(&waker)).is_ready() {
                    *future = Box::pin(std::future::pending());
                }
            }
            panic!("the in-memory HTTP/2 task queue failed to quiesce");
        }
    }

    #[test]
    fn gamma_round2_keep_alive_idle_flags_and_disabled_reset_are_effective() {
        use futures::FutureExt;
        use futures::executor::block_on;
        use hyper::rt::Executor;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for policy in [
            ConnectionKeepAlive::Disabled,
            ConnectionKeepAlive::active_connections(Duration::from_secs(7), Duration::from_secs(3)),
            ConnectionKeepAlive::active_and_idle_connections(Duration::from_secs(7), Duration::from_secs(3)),
        ] {
            let executor = GammaManualExecutor::default();
            let control = tick::ClockControl::new();
            let (sleeps, _observed) = futures::channel::mpsc::unbounded();
            let (io, mut peer) = tokio::io::duplex(8192);
            let connector = GammaH2Connector(std::sync::Arc::new(std::sync::Mutex::new(Some(io))));
            let mut builder = legacy::Client::builder(executor.clone());
            builder
                .http2_only(true)
                .http2_adaptive_window(false)
                .http2_keep_alive_while_idle(true)
                .http2_keep_alive_interval(Duration::from_secs(7))
                .timer(GammaH2Timer {
                    clock: control.to_clock(),
                    sleeps,
                });
            apply_keep_alive(&mut builder, &policy);
            let client = builder.build::<_, http_body_util::Empty<Bytes>>(connector);
            let request_client = client.clone();
            let (sent, received) = std::sync::mpsc::channel();
            executor.execute(async move {
                sent.send(request_client.get("http://in-memory.invalid/".parse().unwrap()).await)
                    .unwrap();
            });
            executor.run_until_stalled();
            let mut preface = [0; 24];
            block_on(peer.read_exact(&mut preface)).unwrap();
            assert_eq!(block_on(gamma_round2_h2_frame(&mut peer)).0, 4);
            block_on(peer.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0])).unwrap();
            executor.run_until_stalled();
            block_on(peer.write_all(&[0, 0, 1, 1, 5, 0, 0, 0, 1, 0x88])).unwrap();
            executor.run_until_stalled();
            let response = received.try_recv().unwrap().unwrap();
            assert_eq!(response.status(), http::StatusCode::OK);
            drop(response);
            executor.run_until_stalled();
            let mut bytes = [0; 8192];
            while let Some(read) = peer.read(&mut bytes).now_or_never() {
                assert!(read.unwrap() > 0);
            }
            control.advance(Duration::from_secs(7));
            executor.run_until_stalled();
            let ping = peer.read(&mut bytes).now_or_never();
            if matches!(policy, ConnectionKeepAlive::ActiveAndIdleConnections { .. }) {
                let count = ping.unwrap().unwrap();
                assert_eq!(count, 17);
                assert_eq!(bytes[3], 6);
            } else {
                assert!(ping.is_none(), "disabled and active-only policies must not ping an idle stream");
            }
            drop((client, peer));
            executor.run_until_stalled();
        }
    }

    async fn gamma_round2_h2_frame(peer: &mut tokio::io::DuplexStream) -> (u8, Vec<u8>) {
        use tokio::io::AsyncReadExt;

        let mut header = [0; 9];
        peer.read_exact(&mut header).await.unwrap();
        let length = usize::from(header[0]) * 65536 + usize::from(header[1]) * 256 + usize::from(header[2]);
        let mut payload = vec![0; length];
        peer.read_exact(&mut payload).await.unwrap();
        (header[3], payload)
    }

    #[tokio::test]
    async fn gamma_round2_http2_keep_alive_uses_the_configured_intervals() {
        use futures::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for policy in [
            ConnectionKeepAlive::active_connections(Duration::from_secs(7), Duration::from_secs(3)),
            ConnectionKeepAlive::active_and_idle_connections(Duration::from_secs(7), Duration::from_secs(3)),
        ] {
            let control = tick::ClockControl::new();
            let (sleeps, mut observed) = futures::channel::mpsc::unbounded();
            let (io, mut peer) = tokio::io::duplex(8192);
            let connector = GammaH2Connector(std::sync::Arc::new(std::sync::Mutex::new(Some(io))));
            let mut options = TransportOptions::default();
            options.supported_http_versions = vec![Version::HTTP_2];
            options.connection_keep_alive = policy;
            options.http_2.adaptive_window = false;
            let mut builder = configure_hyper_builder(Spawner::new_tokio(None), &control.to_clock(), &options);
            builder.timer(GammaH2Timer {
                clock: control.to_clock(),
                sleeps,
            });
            let client = builder.build::<_, http_body_util::Empty<Bytes>>(connector);
            let request = tokio::spawn(async move { client.get("http://in-memory.invalid/".parse().unwrap()).await });
            let mut preface = [0; 24];
            peer.read_exact(&mut preface).await.unwrap();
            assert_eq!(gamma_round2_h2_frame(&mut peer).await.0, 4);
            peer.write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0]).await.unwrap();
            assert_eq!(observed.next().await.unwrap(), Duration::from_secs(7));
            control.advance(Duration::from_secs(7));
            loop {
                let (kind, payload) = gamma_round2_h2_frame(&mut peer).await;
                if kind == 6 {
                    assert_eq!(payload.len(), 8);
                    break;
                }
            }
            let mut timeout = observed.next().await.unwrap();
            if timeout == Duration::from_secs(7) {
                timeout = observed.next().await.unwrap();
            }
            assert_eq!(timeout, Duration::from_secs(3));
            control.advance(Duration::from_secs(3));
            let error = request.await.unwrap().unwrap_err();
            assert_eq!(
                crate::recoverability::detect_recoverability(&error),
                seatbelt::RecoveryInfo::retry()
            );
            let mut source: &(dyn std::error::Error + 'static) = &error;
            loop {
                if let Some(error) = source.downcast_ref::<hyper::Error>() {
                    assert!(error.is_timeout(), "{error:?}");
                    break;
                }
                source = source.source().unwrap();
            }
        }
    }

    async fn gamma_round2_h2_settings(options: Http2Options, keep_alive: ConnectionKeepAlive) -> Vec<(u16, u32)> {
        use tokio::io::AsyncReadExt;

        let (io, mut peer) = tokio::io::duplex(8192);
        let connector = GammaH2Connector(std::sync::Arc::new(std::sync::Mutex::new(Some(io))));
        let mut transport = TransportOptions::default();
        transport.supported_http_versions = vec![Version::HTTP_2];
        transport.http_2 = options;
        transport.connection_keep_alive = keep_alive;
        let builder = configure_hyper_builder(Spawner::new_tokio(None), &Clock::new_frozen(), &transport);
        let client = builder.build::<_, http_body_util::Empty<Bytes>>(connector);
        let request = tokio::spawn(async move { client.get("http://in-memory.invalid/".parse().unwrap()).await });
        let mut preface = [0; 24];
        peer.read_exact(&mut preface).await.unwrap();
        assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
        let mut header = [0; 9];
        peer.read_exact(&mut header).await.unwrap();
        assert_eq!(header[3], 4, "first HTTP/2 frame must be SETTINGS");
        let length = usize::from(header[0]) * 65536 + usize::from(header[1]) * 256 + usize::from(header[2]);
        let mut payload = vec![0; length];
        peer.read_exact(&mut payload).await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        payload
            .chunks_exact(6)
            .map(|setting| {
                (
                    u16::from_be_bytes(setting[..2].try_into().unwrap()),
                    u32::from_be_bytes(setting[2..].try_into().unwrap()),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn gamma_round2_http2_window_settings_are_encoded_on_the_wire() {
        for adaptive in [false, true] {
            let mut options = Http2Options::default();
            options.initial_stream_window_size = Some(123_456);
            options.adaptive_window = adaptive;
            let settings = gamma_round2_h2_settings(options, ConnectionKeepAlive::Disabled).await;
            assert!(settings.contains(&(4, if adaptive { 65_535 } else { 123_456 })), "{settings:?}");
        }
    }

    #[tokio::test]
    async fn gamma_round2_configured_timer_supports_keep_alive_handshake() {
        let mut options = Http2Options::default();
        options.initial_stream_window_size = Some(123_456);
        options.adaptive_window = false;
        let settings = gamma_round2_h2_settings(
            options,
            ConnectionKeepAlive::active_connections(Duration::from_secs(7), Duration::from_secs(3)),
        )
        .await;
        assert!(settings.contains(&(4, 123_456)), "{settings:?}");
    }

    #[test]
    fn gamma_round2_configuration_composes_pool_and_version_options() {
        let clock = Clock::new_frozen();
        let mut options = TransportOptions::default();
        options.supported_http_versions = vec![Version::HTTP_2, Version::HTTP_11];
        options.connection_pool = ConnectionPoolOptions::default()
            .max_connections(7)
            .connection_idle_timeout(Duration::from_secs(11));
        let actual = configure_hyper_builder(Spawner::new_tokio(None), &clock, &options);
        let mut expected = gamma_round2_raw_builder();
        expected.pool_idle_timeout(Duration::from_secs(11)).pool_max_idle_per_host(7);
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    }

    #[test]
    fn gamma_round2_body_builder_setter_keeps_the_supplied_limits() {
        let body = HttpBodyBuilder::new_fake().with_options(http_extensions::HttpBodyOptions::default().buffer_limit(17));
        let expected = format!("{body:?}");
        let builder = make_builder().body_builder(body);
        assert_eq!(format!("{:?}", builder.body_builder.unwrap()), expected);
    }

    fn tls() -> TlsBackend {
        native_tls::TlsConnector::new().unwrap().into()
    }

    fn make_builder_with(options: TransportOptions) -> HyperTransportBuilder<FakeConnector, crate::testing::FakeStream> {
        HyperTransportBuilder::new(
            FakeConnector::new_success(Bytes::new(), tick::ClockControl::new().auto_advance_timers(true).to_clock()),
            Spawner::new_tokio(None),
            tick::ClockControl::new().auto_advance_timers(true).to_clock(),
            options,
        )
    }

    fn make_builder() -> HyperTransportBuilder<FakeConnector, crate::testing::FakeStream> {
        make_builder_with(TransportOptions::default())
    }

    #[test]
    fn gamma_survivor_builder_coerces_empty_versions_and_retains_meter() {
        let mut options = TransportOptions::default();
        options.supported_http_versions.clear();
        let builder = make_builder_with(options);
        assert_eq!(builder.options.supported_http_versions, [Version::HTTP_11, Version::HTTP_2]);
        let provider = SdkMeterProvider::builder().build();
        let builder = builder.meter(provider.meter("gamma-builder"));
        assert!(builder.meter.is_some());
    }

    fn http_and_https_options() -> TransportOptions {
        let mut options = TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        options
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn builder_defaults_and_setters() {
        let defaults = make_builder();
        assert!(defaults.meter.is_none(), "meter is not part of Debug output");
        insta::assert_debug_snapshot!("defaults", defaults);

        let mut options = TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        options.supported_http_versions = vec![Version::HTTP_2];
        options.connect_timeout = Duration::from_secs(7);
        options.connection_pool.connection_lifetime = ConnectionLifetime::fixed(Duration::from_mins(1));
        options.http_2 = fetch_options::Http2Options::default().initial_stream_window_size(1024 * 1024);
        let configured = make_builder_with(options).pool_index(PoolIndex::new(42));
        insta::assert_debug_snapshot!("configured", configured);
    }

    #[test]
    fn http2_stream_window_is_clamped_at_the_transport_boundary() {
        let mut http_2 = fetch_options::Http2Options::default();
        assert_eq!(initial_stream_window_size(&http_2), None);

        http_2.initial_stream_window_size = Some(MAX_HTTP2_STREAM_WINDOW_SIZE - 1);
        assert_eq!(initial_stream_window_size(&http_2), Some(MAX_HTTP2_STREAM_WINDOW_SIZE - 1));

        http_2.initial_stream_window_size = Some(u32::MAX);

        assert_eq!(initial_stream_window_size(&http_2), Some(MAX_HTTP2_STREAM_WINDOW_SIZE));

        http_2.initial_stream_window_size = Some(MAX_HTTP2_STREAM_WINDOW_SIZE);
        assert_eq!(initial_stream_window_size(&http_2), Some(MAX_HTTP2_STREAM_WINDOW_SIZE));

        http_2.initial_stream_window_size = Some(MAX_HTTP2_STREAM_WINDOW_SIZE + 1);
        assert_eq!(initial_stream_window_size(&http_2), Some(MAX_HTTP2_STREAM_WINDOW_SIZE));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn meter_setter_stores_meter() {
        let provider = SdkMeterProvider::builder().build();
        let m = provider.meter("test");
        let b = make_builder().meter(m);
        assert!(b.meter.is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_applies_transport_options_to_builder() {
        let mut options = TransportOptions::default();
        options.request_filter = RequestFilter::HttpAndHttps;
        options.connect_timeout = Duration::from_secs(7);
        options.supported_http_versions = vec![Version::HTTP_2];
        options.connection_pool.connection_lifetime = ConnectionLifetime::fixed(Duration::from_mins(1));
        options.connection_keep_alive = ConnectionKeepAlive::active_and_idle_connections(None, None);

        let b = make_builder_with(options);
        assert_eq!(b.options.request_filter, RequestFilter::HttpAndHttps);
        assert_eq!(b.options.connect_timeout, Duration::from_secs(7));
        assert_eq!(b.options.supported_http_versions, vec![Version::HTTP_2]);
        assert_eq!(
            b.options.connection_pool.connection_lifetime.resolve(),
            Some(Duration::from_mins(1))
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_applies_active_connections_keep_alive() {
        let mut options = TransportOptions::default();
        options.connection_keep_alive = ConnectionKeepAlive::active_connections(Duration::from_secs(5), Duration::from_secs(10));

        let b = make_builder_with(options);
        assert!(matches!(
            b.options.connection_keep_alive,
            ConnectionKeepAlive::ActiveConnections { .. }
        ));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn new_applies_unlimited_idle_timeout() {
        let mut options = TransportOptions::default();
        options.connection_pool = options.connection_pool.connection_idle_timeout(None);

        let b = make_builder_with(options);
        assert!(matches!(
            b.options.connection_pool.connection_idle_timeout,
            ConnectionIdleTimeout::Unlimited
        ));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn build_with_explicit_meter_yields_working_transport() {
        let provider = SdkMeterProvider::builder().build();
        let response_bytes = Bytes::from_static(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let handler = HyperTransportBuilder::new(
            FakeConnector::new_success(response_bytes, clock.clone()),
            Spawner::new_tokio(None),
            clock,
            http_and_https_options(),
        )
        .body_builder(HttpBodyBuilder::new_fake())
        .meter(provider.meter("test"))
        .build(tls());
        let resp = handler.execute(crate::testing::create_test_request()).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn build_with_h2_only_sets_http2_only_flag() {
        // We can't easily inspect hyper's internal flag, but we can at least
        // exercise the build path with HTTP/2-only configuration to confirm
        // it succeeds without panicking.
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let mut options = TransportOptions::default();
        options.supported_http_versions = vec![Version::HTTP_2];
        let _handler = HyperTransportBuilder::new(
            FakeConnector::new_success(Bytes::new(), clock.clone()),
            Spawner::new_tokio(None),
            clock,
            options,
        )
        .body_builder(HttpBodyBuilder::new_fake())
        .build(tls());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn hyper_transport_clones_share_underlying_service() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let response_bytes = Bytes::from_static(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let handler = HyperTransportBuilder::new(
            FakeConnector::new_success(response_bytes, clock.clone()),
            Spawner::new_tokio(None),
            clock,
            http_and_https_options(),
        )
        .body_builder(HttpBodyBuilder::new_fake())
        .build(tls());
        let cloned = handler.clone();
        let _ = format!("{cloned:?}");
        let resp = cloned.execute(crate::testing::create_test_request()).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn hyper_transport_into_dynamic_service_executes_request() {
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let response_bytes = Bytes::from_static(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let handler = HyperTransportBuilder::new(
            FakeConnector::new_success(response_bytes, clock.clone()),
            Spawner::new_tokio(None),
            clock,
            http_and_https_options(),
        )
        .body_builder(HttpBodyBuilder::new_fake())
        .build(tls());

        let service: DynamicService<HttpRequest, Result<HttpResponse>> = handler.into();
        let resp = service.execute(crate::testing::create_test_request()).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn build_without_body_builder_uses_default() {
        // Pin the default body builder path in `build`: when no `body_builder`
        // is provided, `build` synthesizes one and still produces a working
        // transport.
        let response_bytes = Bytes::from_static(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let clock = tick::ClockControl::new().auto_advance_timers(true).to_clock();
        let handler = HyperTransportBuilder::new(
            FakeConnector::new_success(response_bytes, clock.clone()),
            Spawner::new_tokio(None),
            clock,
            http_and_https_options(),
        )
        .build(tls());
        let resp = handler.execute(crate::testing::create_test_request()).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn spawner_executor_runs_future() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let executor = SpawnerExecutor(Spawner::new_tokio(None));
        let fired = Arc::new(AtomicBool::new(false));
        let fired_clone = Arc::clone(&fired);
        hyper::rt::Executor::execute(&executor, async move {
            fired_clone.store(true, Ordering::SeqCst);
        });
        // Yield briefly so the spawned task can run.
        for _ in 0..50 {
            if fired.load(Ordering::SeqCst) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(fired.load(Ordering::SeqCst));
    }

    #[test]
    fn coerce_options_fills_empty_http_versions_with_defaults() {
        let mut options = TransportOptions::default();
        options.supported_http_versions = vec![];

        coerce_options(&mut options);

        assert_eq!(options.supported_http_versions, vec![Version::HTTP_11, Version::HTTP_2]);
    }
}
