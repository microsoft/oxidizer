// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses mutual TLS (`mTLS`) with the rustls backend.
//!
//! The example loads a client certificate and private key from `PEM` files. It sends a GET request
//! to the specified URL with that client identity.
//!
//! > **Note**: `fetch` does not follow redirects automatically. This example follows absolute URLs
//! > from 3xx `Location` headers.
//!
//! # Usage
//!
//! ```sh
//! cargo run -p fetch --example http_client_mtls --features fetch/tokio,fetch/rustls -- --cert client.pem --key client-key.pem --url https://example.com/api
//! ```
//!
//! For testing, generate a self-signed client certificate:
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

/// Command-line arguments for the rustls `mTLS` example.
#[derive(FromArgs)]
struct Args {
    /// path to the client certificate `PEM` file
    #[argh(option)]
    cert: String,

    /// path to the client private key `PEM` file
    #[argh(option)]
    key: String,

    /// target URL to send the GET request to
    #[argh(option)]
    url: String,
}

/// Maximum number of redirected requests after the initial request.
const MAX_REDIRECTS: u32 = 10;

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // The CI example runner supplies no arguments. Exit before argh reports missing arguments.
    if std::env::args_os().nth(1).is_none() {
        info!("No arguments were provided. See the module documentation for usage.");
        return Ok(());
    }

    let args: Args = argh::from_env();

    info!("Loading client certificate from: {}", args.cert);
    info!("Loading client private key from: {}", args.key);

    let cert_pem = std::fs::read(&args.cert).expect("failed to read certificate PEM file");
    let key_pem = std::fs::read(&args.key).expect("failed to read private key PEM file");

    // Create the mTLS client identity.
    let identity = ClientIdentity::from_pem(&cert_pem, &key_pem).expect("failed to parse PEM identity");

    info!("Loaded the client identity");

    // Build a rustls client with the client identity.
    let client = HttpClient::builder_tokio(fetch::tokio::TokioDeps::default())
        .tls_options(TlsOptions::builder_rustls().client_identity(identity).build())
        .build();

    let mut url = args.url;

    // Send the initial request and at most MAX_REDIRECTS redirected requests.
    for redirect_count in 0..=MAX_REDIRECTS {
        info!("Sending a GET request to {url}");

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

            info!("Following redirect to {location}");
            location.clone_into(&mut url);
            // Consume the redirect response body before continuing.
            drop(response.into_body());
            continue;
        }

        // This is the final response. Format its JSON body.
        let body_text = response.into_body().into_text().await?;
        let json: Value = serde_json::from_str(&body_text).expect("response is not valid JSON");
        let pretty = serde_json::to_string_pretty(&json).expect("failed to pretty-print JSON");

        println!("\n--- Response Body ---");
        println!("{pretty}");
        println!("--- End ---");

        if redirect_count > 0 {
            info!("Followed {redirect_count} redirect(s)");
        }
        return Ok(());
    }

    eprintln!("Error: too many redirects (>{MAX_REDIRECTS})");
    process::exit(1);
}
