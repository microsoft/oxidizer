// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public contract tests for the Tower HTTP readiness adapter.

mod support;

use futures::executor::block_on;
use health::Health;
use health_probe::http::readiness_tower::service;
use http::Request;
use support::{STATES, assert_headers, expected, monitor_in, wait_for_monitor};
use tower::ServiceExt;
use tower_service::Service;

#[test]
fn reports_readiness() {
    for state in STATES {
        let (monitor, _component, _publisher) = monitor_in(state);
        let response = block_on(service(monitor).oneshot(Request::new("ignored"))).unwrap();
        let (status, body) = expected(state);
        assert_eq!(response.status(), status);
        assert_eq!(response.body(), body);
        assert_headers(response.headers());
    }
}

#[test]
fn reads_current_state_for_each_request() {
    let (monitor, _component, mut publisher) = monitor_in(Health::Unhealthy);
    let observer = monitor.clone();
    let mut service = service(monitor);

    let response = block_on(service.call(Request::new("ignored"))).unwrap();
    assert_eq!(response.status(), http::StatusCode::SERVICE_UNAVAILABLE);

    publisher.publish(Health::Healthy, [("case", "updated")]);
    wait_for_monitor(&observer, Health::Healthy);

    let response = block_on(service.call(Request::new("ignored"))).unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
}
