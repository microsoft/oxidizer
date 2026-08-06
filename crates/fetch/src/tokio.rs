// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tokio-runtime entry points for [`HttpClient`].
//!
//! This module groups the Tokio runtime dependencies ([`TokioDeps`]), the transport-specific
//! tuning knobs ([`TokioTransportOptions`]), and the factory methods that produce HTTP clients
//! backed by the Tokio runtime and the
//! [`fetch_hyper`] transport. They are gated behind the `tokio` feature combined with a
//! TLS backend (`rustls` and/or `native-tls`).

use anyspawn::Spawner;
use fetch_hyper::HyperTransportBuilder;
use fetch_options::{SocketOptions, TransportOptions};
use fetch_tls::{TlsBackend, TlsBackendBuilder};
use http::uri::Scheme;
use http_extensions::{HttpError, Result};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioIo;
use seatbelt::RecoveryInfo;
use templated_uri::BaseUri;
use thread_aware::ThreadAware;
use tick::Clock;
use tower_service::Service as _;

use crate::custom::{CustomContext, CustomDeps, Isolation};
use crate::error_labels::LABEL_SCHEME_NOT_ALLOWED;
use crate::handlers::TransportHandler;
use crate::tls::TlsOptions;
use crate::{HttpClient, HttpClientBuilder};

/// Configuration dependencies for Tokio runtime HTTP operations.
///
/// Contains the necessary dependencies for HTTP client operations in a Tokio
/// environment, including clock access and memory management.
#[derive(Debug, Clone, ThreadAware)]
#[fundle::deps]
pub struct TokioDeps {
    /// Clock for timing operations and timeouts.
    pub clock: Clock,
    /// Memory pool for usage-neutral memory allocations.
    pub global_pool: bytesbuf::mem::GlobalPool,
}

impl Default for TokioDeps {
    fn default() -> Self {
        Self::with_clock(&Clock::new_tokio())
    }
}

impl TokioDeps {
    /// Creates `TokioDeps` with the given clock and a dedicated HTTP-client memory pool.
    #[must_use]
    pub fn with_clock(clock: &Clock) -> Self {
        Self {
            global_pool: bytesbuf::mem::GlobalPool::new(),
            clock: clock.clone(),
        }
    }
}

/// Tuning knobs specific to the Tokio transport.
///
/// These settings are deliberately *not* part of
/// [`TransportOptions`], because they describe how this
/// transport dials `TCP` sockets rather than a policy every transport can honor. A transport
/// that does not own its sockets (`WinHTTP`, for instance) has no way to apply them, so
/// accepting them on the shared, transport-agnostic surface would silently ignore them.
///
/// Pass an instance to [`HttpClient::builder_tokio_with_options`] to apply it.
///
/// # Examples
///
/// ```
/// use fetch::options::SocketOptions;
/// use fetch::tokio::TokioTransportOptions;
///
/// let options = TokioTransportOptions::default().socket(
///     SocketOptions::default()
///         .no_delay(true)
///         .send_buffer_size(256 * 1024),
/// );
/// ```
// `Copy` is deliberately not derived: this type exists to grow, and the first knob that is
// not a plain scalar would force its removal. `Http2Options` and `ConnectionPoolOptions` are
// `Clone`-only for the same reason.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TokioTransportOptions {
    /// Socket-level tuning applied to every outbound `TCP` connection.
    pub socket: SocketOptions,
}

impl TokioTransportOptions {
    /// Sets the socket-level tuning applied to every outbound `TCP` connection.
    ///
    /// # Examples
    ///
    /// ```
    /// use fetch::options::SocketOptions;
    /// use fetch::tokio::TokioTransportOptions;
    ///
    /// let options = TokioTransportOptions::default().socket(SocketOptions::default().no_delay(true));
    /// assert_eq!(options.socket.no_delay, Some(true));
    /// ```
    #[must_use]
    pub fn socket(mut self, socket: SocketOptions) -> Self {
        self.socket = socket;
        self
    }
}

