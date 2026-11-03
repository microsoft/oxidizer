// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! # HTTP client telemetry
//!
//! This example:
//!
//! 1. Creates a meter provider that writes to the console.
//! 2. Uses that provider with an HTTP client.
//! 3. Sends requests that record metrics.
//! 4. Classifies URI components for telemetry.
//! 5. Uses a fake handler without network I/O.

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

    // Create an OpenTelemetry meter provider that writes metrics to stdout.
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(opentelemetry_stdout::MetricExporter::default())
        .build();

    // Use the custom meter provider instead of the global provider.
    // The fake handler returns responses without network I/O.
    let client = HttpClient::builder_fake(StatusCode::OK, FakeDeps::default())
        .meter_provider(meter_provider)
        .build();

    // Send 10 requests with a string URI.
    for _ in 0..10 {
        _ = client
            .get("https://example.com/path/to/resource")
            // Add a custom attribute to this request.
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

    // Send 10 requests with a templated URI.
    for _ in 0..10 {
        _ = client.get(target.clone()).fetch().await?.into_body();
    }

    println!("All requests are complete.");

    Ok(())
}
