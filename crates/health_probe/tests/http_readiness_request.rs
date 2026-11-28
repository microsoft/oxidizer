// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public contract tests for the `RequestHandler` HTTP readiness adapter.

mod support;

use futures::executor::block_on;
use health_probe::http::readiness_request::request_handler;
use http::Request;
use http_extensions::HttpBodyBuilder;
use layered::Service;
use support::{STATES, assert_headers, expected, monitor_in};

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
