// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses mutual TLS (`mTLS`) with the platform native TLS backend. The backend is `SChannel` on
//! Windows, Security Framework on macOS, and `OpenSSL` on Linux.
//!
//! The example loads a client certificate and `PKCS#8` private key from `PEM` files. It sends a GET
//! request to the specified URL with that client identity.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p fetch --example fetch_native_tls_mtls --features fetch/native-tls,fetch/tokio -- --cert client.pem --key client-key.pem --url https://example.com/api
//! ```
//!
//! For testing, generate a self-signed certificate and `PKCS#8` key:
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

/// Command-line arguments for the native TLS `mTLS` example.
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

    // The CI example runner supplies no arguments. Exit before argh reports missing arguments.
    if std::env::args_os().nth(1).is_none() {
        info!("No arguments were provided. See the module documentation for usage.");
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

    info!("Loaded the client identity");

    // Build a native TLS client with the client identity.
    let client = HttpClient::builder_tokio(TokioDeps::default())
        .tls_options(TlsOptions::builder().client_identity(identity).build())
        .build();

    info!("Sending a GET request to {}", args.url);

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
