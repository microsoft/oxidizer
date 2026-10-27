// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tokio-runtime entry points for [`HttpClient`].
//!
//! [`TokioDeps`][crate::tokio::TokioDeps] contains runtime dependencies.
//! [`TokioTransportOptions`][crate::tokio::TokioTransportOptions] contains transport-specific
//! options. The factory methods create clients that use Tokio and [`fetch_hyper`].
//!
//! These APIs require the `tokio` feature and at least one TLS backend: `rustls` or `native-tls`.

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

/// Dependencies for HTTP operations on Tokio.
///
/// It contains the clock and memory pool.
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

/// Options specific to the Tokio transport.
///
/// These options control the `TCP` sockets that the bundled transport opens. They are separate
/// from [`TransportOptions`] because a transport that does not own sockets cannot apply them.
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
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TokioTransportOptions {
    /// Socket options for each outbound `TCP` connection.
    pub socket: SocketOptions,
}

impl TokioTransportOptions {
    /// Sets the socket options for each outbound `TCP` connection.
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
    /// Creates an HTTP client builder for Tokio.
    ///
    /// This method uses the default transport-specific options. Use
    /// [`builder_tokio_with_options`][Self::builder_tokio_with_options] to pass
    /// [`TokioTransportOptions`].
    ///
    /// Available only when compiled with the `tokio` feature and a TLS backend
    /// (`rustls` and/or `native-tls`).
    pub fn builder_tokio(deps: impl Into<TokioDeps>) -> HttpClientBuilder {
        Self::builder_tokio_with_options(deps, TokioTransportOptions::default())
    }

    /// Creates an HTTP client builder for Tokio with transport-specific options.
    ///
    /// `options` contains settings that only this transport can apply. Shared settings remain on
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

        Self::builder_custom_internal(
            crate::constants::TOKIO_RUNTIME_NAME,
            crate::constants::HYPER_TRANSPORT_NAME,
            move |cx| TransportHandler(build_tokio_handler(cx, &options).into()),
            Isolation::Shared,
            CustomDeps {
                clock,
                global_pool,
                extras: deps,
            },
        )
    }

    /// Creates an HTTP client for Tokio.
    ///
    /// This method uses the default configuration. Use [`builder_tokio`][Self::builder_tokio] to
    /// change the configuration or pass custom [`TokioDeps`].
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
/// Opens a TCP stream to the request authority. The transport adds TLS.
///
/// [`HttpConnector`] performs name resolution and RFC 6555 address-family fallback for hosts with
/// IPv4 and IPv6 addresses. It starts the fallback family after its delay, or immediately after the
/// preferred family fails. This type converts [`BaseUri`] into [`http::Uri`] and applies
/// [`SocketOptions`] through hyper.
///
/// Hyper logs rejected socket options but does not fail the connection.
#[derive(Clone, Debug)]
struct TokioConnector {
    connector: HttpConnector,
}

impl TokioConnector {
    /// Builds a connector that attempts `options` for every connection it opens.
    fn new(options: SocketOptions) -> Self {
        let mut connector = HttpConnector::new();

        // The transport adds TLS after this connector. HTTPS targets retain their scheme, so
        // enforce_http(true) rejects them. execute validates the scheme instead.
        connector.enforce_http(false);

        if let Some(no_delay) = options.no_delay {
            connector.set_nodelay(no_delay);
        }

        connector.set_send_buffer_size(options.send_buffer_size.map(widen_socket_buffer_size));
        connector.set_recv_buffer_size(options.receive_buffer_size.map(widen_socket_buffer_size));

        // ClientConnector applies one connect_timeout to resolution and all connection attempts.

        Self { connector }
    }
}

impl layered::Service<BaseUri> for TokioConnector {
    type Out = Result<TokioIo<::tokio::net::TcpStream>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        // RequestFilter::HttpAndHttps admits any scheme. Because enforce_http(false) disables the
        // hyper guard, this guard rejects FTP before dialing.
        let scheme = input.origin().scheme();
        if scheme != &Scheme::HTTP && scheme != &Scheme::HTTPS {
            return Err(HttpError::other(
                format!("the connector supports only http and https, not '{scheme}'"),
                RecoveryInfo::never(),
                LABEL_SCHEME_NOT_ALLOWED,
            ));
        }

        // Resolve the port through templated_uri instead of the hyper scheme defaults.
        let port = input.try_effective_port()?;
        let uri = http::Uri::from(input.with_port(port));

