// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! External Windows definitions and typed event binding validation.

// Public startup reads a checked-in fixture. Injected reader, parsing, and
// native-shaped lifetime checks run in the library's isolated Miri suite.
#![cfg(not(miri))]

use std::error::Error as _;
use std::ops::ControlFlow;
use std::path::PathBuf;

use observed::metadata::{EventDescription, InstrumentKind, MetricValueKind};
use observed::processing::{FieldVisitorFn, MetricBindingError};
use observed::{Event, event};
use observed_perf_counters::{Error, PerfCounterProcessor, PerfCounterProcessorBuilder};
use ohno::ErrorExt as _;

const CATEGORY: &str = "Observed Windows consumer test <&\"'> 中文🦀";

testing_aids::init_tracing!();

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("test_data")
        .join("ConsumerCounters.man")
}

#[expect(clippy::unwrap_used, reason = "test fixture supplies a known valid external manifest")]
fn builder() -> PerfCounterProcessorBuilder {
    PerfCounterProcessor::builder()
        .manifest_file(manifest_path())
        .unwrap()
        .category(CATEGORY)
        .instance("worker")
}

#[expect(clippy::unwrap_used, reason = "test assertion requires the preserved binding cause")]
fn primary_binding_message(error: &Error) -> String {
    error
        .find_source::<MetricBindingError>()
        .unwrap()
        .message()
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

#[event("request.completed")]
#[counter(name = "Requests", desc = "application metadata is not the Windows schema")]
#[counter(bytes_sent, name = "Bytes Sent", unit = "By")]
struct RequestCompleted {
    #[unredacted]
    bytes_sent: u32,
}

struct UntypedEvent;

impl Event for UntypedEvent {
    const DESCRIPTION: EventDescription = observed::__private::event_description(
        "untyped",
        None,
        None,
        Some(observed::__private::metric_description(
            "count",
            InstrumentKind::Counter,
            "",
            "",
            MetricValueKind::U64,
        )),
        false,
        &[],
    );

    fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}

#[event("mixed")]
#[counter(bytes_sent, name = "Bytes Sent")]
#[histogram(depth, name = "Queue Depth")]
struct Mixed {
    #[unredacted]
    bytes_sent: u32,
    #[unredacted]
    depth: u32,
}

#[event("log.only")]
#[info]
struct LogOnly;

#[event("request.other")]
#[counter(value, name = "Bytes Sent")]
struct DuplicateName {
    #[unredacted]
    value: u64,
}

#[event("unmapped")]
#[counter(value, name = "Not in the manifest")]
struct Unmapped {
    #[unredacted]
    value: u64,
}

#[test]
fn missing_manifest_category_or_instance_is_an_explicit_error() {
    for builder in [
        PerfCounterProcessor::builder().category(CATEGORY).instance("worker"),
        PerfCounterProcessor::builder()
            .manifest_file(manifest_path())
            .unwrap()
            .instance("worker"),
        PerfCounterProcessor::builder()
            .manifest_file(manifest_path())
            .unwrap()
            .category(CATEGORY),
    ] {
        assert!(
            builder
                .bind_event::<RequestCompleted>()
                .build()
                .unwrap_err()
                .is_invalid_configuration()
        );
    }
}

#[test]
fn category_selects_an_existing_counter_set_instead_of_creating_one() {
    let error = builder()
        .category("Does not exist")
        .bind_event::<RequestCompleted>()
        .build()
        .unwrap_err();
    assert!(error.is_invalid_configuration());
    assert!(error.to_string().contains("Does not exist"));
}

#[test]
fn missing_instrument_name_fails_without_starting_a_provider() {
    let error = builder().bind_event::<Unmapped>().build().unwrap_err();
    assert!(error.is_invalid_configuration());
    assert!(error.to_string().contains("Not in the manifest"));
}

#[test]
fn binding_failure_retains_a_typed_source() {
    let error = builder().bind_event::<UntypedEvent>().build().unwrap_err();
    assert!(error.is_invalid_configuration());
    assert!(error.source().unwrap().is::<MetricBindingError>());
}

#[test]
fn any_unsupported_instrument_rejects_the_entire_event_at_startup() {
    let error = builder().bind_event::<Mixed>().build().unwrap_err();
    assert!(error.is_invalid_configuration());
    assert_eq!(
        error.message().lines().next().unwrap(),
        "invalid performance-counter configuration: metric `Queue Depth` requires an unsigned counter or gauge, or a signed up/down counter (received Histogram/U64)"
    );
}

#[test]
fn events_without_metrics_are_rejected() {
    let error = builder().bind_event::<LogOnly>().build().unwrap_err();
    assert_eq!(
        primary_binding_message(&error),
        "cannot bind metrics on event `log.only`: the event must declare at least one metric instrument"
    );
}

#[test]
fn duplicate_event_registration_is_rejected_instead_of_counting_twice() {
    let error = builder()
        .bind_event::<RequestCompleted>()
        .bind_event::<RequestCompleted>()
        .build()
        .unwrap_err();
    assert_eq!(
        primary_binding_message(&error),
        "cannot bind metrics on event `request.completed`: the event is already registered"
    );
}

#[test]
fn later_bindings_cannot_overwrite_the_first_configuration_error() {
    let error = builder()
        .bind_event::<LogOnly>()
        .bind_event::<RequestCompleted>()
        .bind_event::<Mixed>()
        .build()
        .unwrap_err();
    assert_eq!(
        primary_binding_message(&error),
        "cannot bind metrics on event `log.only`: the event must declare at least one metric instrument"
    );
}

#[test]
fn two_events_cannot_accidentally_write_to_the_same_declared_counter() {
    let error = builder()
        .bind_event::<RequestCompleted>()
        .bind_event::<DuplicateName>()
        .build()
        .unwrap_err();
    assert!(error.is_invalid_configuration());
}

#[test]
fn public_types_are_send_and_sync() {
    fn assert_traits<T: Send + Sync>() {}
    assert_traits::<PerfCounterProcessor>();
    assert_traits::<PerfCounterProcessorBuilder>();
    assert_traits::<Error>();
}

#[event("integer.metrics")]
#[counter(bytes_sent, name = "Bytes")]
#[updown_counter(active, name = "Active")]
#[gauge(depth, name = "Depth")]
struct IntegerMetrics {
    #[unredacted]
    bytes_sent: u32,
    #[unredacted]
    active: i32,
    #[unredacted]
    depth: u32,
}

#[test]
#[cfg_attr(
    miri,
    ignore = "native startup is covered natively; supported preparation is tested by schema unit tests"
)]
fn multiple_sets_and_supported_integer_kinds_are_selected_from_the_same_file() {
    let result = builder()
        .category("IntegerMetrics")
        .instance("integer-test")
        .bind_event::<IntegerMetrics>()
        .build();
    #[cfg(all(windows, target_pointer_width = "64"))]
    result.unwrap();
    #[cfg(not(all(windows, target_pointer_width = "64")))]
    assert!(result.unwrap_err().is_unsupported_platform());
}

#[test]
fn unsupported_numeric_types_are_startup_errors() {
    #[event("float.gauge")]
    #[gauge(value, name = "Float")]
    struct FloatGauge {
        #[unredacted]
        value: f64,
    }
    #[event("signed.gauge")]
    #[gauge(value, name = "Signed")]
    struct SignedGauge {
        #[unredacted]
        value: i64,
    }
    for (builder, expected) in [
        (
            builder().bind_event::<FloatGauge>(),
            "invalid performance-counter configuration: metric `Float` requires an unsigned counter or gauge, or a signed up/down counter (received Gauge/F64)",
        ),
        (
            builder().bind_event::<SignedGauge>(),
            "invalid performance-counter configuration: metric `Signed` requires an unsigned counter or gauge, or a signed up/down counter (received Gauge/I64)",
        ),
    ] {
        let error = builder.build().unwrap_err();
        assert_eq!(error.message().lines().next().unwrap(), expected);
    }
}

#[cfg(not(all(windows, target_pointer_width = "64")))]
#[test]
fn unsupported_platform_never_returns_a_noop_processor() {
    assert!(
        builder()
            .bind_event::<RequestCompleted>()
            .build()
            .unwrap_err()
            .is_unsupported_platform()
    );
}
