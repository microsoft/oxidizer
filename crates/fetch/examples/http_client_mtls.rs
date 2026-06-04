// Copyright (c) Microsoft Corporation.

//! Mutual TLS (mTLS) authentication with the fetch HTTP client using the rustls backend.
//!
//! It loads a client certificate and private key from PEM files, configures the HTTP client
//! with a client identity, and performs a GET request to a user-specified URL.
//!
//! Note: The fetch crate does not follow redirects automatically, so this example
//! manually follows 3xx redirects by reading the `Location` header.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p fetch --example http_client_mtls --features fetch/tokio,fetch/rustls -- --cert client.pem --key client-key.pem --url https://example.com/api
//! ```
//!
//! You can generate a self-signed client certificate for testing with:
//! ```sh
//! openssl req -x509 -newkey rsa:2048 -keyout client-key.pem -out client.pem -days 365 -nodes -subj "/CN=test"
//! ```

use std::process;

use argh::FromArgs;
use fetch::HttpClient;
use fetch::tls::{ClientIdentity, TlsOptions};
use serde_json::Value;
use tracing::info;

#[path = "util/utils.rs"]
mod utils;

/// Demonstrates mutual TLS (mTLS) authentication with the fetch HTTP client.
#[derive(FromArgs)]
struct Args {
    /// path to the client certificate PEM file
    #[argh(option)]
    cert: String,

    /// path to the client private key PEM file
    #[argh(option)]
    key: String,

    /// target URL to send the GET request to
    #[argh(option)]
    url: String,
}

/// Maximum number of redirects to follow before giving up.
const MAX_REDIRECTS: u32 = 10;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    let args: Args = argh::from_env();

    info!("Loading client certificate from: {}", args.cert);
    info!("Loading client private key from: {}", args.key);

    let cert_pem = std::fs::read(&args.cert).expect("failed to read certificate PEM file");
    let key_pem = std::fs::read(&args.key).expect("failed to read private key PEM file");

    // Create the client identity for mTLS.
    let identity = ClientIdentity::from_pem(&cert_pem, &key_pem).expect("failed to parse PEM identity");

    info!("Client identity loaded successfully");

    // Build the HTTP client with the client identity configured.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .tls_options(TlsOptions::builder_rustls().client_identity(identity).build())
        .build();

    let mut url = args.url;

    // The fetch crate does not follow redirects automatically,
    // so we follow 3xx redirects manually up to MAX_REDIRECTS times.
    for redirect_count in 0..=MAX_REDIRECTS {
        info!("Sending GET request to {url} ...");

        let response = client.get(url.as_str()).fetch().await?;
        let status = response.status();
        info!("Response status: {status}");

        if status.is_redirection() {
            let location = response
                .headers()
                .get(http::header::LOCATION)
                .expect("redirect response missing Location header")
                .to_str()
                .expect("Location header is not valid UTF-8");

            info!("Following redirect to: {location}");
            location.clone_into(&mut url);
            // Consume the redirect response body before continuing.
            drop(response.into_body());
            continue;
        }

        // Not a redirect — pretty-print the JSON response body.
        let body_text = response.into_body().into_text().await?;
        let json: Value = serde_json::from_str(&body_text).expect("response is not valid JSON");
        let pretty = serde_json::to_string_pretty(&json).expect("failed to pretty-print JSON");

        println!("\n--- Response Body ---");
        println!("{pretty}");
        println!("--- End ---");

        if redirect_count > 0 {
            info!("Followed {redirect_count} redirect(s) to reach final response");
        }
        return Ok(());
    }

    eprintln!("Error: too many redirects (>{MAX_REDIRECTS})");
    process::exit(1);
}
