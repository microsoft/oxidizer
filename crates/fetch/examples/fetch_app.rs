// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses [`fundle`] to build an HTTP client with a custom rustls certificate verifier.

use std::sync::Arc;

use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch::HttpClient;
use fetch::tls::TlsOptions;
use observed::Sink;
use ohno::ErrorExt;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;
use tick::Clock;

#[fundle::bundle]
struct App {
    clock: Clock,
    memory: OpaqueMemory,
    sink: Sink,
    client: HttpClient,
}

/// Builds a custom rustls server certificate verifier.
///
/// This implementation delegates to the platform trust store through
/// [`rustls_platform_verifier::Verifier`]. The hook accepts another [`ServerCertVerifier`].
fn custom_verifier(provider: Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> {
    Arc::new(
        rustls_platform_verifier::Verifier::new(provider)
            .expect("the platform certificate verifier must initialize with the supplied crypto provider"),
    )
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Use fundle to construct App.
    let app = App::builder()
        .clock(|_| Clock::new_tokio())
        .memory(|_| OpaqueMemory::new(GlobalPool::new()))
        // An application passes its own sink. `Sink::noop()` drops every event.
        .sink(|_| Sink::noop())
        .client(|x| {
            HttpClient::builder_tokio(x)
                .tls_options(TlsOptions::builder_rustls().server_certificate_verifier(custom_verifier).build())
                .build()
        })
        .build();

    // Send a request.
    match app.client.get("https://www.example.com").fetch().await {
        Ok(response) => {
            println!("Request succeeded, status: {}", response.status());
        }
        Err(e) => {
            println!("Request failed, message: {}", e.message());
        }
    }

    Ok(())
}
