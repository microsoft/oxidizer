// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! TLS configuration for HTTP clients.
//!
//! [`fetch_tls`] defines the TLS types. This module re-exports them. The `rustls` and `native-tls`
//! Cargo features select the available backends.

#[cfg(any(
    all(feature = "arty", any(feature = "rustls", feature = "native-tls")),
    all(feature = "tokio", any(feature = "rustls", feature = "native-tls")),
    test
))]
use fetch_options::TransportOptions;
#[cfg(any(feature = "native-tls", test))]
pub use fetch_tls::NativeTlsOptions;
#[cfg(any(feature = "rustls", test))]
pub use fetch_tls::RustlsOptions;
pub use fetch_tls::{AutoBackend, ClientIdentity, ClientIdentityError, TlsOptions, TlsOptionsBuilder};
#[cfg(any(
    all(feature = "arty", any(feature = "rustls", feature = "native-tls")),
    all(feature = "tokio", any(feature = "rustls", feature = "native-tls")),
    test
))]
use fetch_tls::{TlsBackend, TlsBackendBuilder};

/// Materializes client TLS settings for a bundled Hyper transport.
#[cfg(any(
    all(feature = "arty", any(feature = "rustls", feature = "native-tls")),
    all(feature = "tokio", any(feature = "rustls", feature = "native-tls")),
    test
))]
pub(crate) fn build_tls_backend(options: &TransportOptions, tls: TlsOptions) -> TlsBackend {
    let mut builder = TlsBackendBuilder::new();
    if !options.supported_http_versions.is_empty() {
        builder = builder.supported_http_versions(&options.supported_http_versions);
    }

    #[cfg(any(feature = "rustls", test))]
    {
        let provider = std::sync::Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
        let verifier = std::sync::Arc::new(
            rustls_platform_verifier::Verifier::new(std::sync::Arc::clone(&provider))
                .expect("the platform certificate verifier must initialize with the aws-lc-rs crypto provider"),
        );
        builder = builder.configure_rustls(provider, verifier);
    }

    #[cfg(all(feature = "native-tls", not(any(feature = "rustls", test))))]
    {
        builder = builder.defaults_to_native_tls();
    }

    builder
        .build_backend(tls)
        .expect("TLS backend construction must succeed for the configured TlsOptions")
}
