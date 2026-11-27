// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The native-only feature configuration must select an automatic TLS backend.

#![cfg(all(feature = "tokio", feature = "native-tls", not(feature = "rustls")))]

testing_aids::init_tracing!();

static MEMORY: std::sync::LazyLock<bytesbuf::mem::GlobalPool> = std::sync::LazyLock::new(bytesbuf::mem::GlobalPool::new);

#[test]
#[cfg_attr(miri, ignore = "native tests cover platform TLS initialization and ALPN configuration")]
fn native_only_unpinned_tls_builds_with_explicit_transport_options_and_alpn() {
    for version in [http::Version::HTTP_11, http::Version::HTTP_2] {
        let client = fetch::HttpClient::builder_tokio_with_options(
            fetch::tokio::TokioDeps::new(&tick::Clock::new_frozen(), MEMORY.clone()),
            fetch::tokio::TokioTransportOptions::default().socket(fetch::options::SocketOptions::default().no_delay(false)),
        )
        .tls_options(fetch::tls::TlsOptions::default())
        .supported_http_versions(&[version])
        .minimal_pipeline()
        .build();
        drop(client);
    }
}
