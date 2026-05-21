// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! [`HyperTransportBuilder`]: the public entry point for assembling a
//! [`HyperTransport`].

use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use anyspawn::Spawner;
use http::Version;
use http_extensions::{HttpBodyBuilder, HttpRequest, HttpResponse, Result};
use hyper_util::client::legacy;
use layered::{DynamicService, DynamicServiceExt, Service};
use opentelemetry::metrics::Meter;
use tick::Clock;

use crate::HyperIo;
use crate::connection::Connect;
use crate::connection::hyper_handler::build_hyper_handler;
use crate::options::{ConnectionLifetime, RequestFilter};
use crate::tls::TlsBackend;

/// A type-erased Hyper request handler.
#[derive(Clone, Debug)]
pub struct HyperTransport {
    service: DynamicService<HttpRequest, Result<HttpResponse>>,
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

/// Default connect timeout applied by [`HyperTransportBuilder`].
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

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
/// Generic over:
///
/// - `C` — the user-supplied [`Connect`] service that opens raw TCP
///   connections,
/// - `S` — the stream type produced by `C`.
///
/// Knobs that drive logic in this crate (`TLS` backend, request filtering,
/// connect timeout, pool aging, telemetry) live as setters on this builder.
/// Knobs that pass straight through to `hyper`'s [`legacy::Builder`] (pool
/// size, keep-alive, HTTP/2 tuning, …) are configured through the
/// [`configure_hyper`](Self::configure_hyper) escape hatch.
///
/// # Examples
///
/// ```ignore
/// use anyspawn::Spawner;
/// use fetch_hyper::{HyperTransport, HyperTransportBuilder, TlsBackend};
/// use http_extensions::HttpBodyBuilder;
///
/// # fn make_connector() -> impl fetch_hyper::Connect<MyStream> { todo!() }
/// # fn make_tls() -> TlsBackend { todo!() }
/// # struct MyStream;
/// # impl fetch_hyper::HyperIo for MyStream {}
/// # async fn run() {
/// let transport: HyperTransport = HyperTransportBuilder::new(
///     make_connector(),
///     Spawner::new_tokio(),
///     tick::Clock::new_tokio(),
///     make_tls(),
///     HttpBodyBuilder::new_fake(),
/// )
/// .configure_hyper(|builder| {
///     builder.pool_max_idle_per_host(8);
/// })
/// .build();
/// # let _ = transport;
/// # }
/// ```
pub struct HyperTransportBuilder<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    pub(crate) connector: C,
    pub(crate) clock: Clock,
    pub(crate) tls: TlsBackend,
    pub(crate) body_builder: HttpBodyBuilder,
    pub(crate) request_filter: RequestFilter,
    pub(crate) supported_http_versions: Vec<Version>,
    pub(crate) connection_lifetime: ConnectionLifetime,
    pub(crate) connect_timeout: Duration,
    pub(crate) pool_index: usize,
    pub(crate) meter: Option<Meter>,
    pub(crate) hyper_builder: legacy::Builder,
    pub(crate) _marker: PhantomData<fn() -> S>,
}

impl<C, S> fmt::Debug for HyperTransportBuilder<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("request_filter", &self.request_filter)
            .field("supported_http_versions", &self.supported_http_versions)
            .field("connect_timeout", &self.connect_timeout)
            .field("connection_lifetime", &self.connection_lifetime)
            .field("pool_index", &self.pool_index)
            .finish_non_exhaustive()
    }
}

impl<C, S> HyperTransportBuilder<C, S>
where
    C: Connect<S>,
    S: HyperIo,
{
    /// Creates a new builder.
    ///
    /// `connector` is any [`Connect`]-implementing service.
    /// `spawner` is an [`anyspawn::Spawner`] used to drive `hyper`'s background
    /// tasks. `clock` drives our connect-timeout and connection-age accounting
    /// and is also used as timer for `hyper`.
    ///
    /// The [`HttpBodyBuilder`] is used to wrap incoming response bodies.
    #[must_use]
    pub fn new(connector: C, spawner: Spawner, clock: Clock, tls: impl Into<TlsBackend>, body_builder: HttpBodyBuilder) -> Self {
        let timer = crate::timer::ClockTimer::new(clock.clone());
        let mut hyper_builder = legacy::Client::builder(SpawnerExecutor(spawner));
        hyper_builder.timer(timer.clone()).pool_timer(timer);

        Self {
            connector,
            clock,
            body_builder,
            request_filter: RequestFilter::default(),
            supported_http_versions: vec![Version::HTTP_11, Version::HTTP_2],
            connection_lifetime: ConnectionLifetime::default(),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            pool_index: 0,
            meter: None,
            hyper_builder,
            _marker: PhantomData,
            tls: tls.into(),
        }
    }

    /// Restricts which URL schemes (`http`/`https`) are accepted.
    #[must_use]
    pub fn request_filter(mut self, filter: RequestFilter) -> Self {
        self.request_filter = filter;
        self
    }

    /// Sets the negotiable HTTP versions for outgoing requests.
    #[must_use]
    pub fn supported_http_versions(mut self, versions: &[Version]) -> Self {
        self.supported_http_versions = versions.to_vec();
        self
    }

    /// Caps how long the transport waits for a `TCP`+`TLS` connection to be
    /// established before failing with a timeout error.
    #[must_use]
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Caps the total wall-clock lifetime of a pooled connection.
    #[must_use]
    pub fn connection_lifetime(mut self, lifetime: ConnectionLifetime) -> Self {
        self.connection_lifetime = lifetime;
        self
    }

    /// Sets the pool index used to tag connection-level telemetry.
    #[must_use]
    pub fn pool_index(mut self, pool_index: usize) -> Self {
        self.pool_index = pool_index;
        self
    }

    /// Sets the OpenTelemetry [`Meter`] used to record connection metrics.
    #[must_use]
    pub fn meter(mut self, meter: Meter) -> Self {
        self.meter = Some(meter);
        self
    }

    /// Invokes a callback that further tunes `hyper`'s [`legacy::Builder`].
    ///
    /// The callback runs *immediately*, after this crate's own defaults
    /// (timer, pool timer) have been applied, so it can override any of them
    /// (e.g. pool sizing, keep-alive, HTTP/2 initial windows, …).
    ///
    /// Note: the `http2_only` flag implied by
    /// [`supported_http_versions`](Self::supported_http_versions) is applied
    /// at [`build`](Self::build) time and therefore takes precedence over any
    /// value set here.
    #[must_use]
    pub fn configure_hyper<F>(mut self, configure: F) -> Self
    where
        F: FnOnce(&mut legacy::Builder),
    {
        configure(&mut self.hyper_builder);
        self
    }

    /// Builds the configured [`HyperTransport`].
    ///
    /// Requires at least one `TLS` feature (`rustls` or `native-tls`) to be
    /// enabled — otherwise [`TlsBackend`] cannot be constructed and the
    /// transport pipeline is not compiled.
    #[must_use]
    pub fn build(self) -> HyperTransport {
        let meter = self.meter.clone().unwrap_or_else(|| opentelemetry::global::meter("fetch_hyper"));

        HyperTransport::new(build_hyper_handler(self, &meter).into_dynamic())
    }
}
