// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! # HTTP Client Telemetry Example
//!
//! This example demonstrates how to configure and use OpenTelemetry metrics with the fetch HTTP client.
//! It shows:
//!
//! 1. Creating a custom meter provider with console output
//! 2. Configuring an HTTP client to use that meter provider
//! 3. Making requests with telemetry collection
//! 4. Classification of URI components for safe telemetry
//! 5. Using a fake handler for testing without real network requests

use data_privacy::{DataClass, Sensitive};
use fetch::HttpClient;
use fetch::fake::FakeDeps;
use fetch::telemetry::TelemetryAttributes;
use http::StatusCode;
use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use templated_uri::{BaseUri, EscapedString, Uri, templated};

const UNKNOWN: DataClass = DataClass::new("unknown", "unknown");

#[path = "util/utils.rs"]
mod utils;

#[templated(template = "/path/to{/public,secret}")]
#[derive(Clone)]
struct ResourcePath {
    #[unredacted]
    public: EscapedString,
    secret: Sensitive<EscapedString>,
}

#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    utils::init_tracing();

    // Create a custom OpenTelemetry meter provider that outputs metrics to stdout.
    // In a real application, you would configure this to send metrics to your monitoring system.
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(opentelemetry_stdout::MetricExporter::default())
        .build();

    // Configure an HTTP client with our custom meter provider. The client will record metrics
    // using this provider instead of the global one.
    //
    // Instead of making real HTTP requests, we use a fake handler that returns
    // simulated responses after a short delay.
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
        .meter_provider(meter_provider)
        .build();

    // Make 10 requests to generate metrics for a request made with simple URI input.
    // The client will record telemetry for each request.
    for _ in 0..10 {
        _ = client
            .get("https://example.com/path/to/resource")
            // You can also attach telemetry attributes for dynamic enrichment.
            .extension(TelemetryAttributes::from_iter([KeyValue::new("extra", "extra_value")]))
            .fetch()
            .await?
            .into_body();
    }

    let resource_path = ResourcePath {
        public: EscapedString::from_static("public_resource"),
        secret: Sensitive::new(EscapedString::from_static("secret_resource"), UNKNOWN),
    };

    let target = Uri::default()
        .with_base(BaseUri::from_static("https://example.com"))
        .with_path_and_query(resource_path);

    // Make 10 requests to generate metrics for a request made with a templated URI.
    for _ in 0..10 {
        _ = client.get(target.clone()).fetch().await?.into_body();
    }

    println!("All requests completed! Check the console output for metrics.");

    Ok(())
}
