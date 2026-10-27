// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Uses [`fundle`] to build an HTTP client with an OpenTelemetry meter provider and a custom rustls
//! certificate verifier.

use std::sync::Arc;

use bytesbuf::mem::GlobalPool;
use fetch::HttpClient;
use fetch::tls::TlsOptions;
use ohno::ErrorExt;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;
use tick::Clock;

#[fundle::bundle]
struct App {
    clock: Clock,
    global_pool: GlobalPool,
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
    // Create the meter provider that the HTTP client uses.
    let meter_provider = initialize_meter_provider();

    // Use fundle to construct App.
    let app = App::builder()
        .clock(|_| Clock::new_tokio())
        .global_pool(|_| GlobalPool::new())
        .client({
            let meter_provider = meter_provider.clone();
            move |x| {
                HttpClient::builder_tokio(x)
                    .meter_provider(meter_provider.clone())
                    .tls_options(TlsOptions::builder_rustls().server_certificate_verifier(custom_verifier).build())
                    .build()
            }
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

fn initialize_meter_provider() -> SdkMeterProvider {
    SdkMeterProvider::builder()
        .with_periodic_exporter(opentelemetry_stdout::MetricExporter::default())
        .build()
}