impl HttpClient {
    /// Creates a new HTTP client builder for the Tokio runtime.
    ///
    /// This factory method provides a builder specifically configured for Tokio.
    /// Use this when working with Tokio-based applications.
    ///
    /// Transport-specific tuning is left at its defaults; use
    /// [`builder_tokio_with_options`][Self::builder_tokio_with_options] to supply
    /// [`TokioTransportOptions`].
    ///
    /// Available only when compiled with the `tokio` feature and a TLS backend
    /// (`rustls` and/or `native-tls`).
    pub fn builder_tokio(deps: impl Into<TokioDeps>) -> HttpClientBuilder {
        Self::builder_tokio_with_options(deps, TokioTransportOptions::default())
    }

    /// Creates a new HTTP client builder for the Tokio runtime with transport-specific tuning.
    ///
    /// Identical to [`builder_tokio`][Self::builder_tokio], except that `options` carries the
    /// knobs only this transport can honor. Everything expressible on every transport stays on
    /// [`HttpClientBuilder`].
    ///
    /// Available only when compiled with the `tokio` feature and a TLS backend
    /// (`rustls` and/or `native-tls`).
    ///
    /// # Examples
    ///
    /// ```
    /// # use fetch::HttpClient;
    /// # use fetch::options::SocketOptions;
    /// # use fetch::tokio::{TokioDeps, TokioTransportOptions};
    /// # fn build() -> fetch::HttpClientBuilder {
    /// HttpClient::builder_tokio_with_options(
    ///     TokioDeps::default(),
    ///     TokioTransportOptions::default().socket(SocketOptions::default().no_delay(true)),
    /// )
    /// # }
    /// ```
    pub fn builder_tokio_with_options(deps: impl Into<TokioDeps>, options: TokioTransportOptions) -> HttpClientBuilder {
        let deps = deps.into();
        let clock = deps.clock.clone();
        let global_pool = deps.global_pool.clone();

        // Re-layer on top of the in-crate `builder_custom_internal` path: the
        // full `TokioDeps` rides through `CustomDeps::extras` so that the
        // per-slot factory has the same data it had with the previous direct
        // transport factory call.
        Self::builder_custom_internal(
            crate::constants::TOKIO_RUNTIME_NAME,
            crate::constants::HYPER_TRANSPORT_NAME,
            tokio_transport_factory(options),
            Isolation::Shared,
            CustomDeps {
                clock,
                global_pool,
                extras: deps,
            },
        )
    }

    /// Creates a new HTTP client for the Tokio runtime.
    ///
    /// This method creates a fully configured HTTP client instance with the default
    /// configuration. Use [`builder_tokio`][Self::builder_tokio] if you want to customize the
    /// client (e.g. supply a custom [`TokioDeps`]) before creating it.
    ///
    /// Available only when compiled with the `tokio` feature and a TLS backend
    /// (`rustls` and/or `native-tls`).
    #[must_use]
    pub fn new_tokio() -> Self {
        Self::builder_tokio(TokioDeps::default()).build()
    }
}

/// Plain-TCP connector for the Tokio transport.
///
/// Named pipes / Unix-domain sockets are intentionally not supported; the
/// connector opens a TCP stream to the request authority and hands the wrapped
/// stream to hyper. TLS, when required, is layered on top by the transport.
///
/// The actual dialing is delegated to hyper's own [`HttpConnector`], which already
/// implements name resolution, Happy Eyeballs (RFC 8305) IPv4/IPv6 racing, and
/// pre-connect application of the socket buffer sizes. This type only translates
/// [`BaseUri`] into an [`http::Uri`] and [`SocketOptions`] into hyper's setters.
///
/// Applying the socket options is best-effort: hyper logs a warning and proceeds when the
/// kernel rejects a requested value, so a connection is never failed over a tuning knob.
///
/// Keepalive, local-address binding, `SO_REUSEADDR` and `TCP_USER_TIMEOUT` are also exposed by
/// [`HttpConnector`] but are deliberately left at its defaults, because [`SocketOptions`] does
/// not model them yet.
#[derive(Clone, Debug)]
struct TokioConnector {
    connector: HttpConnector,
}

