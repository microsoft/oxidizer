// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use health::{Aggregator, Component, Health, Monitor, Publisher};
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderMap, StatusCode};
use observed::Sink;
use testing_aids::is_mutation_testing;

pub(crate) const STATES: [Health; 3] = [Health::Healthy, Health::Degraded, Health::Unhealthy];

pub(crate) fn monitor_in(state: Health) -> (Monitor, Component, Publisher) {
    let aggregator = Aggregator::new(Sink::noop());
    let component = aggregator.component("database");
    let monitor = aggregator.monitor();
    let publisher = component.publisher_with(state, [("case", state.to_string())]);
    wait_until(|| monitor.state() == state);
    (monitor, component, publisher)
}

pub(crate) fn wait_for_component(component: &Component, state: Health) {
    wait_until(|| component.state() == state);
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

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = (!is_mutation_testing()).then(|| std::time::Instant::now() + std::time::Duration::from_secs(5));
    while !condition() {
        assert!(
            deadline.is_none_or(|deadline| std::time::Instant::now() < deadline),
            "health state did not update before the test deadline"
        );
        std::thread::yield_now();
    }
}
