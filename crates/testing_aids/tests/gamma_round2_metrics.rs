// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Metric assertion helpers must reject missing attributes and wrong cardinality.

use opentelemetry::KeyValue;
use opentelemetry::metrics::MeterProvider;
use testing_aids::MetricTester;

testing_aids::init_tracing!();

#[test]
fn gamma_round2_collection_flushes_recorded_attributes() {
    let tester = MetricTester::new();
    let counter = tester.meter_provider().meter("round2").u64_counter("requests").build();
    let attribute = KeyValue::new("route", "round2");
    counter.add(1, std::slice::from_ref(&attribute));
    assert_eq!(tester.collect_attributes(), [attribute]);
}

#[test]
#[should_panic(expected = "not found in collected attributes")]
fn gamma_round2_containment_rejects_an_absent_attribute() {
    MetricTester::new().assert_attributes_contain(&[KeyValue::new("missing", true)]);
}

#[test]
#[should_panic(expected = "expected 1 attributes, got 0")]
fn gamma_round2_exact_assertion_checks_the_requested_count() {
    MetricTester::new().assert_attributes(&[], Some(1));
}

#[test]
#[should_panic(expected = "not found in collected attributes")]
fn gamma_round2_exact_assertion_checks_values_without_a_count() {
    MetricTester::new().assert_attributes(&[KeyValue::new("missing", 42)], None);
}

#[test]
fn gamma_round2_event_formatting_propagates_fmt_errors_without_panicking() {
    use std::fmt;

    use tracing_subscriber::util::SubscriberInitExt;

    struct Fails;

    impl fmt::Debug for Fails {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    let capture = testing_aids::tracing_logs::Capture::new();
    let _guard = capture.subscriber().set_default();
    tracing::info!(value = ?Fails, "discard this incomplete event");
    tracing::info!(target: "round2", "healthy event");
    let output = capture.output();
    assert!(output.contains("healthy event"), "{output}");
    assert!(!output.contains('\u{1b}'), "{output}");
}
