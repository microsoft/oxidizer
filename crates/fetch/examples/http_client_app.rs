// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! This example demonstrates how to use [`fundle`] to build an HTTP client application with
//! telemetry (an OpenTelemetry meter provider) and a custom rustls certificate verifier.

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
/// This example delegates to the platform trust store via
/// [`rustls_platform_verifier::Verifier`], but the same hook accepts any custom
/// [`ServerCertVerifier`] implementation.
fn custom_verifier(provider: Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> {
    Arc::new(
        rustls_platform_verifier::Verifier::new(provider)
            .expect("the platform certificate verifier must initialize with the supplied crypto provider"),
    )
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    // Telemetry is configured up front and shared with the HTTP client.
    let meter_provider = initialize_meter_provider();

    // Initialize and set up the App instance; fundle ensures all fields are properly constructed.
    let app = App::builder()
        .clock(|_| Clock::new_tokio())
        .global_pool(|_| GlobalPool::new())
        .client({
            let meter_provider = meter_provider.clone();
            move |x| {
                HttpClient::builder_tokio(x)
                    .meter_provider(&meter_provider)
                    .tls_options(TlsOptions::builder_rustls().server_certificate_verifier(custom_verifier).build())
                    .build()
            }
        })
        .build();

    // Use the client.
    match app.client.get("https://www.example.com").fetch().await {
        Ok(response) => {
            println!("response success, status: {}", response.status());
        }
        Err(e) => {
            println!("response error, message: {}", e.message());
        }
    }

    Ok(())
}

fn initialize_meter_provider() -> SdkMeterProvider {
    SdkMeterProvider::builder()
        .with_periodic_exporter(opentelemetry_stdout::MetricExporter::default())
        .build()
}
