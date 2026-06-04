// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates mutual TLS (`mTLS`) authentication with the fetch HTTP client
//! using the platform native TLS backend (`SChannel` on Windows, Security Framework on macOS,
//! `OpenSSL` on Linux).
//!
//! It loads a client certificate and `PKCS#8` private key from `PEM` files, configures the HTTP
//! client with a client identity, and performs a GET request to a user-specified URL.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p fetch --example http_client_native_tls_mtls --features fetch/native-tls,fetch/tokio -- --cert client.pem --key client-key.pem --url https://example.com/api
//! ```
//!
//! You can generate a self-signed client certificate (`PEM` + `PKCS#8` key) for testing with:
//! ```sh
//! openssl req -x509 -newkey rsa:2048 -keyout client-key.pem -out client.pem -days 365 -nodes -subj "/CN=test"
//! ```

use std::process;

use argh::FromArgs;
use fetch::HttpClient;
use fetch::tls::{ClientIdentity, TlsOptions};
use fetch::tokio::TokioDeps;
use serde_json::Value;
use tracing::info;

#[path = "util/utils.rs"]
mod utils;

/// Demonstrates mutual TLS (`mTLS`) authentication with the fetch HTTP client
/// using the platform native TLS backend.
#[derive(FromArgs)]
struct Args {
    /// path to the client certificate `PEM` file
    #[argh(option)]
    cert: String,

    /// path to the `PKCS#8` `PEM` private key file
    #[argh(option)]
    key: String,

    /// target URL to send the GET request to
    #[argh(option)]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // When run without any arguments (e.g. by the workspace example runner in CI),
    // there is no certificate/key/URL to exercise, so exit successfully instead of
    // letting `argh` fail with a missing-argument error (exit code 1).
    if std::env::args_os().nth(1).is_none() {
        info!("No arguments provided; nothing to do. See the module docs for usage.");
        return Ok(());
    }

    let args: Args = argh::from_env();

    if !args.url.starts_with("https://") {
        eprintln!("Error: --url must use the https:// scheme (got '{}')", args.url);
        process::exit(2);
    }

    let identity = {
        info!("Loading client certificate from: {}", args.cert);
        info!("Loading client private key from: {}", args.key);

        let cert_pem = std::fs::read(&args.cert).expect("failed to read certificate PEM file");
        let key_pem = std::fs::read(&args.key).expect("failed to read private key PEM file");

        ClientIdentity::from_pem(&cert_pem, &key_pem).expect("failed to parse PKCS#8 PEM identity")
    };

    info!("Client identity loaded successfully");

    // Build the HTTP client with the native TLS backend and the client identity.
    let client = HttpClient::builder_tokio(TokioDeps::default())
        .tls_options(TlsOptions::builder().client_identity(identity).build())
        .build();

    info!("Sending GET request to {} ...", args.url);

    let response = client.get(args.url.as_str()).fetch().await?;
    info!("Response status: {}", response.status());

    let body_text = response.into_body().into_text().await?;
    let json: Value = serde_json::from_str(&body_text).expect("response is not valid JSON");
    let pretty = serde_json::to_string_pretty(&json).expect("failed to pretty-print JSON");

    println!("\n--- Response Body ---");
    println!("{pretty}");
    println!("--- End ---");

    Ok(())
}
