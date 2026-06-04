// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates advanced HTTP client configuration options including:
//! - Connection keep-alive strategies
//! - Connection pooling configuration
//! - HTTP/2-specific settings
//! - A custom rustls server certificate verifier
//! - Resilience pipeline configuration

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
/// This example simply delegates to the platform trust store via
/// [`rustls_platform_verifier::Verifier`], but the same hook can plug in any
/// custom [`ServerCertVerifier`] implementation (for example, certificate
/// pinning or a private root of trust).
fn custom_verifier(provider: Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> {
    Arc::new(
        rustls_platform_verifier::Verifier::new(provider)
            .expect("the platform certificate verifier must initialize with the supplied crypto provider"),
    )
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // Create an HTTP client with extensive configuration.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        // Customize the certificate validation with a custom rustls verifier.
        .tls_options(TlsOptions::builder_rustls().server_certificate_verifier(custom_verifier).build())
        // Configure connection keep-alive to maintain connections for better performance.
        // This keeps both active and idle connections alive with periodic probes.
        .connection_keep_alive(ConnectionKeepAlive::active_and_idle_connections(
            Duration::from_secs(30), // Keep-alive probe interval
            Duration::from_secs(5),  // Keep-alive probe timeout
        ))
        // Configure connection pooling for optimal connection reuse.
        .connection_pool_options(
            ConnectionPoolOptions::default()
                // Limit to 10 connections per host to control resource usage.
                .max_connections(10)
                // Keep idle connections for 5 seconds before closing them.
                .connection_idle_timeout(Duration::from_secs(5))
                // Limit max connection lifetime to 60 seconds.
                .connection_lifetime(ConnectionLifetime::fixed(Duration::from_mins(1))),
        )
        // Configure HTTP/2-specific options:
        // Allow up to 100 concurrent streams per HTTP/2 connection.
        .http2_options(Http2Options::default().initial_max_send_streams(100))
        // Support both HTTP/1.1 and HTTP/2 (this is the default, shown for clarity).
        .supported_http_versions(&[Version::HTTP_11, Version::HTTP_2])
        // Configure the standard pipeline with custom resilience settings.
        .standard_pipeline(|pipeline, _context| {
            // Change the default attempt timeout to 3 seconds.
            pipeline.attempt_timeout(|timeout| timeout.timeout(Duration::from_secs(3)))
        })
        .build();

    info!("Advanced HTTP client created successfully");

    // No requests are made in this example; the focus is on configuration.
    drop(client);

    Ok(())
}
