// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public contract tests for the `RequestHandler` HTTP readiness adapter.

mod support;

use futures::executor::block_on;
use health::Health;
use health_probe::http::readiness_request::request_handler;
use http::Request;
use http_extensions::HttpBodyBuilder;
use layered::Service;
use support::{STATES, assert_headers, expected, monitor_in, wait_for_monitor};

#[test]
fn reports_readiness() {
    for state in STATES {
        let (monitor, _component, _publisher) = monitor_in(state);
        let handler = request_handler(monitor);
        let response = block_on(handler.execute(Request::new(HttpBodyBuilder::new_fake().text("ignored")))).unwrap();
        let (status, body) = expected(state);
        assert_eq!(response.status(), status);
        assert_headers(response.headers());
        let text = block_on(response.into_body().into_text()).unwrap();
        assert_eq!(text, body);
    }
}

#[test]
fn reads_current_state_for_each_request() {
    let (monitor, _component, mut publisher) = monitor_in(Health::Unhealthy);
    let observer = monitor.clone();
    let handler = request_handler(monitor);
    let request = || Request::new(HttpBodyBuilder::new_fake().empty());

    let response = block_on(handler.execute(request())).unwrap();
    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);

    publisher.publish(Health::Healthy, [("case", "updated")]);
    wait_for_monitor(&observer, Health::Healthy);

    let response = block_on(handler.execute(request())).unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
}
