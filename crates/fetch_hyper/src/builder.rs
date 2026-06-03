// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`HyperTransportBuilder`]: the public entry point for assembling a
//! [`HyperTransport`].

use std::fmt;
use std::marker::PhantomData;

use anyspawn::Spawner;
use bytesbuf::mem::GlobalPool;
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
///     Spawner::new_tokio(),
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
        let meter = self.meter.clone().unwrap_or_else(|| opentelemetry::global::meter("fetch_hyper"));
        let body_builder = self
            .body_builder
            .clone()
            .unwrap_or_else(|| HttpBodyBuilder::new(GlobalPool::new(), &self.clock));

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

    apply_pool_options(&mut hyper_builder, &options.connection_pool);
    apply_http2_options(&mut hyper_builder, &options.http_2);
    apply_keep_alive(&mut hyper_builder, &options.connection_keep_alive);
    apply_http_version_preference(&mut hyper_builder, &options.supported_http_versions);

    hyper_builder
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
    hyper_builder
        .http2_initial_max_send_streams(http_2.initial_max_send_streams)
        .http2_adaptive_window(http_2.adaptive_window);
}

#[cfg_attr(test, mutants::skip)] // cannot be verified with hyper APIs
fn apply_keep_alive(hyper_builder: &mut legacy::Builder, keep_alive: &ConnectionKeepAlive) {
    match *keep_alive {
        ConnectionKeepAlive::Disabled => {
            hyper_builder.http2_keep_alive_while_idle(false).http2_keep_alive_interval(None);
        }
        ConnectionKeepAlive::ActiveConnections { interval, timeout } => {
            hyper_builder
                .http2_keep_alive_while_idle(false)
                .http2_keep_alive_interval(interval)
                .http2_keep_alive_timeout(timeout);
        }
        ConnectionKeepAlive::ActiveAndIdleConnections { interval, timeout } => {
            hyper_builder
                .http2_keep_alive_while_idle(true)
                .http2_keep_alive_interval(interval)
                .http2_keep_alive_timeout(timeout);
        }
    }
}

#[cfg_attr(test, mutants::skip)] // cannot be verified with hyper APIs
fn apply_http_version_preference(hyper_builder: &mut legacy::Builder, versions: &[Version]) {
    if versions.iter().all(|v| *v == Version::HTTP_2) {
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

    fn tls() -> TlsBackend {
        native_tls::TlsConnector::new().unwrap().into()
    }

    fn make_builder_with(options: TransportOptions) -> HyperTransportBuilder<FakeConnector, crate::testing::FakeStream> {
        HyperTransportBuilder::new(
            FakeConnector::new_success(Bytes::new(), tick::ClockControl::new().auto_advance_timers(true).to_clock()),
            Spawner::new_tokio(),
            tick::ClockControl::new().auto_advance_timers(true).to_clock(),
            options,
        )
    }

    fn make_builder() -> HyperTransportBuilder<FakeConnector, crate::testing::FakeStream> {
        make_builder_with(TransportOptions::default())
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
        let configured = make_builder_with(options).pool_index(PoolIndex::new(42));
        insta::assert_debug_snapshot!("configured", configured);
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
            Spawner::new_tokio(),
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
            Spawner::new_tokio(),
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
            Spawner::new_tokio(),
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
            Spawner::new_tokio(),
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
            Spawner::new_tokio(),
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

        let executor = SpawnerExecutor(Spawner::new_tokio());
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