#[cfg(test)]
thread_local! {
    /// Records the options handed to the most recent [`TokioConnector`] built on this thread.
    ///
    /// `HttpConnector` exposes no way to read its configuration back, and the connector is
    /// buried inside the transport once built, so this is the only way to assert that the
    /// options given to [`HttpClient::builder_tokio_with_options`] actually reach the
    /// connector instead of being silently dropped.
    ///
    /// Deliberately thread-local rather than a `static Mutex`: every test that builds a Tokio
    /// client constructs a connector, so a process-global recorder would be written
    /// concurrently by unrelated tests and make the assertions below flaky. Thread affinity is
    /// what keeps each test's observations its own.
    ///
    /// The cost of that choice is an invariant for readers to preserve: a test must reset,
    /// build, and assert without crossing a thread. Keep those steps free of `.await` and off
    /// a `flavor = "multi_thread"` runtime, or a migrated task will read `None` and report a
    /// missing option that was in fact delivered.
    static LAST_SOCKET_OPTIONS: std::cell::Cell<Option<SocketOptions>> = const { std::cell::Cell::new(None) };
}

impl TokioConnector {
    /// Builds a connector that applies `options` to every connection it opens.
    fn new(options: SocketOptions) -> Self {
        #[cfg(test)]
        LAST_SOCKET_OPTIONS.set(Some(options));

        let mut connector = HttpConnector::new();

        // TLS is layered on top of this connector by the transport, so `https` targets reach
        // us with their original scheme and `enforce_http(true)` would reject every one of
        // them. The scheme is instead checked explicitly in `execute`.
        connector.enforce_http(false);

        // hyper takes a plain `bool` here. `None` means "use the operating system default",
        // and that default is Nagle enabled on every supported platform, which is exactly
        // what `false` requests.
        connector.set_nodelay(options.no_delay.unwrap_or(false));

        // The effective accessors are used rather than the public fields because the fields
        // can be assigned directly, bypassing the range clamping.
        connector.set_send_buffer_size(options.effective_send_buffer_size().map(to_usize));
        connector.set_recv_buffer_size(options.effective_receive_buffer_size().map(to_usize));

        // The connect budget is deliberately left unset: `ClientConnector` wraps this
        // whole future in `connect_timeout`, so resolution and every connect attempt
        // already share one deadline.

        Self { connector }
    }
}

impl layered::Service<BaseUri> for TokioConnector {
    type Out = Result<TokioIo<::tokio::net::TcpStream>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        // `RequestFilter::HttpAndHttps` admits any scheme, and `enforce_http(false)` disables
        // hyper's own check, so this is the only guard stopping a `ftp://` target from being
        // dialed and then spoken HTTP to.
        let scheme = input.origin().scheme();
        if scheme != &Scheme::HTTP && scheme != &Scheme::HTTPS {
            return Err(HttpError::other(
                "the connector only supports the http and https schemes",
                RecoveryInfo::never(),
                LABEL_SCHEME_NOT_ALLOWED,
            ));
        }

        // hyper derives the port from the scheme when the authority omits it. Resolving the
        // port here instead keeps `templated_uri`'s behavior as the single source of truth.
        let port = input.try_effective_port()?;
        let uri = http::Uri::from(input.with_port(port));

        // `HttpConnector` is a `tower` service, so it needs `&mut self`. Cloning is cheap:
        // the configuration sits behind an `Arc` and the resolver is stateless.
        let mut connector = self.connector.clone();

