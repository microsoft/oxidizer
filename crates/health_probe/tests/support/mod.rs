// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use futures::executor::block_on;
use health::{Aggregator, Component, Health, Monitor, Publisher};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, StatusCode};
use observed::Sink;

pub(crate) const STATES: [Health; 3] = [Health::Healthy, Health::Degraded, Health::Unhealthy];

pub(crate) fn monitor_in(state: Health) -> (Monitor, Component, Publisher) {
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

pub(crate) fn expected(state: Health) -> (StatusCode, &'static str) {
    if state == Health::Healthy {
        (StatusCode::OK, "true")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "false")
    }
}

pub(crate) fn assert_headers(headers: &HeaderMap) {
    assert_eq!(headers[CONTENT_TYPE], "application/json");
    assert_eq!(headers[CACHE_CONTROL], "no-store");
}
