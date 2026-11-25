// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the connection telemetry of the Tokio transport.

use fetch::HttpClient;
use fetch::tokio::TokioDeps;
use observed::Severity;
use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};
use tick::Clock;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[cfg_attr(miri, ignore)]
#[tokio::test]
async fn tokio_transport_emits_connection_events() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (sink, processor) = test_emitter(TEST_ID);
    // The frozen clock makes the setup duration 0.
    let client = HttpClient::builder_tokio(TokioDeps::with_clock(&Clock::new_frozen(), sink))
        .insecure_allow_http()
        .build();

    client.get(server.uri()).fetch().await.unwrap();

    let address = server.address();
    let events: Vec<_> = processor
        .events()
        .into_iter()
        .filter(|event| event.name().starts_with("http.client.connection."))
        .collect();
    assert_eq!(
        events,
        [
            ExpectedEvent::new("http.client.connection.start", Severity::Debug)
                .body("connecting to a remote endpoint")
                .dimension("server.address", address.ip().to_string())
                .dimension("server.port", address.port())
                .dimension("url.full", format!("{}/", server.uri()))
                .dimension("url.scheme", "http"),
            ExpectedEvent::new("http.client.connection.success", Severity::Info)
                .body("connected to server")
                .dimension("http.client.connection.setup.duration", 0.0)
                .dimension("network.protocol.version", "1")
                .dimension("server.address", address.ip().to_string())
                .dimension("server.port", address.port())
                .dimension("url.full", format!("{}/", server.uri()))
                .dimension("url.scheme", "http")
                .metric(),
        ]
    );
}

testing_aids::init_tracing!();
