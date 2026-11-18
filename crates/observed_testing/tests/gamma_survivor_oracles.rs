// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Negative controls for the test harness's comparison oracles.

use observed::__private::EnrichmentEntry;
use observed::metadata::{EventDescription, InstrumentKind, LogDescription, MetricDescription};
use observed::{Event, Severity, emit, event};
use observed_testing::{ExpectedEnrichmentEntry, ExpectedEvent, ExpectedEventDescription, TEST_ID, test_emitter};

#[event("gamma.log")]
#[info("body")]
struct LogEvent {
    #[unredacted]
    value: i64,
}

#[event("gamma.metric")]
#[info("metric body")]
#[counter(name = "gamma.count")]
struct MetricEvent;

#[event("gamma.round2.signal_less")]
#[counter(name = "gamma.round2.count")]
struct SignalLessEvent;

#[event("nov4.signal_less")]
struct NoSignalEvent;

#[test]
fn nov4_without_severity_does_not_implicitly_require_a_metric() {
    let (sink, processor) = test_emitter(TEST_ID);
    emit!(sink, NoSignalEvent);
    let event = processor.single_event();
    assert!(!event.contains_metric());
    let expected = ExpectedEvent::without_severity("nov4.signal_less");
    assert_eq!(event, expected);
    assert_eq!(expected, event);
    assert_ne!(event, ExpectedEvent::without_severity("nov4.signal_less").metric());
}

#[test]
fn gamma_round2_without_severity_does_not_expect_the_disabled_flag() {
    let (sink, processor) = test_emitter(TEST_ID);
    emit!(sink, SignalLessEvent);
    let event = processor.single_event();
    assert_eq!(event, ExpectedEvent::without_severity("gamma.round2.signal_less").metric());
    assert_ne!(
        event,
        ExpectedEvent::without_severity("gamma.round2.signal_less").metric().disabled()
    );
}

#[test]
fn gamma_survivor_captured_event_equality_checks_every_dimension_in_both_directions() {
    let (sink, processor) = test_emitter(TEST_ID);
    assert!(processor.is_empty());
    emit!(sink, LogEvent { value: 7 });
    assert!(!processor.is_empty());
    let event = processor.single_event();
    assert!(!event.contains_metric());
    let positive = ExpectedEvent::new("gamma.log", Severity::Info)
        .body("body")
        .dimension("value", 7_i64);
    assert_eq!(event, positive);
    assert_eq!(positive, event);
    let negatives = [
        ExpectedEvent::new("wrong", Severity::Info).body("body").dimension("value", 7_i64),
        ExpectedEvent::new("gamma.log", Severity::Warn)
            .body("body")
            .dimension("value", 7_i64),
        ExpectedEvent::new("gamma.log", Severity::Info)
            .body("wrong")
            .dimension("value", 7_i64),
        ExpectedEvent::new("gamma.log", Severity::Info)
            .body("body")
            .dimension("value", 8_i64),
        ExpectedEvent::new("gamma.log", Severity::Info)
            .body("body")
            .dimension("value", 7_i64)
            .metric(),
        ExpectedEvent::new("gamma.log", Severity::Info)
            .body("body")
            .dimension("value", 7_i64)
            .disabled(),
    ];
    for expected in negatives {
        assert_ne!(event, expected);
        assert_ne!(expected, event);
    }
}

#[test]
fn gamma_survivor_enrichment_equality_checks_key_value_exclusion_and_metric_key() {
    let entry = EnrichmentEntry::unclassified("key", 7_i64);
    let positive = ExpectedEnrichmentEntry::new("key", 7_i64);
    assert_eq!(entry, positive);
    assert_eq!(positive, entry);
    for expected in [
        ExpectedEnrichmentEntry::new("other", 7_i64),
        ExpectedEnrichmentEntry::new("key", 8_i64),
        ExpectedEnrichmentEntry::new("key", 7_i64).exclude_from_logs(),
        ExpectedEnrichmentEntry::new("key", 7_i64).metric_dimension_named("metric"),
    ] {
        assert_ne!(entry, expected);
        assert_ne!(expected, entry);
    }
    let metric = EnrichmentEntry::unclassified("key", 7_i64).with_metric_dimension("metric");
    assert_eq!(metric, ExpectedEnrichmentEntry::new("key", 7_i64).metric_dimension_named("metric"));
    assert_ne!(metric, ExpectedEnrichmentEntry::new("key", 7_i64).metric_dimension_named("other"));
}

#[test]
fn gamma_survivor_description_equality_checks_each_flag_and_metric_identity() {
    let description = MetricEvent::DESCRIPTION;
    let expected = ExpectedEventDescription::new("gamma.metric", Severity::Info)
        .body("metric body")
        .log()
        .event_metric("gamma.count", InstrumentKind::Counter);
    assert_eq!(description, expected);
    assert_eq!(expected, description);
    let negatives = [
        ExpectedEventDescription::new("other", Severity::Info)
            .body("metric body")
            .log()
            .event_metric("gamma.count", InstrumentKind::Counter),
        ExpectedEventDescription::new("gamma.metric", Severity::Warn)
            .body("metric body")
            .log()
            .event_metric("gamma.count", InstrumentKind::Counter),
        ExpectedEventDescription::new("gamma.metric", Severity::Info)
            .body("wrong")
            .log()
            .event_metric("gamma.count", InstrumentKind::Counter),
        ExpectedEventDescription::new("gamma.metric", Severity::Info)
            .body("metric body")
            .event_metric("gamma.count", InstrumentKind::Counter),
        ExpectedEventDescription::new("gamma.metric", Severity::Info)
            .body("metric body")
            .log(),
        ExpectedEventDescription::new("gamma.metric", Severity::Info)
            .body("metric body")
            .log()
            .event_metric("gamma.count", InstrumentKind::Counter)
            .disabled(),
        ExpectedEventDescription::new("gamma.metric", Severity::Info)
            .body("metric body")
            .log()
            .event_metric("other", InstrumentKind::Counter),
        ExpectedEventDescription::new("gamma.metric", Severity::Info)
            .body("metric body")
            .log()
            .event_metric("gamma.count", InstrumentKind::Gauge),
    ];
    for expected in negatives {
        assert_ne!(description, expected);
        assert_ne!(expected, description);
    }
    let no_log = EventDescription::new(
        "gamma.metric",
        None,
        None,
        Some(MetricDescription::new("gamma.count", InstrumentKind::Counter, "", "")),
        false,
        false,
    );
    assert_eq!(
        no_log,
        ExpectedEventDescription::new("gamma.metric", Severity::Info).event_metric("gamma.count", InstrumentKind::Counter)
    );
    assert_ne!(
        no_log,
        ExpectedEventDescription::new("gamma.metric", Severity::Info)
            .log()
            .event_metric("gamma.count", InstrumentKind::Counter)
    );
    let no_metric = EventDescription::new(
        "gamma.metric",
        None,
        Some(LogDescription::new("gamma.metric", Severity::Info, Some("metric body"))),
        None,
        true,
        false,
    );
    assert_ne!(no_metric, expected);
}
