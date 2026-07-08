// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration test verifying that the client name configured via
//! [`HttpClientBuilder::name`] is reported as the `http.client.name`
//! instrumentation-scope attribute on the metrics the client records.

use fetch::HttpClient;
use http::StatusCode;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn named_client_reports_http_client_name_scope_attribute() {
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder().with_periodic_exporter(exporter.clone()).build();

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .insecure_allow_http()
        .name("my_named_client")
        .meter_provider(&provider)
        .build();

    client.get("http://example.com").fetch().await.unwrap();
    provider.force_flush().unwrap();

    let metrics = exporter.get_finished_metrics().unwrap();
    let client_name = metrics.iter().flat_map(ResourceMetrics::scope_metrics).find_map(|scope_metric| {
        scope_metric
            .scope()
            .attributes()
            .find(|attribute| attribute.key.as_str() == "http.client.name")
            .map(|attribute| attribute.value.as_str().into_owned())
    });

    assert_eq!(client_name.as_deref(), Some("my_named_client"));
}
