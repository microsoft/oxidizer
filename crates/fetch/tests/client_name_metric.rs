// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration test verifying that the client name configured via
//! [`HttpClientBuilder::name`] is reported as the `http.client.name` metric
//! attribute on the request metrics recorded by the standard pipeline.

use fetch::HttpClient;
use http::StatusCode;
use opentelemetry::KeyValue;
use testing_aids::MetricTester;
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn named_client_reports_http_client_name_attribute() {
    let tester = MetricTester::new();

    let client = HttpClient::builder_fake(StatusCode::OK, &Clock::new_frozen())
        .insecure_allow_http()
        .name("my_named_client")
        .meter_provider(tester.meter_provider())
        .build();

    client.get("http://example.com").fetch().await.unwrap();

    tester.assert_attributes_contain(&[KeyValue::new("http.client.name", "my_named_client")]);
}
