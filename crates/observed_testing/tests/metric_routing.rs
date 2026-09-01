// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for metric signal routing via instrument-specific metric attributes.
//!
//! Covers DESIGN.md requirements:
//! - A metric is produced only when an event field is marked as a metric type
//! - Events without metric fields produce LOG only (no METRIC signal)
//! - Events with metric fields produce both LOG and METRIC signals
//! - Multiple metric fields produce multiple metric descriptions
//! - `exclude_from_logs` produces METRIC-only signals
//!
//! A metric instrument's value field must be `#[unredacted]` and a numeric
//! primitive. Both rules are enforced by `#[event(...)]` at compile time, so
//! their negative cases live in the macro's own tests; what is checked here is
//! that the accepted shapes actually reach a processor as a measurement.

use observed::{Event, Severity, Value, emit, event};
use observed_testing::types::PublicString;
use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};

#[expect(clippy::inline_always, reason = "this is a one line assertion")]
#[track_caller]
#[inline(always)]
fn assert_f64_eq(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-12);
}

#[test]
fn event_without_metrics_is_log_only() {
    #[event("test.probe")]
    #[info]
    pub(crate) struct LogEvent {
        #[unredacted]
        pub value: i64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(sink, LogEvent { value: 1 });

    let event = processor.single_event();
    assert_eq!(event, ExpectedEvent::new("test.probe", Severity::Info).dimension("value", 1i64),);
    assert_eq!(event.field_metrics().len(), 0);
}

#[test]
fn event_with_metric_field() {
    #[event("http.server.request")]
    #[info("HTTP request handled")]
    #[histogram(duration, name = "http.server.request.duration")]
    pub(crate) struct HttpServerRequest {
        #[unredacted]
        pub status: i64,
        #[unredacted]
        pub retries: u32,
        #[unredacted]
        pub cache_hit: bool,
        #[unredacted]
        pub duration: f64,
        pub method: PublicString,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        HttpServerRequest {
            status: 200,
            retries: 0,
            cache_hit: false,
            duration: 0.042,
            method: PublicString("GET".into()),
        }
    );

    let event = processor.single_event();
    assert_eq!(event.name(), "http.server.request");
    assert_eq!(event.severity(), Some(Severity::Info));
    assert_eq!(event.body(), Some("HTTP request handled"));
    assert!(event.description().is_log());
    assert!(event.description().contains_metrics());

    let dimensions = event.dimensions();
    assert_eq!(
        dimensions,
        vec![
            ("cache_hit".to_owned(), Value::from(false)),
            ("method".to_owned(), Value::from("GET")),
            ("retries".to_owned(), Value::from(0i64)),
            ("status".to_owned(), Value::from(200i64)),
        ]
    );

    let field_metrics = event.field_metrics();
    assert_eq!(field_metrics.len(), 1);
    assert_eq!(field_metrics[0].field_key(), "duration");
    assert_eq!(field_metrics[0].metric().instrument_name(), "http.server.request.duration");
    assert_eq!(field_metrics[0].metric().kind(), observed::metadata::InstrumentKind::Histogram);
    assert_f64_eq(field_metrics[0].value(), 0.042);
}

#[test]
fn multiple_metrics_on_single_event() {
    #[event("http.batch")]
    #[info]
    #[histogram(duration_ms, name = "http.batch.duration")]
    #[gauge(size, name = "http.batch.size")]
    pub(crate) struct HttpBatch {
        #[unredacted]
        pub duration_ms: f64,
        #[unredacted]
        pub size: i64,
        #[unredacted]
        pub request_id: i64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        HttpBatch {
            duration_ms: 100.0,
            size: 50,
            request_id: 1
        }
    );

    let event = processor.single_event();
    let field_metrics = event.field_metrics();
    assert_eq!(field_metrics.len(), 2);

    let duration_metric = field_metrics
        .iter()
        .find(|m| m.metric().instrument_name() == "http.batch.duration")
        .expect("missing http.batch.duration metric");
    assert_eq!(duration_metric.metric().kind(), observed::metadata::InstrumentKind::Histogram);
    assert_eq!(duration_metric.field_key(), "duration_ms");
    assert_f64_eq(duration_metric.value(), 100.0);

    let size_metric = field_metrics
        .iter()
        .find(|m| m.metric().instrument_name() == "http.batch.size")
        .expect("missing http.batch.size metric");
    assert_eq!(size_metric.metric().kind(), observed::metadata::InstrumentKind::Gauge);
    assert_eq!(size_metric.field_key(), "size");
    assert_f64_eq(size_metric.value(), 50.0);

    assert_eq!(event.dimensions(), vec![("request_id".to_owned(), Value::from(1i64))]);
}

#[test]
fn metric_value_field_is_separated_from_captured_attributes() {
    #[event("query.executed")]
    #[info]
    #[histogram(duration, name = "query.duration")]
    pub(crate) struct QueryExecuted {
        #[unredacted]
        pub duration: f64,
        #[unredacted]
        pub query_hash: i64,
        pub db_name: PublicString,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        QueryExecuted {
            duration: 0.012,
            query_hash: 42,
            db_name: PublicString("main_db".into()),
        }
    );

    let event = processor.single_event();
    assert!(event.description().is_log());
    assert!(event.description().contains_metrics());
    assert_eq!(
        event.dimensions(),
        vec![
            ("db_name".to_owned(), Value::from("main_db")),
            ("query_hash".to_owned(), Value::from(42i64)),
        ]
    );

    let field_metrics = event.field_metrics();
    assert_eq!(field_metrics.len(), 1);
    assert_eq!(field_metrics[0].field_key(), "duration");
    assert_eq!(field_metrics[0].metric().instrument_name(), "query.duration");
    assert_eq!(field_metrics[0].metric().kind(), observed::metadata::InstrumentKind::Histogram);
    assert_f64_eq(field_metrics[0].value(), 0.012);
}

// ---------------------------------------------------------------------------
// exclude_from_logs - metric-only events
// ---------------------------------------------------------------------------

#[test]
fn exclude_from_logs_event_is_metric_only() {
    #[event("system.memory.usage")]
    #[counter(name = "system.memory.usage")]
    #[gauge(bytes, name = "system.memory.usage")]
    pub(crate) struct MemoryUsage {
        #[unredacted]
        pub bytes: i64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(sink, MemoryUsage { bytes: 1024 });

    let event = processor.single_event();
    assert_eq!(event.name(), "system.memory.usage");
    // Metric-only: no severity attribute, so the event carries no log signal
    // and therefore no severity at all.
    assert_eq!(event.severity(), None);
    assert_eq!(event.body(), None);
    assert_eq!(event.dimensions(), Vec::<(String, Value)>::new());

    let field_metrics = event.field_metrics();
    assert_eq!(field_metrics.len(), 1);
    assert_eq!(field_metrics[0].field_key(), "bytes");
    assert_eq!(field_metrics[0].metric().instrument_name(), "system.memory.usage");
    assert_eq!(field_metrics[0].metric().kind(), observed::metadata::InstrumentKind::Gauge);
    assert_f64_eq(field_metrics[0].value(), 1024.0);

    let desc = MemoryUsage::DESCRIPTION;
    assert!(!desc.is_log());
    assert!(desc.contains_metrics());
    assert!(desc.metric().is_some());
}

#[test]
fn event_level_metric_is_exposed_on_captured_event() {
    #[event("fetch.request.get")]
    #[counter(name = "fetch.request.get")]
    pub(crate) struct GetRequest {
        #[dimension(metric = "bytes")]
        #[unredacted]
        pub bytes: i64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(sink, GetRequest { bytes: 2048 });

    let event = processor.single_event();
    assert_eq!(event.name(), "fetch.request.get");
    // Metric-only: no severity attribute, so no log signal and no severity.
    assert_eq!(event.severity(), None);
    assert_eq!(event.body(), None);
    // The captured event is distinguishable from an explicit Info log: it
    // matches the no-severity expectation and not an `Info` one.
    assert_eq!(
        event,
        ExpectedEvent::without_severity("fetch.request.get")
            .dimension("bytes", 2048i64)
            .metric()
    );
    assert_ne!(
        event,
        ExpectedEvent::new("fetch.request.get", Severity::Info)
            .dimension("bytes", 2048i64)
            .metric()
    );
    assert!(event.contains_metric());
    assert_eq!(event.field_metrics().len(), 0);
    assert_eq!(event.dimensions(), vec![("bytes".to_owned(), Value::from(2048i64))]);

    let event_metric = event.event_metric().expect("event metric");
    assert_eq!(event_metric.instrument_name(), "fetch.request.get");
    assert_eq!(event_metric.kind(), observed::metadata::InstrumentKind::Counter);
}

// ================================================================================================
// The metric value contract: an accepted value field really does record a measurement
// ================================================================================================

/// A `u64` counter records its measurement.
///
/// `u64` is the natural type for a byte or request counter, and it is the width
/// `Value` could not carry before: the expansion failed with
/// `Value: From<u64> is not satisfied`.
#[test]
fn u64_counter_records_its_value() {
    #[event("bytes.written")]
    #[info]
    #[counter(bytes)]
    struct BytesWritten {
        #[unredacted]
        bytes: u64,
    }

    let (sink, processor) = test_emitter(TEST_ID);
    emit!(sink, BytesWritten { bytes: 7 });

    let event = processor.single_event();
    let field_metrics = event.field_metrics();
    assert_eq!(field_metrics.len(), 1);
    assert_f64_eq(field_metrics[0].value(), 7.0);
}

/// A `u64` above `i64::MAX` survives intact.
///
/// This is the case that folding `u64` into `Value::I64` would have corrupted,
/// which is why `Value::U64` exists.
#[test]
fn u64_counter_preserves_values_above_i64_max() {
    #[event("huge.counter")]
    #[info]
    #[counter(n)]
    struct HugeCounter {
        #[unredacted]
        n: u64,
    }

    let big = u64::MAX;
    let (sink, processor) = test_emitter(TEST_ID);
    emit!(sink, HugeCounter { n: big });

    // A metric value field is not a dimension, so read it back off the
    // instrument. Folding `u64` into `I64` would have wrapped this to -1.
    let recorded = processor.single_event().field_metrics()[0].value();
    assert!(recorded > 1.8e19, "u64::MAX must not be truncated through i64, got {recorded}");
}

/// Every integer width `Value` accepts round-trips through a gauge.
#[test]
fn gauge_records_each_supported_width() {
    #[event("widths.gauge")]
    #[info]
    #[gauge(small_signed)]
    struct SmallSigned {
        #[unredacted]
        small_signed: i8,
    }

    #[event("widths.gauge.unsigned")]
    #[info]
    #[gauge(small_unsigned)]
    struct SmallUnsigned {
        #[unredacted]
        small_unsigned: u16,
    }

    #[event("widths.gauge.size")]
    #[info]
    #[gauge(count)]
    struct SizeGauge {
        #[unredacted]
        count: usize,
    }

    let (sink, processor) = test_emitter(TEST_ID);
    emit!(sink, SmallSigned { small_signed: -5 });
    emit!(sink, SmallUnsigned { small_unsigned: 65535 });
    emit!(sink, SizeGauge { count: 12 });

    let events = processor.events();
    assert_eq!(events.len(), 3);
    assert_f64_eq(events[0].field_metrics()[0].value(), -5.0);
    assert_f64_eq(events[1].field_metrics()[0].value(), 65535.0);
    assert_f64_eq(events[2].field_metrics()[0].value(), 12.0);
}

/// A float gauge records a fractional measurement rather than a truncated one.
#[test]
fn gauge_records_a_float_value() {
    #[event("temperature")]
    #[info]
    #[gauge(celsius)]
    struct Temperature {
        #[unredacted]
        celsius: f64,
    }

    let (sink, processor) = test_emitter(TEST_ID);
    emit!(sink, Temperature { celsius: 21.5 });

    assert_f64_eq(processor.single_event().field_metrics()[0].value(), 21.5);
}
