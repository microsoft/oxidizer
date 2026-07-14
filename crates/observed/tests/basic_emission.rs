// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for basic event emission: struct -> `emit!` -> processor -> captured event.
//!
//! Covers DESIGN.md requirements:
//! - Typed, compile-time validated events
//! - Single call for all signals
//! - Source location capture
//! - Zero-cost when inactive (noop sink)

use observed::{Event, Severity, Sink, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::types::{PiiString, PublicBool, PublicI64, PublicString};
use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};

#[derive(Debug, Event)]
#[event(name = "app.warning")]
#[log(severity = warn, message = "Something went wrong")]
struct AppWarning {
    code: PublicI64,
    recoverable: PublicBool,
}

#[test]
fn log_event() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        AppWarning {
            code: PublicI64(42),
            recoverable: PublicBool(true)
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("app.warning", Severity::Warn)
            .body("Something went wrong")
            .dimension("code", "42")
            .dimension("recoverable", "true")
            .log(),
    );
}

#[test]
fn log_and_metric_event() {
    #[derive(Debug, Event)]
    #[event(name = "http.server.request")]
    #[log(severity = info, message = "HTTP request handled")]
    #[metric(kind = histogram, field = duration, name = "http.server.request.duration")]
    struct HttpServerRequest {
        status: PublicI64,
        retries: PublicI64,
        cache_hit: PublicBool,
        // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
        #[unredacted]
        duration: f64,
        method: PublicString,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        HttpServerRequest {
            status: PublicI64(200),
            retries: PublicI64(3),
            cache_hit: PublicBool(false),
            duration: 0.042,
            method: PublicString("GET".into()),
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("http.server.request", Severity::Info)
            .body("HTTP request handled")
            .dimension("cache_hit", "false")
            .dimension("duration", 0.042)
            .dimension("method", "GET")
            .dimension("retries", "3")
            .dimension("status", "200")
            .log()
            .metric()
    );
}

#[test]
fn event_with_custom_field_name() {
    #[derive(Debug, Event)]
    #[event(name = "db.error")]
    #[log(severity = error)]
    pub(crate) struct DbError {
        #[dimension(log = "db.system")]
        pub system_id: PublicI64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(
        sink,
        DbError {
            system_id: PublicI64(5), // #[log(name = "db.system")]
        }
    );

    // The field `system_id` is renamed to `db.system` via #[log(name = "db.system")]
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("db.error", Severity::Error).dimension("db.system", "5").log(),
    );
}

#[test]
fn emit_already_constructed_event() {
    let (sink, processor) = test_emitter(TEST_ID);

    let event = ProbeEvent::new(204);
    emit!(sink, event);

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info).dimension("value", "204").log()
    );
}

#[test]
fn emit_event_with_no_fields() {
    #[derive(Debug, Event)]
    #[event(name = "internal.heartbeat")]
    #[log(severity = trace)]
    pub(crate) struct Heartbeat;

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(sink, Heartbeat);

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("internal.heartbeat", Severity::Trace).log(),
    );
}

#[test]
fn source_file_and_line() {
    let (sink, processor) = test_emitter(TEST_ID);

    emit!(sink, ProbeEvent::new(1));

    let event = processor.single_event();
    assert_eq!(event.source_file(), Some(file!()));
    assert_eq!(event.source_line(), Some(line!() - 4));
}

#[test]
fn noop_emitter_skips_construction() {
    #[derive(Debug, Event)]
    #[event(name = "panic.event")]
    #[log(severity = info)]
    pub(crate) struct PanicingEvent;

    impl PanicingEvent {
        pub(crate) fn new() -> Self {
            panic!("This event should never be constructed");
        }
    }

    let noop = Sink::noop();
    emit!(noop, PanicingEvent::new());
}

#[test]
fn multiple_events_accumulate() {
    #[derive(Debug, Event)]
    #[event(name = "cache.hit")]
    #[log(severity = debug)]
    #[metric(kind = histogram, field = lookup_ms, name = "cache.lookup.duration")]
    struct CacheHit {
        key_hash: PublicI64,
        size_bytes: PublicI64,
        // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
        #[unredacted]
        lookup_ms: f64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    emit!(sink, ProbeEvent::new(1));
    emit!(
        sink,
        AppWarning {
            code: PublicI64(1),
            recoverable: PublicBool(false)
        }
    );
    emit!(
        sink,
        CacheHit {
            key_hash: PublicI64(42),
            size_bytes: PublicI64(1024),
            lookup_ms: 0.5
        }
    );

    let events = processor.events();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0],
        ExpectedEvent::new("test.probe", Severity::Info).dimension("value", "1").log()
    );
    assert_eq!(
        events[1],
        ExpectedEvent::new("app.warning", Severity::Warn)
            .body("Something went wrong")
            .dimension("code", "1")
            .dimension("recoverable", "false")
            .log()
    );
    assert_eq!(
        events[2],
        ExpectedEvent::new("cache.hit", Severity::Debug)
            .dimension("key_hash", "42")
            .dimension("lookup_ms", 0.5f64)
            .dimension("size_bytes", "1024")
            .log()
            .metric()
    );
}

#[test]
#[expect(clippy::struct_field_names, reason = "test struct for all dimension types")]
fn dimensions_types() {
    #[derive(Debug, Event)]
    #[event(name = "dimension.types")]
    #[log(severity = info)]
    pub(crate) struct DimensionTypes {
        #[unredacted]
        i32_field: i32,
        #[unredacted]
        i64_field: i64,
        #[unredacted]
        u32_field: u32,
        // u64_field: u64, - not supported
        #[unredacted]
        f32_field: f32,
        #[unredacted]
        f64_field: f64,
        #[unredacted]
        bool_field: bool,
        string_field: PublicString,
    }

    let (sink, processor) = test_emitter(TEST_ID);
    emit!(
        sink,
        DimensionTypes {
            i32_field: -123,
            i64_field: 456,
            u32_field: 789,
            f32_field: 7.14,
            f64_field: 6.14,
            bool_field: true,
            string_field: PublicString("test".into()),
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("dimension.types", Severity::Info)
            .dimension("i32_field", -123i64) // i32 is converted to i64
            .dimension("i64_field", 456i64)
            .dimension("u32_field", 789i64) // u32 is converted to i64
            .dimension("f32_field", 7.14f32) // f32 is converted to f64
            .dimension("f64_field", 6.14f64)
            .dimension("bool_field", true)
            .dimension("string_field", "test")
            .log()
    );
}

#[test]
fn borrowed_classified_fields() {
    #[derive(Debug, Event)]
    #[event(name = "user.action")]
    #[log(severity = info)]
    struct UserAction<'a> {
        name: &'a PiiString,
        label: &'a PublicString,
        #[unredacted]
        count: i64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    let name = PiiString("alice".into());
    let label = PublicString("click".into());
    emit!(
        sink,
        UserAction {
            name: &name,
            label: &label,
            count: 3,
        }
    );

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .dimension("count", 3i64)
            .dimension("label", "click")
            .dimension("name", "alice")
            .log(),
    );
}