        std::future::poll_fn(|cx| connector.poll_ready(cx))
            .await
            .map_err(map_connect_error)?;

        connector.call(uri).await.map_err(map_connect_error)
    }
}

/// Widens a socket buffer size for hyper's `usize`-typed setters.
///
/// Sizes are clamped to [`MAX_SOCKET_BUFFER_SIZE`][fetch_options::MAX_SOCKET_BUFFER_SIZE] by the
/// caller, so the saturating fallback is unreachable on any target with a pointer at least 32
/// bits wide.
#[inline]
fn to_usize(size: u32) -> usize {
    usize::try_from(size).unwrap_or(usize::MAX)
}

/// Converts a connector failure into an [`HttpError`].
///
/// hyper's connect error carries only a static summary message, so the underlying
/// [`std::io::Error`] is recovered from the source chain first. That keeps the error label and
/// the retry classification identical to a directly propagated I/O failure, which the transport
/// relies on to decide whether a connection attempt may be retried.
///
/// This is generic rather than taking hyper's error type because that type is not publicly
/// nameable; it is only ever instantiated with it.
fn map_connect_error<E: std::error::Error + Send + Sync + 'static>(error: E) -> HttpError {
    let kind = io_error_kind(&error);
    std::io::Error::new(kind, error).into()
}

/// Finds the [`std::io::ErrorKind`] of the first [`std::io::Error`] in `error`'s source chain.
///
/// Falls back to [`std::io::ErrorKind::Other`], which [`RecoveryInfo`] classifies as never
/// recoverable. Every hyper connect error without an I/O cause reports a malformed target
/// (missing scheme, missing host), which is permanent, so suppressing retries is the correct
/// classification rather than merely the safe one.
fn io_error_kind(mut error: &(dyn std::error::Error + 'static)) -> std::io::ErrorKind {
    loop {
        if let Some(io_error) = error.downcast_ref::<std::io::Error>() {
            return io_error.kind();
        }

        match error.source() {
            Some(source) => error = source,
            None => return std::io::ErrorKind::Other,
        }
    }
}

/// Builds the per-pool-slot transport factory for the Tokio transport.
///
/// `options` is captured by value and shared by reference with every handler the client
/// builds, so each pool slot on every core sees the configuration the caller supplied rather
/// than a default. Extracted from [`HttpClient::builder_tokio_with_options`] so that
/// propagation is testable without standing up a full client.
fn tokio_transport_factory(
    options: TokioTransportOptions,
) -> impl Fn(CustomContext<TokioDeps>) -> TransportHandler + Send + Sync + 'static {
    move |cx| TransportHandler(build_tokio_handler(cx, &options).into())
}

fn build_tokio_handler(cx: CustomContext<TokioDeps>, options: &TokioTransportOptions) -> fetch_hyper::HyperTransport {
    let tls_backend = build_tls_backend(&cx.options, cx.tls);
    let connector = TokioConnector::new(options.socket);

    HyperTransportBuilder::new(connector, Spawner::new_tokio(), cx.clock, cx.options)
        .body_builder(cx.body_builder)
        .pool_index(cx.pool_index)
        .meter(cx.meter)
        .build(tls_backend)
}

