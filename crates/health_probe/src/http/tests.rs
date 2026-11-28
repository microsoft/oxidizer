// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Contract tests for the HTTP readiness adapters.

use futures::executor::block_on;
use health::{Aggregator, Component, Health, Monitor, Publisher};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, Request, StatusCode};
use http_extensions::HttpBodyBuilder;
use layered::Service;
use observed::Sink;
use tower::ServiceExt;

use crate::http::readiness_request::request_handler;
use crate::http::readiness_tower::service;

const STATES: [Health; 3] = [Health::Healthy, Health::Degraded, Health::Unhealthy];

fn monitor_in(state: Health) -> (Monitor, Component, Publisher) {
    let aggregator = Aggregator::new(Sink::noop());
    let component = aggregator.component("database");
    let mut monitor = aggregator.monitor();
    let publisher = component.publisher_with(state, [("case", state.to_string())]);
    block_on(async {
        loop {
            let report = component.report().await;
            if report.publisher_count(state) == 1 && monitor.state() == state {
                break;
            }
            monitor.changed().await;
        }
    });
    (monitor, component, publisher)
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

#[test]
fn tower_service_reports_readiness() {
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
fn request_handler_reports_readiness() {
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
