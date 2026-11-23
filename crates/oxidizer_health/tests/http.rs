// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contract tests for the HTTP readiness adapters.

#![cfg(any(feature = "http", feature = "request-handler"))]

use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, StatusCode};
use oxidizer_health::{Aggregator, Component, Filter, Health, Publisher};

testing_aids::init_tracing!();

const STATES: [Health; 3] = [Health::Healthy, Health::Degraded, Health::Unhealthy];

fn aggregator_in(state: Health) -> (Aggregator, Component, Publisher) {
    let aggregator = Aggregator::new();
    let component = aggregator.component("database");
    let mut monitor = aggregator.monitor();
    let mut publisher = component.publisher();
    publisher.publish(state, [("case", state.to_string())]);
    futures::executor::block_on(async {
        loop {
            let report = component.report(Filter::ALL).await;
            if report.signal_count(state) == 1 && monitor.state() == state {
                break;
            }
            monitor.changed().await;
        }
    });
    (aggregator, component, publisher)
}

fn expected(state: Health) -> (StatusCode, &'static str) {
    if state == Health::Healthy {
        (StatusCode::OK, "true")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "false")
    }
}

fn assert_headers(headers: &HeaderMap) {
    assert_eq!(headers[CONTENT_TYPE], "application/json");
    assert_eq!(headers[CACHE_CONTROL], "no-store");
}

#[cfg(feature = "http")]
#[test]
fn tower_service_reports_readiness() {
    use futures::executor::block_on;
    use http::Request;
    use tower::ServiceExt;

    for state in STATES {
        let (aggregator, _component, _publisher) = aggregator_in(state);
        let response = block_on(oxidizer_health::http::service(aggregator).oneshot(Request::new("ignored"))).unwrap();
        let (status, body) = expected(state);
        assert_eq!(response.status(), status);
        assert_eq!(response.body(), body);
        assert_headers(response.headers());
    }
}

#[cfg(feature = "request-handler")]
#[test]
fn request_handler_reports_readiness() {
    use futures::executor::block_on;
    use http::Request;
    use http_extensions::HttpBodyBuilder;
    use layered::Service;

    for state in STATES {
        let (aggregator, _component, _publisher) = aggregator_in(state);
        let handler = oxidizer_health::http::request_handler(aggregator);
        let response = block_on(handler.execute(Request::new(HttpBodyBuilder::new_fake().text("ignored")))).unwrap();
        let (status, body) = expected(state);
        assert_eq!(response.status(), status);
        assert_headers(response.headers());
        let text = block_on(response.into_body().into_text()).unwrap();
        assert_eq!(text, body);
    }
}