        // HttpConnector needs &mut self. Its clone shares configuration and has no resolver state.
        let mut connector = self.connector.clone();

        std::future::poll_fn(|cx| connector.poll_ready(cx))
            .await
            .map_err(map_connect_error)?;

        connector.call(uri).await.map_err(map_connect_error)
    }
}

/// Widens a socket buffer size for hyper's `usize`-typed setters.
#[inline]
fn widen_socket_buffer_size(size: u32) -> usize {
    usize::try_from(size).expect("fetch supports only targets with pointers at least 32 bits wide")
}

/// Converts a connector failure into an [`HttpError`].
///
/// A hyper connect error contains only a static summary. This function reads the underlying
/// [`std::io::Error`] from the source chain. The resulting label and retry classification then
/// match a directly propagated I/O error.
///
/// This function is generic because hyper does not expose a name for its error type. It is called
/// only with that type.
fn map_connect_error<E: std::error::Error + Send + Sync + 'static>(error: E) -> HttpError {
    let kind = io_error_kind(&error);
    std::io::Error::new(kind, error).into()
}

/// Finds the [`std::io::ErrorKind`] of the first [`std::io::Error`] in `error`'s source chain.
///
/// If the chain has no I/O error, returns [`std::io::ErrorKind::Other`].
/// [`RecoveryInfo`] classifies it as never recoverable. A hyper connect error without an I/O cause
/// reports a permanent malformed target, such as a missing scheme or host.
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

fn build_tokio_handler(cx: CustomContext<TokioDeps>, options: &TokioTransportOptions) -> fetch_hyper::HyperTransport {
    let tls_backend = build_tls_backend(&cx.options, cx.tls);
    let connector = TokioConnector::new(options.socket);

    HyperTransportBuilder::new(connector, Spawner::new_tokio(), cx.clock, cx.options)
        .body_builder(cx.body_builder)
        .pool_index(cx.pool_index)
        .meter(cx.meter)
        .build(tls_backend)
}

