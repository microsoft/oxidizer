// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public contract tests for the Tower HTTP readiness adapter.

mod support;

use futures::executor::block_on;
use health_probe::http::readiness_tower::service;
use http::Request;
use support::{STATES, assert_headers, expected, monitor_in};
use tower::ServiceExt;

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
