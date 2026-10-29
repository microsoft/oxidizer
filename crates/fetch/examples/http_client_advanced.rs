// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configures these HTTP client options:
//! - Connection keep-alive strategies
//! - Connection pooling
//! - HTTP/2-specific settings
//! - A custom rustls server certificate verifier
//! - Resilience pipeline

use std::sync::Arc;
use std::time::Duration;

use fetch::HttpClient;
use fetch::options::{ConnectionKeepAlive, ConnectionLifetime, ConnectionPoolOptions, Http2Options};
use fetch::tls::TlsOptions;
use http::Version;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;
use tracing::info;

#[path = "util/utils.rs"]
mod utils;

/// Builds a custom rustls server certificate verifier.
///
/// This implementation delegates to the platform trust store through
/// [`rustls_platform_verifier::Verifier`]. The hook also accepts another
/// [`ServerCertVerifier`], such as certificate pinning or a private root of trust.
fn custom_verifier(provider: Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> {
    Arc::new(
        rustls_platform_verifier::Verifier::new(provider)
            .expect("the platform certificate verifier must initialize with the supplied crypto provider"),
    )
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // Create a client with custom transport and resilience options.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        // Use a custom rustls certificate verifier.
        .tls_options(TlsOptions::builder_rustls().server_certificate_verifier(custom_verifier).build())
        // Keep active and idle connections alive with periodic probes.
        .connection_keep_alive(ConnectionKeepAlive::active_and_idle_connections(
            Duration::from_secs(30), // Keep-alive probe interval
            Duration::from_secs(5),  // Keep-alive probe timeout
        ))
        // Configure connection pooling.
        .connection_pool_options(
            ConnectionPoolOptions::default()
                // Keep at most 10 idle connections for each host.
                .max_connections(10)
                // Close an idle connection after 5 seconds.
                .connection_idle_timeout(Duration::from_secs(5))
                // Limit a connection lifetime to 60 seconds.
                .connection_lifetime(ConnectionLifetime::fixed(Duration::from_mins(1))),
        )
        // Configure HTTP/2.
        // Start with a cap of 100 streams. The peer settings can change this value.
        .http2_options(Http2Options::default().initial_max_send_streams(100))
        // Support HTTP/1.1 and HTTP/2. This is the default.
        .supported_http_versions(&[Version::HTTP_11, Version::HTTP_2])
        // Change resilience in the standard pipeline.
        .standard_pipeline(|pipeline, _context| {
            // Set the attempt timeout to 3 seconds.
            pipeline.attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(3)))
        })
        .build();

    info!("Created the configured HTTP client");

    // This example creates the client but does not send a request.
    drop(client);

    Ok(())
}
