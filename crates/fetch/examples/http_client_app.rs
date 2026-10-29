// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses [`fundle`] to build an HTTP client with an OpenTelemetry meter provider and a custom rustls
//! certificate verifier.

use std::sync::Arc;

use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch::HttpClient;
use fetch::tls::TlsOptions;
use ohno::ErrorExt;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;
use rustls::pki_types::CertificateDer;
use tick::Clock;

#[path = "util/endpoint.rs"]
mod endpoint;

#[fundle::bundle]
struct App {
    clock: Clock,
    memory: OpaqueMemory,
    client: HttpClient,
}

/// Builds a custom rustls server certificate verifier.
///
/// This implementation delegates to the platform trust store through
/// [`rustls_platform_verifier::Verifier`]. The hook accepts another [`ServerCertVerifier`].
fn custom_verifier(provider: Arc<CryptoProvider>, extra_roots: Vec<CertificateDer<'static>>) -> Arc<dyn ServerCertVerifier> {
    let verifier = if extra_roots.is_empty() {
        rustls_platform_verifier::Verifier::new(provider)
    } else {
        rustls_platform_verifier::Verifier::new_with_extra_roots(extra_roots, provider)
    };
    Arc::new(verifier.expect("the platform certificate verifier must initialize with the supplied crypto provider"))
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    let endpoint = endpoint::Endpoint::new("https://www.example.com").await?;
    // Create the meter provider that the HTTP client uses.
    let meter_provider = initialize_meter_provider();

    // Use fundle to construct App.
    let app = App::builder()
        .clock(|_| Clock::new_tokio())
        .memory(|_| OpaqueMemory::new(GlobalPool::new()))
        .client({
            let meter_provider = meter_provider.clone();
            let extra_roots = endpoint.root_certificate.iter().cloned().collect::<Vec<_>>();
            move |x| {
                let extra_roots = extra_roots.clone();
                HttpClient::builder_tokio(x)
                    .meter_provider(meter_provider.clone())
                    .tls_options(
                        TlsOptions::builder_rustls()
                            .server_certificate_verifier(move |provider| custom_verifier(provider, extra_roots.clone()))
                            .build(),
                    )
                    .build()
            }
        })
        .build();

    // Send a request.
    match app.client.get(endpoint.url.as_str()).fetch().await {
        Ok(response) => {
            println!("Request succeeded, status: {}", response.status());
        }
        Err(e) => {
            if endpoint.is_automated() {
                return Err(e.into());
            }
            println!("Request failed, message: {}", e.message());
        }
    }

    endpoint.finish(1).await?;
    Ok(())
}

fn initialize_meter_provider() -> SdkMeterProvider {
    SdkMeterProvider::builder()
        .with_periodic_exporter(opentelemetry_stdout::MetricExporter::default())
        .build()
}