/// Materializes the client's [`TlsOptions`] into a concrete [`TlsBackend`].
///
/// When the `rustls` feature is enabled, rustls is wired up with the aws-lc-rs
/// crypto provider and the platform certificate verifier (the OS trust store),
/// and rustls becomes the default backend. When only `native-tls` is enabled it
/// becomes the default backend instead.
fn build_tls_backend(options: &TransportOptions, tls: TlsOptions) -> TlsBackend {
    let mut builder = TlsBackendBuilder::new();
    if !options.supported_http_versions.is_empty() {
        builder = builder.supported_http_versions(&options.supported_http_versions);
    }

    #[cfg(any(feature = "rustls", test))]
    {
        // aws-lc-rs is the default crypto provider when rustls is enabled.
        let provider = std::sync::Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
        let verifier = std::sync::Arc::new(
            rustls_platform_verifier::Verifier::new(std::sync::Arc::clone(&provider))
                .expect("the platform certificate verifier must initialize with the aws-lc-rs crypto provider"),
        );
        // `configure_rustls` auto-promotes rustls to the default backend.
        builder = builder.configure_rustls(provider, verifier);
    }

    #[cfg(all(feature = "native-tls", not(any(feature = "rustls", test))))]
    {
        builder = builder.defaults_to_native_tls();
    }

    // `build_backend` is fallible (invalid client identity material, missing
    // backend configuration), but `build()` on the transport is infallible. Any
    // failure here reflects a misconfigured `TlsOptions` supplied by the caller,
    // which is a programming error surfaced eagerly at client construction.
    builder
        .build_backend(tls)
        .expect("TLS backend construction must succeed for the configured TlsOptions")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::StatusCode;
    use http_extensions::FakeHandler;
    use thread_aware::ThreadAware;
    use thread_aware::affinity::pinned_affinities;
    use tick::Clock;

    use super::TokioDeps;
    use crate::pipeline::Pipeline;
    use crate::{HttpClient, HttpResponseBuilder};

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_builder_tokio() {
        let clock = Clock::new_tokio();
        let client = HttpClient::builder_tokio(TokioDeps::with_clock(&clock)).minimal_pipeline().build();

        assert!(matches!(client.pipeline(), Pipeline::Minimal(_)));

        if let Pipeline::Minimal(dispatch) = client.pipeline() {
            assert!(matches!(dispatch.mode, crate::handlers::DispatchMode::Single(_)));
        }
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn builder_tokio_with_options_propagates_options_to_every_pool_slot() {
        use opentelemetry::metrics::MeterProvider as _;
        use opentelemetry_sdk::metrics::SdkMeterProvider;

        use crate::custom::CustomContext;
        use crate::options::{ClientOptions, PoolIndex};
        use crate::tls::TlsOptions;

        // This is the invariant the relocation introduces: the socket knobs no longer ride on
        // the shared `TransportOptions`, so they reach the connector only if the transport
        // factory carries them. `HttpConnector` cannot be read back, so `TokioConnector::new`
        // records what it was handed and this asserts on that.
        let clock = Clock::new_tokio();
        let deps = TokioDeps::with_clock(&clock);
        let expected = fetch_options::SocketOptions::default().no_delay(true).send_buffer_size(64 * 1024);
        let factory = super::tokio_transport_factory(super::TokioTransportOptions::default().socket(expected));

        let provider = SdkMeterProvider::builder().build();
        let client_options = ClientOptions::default();

        // Two slots, as a multi-pool client would build: the factory must not consume its
        // configuration on first use.
        for slot in [0, 1] {
            super::LAST_SOCKET_OPTIONS.set(None);

            let cx = CustomContext {
                body_builder: crate::custom::create_body_builder(&deps.global_pool, &clock, &client_options),
                clock: clock.clone(),
                pool_index: PoolIndex::new(slot),
                extras: deps.clone(),
                options: client_options.transport.clone(),
                tls: TlsOptions::default(),
                meter: provider.meter("test"),
            };

            let _handler = factory(cx);

            assert_eq!(
                super::LAST_SOCKET_OPTIONS.get(),
                Some(expected),
                "slot {slot} did not receive the configured socket options"
            );
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn to_usize_widens_without_altering_the_value() {
        assert_eq!(super::to_usize(0), 0);
        assert_eq!(super::to_usize(1), 1);
        assert_eq!(super::to_usize(fetch_options::MAX_SOCKET_BUFFER_SIZE), 67_108_864);
        assert_eq!(super::to_usize(u32::MAX), 4_294_967_295);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn tokio_transport_options_default() {
        insta::assert_debug_snapshot!(super::TokioTransportOptions::default());
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn configure_tokio_transport_options() {
        let options = super::TokioTransportOptions::default().socket(fetch_options::SocketOptions::default().no_delay(true));

        assert_eq!(options.socket.no_delay, Some(true));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn assert_tokio_transport_options_type() {
        static_assertions::assert_impl_all!(
            super::TokioTransportOptions: Send,
            Sync,
            Clone,
            std::fmt::Debug,
            Default
        );
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn test_new_tokio() {
        let clock = Clock::new_tokio();
        let client = HttpClient::builder_tokio(TokioDeps::with_clock(&clock)).build();

        assert!(client.pipeline().is_standard());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn new_tokio_uses_default_deps() {
        // `new_tokio` builds the client from `TokioDeps::default()`, exercising the default
        // dependency wiring (including `Clock::new_tokio`) and the standard pipeline.
        let client = HttpClient::new_tokio();

        assert!(client.pipeline().is_standard());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn tokio_client_works_after_relocation() {
        let affinities = pinned_affinities(&[2]);
        let clock = Clock::new_tokio();

        let mut client = HttpClient::builder_tokio(TokioDeps::with_clock(&clock))
            .custom_pipeline(|_root, _ctx| FakeHandler::from_fn(|_request| HttpResponseBuilder::new_fake().status(StatusCode::OK).build()))
            .build();

        // Verify the client works before relocation.
        let response = client.get("https://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Relocate the client to a different affinity.
        client.relocate(None, affinities[0]);

        // Verify the relocated client still serves requests correctly.
        let response = client.get("https://example.com/after-relocation").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn build_tls_backend_skips_empty_supported_http_versions() {
        use fetch_options::TransportOptions;

        use crate::tls::TlsOptions;

        // An empty `supported_http_versions` means "no preference", so
        // `build_tls_backend` must leave the builder's own default versions in place.
        // It must NOT forward the empty list to `TlsBackendBuilder::supported_http_versions`,
        // which panics on an empty slice. The `!is_empty()` guard is what prevents that
        // panic; without it, materializing the backend here panics.
        let mut options = TransportOptions::default();
        options.supported_http_versions = Vec::new();

        // Must not panic: the empty list has to be skipped, not forwarded.
        let _backend = super::build_tls_backend(&options, TlsOptions::default());
    }

    /// Dials `127.0.0.1:port` over `http` through a [`TokioConnector`] configured with `options`.
    async fn connect_with(options: fetch_options::SocketOptions, port: u16) -> http_extensions::Result<tokio::net::TcpStream> {
        connect_to(options, &format!("http://127.0.0.1:{port}")).await
    }

    /// Dials `uri` through a [`TokioConnector`] configured with `options`.
    async fn connect_to(options: fetch_options::SocketOptions, uri: &str) -> http_extensions::Result<tokio::net::TcpStream> {
        use layered::Service as _;

        let base_uri = templated_uri::BaseUri::try_from(uri).unwrap();

        super::TokioConnector::new(options)
            .execute(base_uri)
            .await
            .map(hyper_util::rt::TokioIo::into_inner)
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_connects_with_default_options() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Default options request no tuning at all, so the operating system defaults apply.
        let stream = connect_with(SocketOptions::default(), port).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_applies_no_delay() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // `no_delay` is applied to the connected stream, so it is observable on the result.
        let stream = connect_with(SocketOptions::default().no_delay(true), port).await.unwrap();
        assert!(stream.nodelay().unwrap());

        let stream = connect_with(SocketOptions::default().no_delay(false), port).await.unwrap();
        assert!(!stream.nodelay().unwrap());

        // `None` leaves the operating system default in place, which keeps Nagle enabled.
        let stream = connect_with(SocketOptions::default(), port).await.unwrap();
        assert!(!stream.nodelay().unwrap());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_connects_with_all_options_set() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // The kernel is free to clamp or scale the requested buffer sizes, and hyper only warns
        // when it rejects one outright, so the sizes are not observable on the result. What is
        // asserted is that requesting them does not prevent the connection from being made.
        let options = SocketOptions::default()
            .receive_buffer_size(64 * 1024)
            .send_buffer_size(64 * 1024)
            .no_delay(true);

        let stream = connect_with(options, port).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);
        assert!(stream.nodelay().unwrap());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_clamps_directly_assigned_buffer_sizes() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // The fields are public, so a caller can bypass the builder's clamping. Zero in
        // particular must not reach the kernel: on Windows it disables send buffering outright.
        let mut options = SocketOptions::default();
        options.send_buffer_size = Some(0);
        options.receive_buffer_size = Some(0);

        let stream = connect_with(options, port).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_accepts_https_targets() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // TLS is layered on top of this connector by the transport, so `https` targets arrive
        // with their original scheme and must still be dialed as plain TCP. This is the only
        // reason `enforce_http(false)` is set; flipping it back would break every HTTPS request.
        let stream = connect_to(SocketOptions::default(), &format!("https://127.0.0.1:{port}"))
            .await
            .unwrap();

        assert_eq!(stream.peer_addr().unwrap().port(), port);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_rejects_non_http_schemes() {
        use ohno::Labeled as _;
        use seatbelt::{Recovery as _, RecoveryKind};

        // `RequestFilter::HttpAndHttps` admits any scheme and `enforce_http(false)` disables
        // hyper's own check, so without the explicit guard an `ftp://` target would be dialed
        // and then spoken HTTP to. The port is given explicitly so that the request is rejected
        // by the scheme guard rather than incidentally by the missing-default-port check.
        let error = connect_to(fetch_options::SocketOptions::default(), "ftp://127.0.0.1:21")
            .await
            .unwrap_err();

        assert_eq!(error.label().as_str(), "scheme_not_allowed");
        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_surfaces_connect_failure_as_recoverable_io_error() {
        use seatbelt::{Recovery as _, RecoveryInfo};

        // Binding then dropping the listener yields a port nothing is listening on, so the
        // connect fails. The port could in principle be re-bound by another process before the
        // connect, but the window is a few microseconds inside one test.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let error = connect_with(fetch_options::SocketOptions::default().send_buffer_size(64 * 1024), port)
            .await
            .unwrap_err();

        // hyper's own `Display` is only a static summary, so the operating system detail has to
        // survive in the source chain for the message to be diagnosable at all.
        assert!(
            std::error::Error::source(&error).is_some(),
            "the underlying I/O cause must be preserved, got: {error}"
        );
        assert_eq!(
            error.recovery().kind(),
            RecoveryInfo::from(std::io::ErrorKind::ConnectionRefused).kind(),
            "the refused connection must keep the recovery classification of a plain I/O error"
        );
    }

    /// An error that is not itself an [`std::io::Error`] but wraps one, mirroring the shape of
    /// hyper's connect error.
    #[derive(Debug)]
    struct WrappingError(std::io::Error);

    impl std::fmt::Display for WrappingError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("tcp connect error")
        }
    }

    impl std::error::Error for WrappingError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn io_error_kind_walks_the_source_chain() {
        // hyper's connect error is not an `io::Error` and only carries a static message, so the
        // kind has to be recovered from its source rather than from the outermost error.
        let wrapped = WrappingError(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"));
        assert_eq!(super::io_error_kind(&wrapped), std::io::ErrorKind::TimedOut);

        // An error that is itself an I/O error is matched without walking any further.
        let direct = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        assert_eq!(super::io_error_kind(&direct), std::io::ErrorKind::ConnectionRefused);

        // An error chain without any I/O error falls back to the conservative default.
        let opaque = http::Uri::try_from("::not a uri::").unwrap_err();
        assert_eq!(super::io_error_kind(&opaque), std::io::ErrorKind::Other);
    }
}
