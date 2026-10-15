// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! External Windows definitions and typed event binding validation.

// Every scenario uses real file-system startup. Parsing and native-shaped
// slot/lifetime checks remain covered by the library's isolated Miri suite.
#![cfg(not(miri))]

use std::any::TypeId;
use std::error::Error as _;
use std::io::Write;
use std::ops::ControlFlow;
use std::path::PathBuf;

use observed::metadata::{EventDescription, InstrumentKind, MetricDescription, MetricValueKind};
use observed::processing::{FieldVisitorFn, MetricBindingError};
use observed::{Event, event};
use observed_perf_counters::{Error, PerfCounterProcessor, PerfCounterProcessorBuilder};
use ohno::ErrorExt as _;
use tempfile::NamedTempFile;

const CATEGORY: &str = "Observed Windows consumer test <&\"'> 中文🦀";
const MANIFEST: &str = include_str!("test_data/ConsumerCounters.man");

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
    const DESCRIPTION: EventDescription = EventDescription::new(
        "untyped",
        None,
        None,
        Some(
            MetricDescription::builder("count", InstrumentKind::Counter)
                .value_kind(MetricValueKind::U64)
                .build(),
        ),
        false,
        false,
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
    assert!(
        error
            .to_string()
            .contains("requires an unsigned counter or gauge, or a signed up/down counter")
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
fn file_io_errors_preserve_the_path_and_original_cause() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing.man");
    let error = PerfCounterProcessor::builder().manifest_file(&path).unwrap_err();
    assert!(error.to_string().contains("missing.man"));
    assert_eq!(error.find_source::<std::io::Error>().unwrap().kind(), std::io::ErrorKind::NotFound);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "native startup is covered natively; parser and mock provider lifetimes run under Miri"
)]
fn file_is_loaded_once_and_not_needed_after_initialization() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(MANIFEST.as_bytes()).unwrap();
    let loaded = PerfCounterProcessor::builder().manifest_file(file.path()).unwrap();
    file.write_all(b"this does not modify the already parsed definitions").unwrap();
    drop(file);
    let result = loaded
        .category(CATEGORY)
        .instance("loaded-once")
        .bind_event::<RequestCompleted>()
        .build();
    #[cfg(all(windows, target_pointer_width = "64"))]
    result.unwrap();
    #[cfg(not(all(windows, target_pointer_width = "64")))]
    assert!(result.unwrap_err().is_unsupported_platform());
}

#[test]
fn malformed_xml_invalid_utf8_and_oversized_files_are_not_accepted() {
    for contents in [b"<broken>".as_slice(), &[0xff], &vec![b' '; 1024 * 1024 + 1]] {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(contents).unwrap();
        PerfCounterProcessor::builder().manifest_file(file.path()).unwrap_err();
    }
}

#[test]
fn valid_xml_at_the_file_size_limit_is_accepted_but_one_extra_byte_is_rejected() {
    const LIMIT: usize = 1024 * 1024;
    for size in [LIMIT - 1, LIMIT, LIMIT + 1] {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(MANIFEST.as_bytes()).unwrap();
        file.write_all(&vec![b' '; size - MANIFEST.len()]).unwrap();
        let result = PerfCounterProcessor::builder().manifest_file(file.path());
        if size <= LIMIT {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert!(error.is_invalid_configuration());
            assert!(error.to_string().contains("manifest exceeds one MiB"));
        }
    }
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
        const DESCRIPTION: EventDescription = EventDescription::new("missing.type", Some(TypeId::of::<Self>()), None, None, true, false)
            .with_fields(&[observed::metadata::FieldDescriptor::new(
                "value",
                None,
                Some(observed::metadata::MetricFieldEntry::instrument(
                    "value",
                    MetricDescription::new("Unknown", InstrumentKind::Counter, "", ""),
                )),
            )
            .with_field_index(0)]);
        fn visit_fields(&self, _: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }
    }
    for builder in [
        builder().bind_event::<FloatGauge>(),
        builder().bind_event::<SignedGauge>(),
        builder().bind_event::<MissingType>(),
    ] {
        assert!(builder.build().unwrap_err().is_invalid_configuration());
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
