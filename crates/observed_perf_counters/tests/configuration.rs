// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cross-platform whole-event deployment and binding validation.

use std::error::Error as _;

use observed::metadata::{EventDescription, InstrumentKind, MetricDescription, MetricValueKind};
use observed::processing::FieldVisitorFn;
use observed::{Event, event};
use observed_perf_counters::PerfCounterProcessor;

#[event("request.completed")]
#[counter(name = "Requests", desc = "completed requests")]
#[counter(bytes_sent, name = "Bytes Sent", unit = "By", desc = "completed request bytes")]
struct RequestCompleted {
    #[unredacted]
    bytes_sent: u32,
}

struct UntypedEvent;

impl Event for UntypedEvent {
    const DESCRIPTION: EventDescription = EventDescription::new("untyped", None, None, None, false, false);
    const METRICS: &'static [MetricDescription] =
        &[MetricDescription::new("count", InstrumentKind::Counter, "", "").with_value_kind(MetricValueKind::U64)];
    fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> std::ops::ControlFlow<()> {
        std::ops::ControlFlow::Continue(())
    }
}

#[event("mixed")]
#[counter(bytes_sent, name = "Bytes Sent")]
#[histogram(depth, name = "Depth")]
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

#[test]
fn manifest_uses_every_declared_counter_and_its_metadata_without_native_startup() {
    let manifest = PerfCounterProcessor::builder()
        .category("MyService")
        .instance("worker-7420")
        .bind_event::<RequestCompleted>()
        .manifest("MyServiceCounters.dll")
        .unwrap();
    assert_eq!(manifest.matches("<counter id=").count(), 2);
    assert!(manifest.contains("name=\"Bytes Sent\""));
    assert!(manifest.contains("name=\"Requests\""));
    assert!(manifest.contains("description=\"completed request bytes [unit: By]\""));
    assert!(manifest.contains("description=\"completed requests\""));
    assert!(manifest.contains("type=\"perf_counter_large_rawcount\""));
    assert!(manifest.contains("counterAttribute name=\"reference\""));
}

#[test]
fn missing_category_or_instance_is_invalid() {
    assert!(
        PerfCounterProcessor::builder()
            .bind_event::<RequestCompleted>()
            .manifest("resources.dll")
            .unwrap_err()
            .is_invalid_configuration()
    );
}

#[test]
fn binding_failure_retains_a_typed_source() {
    let error = PerfCounterProcessor::builder()
        .category("Service")
        .instance("worker")
        .bind_event::<UntypedEvent>()
        .manifest("MyServiceCounters.dll")
        .unwrap_err();
    assert!(error.is_invalid_configuration());
    assert!(error.source().unwrap().is::<observed::processing::MetricBindingError>());
}

#[test]
fn any_unsupported_instrument_rejects_the_entire_event_at_startup() {
    let error = PerfCounterProcessor::builder()
        .category("Service")
        .instance("worker")
        .bind_event::<Mixed>()
        .build()
        .unwrap_err();
    assert!(error.is_invalid_configuration());
    assert!(
        error
            .to_string()
            .contains("metric `Depth` requires an unsigned counter or gauge, or a signed up/down counter")
    );
}

#[test]
fn events_without_metrics_are_rejected() {
    let error = PerfCounterProcessor::builder()
        .category("Service")
        .instance("worker")
        .bind_event::<LogOnly>()
        .manifest("MyServiceCounters.dll")
        .unwrap_err();
    assert_eq!(
        error.source().unwrap().to_string(),
        "cannot bind metrics on event `log.only`: the event must declare at least one metric instrument"
    );
}

#[test]
fn duplicate_event_registration_is_rejected_instead_of_counting_twice() {
    let error = PerfCounterProcessor::builder()
        .category("Service")
        .instance("worker")
        .bind_event::<RequestCompleted>()
        .bind_event::<RequestCompleted>()
        .manifest("MyServiceCounters.dll")
        .unwrap_err();
    assert_eq!(
        error.source().unwrap().to_string(),
        "cannot bind metrics on event `request.completed`: the event is already registered"
    );
}

#[test]
fn later_bindings_cannot_overwrite_the_first_configuration_error() {
    let error = PerfCounterProcessor::builder()
        .category("Service")
        .instance("worker")
        .bind_event::<LogOnly>()
        .bind_event::<RequestCompleted>()
        .bind_event::<Mixed>()
        .manifest("resources.dll")
        .unwrap_err();
    assert_eq!(
        error.source().unwrap().to_string(),
        "cannot bind metrics on event `log.only`: the event must declare at least one metric instrument",
    );
}

#[test]
fn duplicate_metric_names_across_events_are_rejected() {
    let error = PerfCounterProcessor::builder()
        .category("Service")
        .instance("worker")
        .bind_event::<RequestCompleted>()
        .bind_event::<DuplicateName>()
        .manifest("MyServiceCounters.dll")
        .unwrap_err();
    assert!(error.is_invalid_configuration());
}

#[test]
fn public_types_are_send_and_sync() {
    fn assert_traits<T: Send + Sync>() {}
    assert_traits::<observed_perf_counters::PerfCounterProcessor>();
    assert_traits::<observed_perf_counters::PerfCounterProcessorBuilder>();
    assert_traits::<observed_perf_counters::Error>();
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
fn all_supported_integer_kinds_register_in_the_same_native_schema() {
    let manifest = PerfCounterProcessor::builder()
        .category("IntegerMetrics")
        .instance("worker")
        .bind_event::<IntegerMetrics>()
        .manifest("IntegerMetrics.dll")
        .unwrap();
    for name in ["Active", "Bytes", "Depth"] {
        assert!(manifest.contains(&format!("name=\"{name}\"")));
    }
    assert_eq!(manifest.matches("type=\"perf_counter_large_rawcount\"").count(), 3);
}

#[test]
fn unsupported_numeric_types_and_missing_type_metadata_are_startup_errors() {
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
    struct MissingType;
    impl Event for MissingType {
        const DESCRIPTION: EventDescription =
            EventDescription::new("missing.type", Some(std::any::TypeId::of::<Self>()), None, None, true, false);
        const METRICS: &'static [MetricDescription] = &[MetricDescription::new("Unknown", InstrumentKind::Counter, "", "")];
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> std::ops::ControlFlow<()> {
            std::ops::ControlFlow::Continue(())
        }
    }
    let builder = || PerfCounterProcessor::builder().category("InvalidType").instance("worker");
    assert!(
        builder()
            .bind_event::<FloatGauge>()
            .manifest("InvalidType.dll")
            .unwrap_err()
            .is_invalid_configuration()
    );
    assert!(
        builder()
            .bind_event::<SignedGauge>()
            .manifest("InvalidType.dll")
            .unwrap_err()
            .is_invalid_configuration()
    );
    assert!(
        builder()
            .bind_event::<MissingType>()
            .manifest("InvalidType.dll")
            .unwrap_err()
            .is_invalid_configuration()
    );
}

#[cfg(not(all(windows, target_pointer_width = "64")))]
#[test]
fn unsupported_platform_never_returns_a_noop_processor() {
    assert!(
        PerfCounterProcessor::builder()
            .category("MyService")
            .instance("worker")
            .bind_event::<RequestCompleted>()
            .build()
            .unwrap_err()
            .is_unsupported_platform()
    );
}