/// Builds a [`TlsBackend`] from the client [`TlsOptions`].
///
/// With `rustls`, the backend uses aws-lc-rs and the platform trust store. rustls is the default.
/// If only `native-tls` is enabled, native TLS is the default.
fn build_tls_backend(options: &TransportOptions, tls: TlsOptions) -> TlsBackend {
    let mut builder = TlsBackendBuilder::new();
    if !options.supported_http_versions.is_empty() {
        builder = builder.supported_http_versions(&options.supported_http_versions);
    }

    #[cfg(any(feature = "rustls", test))]
    {
        // rustls uses aws-lc-rs by default.
        let provider = std::sync::Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
        let verifier = std::sync::Arc::new(
            rustls_platform_verifier::Verifier::new(std::sync::Arc::clone(&provider))
                .expect("the platform certificate verifier must initialize with the aws-lc-rs crypto provider"),
        );
        // configure_rustls makes rustls the default backend.
        builder = builder.configure_rustls(provider, verifier);
    }

    #[cfg(all(feature = "native-tls", not(any(feature = "rustls", test))))]
    {
        builder = builder.defaults_to_native_tls();
    }

    // Transport build is infallible, so invalid TlsOptions are a programming error reported
    // during client construction.
    builder
        .build_backend(tls)
        .expect("TLS backend construction must succeed for the configured TlsOptions")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::StatusCode;
    use http_extensions::FakeHandler;
    use thread_aware::Relocator;
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
    fn socket_buffer_sizes_are_widened_verbatim() {
        assert_eq!(super::widen_socket_buffer_size(0), 0);
        assert_eq!(super::widen_socket_buffer_size(1), 1);
        assert_eq!(super::widen_socket_buffer_size(u32::MAX), u32::MAX as usize);
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
        // new_tokio uses TokioDeps::default, Clock::new_tokio, and the standard pipeline.
        let client = HttpClient::new_tokio();

        assert!(client.pipeline().is_standard());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn tokio_client_works_after_relocation() {
        let clock = Clock::new_tokio();

        let mut client = HttpClient::builder_tokio(TokioDeps::with_clock(&clock))
            .custom_pipeline(|_root, _ctx| FakeHandler::from_fn(|_request| HttpResponseBuilder::new_fake().status(StatusCode::OK).build()))
            .build();

        // Send a request before relocation.
        let response = client.get("https://example.com").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Move the client to another thread coordinate.
        _ = Relocator::between_threads().source(false).relocate(&mut client);

        // Send a request after relocation.
        let response = client.get("https://example.com/after-relocation").fetch().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn build_tls_backend_skips_empty_supported_http_versions() {
        use fetch_options::TransportOptions;

        use crate::tls::TlsOptions;

        // An empty supported_http_versions list means no preference. Do not pass it to
        // TlsBackendBuilder::supported_http_versions because that method panics on an empty slice.
        let mut options = TransportOptions::default();
        options.supported_http_versions = Vec::new();

        // Skip the empty list.
        let _backend = super::build_tls_backend(&options, TlsOptions::default());
    }

    /// Dials `127.0.0.1:port` over `http` through a [`TokioConnector`] configured with `options`.
    async fn connect_with(options: fetch_options::SocketOptions, port: u16) -> http_extensions::Result<tokio::net::TcpStream> {
        connect_to(options, &format!("http://127.0.0.1:{port}")).await
    }

    /// Dials `uri` through a [`TokioConnector`] configured with `options`.
    async fn connect_to(options: fetch_options::SocketOptions, uri: &str) -> http_extensions::Result<tokio::net::TcpStream> {
        use layered::Service as _;

        let base_uri = templated_uri::BaseUri::try_from(uri)?;

        super::TokioConnector::new(options)
            .execute(base_uri)
            .await
            .map(hyper_util::rt::TokioIo::into_inner)
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_connects_with_default_options() -> http_extensions::Result<()> {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();

        // Default options request no tuning and leave the effective behavior to lower layers.
        let stream = connect_with(SocketOptions::default(), port).await?;
        assert_eq!(stream.peer_addr()?.port(), port);
        Ok(())
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_applies_no_delay() -> http_extensions::Result<()> {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();

        // `no_delay` is applied to the connected stream, so it is observable on the result.
        let stream = connect_with(SocketOptions::default().no_delay(true), port).await?;
        assert!(stream.nodelay()?);

        let stream = connect_with(SocketOptions::default().no_delay(false), port).await?;
        assert!(!stream.nodelay()?);

        Ok(())
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_connects_with_all_options_set() -> http_extensions::Result<()> {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();

        // The kernel is free to clamp or scale the requested buffer sizes, and hyper only warns
        // when it rejects one outright, so the sizes are not observable on the result. What is
        // asserted is that requesting them does not prevent the connection from being made.
        let options = SocketOptions::default()
            .receive_buffer_size(64 * 1024)
            .send_buffer_size(64 * 1024)
            .no_delay(true);

        let stream = connect_with(options, port).await?;
        assert_eq!(stream.peer_addr()?.port(), port);
        assert!(stream.nodelay()?);
        Ok(())
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_accepts_https_targets() -> http_extensions::Result<()> {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();

        // The transport adds TLS after this connector. HTTPS targets retain their scheme but this
        // connector must open plain TCP. Therefore, enforce_http(false) is required.
        let stream = connect_to(SocketOptions::default(), &format!("https://127.0.0.1:{port}")).await?;

        assert_eq!(stream.peer_addr()?.port(), port);
        Ok(())
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_rejects_non_http_schemes() -> http_extensions::Result<()> {
        use ohno::Labeled as _;
        use seatbelt::{Recovery as _, RecoveryKind};

        // RequestFilter::HttpAndHttps admits any scheme and enforce_http(false) disables the hyper
        // check. The explicit guard rejects FTP before dialing. The explicit port makes the scheme
        // guard, not default-port resolution, reject the request.
        let error = connect_to(fetch_options::SocketOptions::default(), "ftp://127.0.0.1:21")
            .await
            .expect_err("the connector rejects every non-HTTP scheme before dialing");

        assert_eq!(error.label().as_str(), "scheme_not_allowed");
        assert!(
            error.to_string().contains("'ftp'"),
            "the diagnostic must identify the rejected scheme: {error}"
        );
        assert_eq!(error.recovery().kind(), RecoveryKind::Never);
        Ok(())
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connector_surfaces_connect_failure_as_recoverable_io_error() -> http_extensions::Result<()> {
        use seatbelt::{Recovery as _, RecoveryInfo};

        // Drop the listener so the connection fails. Another process can bind the port during the
        // short interval before the connection, but that interval is only a few microseconds.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let error = connect_with(fetch_options::SocketOptions::default().send_buffer_size(64 * 1024), port)
            .await
            .expect_err("the listener was dropped before dialing, so the connection must be refused");

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
        Ok(())
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
