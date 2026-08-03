// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tokio-runtime entry points for [`HttpClient`].
//!
//! This module groups the Tokio runtime dependencies ([`TokioDeps`]) and the
//! factory methods that produce HTTP clients backed by the Tokio runtime and the
//! [`fetch_hyper`] transport. They are gated behind the `tokio` feature combined with a
//! TLS backend (`rustls` and/or `native-tls`).

use anyspawn::Spawner;
use fetch_hyper::HyperTransportBuilder;
use fetch_options::{SocketOptions, TransportOptions};
use fetch_tls::{TlsBackend, TlsBackendBuilder};
use http_extensions::Result;
use hyper_util::rt::TokioIo;
use templated_uri::BaseUri;
use thread_aware::ThreadAware;
use tick::Clock;

use crate::custom::{CustomContext, CustomDeps, Isolation};
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

impl HttpClient {
    /// Creates a new HTTP client builder for the Tokio runtime.
    ///
    /// This factory method provides a builder specifically configured for Tokio.
    /// Use this when working with Tokio-based applications.
    ///
    /// Available only when compiled with the `tokio` feature and a TLS backend
    /// (`rustls` and/or `native-tls`).
    pub fn builder_tokio(deps: impl Into<TokioDeps>) -> HttpClientBuilder {
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
            |cx| TransportHandler(build_tokio_handler(cx).into()),
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
#[derive(Clone, Copy, Debug)]
struct TokioConnector {
    socket: SocketOptions,
}

impl layered::Service<BaseUri> for TokioConnector {
    type Out = Result<TokioIo<::tokio::net::TcpStream>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        let host = input.authority().host();
        let port = input.try_effective_port()?;
        let stream = connect_tcp(host, port, &self.socket).await?;
        Ok(TokioIo::new(stream))
    }
}

/// Opens a TCP connection to `host:port` with the configured socket options applied.
///
/// When no pre-connect tuning is requested this takes the cheap path and lets `tokio`
/// resolve and connect in one step; otherwise the address is resolved up front so the
/// buffer sizes can be set on the unconnected socket.
///
/// The overall connect budget is not enforced here: [`ClientConnector`][fetch_hyper] wraps
/// this whole future in `connect_timeout`, so resolution and every connect attempt already
/// share one deadline.
///
/// # Errors
///
/// Returns an error when host name resolution fails, when no resolved address accepts the
/// connection, or when the kernel rejects one of the requested socket options.
async fn connect_tcp(host: &str, port: u16, options: &SocketOptions) -> Result<::tokio::net::TcpStream> {
    let stream = if options.requires_pre_connect_setup() {
        connect_tuned(host, port, options).await?
    } else {
        let stream = ::tokio::net::TcpStream::connect((host, port)).await?;

        // On the fast path there is no unconnected socket to configure, so `no_delay` is
        // applied here. The tuned path already set it before connecting.
        if let Some(no_delay) = options.no_delay {
            stream.set_nodelay(no_delay)?;
        }

        stream
    };

    Ok(stream)
}

/// Resolves `host:port` and connects with the pre-connect socket options applied.
///
/// Addresses are tried in resolution order, mirroring [`tokio::net::TcpStream::connect`];
/// the error from the last attempt is surfaced when every address fails.
///
/// # Errors
///
/// Returns the resolution error when the host cannot be resolved, otherwise the error from
/// the final connect attempt.
#[cfg_attr(test, mutants::skip)] // the empty-resolution fallback is unreachable in practice
async fn connect_tuned(host: &str, port: u16, options: &SocketOptions) -> Result<::tokio::net::TcpStream> {
    let mut last_error = None;

    for address in ::tokio::net::lookup_host((host, port)).await? {
        match connect_to_address(address, options).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }

    // A successful resolution always yields at least one address, so this fallback should be
    // unreachable. It mirrors the kind and wording `TcpStream::connect` uses for the same
    // condition so that both paths classify identically for retry policies.
    Err(last_error
        .unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "could not resolve to any address"))
        .into())
}

/// Connects to a single resolved address, applying the socket options before connecting.
///
/// Buffer sizes have to be set while the socket is still unconnected for `TCP` window
/// scaling to pick them up during the handshake. `no_delay` is set here too so the very
/// first writes after the handshake are already un-Nagled.
///
/// # Errors
///
/// Returns an error when the socket cannot be created, when the kernel rejects one of the
/// requested options, or when the connection attempt fails.
async fn connect_to_address(address: std::net::SocketAddr, options: &SocketOptions) -> std::io::Result<::tokio::net::TcpStream> {
    let socket = if address.is_ipv4() {
        ::tokio::net::TcpSocket::new_v4()?
    } else {
        ::tokio::net::TcpSocket::new_v6()?
    };

    if let Some(size) = options.receive_buffer_size {
        socket.set_recv_buffer_size(size)?;
    }

    if let Some(size) = options.send_buffer_size {
        socket.set_send_buffer_size(size)?;
    }

    if let Some(no_delay) = options.no_delay {
        socket.set_nodelay(no_delay)?;
    }

    socket.connect(address).await
}

fn build_tokio_handler(cx: CustomContext<TokioDeps>) -> fetch_hyper::HyperTransport {
    let tls_backend = build_tls_backend(&cx.options, cx.tls);
    let connector = TokioConnector { socket: cx.options.socket };

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

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_tcp_uses_fast_path_without_pre_connect_options() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Default options request no tuning at all, so the single-step connect path is used
        // and the operating system defaults are left untouched.
        let options = SocketOptions::default();
        assert!(!options.requires_pre_connect_setup());

        let stream = super::connect_tcp("127.0.0.1", port, &options).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_tcp_applies_no_delay() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // `no_delay` is applied to the connected stream, so it is observable on the result.
        let stream = super::connect_tcp("127.0.0.1", port, &SocketOptions::default().no_delay(true))
            .await
            .unwrap();
        assert!(stream.nodelay().unwrap());

        let stream = super::connect_tcp("127.0.0.1", port, &SocketOptions::default().no_delay(false))
            .await
            .unwrap();
        assert!(!stream.nodelay().unwrap());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_tcp_applies_buffer_sizes_before_connecting() {
        use fetch_options::SocketOptions;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        // Buffer sizes force the resolve-then-connect path. The kernel is free to clamp or
        // scale the requested sizes, so only the successful connect is asserted here.
        let options = SocketOptions::default().receive_buffer_size(64 * 1024).send_buffer_size(64 * 1024);
        assert!(options.requires_pre_connect_setup());

        let stream = super::connect_tcp("127.0.0.1", port, &options).await.unwrap();
        assert_eq!(stream.peer_addr().unwrap().port(), port);

        // `no_delay` must also be honored on the tuned path, where it is set pre-connect.
        let options = options.no_delay(true);
        let stream = super::connect_tcp("127.0.0.1", port, &options).await.unwrap();
        assert!(stream.nodelay().unwrap());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn connect_tcp_surfaces_error_when_no_address_connects() {
        use fetch_options::SocketOptions;

        // Binding then dropping the listener yields a port nothing is listening on, so every
        // resolved address fails and the last error has to be surfaced rather than swallowed.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let options = SocketOptions::default().send_buffer_size(64 * 1024);
        let error = super::connect_tcp("127.0.0.1", port, &options).await.unwrap_err();

        assert!(!error.to_string().is_empty(), "the connect failure must carry a message");
    }
}
