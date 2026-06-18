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
use fetch_options::TransportOptions;
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
            crate::constants::HYPER_ON_TOKIO_TRANSPORT_NAME,
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
#[derive(Clone)]
struct TokioConnector;

impl layered::Service<BaseUri> for TokioConnector {
    type Out = Result<TokioIo<::tokio::net::TcpStream>>;

    async fn execute(&self, input: BaseUri) -> Self::Out {
        let host = input.authority().host();
        let port = input.try_effective_port()?;
        let stream = ::tokio::net::TcpStream::connect((host, port)).await?;
        Ok(TokioIo::new(stream))
    }
}

fn build_tokio_handler(cx: CustomContext<TokioDeps>) -> fetch_hyper::HyperTransport {
    let tls_backend = build_tls_backend(&cx.options, cx.tls);

    HyperTransportBuilder::new(TokioConnector, Spawner::new_tokio(), cx.clock, cx.options)
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
}
