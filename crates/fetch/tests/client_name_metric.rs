// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests verifying that the client name configured via
//! [`HttpClientBuilder::name`] is reported as the `http.client.name`
//! instrumentation-scope attribute on the metrics the client records,
//! regardless of whether the name is set before or after the meter provider.

use fetch::HttpClient;
use http::StatusCode;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use tick::Clock;

/// Returns the `http.client.name` scope attribute recorded by the exporter, if any.
fn recorded_client_name(exporter: &InMemoryMetricExporter) -> Option<String> {
    let metrics = exporter.get_finished_metrics().expect("finished metrics must be retrievable");
    metrics.iter().flat_map(ResourceMetrics::scope_metrics).find_map(|scope_metric| {
        scope_metric
            .scope()
            .attributes()
            .find(|attribute| attribute.key.as_str() == "http.client.name")
            .map(|attribute| attribute.value.as_str().into_owned())
    })
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn client_name_set_before_meter_provider_is_reported() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .insecure_allow_http()
        .name("named_before_provider")
        .meter_provider(provider.clone())
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(recorded_client_name(&exporter).as_deref(), Some("named_before_provider"));
}

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn client_name_set_after_meter_provider_is_reported() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();

    // Set the meter provider *before* the name to exercise the deferred-scope path:
    // the custom meter is only materialized at build time, so the later name still applies.
    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .insecure_allow_http()
        .meter_provider(provider.clone())
        .name("named_after_provider")
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    assert_eq!(recorded_client_name(&exporter).as_deref(), Some("named_after_provider"));
}
