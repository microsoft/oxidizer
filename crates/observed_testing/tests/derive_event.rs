// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the `#[event(...)]` attribute macro.
//!
//! Verifies all struct-level and field-level annotations by inspecting the
//! generated `Event` trait implementation directly:
//! - `DESCRIPTION` constant (name, severity, body, signals, metrics)
//! - `body()` method
//! - `visit_fields()` method (field extraction through passthrough redaction)
//!
//! These tests do **not** use `emit!` - they exercise only the derived trait.

use std::ops::ControlFlow;

use observed::metadata::InstrumentKind;
use observed::{Event, Severity, Value, event};
use observed_testing::types::{PiiString, PublicString};
use observed_testing::{ExpectedEventDescription, MicrosoftEnterpriseDataTaxonomy, passthrough_redaction_engine};

/// Collected field entry from `visit_fields`.
struct CollectedField {
    field_name: String,
    log_key: Option<String>,
    metric_key: Option<String>,
    value: Value,
}

/// Helper: extract fields from an event via `visit_fields`.
///
/// Returns each field's name, optional log key, optional metric key, and value.
fn collect_fields(event: &impl Event, engine: &data_privacy::RedactionEngine) -> Vec<CollectedField> {
    let mut fields = Vec::new();
    let _ = event.visit_fields(&mut |desc, get_value| {
        let value = get_value(engine);
        fields.push(CollectedField {
            field_name: desc.field_name().to_owned(),
            log_key: desc.log().map(|l| l.key().to_owned()),
            metric_key: desc.metric().map(|m| m.key().to_owned()),
            value,
        });
        ControlFlow::Continue(())
    });
    fields.sort_by(|a, b| a.field_name.cmp(&b.field_name));
    fields
}

/// Expected field entry for assertions.
struct ExpectedField {
    field_name: &'static str,
    log_key: Option<&'static str>,
    metric_key: Option<&'static str>,
    value: Value,
}

/// Helper: sort actual and expected entries by field name, then assert all four
/// components match exactly.
fn sort_and_compare(actual: &[CollectedField], expected: &[ExpectedField]) {
    let mut expected_sorted: Vec<_> = expected.iter().collect();
    expected_sorted.sort_by_key(|e| e.field_name);

    let mut actual_sorted: Vec<_> = actual.iter().collect();
    actual_sorted.sort_by(|a, b| a.field_name.cmp(&b.field_name));

    assert_eq!(
        actual_sorted.len(),
        expected_sorted.len(),
        "entry count mismatch\n  actual fields: {:?}\n  expected fields: {:?}",
        actual_sorted.iter().map(|f| f.field_name.as_str()).collect::<Vec<_>>(),
        expected_sorted.iter().map(|f| f.field_name).collect::<Vec<_>>(),
    );

    for (a, e) in actual_sorted.iter().zip(expected_sorted.iter()) {
        assert_eq!(a.field_name.as_str(), e.field_name, "field_name mismatch");
        assert_eq!(a.log_key.as_deref(), e.log_key, "log_key mismatch for field `{}`", e.field_name);
        assert_eq!(
            a.metric_key.as_deref(),
            e.metric_key,
            "metric_key mismatch for field `{}`",
            e.field_name,
        );
        assert_eq!(a.value, e.value, "value mismatch for field `{}`", e.field_name);
    }
}

#[test]
fn all_severity_levels() {
    #[event("sev.trace")]
    #[trace]
    struct TraceSev {
        #[unredacted]
        v: i64,
    }

    #[event("sev.debug")]
    #[debug]
    struct DebugSev {
        #[unredacted]
        v: i64,
    }

    #[event("sev.info")]
    #[info]
    struct InfoSev {
        #[unredacted]
        v: i64,
    }

    #[event("sev.warn")]
    #[warning]
    struct WarnSev {
        #[unredacted]
        v: i64,
    }

    #[event("sev.error")]
    #[error]
    struct ErrorSev {
        #[unredacted]
        v: i64,
    }

    #[event("sev.fatal")]
    #[fatal]
    struct FatalSev {
        #[unredacted]
        v: i64,
    }

    assert_eq!(
        TraceSev::DESCRIPTION,
        ExpectedEventDescription::new("sev.trace", Severity::Trace).log()
    );
    assert_eq!(
        DebugSev::DESCRIPTION,
        ExpectedEventDescription::new("sev.debug", Severity::Debug).log()
    );
    assert_eq!(
        InfoSev::DESCRIPTION,
        ExpectedEventDescription::new("sev.info", Severity::Info).log()
    );
    assert_eq!(
        WarnSev::DESCRIPTION,
        ExpectedEventDescription::new("sev.warn", Severity::Warn).log()
    );
    assert_eq!(
        ErrorSev::DESCRIPTION,
        ExpectedEventDescription::new("sev.error", Severity::Error).log()
    );
    assert_eq!(
        FatalSev::DESCRIPTION,
        ExpectedEventDescription::new("sev.fatal", Severity::Fatal).log()
    );
}

#[test]
fn custom_event_name() {
    #[event("custom.event.name")]
    #[info]
    struct CustomNamedEvent {
        #[unredacted]
        x: i64,
    }

    assert_eq!(
        CustomNamedEvent::DESCRIPTION,
        ExpectedEventDescription::new("custom.event.name", Severity::Info).log(),
    );
}

#[test]
fn default_event_name_is_struct_name() {
    #[event("DefaultNamedEvent")]
    #[info]
    struct DefaultNamedEvent {
        #[unredacted]
        x: i64,
    }

    assert_eq!(
        DefaultNamedEvent::DESCRIPTION,
        ExpectedEventDescription::new("DefaultNamedEvent", Severity::Info).log(),
    );
}

#[test]
fn body_in_description() {
    #[event("with.body")]
    #[info("Something happened")]
    struct WithBody {
        #[unredacted]
        v: i64,
    }
    assert_eq!(
        WithBody::DESCRIPTION,
        ExpectedEventDescription::new("with.body", Severity::Info)
            .body("Something happened")
            .log(),
    );
}

#[test]
fn no_body_in_description() {
    #[event("no.body")]
    #[info]
    struct WithoutBody {
        #[unredacted]
        v: i64,
    }
    assert_eq!(
        WithoutBody::DESCRIPTION,
        ExpectedEventDescription::new("no.body", Severity::Info).log(),
    );
}

#[test]
fn disabled_event() {
    #[event("internal.debug", disabled)]
    #[debug]
    struct DisabledEvent {
        #[unredacted]
        detail: i64,
    }

    assert_eq!(
        DisabledEvent::DESCRIPTION,
        ExpectedEventDescription::new("internal.debug", Severity::Debug).log().disabled(),
    );
}

#[test]
fn exclude_from_logs_without_metric_produces_empty_signals() {
    #[event("metric.only", disabled)]
    #[info]
    struct MetricOnlyNoFields {
        #[unredacted]
        v: i64,
    }

    // Disabled log signal - no metric
    assert_eq!(
        MetricOnlyNoFields::DESCRIPTION,
        ExpectedEventDescription::new("metric.only", Severity::Info).log().disabled(),
    );
}

#[test]
fn exclude_from_logs_with_metric_produces_metric_only() {
    #[event("metric.only.with_metric")]
    #[counter(name = "metric.only.with_metric")]
    struct MetricOnlyWithMetric {
        #[dimension(metric = "v")]
        #[unredacted]
        v: i64,
    }

    assert_eq!(
        MetricOnlyWithMetric::DESCRIPTION,
        ExpectedEventDescription::new("metric.only.with_metric", Severity::Info).metric(),
    );
}

#[test]
fn event_level_metric_metadata_in_description() {
    #[event("event.level.metric")]
    #[info]
    #[counter(name = "event.level.count")]
    struct EventLevelMetric {
        #[unredacted]
        v: i64,
    }

    assert_eq!(
        EventLevelMetric::DESCRIPTION,
        ExpectedEventDescription::new("event.level.metric", Severity::Info)
            .log()
            .metric()
            .event_metric("event.level.count", InstrumentKind::Counter),
    );
}

#[test]
fn renamed_fields_extract_correctly() {
    #[event("rename.test")]
    #[info]
    struct RenameTest {
        #[dimension(log = "otel.status_code")]
        #[unredacted]
        status: i64,
        #[dimension(log = "http.request.method")]
        method: PublicString,
        #[unredacted]
        untouched: bool,
    }

    let engine = passthrough_redaction_engine();
    let event = RenameTest {
        status: 200,
        method: PublicString("POST".into()),
        untouched: true,
    };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "method",
                log_key: Some("http.request.method"),
                metric_key: None,
                value: Value::from("POST"),
            },
            ExpectedField {
                field_name: "status",
                log_key: Some("otel.status_code"),
                metric_key: None,
                value: Value::from(200i64),
            },
            ExpectedField {
                field_name: "untouched",
                log_key: Some("untouched"),
                metric_key: None,
                value: Value::from(true),
            },
        ],
    );
}

#[test]
fn event_with_exclude_flags() {
    #[event("field.exclude")]
    #[info]
    struct FieldExclude {
        #[dimension(log = exclude, metric = "metric_only_field")]
        #[unredacted]
        metric_only_field: i64,
        #[unredacted]
        log_only_field: i64,
        #[unredacted]
        #[dimension(metric = "both")]
        both: i64,
    }

    // Field-level exclude flags are routing hints, not signal-level changes.
    assert_eq!(
        FieldExclude::DESCRIPTION,
        ExpectedEventDescription::new("field.exclude", Severity::Info).log(),
    );

    let engine = passthrough_redaction_engine();
    let event = FieldExclude {
        metric_only_field: 1,
        log_only_field: 2,
        both: 3,
    };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "both",
                log_key: Some("both"),
                metric_key: Some("both"),
                value: Value::from(3i64),
            },
            ExpectedField {
                field_name: "log_only_field",
                log_key: Some("log_only_field"),
                metric_key: None,
                value: Value::from(2i64),
            },
            ExpectedField {
                field_name: "metric_only_field",
                log_key: None,
                metric_key: Some("metric_only_field"),
                value: Value::from(1i64),
            },
        ],
    );
}

#[test]
fn all_instrument_kinds() {
    #[event("metric.histogram")]
    #[info]
    #[histogram(duration, name = "op.duration")]
    struct HistogramEvent {
        #[unredacted]
        duration: f64,
    }

    #[event("metric.gauge")]
    #[info]
    #[gauge(size, name = "pool.size")]
    struct GaugeEvent {
        #[unredacted]
        size: i64,
    }

    #[event("metric.updown")]
    #[info]
    #[updown_counter(active, name = "conn.active")]
    struct UpDownCounterEvent {
        #[unredacted]
        active: i64,
    }

    assert_eq!(
        HistogramEvent::DESCRIPTION,
        ExpectedEventDescription::new("metric.histogram", Severity::Info).log().metric(),
    );
    assert_eq!(
        GaugeEvent::DESCRIPTION,
        ExpectedEventDescription::new("metric.gauge", Severity::Info).log().metric(),
    );
    assert_eq!(
        UpDownCounterEvent::DESCRIPTION,
        ExpectedEventDescription::new("metric.updown", Severity::Info).log().metric(),
    );
}

#[test]
fn multiple_metrics_description() {
    #[event("metric.multi")]
    #[info]
    #[histogram(duration, name = "req.duration")]
    #[gauge(size, name = "req.size")]
    #[updown_counter(count, name = "req.count")]
    struct MultiMetricEvent {
        #[unredacted]
        duration: f64,
        #[unredacted]
        size: i64,
        #[unredacted]
        count: i64,
        #[unredacted]
        tag: bool,
    }

    assert_eq!(
        MultiMetricEvent::DESCRIPTION,
        ExpectedEventDescription::new("metric.multi", Severity::Info).log().metric(),
    );
}

#[test]
fn no_metric_field_means_log_only() {
    #[event("no.metric")]
    #[info]
    struct NoMetric {
        #[unredacted]
        v: i64,
    }

    assert_eq!(
        NoMetric::DESCRIPTION,
        ExpectedEventDescription::new("no.metric", Severity::Info).log(),
    );
}

#[test]
fn metric_with_renamed_field() {
    #[event("renamed.metric")]
    #[info]
    #[histogram(dur_ms, name = "http.request.duration")]
    struct RenamedMetric {
        #[dimension(log = "http.duration")]
        #[unredacted]
        dur_ms: f64,
    }

    assert_eq!(
        RenamedMetric::DESCRIPTION,
        ExpectedEventDescription::new("renamed.metric", Severity::Info).log().metric(),
    );
}

#[test]
fn metric_field_value_extraction() {
    #[event("metric.updown")]
    #[info]
    #[updown_counter(active, name = "conn.active")]
    struct UpDownCounterEvent {
        #[unredacted]
        active: i64,
    }

    let engine = passthrough_redaction_engine();
    let event = UpDownCounterEvent { active: 5 };
    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[ExpectedField {
            field_name: "active",
            log_key: Some("active"),
            metric_key: Some("active"),
            value: Value::from(5i64),
        }],
    );
}

#[test]
fn primitive_values() {
    #[event("primitives")]
    #[info]
    #[expect(clippy::struct_field_names, reason = "it's ok")]
    struct PrimitiveEvent {
        #[unredacted]
        a_i64: i64,
        #[unredacted]
        a_u32: u32,
        #[unredacted]
        a_f64: f64,
        #[unredacted]
        a_bool: bool,
        #[unredacted]
        a_duration: f64,
    }

    let engine = passthrough_redaction_engine();
    let event = PrimitiveEvent {
        a_i64: -42,
        a_u32: 100,
        a_f64: 89.14,
        a_bool: true,
        a_duration: 5.0,
    };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "a_bool",
                log_key: Some("a_bool"),
                metric_key: None,
                value: Value::from(true),
            },
            ExpectedField {
                field_name: "a_duration",
                log_key: Some("a_duration"),
                metric_key: None,
                value: Value::from(5.0),
            },
            ExpectedField {
                field_name: "a_f64",
                log_key: Some("a_f64"),
                metric_key: None,
                value: Value::from(89.14),
            },
            ExpectedField {
                field_name: "a_i64",
                log_key: Some("a_i64"),
                metric_key: None,
                value: Value::from(-42i64),
            },
            ExpectedField {
                field_name: "a_u32",
                log_key: Some("a_u32"),
                metric_key: None,
                value: Value::from(100i64),
            },
        ],
    );
}

#[test]
fn classified_values_through_redaction() {
    #[event("classified")]
    #[info]
    struct ClassifiedEvent {
        public: PublicString,
        personal: PiiString,
    }

    let engine = passthrough_redaction_engine();
    let event = ClassifiedEvent {
        public: PublicString("hello".into()),
        personal: PiiString("user@example.com".into()),
    };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "personal",
                log_key: Some("personal"),
                metric_key: None,
                value: Value::from("user@example.com"),
            },
            ExpectedField {
                field_name: "public",
                log_key: Some("public"),
                metric_key: None,
                value: Value::from("hello"),
            },
        ],
    );
}

#[test]
fn combined() {
    #[event("http.outgoing_request")]
    #[warning("Outgoing HTTP request")]
    #[histogram(duration, name = "http.client.request.duration")]
    struct OutgoingRequest {
        #[dimension(log = "http.request.method")]
        method: PublicString,

        #[dimension(log = "http.response.status_code")]
        #[unredacted]
        status: i64,

        #[unredacted]
        request_id: i64,

        #[unredacted]
        internal_tag: i64,

        #[unredacted]
        duration: f64,
    }

    assert_eq!(
        OutgoingRequest::DESCRIPTION,
        ExpectedEventDescription::new("http.outgoing_request", Severity::Warn)
            .body("Outgoing HTTP request")
            .log()
            .metric(),
    );

    let engine = passthrough_redaction_engine();
    let event = OutgoingRequest {
        method: PublicString("GET".into()),
        status: 200,
        request_id: 42,
        internal_tag: 7,
        duration: 0.15,
    };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "duration",
                log_key: Some("duration"),
                metric_key: Some("duration"),
                value: Value::from(0.15),
            },
            ExpectedField {
                field_name: "internal_tag",
                log_key: Some("internal_tag"),
                metric_key: None,
                value: Value::from(7i64),
            },
            ExpectedField {
                field_name: "method",
                log_key: Some("http.request.method"),
                metric_key: None,
                value: Value::from("GET"),
            },
            ExpectedField {
                field_name: "request_id",
                log_key: Some("request_id"),
                metric_key: None,
                value: Value::from(42i64),
            },
            ExpectedField {
                field_name: "status",
                log_key: Some("http.response.status_code"),
                metric_key: None,
                value: Value::from(200i64),
            },
        ],
    );
}

#[test]
fn single_lifetime() {
    #[event("borrowed.single")]
    #[info]
    struct BorrowedEvent<'a> {
        #[unredacted]
        message: &'a str,
    }
    assert_eq!(
        BorrowedEvent::DESCRIPTION,
        ExpectedEventDescription::new("borrowed.single", Severity::Info).log(),
    );

    let engine = passthrough_redaction_engine();
    let msg = String::from("hello borrowed");
    let event = BorrowedEvent { message: &msg };
    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[ExpectedField {
            field_name: "message",
            log_key: Some("message"),
            metric_key: None,
            value: Value::from("hello borrowed"),
        }],
    );
}

#[test]
fn multiple_lifetimes() {
    #[event("borrowed.multi")]
    #[warning]
    struct MultiLifetimeEvent<'a, 'b> {
        #[unredacted]
        label: &'a str,
        #[unredacted]
        detail: &'b str,
    }

    assert_eq!(
        MultiLifetimeEvent::DESCRIPTION,
        ExpectedEventDescription::new("borrowed.multi", Severity::Warn).log(),
    );

    let engine = passthrough_redaction_engine();
    let label = String::from("request");
    let detail = String::from("timed out");
    let event = MultiLifetimeEvent {
        label: &label,
        detail: &detail,
    };
    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "detail",
                log_key: Some("detail"),
                metric_key: None,
                value: Value::from("timed out"),
            },
            ExpectedField {
                field_name: "label",
                log_key: Some("label"),
                metric_key: None,
                value: Value::from("request"),
            },
        ],
    );
}

#[test]
fn borrowed_mixed_with_owned_value_extraction() {
    #[event("borrowed.mixed")]
    #[info]
    struct BorrowedMixedEvent<'a> {
        #[unredacted]
        label: &'a str,
        #[unredacted]
        tag: i64,
    }

    let engine = passthrough_redaction_engine();
    let label = String::from("mix");
    let event = BorrowedMixedEvent { label: &label, tag: 1 };
    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "label",
                log_key: Some("label"),
                metric_key: None,
                value: Value::from("mix"),
            },
            ExpectedField {
                field_name: "tag",
                log_key: Some("tag"),
                metric_key: None,
                value: Value::from(1i64),
            },
        ],
    );
}

#[test]
fn unannotated_fields_accessible_through_visit_fields() {
    // Fields without any attribute annotations (no #[unredacted], no metric, no #[dimension])
    // must still be visited. They follow the default redaction path.
    #[event("bare.fields")]
    #[info]
    struct BareFields {
        plain_public: PublicString,
        plain_pii: PiiString,
    }

    let engine = passthrough_redaction_engine();
    let event = BareFields {
        plain_public: PublicString("visible".into()),
        plain_pii: PiiString("secret@example.com".into()),
    };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "plain_pii",
                log_key: Some("plain_pii"),
                metric_key: None,
                value: Value::from("secret@example.com"),
            },
            ExpectedField {
                field_name: "plain_public",
                log_key: Some("plain_public"),
                metric_key: None,
                value: Value::from("visible"),
            },
        ],
    );
}

#[test]
fn dimension_fields_are_dimensions_on_metric_only_events() {
    // For metric-only events (no severity attribute), fields opt into metric
    // dimensions with #[dimension] and are then accessible through visit_fields.
    // Unmarked fields are not routed anywhere.
    #[event("system.memory.usage")]
    #[counter(name = "system.memory.usage")]
    struct MetricBareFields {
        #[dimension(metric = "plain_public")]
        plain_public: PublicString,
        #[dimension(metric = "plain_pii")]
        plain_pii: PiiString,
    }

    let engine = passthrough_redaction_engine();
    let event = MetricBareFields {
        plain_public: PublicString("visible".into()),
        plain_pii: PiiString("secret@example.com".into()),
    };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "plain_pii",
                log_key: None,
                metric_key: Some("plain_pii"),
                value: Value::from("secret@example.com"),
            },
            ExpectedField {
                field_name: "plain_public",
                log_key: None,
                metric_key: Some("plain_public"),
                value: Value::from("visible"),
            },
        ],
    );
}

#[test]
fn reference_to_redactable_type() {
    #[event("borrowed.classified")]
    #[info]
    struct BorrowedClassified<'a> {
        name: &'a PiiString,
        #[unredacted]
        count: i64,
    }

    let engine = passthrough_redaction_engine();
    let pii = PiiString("alice".into());
    let event = BorrowedClassified { name: &pii, count: 7 };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "count",
                log_key: Some("count"),
                metric_key: None,
                value: Value::from(7i64),
            },
            ExpectedField {
                field_name: "name",
                log_key: Some("name"),
                metric_key: None,
                value: Value::from("alice"),
            },
        ],
    );
}

/// Parenthesized reference types like `(&'a T)` can appear from macro
/// expansions. Without unwrapping `Type::Paren` in the derive macro, the
/// codegen would generate `&self.field` (the owned-type path) producing
/// `&&T`, which fails because `&T` has no `RedactedDisplay` impl.
#[test]
#[expect(unused_parens, reason = "the test is specifically about handling parenthesized types")]
fn parenthesized_reference_to_redactable_type() {
    // A helper macro that wraps the field type in parentheses, producing
    // `Type::Paren(Type::Reference(...))` in the proc-macro token stream.
    macro_rules! define_event {
        ($ty:ty) => {
            #[event("paren.ref")]
            #[info]
            struct ParenRefEvent<'a> {
                name: ($ty),
                #[unredacted]
                count: i64,
            }
        };
    }

    define_event!(&'a PiiString);

    let engine = passthrough_redaction_engine();
    let pii = PiiString("bob".into());
    let event = ParenRefEvent { name: &pii, count: 5 };

    let entries = collect_fields(&event, &engine);
    sort_and_compare(
        &entries,
        &[
            ExpectedField {
                field_name: "count",
                log_key: Some("count"),
                metric_key: None,
                value: Value::from(5i64),
            },
            ExpectedField {
                field_name: "name",
                log_key: Some("name"),
                metric_key: None,
                value: Value::from("bob"),
            },
        ],
    );
}

// ================================================================================================
// Generated-code hygiene
// ================================================================================================

/// A field written with a raw identifier is exported under its domain key.
///
/// `Ident::to_string` preserves Rust's `r#` escape, so deriving the default key
/// from it would leak a language detail into telemetry as `"r#type"`.
#[test]
fn raw_identifier_field_uses_its_plain_name() {
    #[event("http.request")]
    #[info]
    struct RawIdentifierField {
        #[unredacted]
        r#type: i64,
    }

    let engine = passthrough_redaction_engine();
    let fields = collect_fields(&RawIdentifierField { r#type: 1 }, &engine);

    sort_and_compare(
        &fields,
        &[ExpectedField {
            field_name: "type",
            log_key: Some("type"),
            metric_key: None,
            value: Value::from(1_i64),
        }],
    );
}

/// A consumer type named `Value` must survive the expansion.
///
/// The `#[data_class(...)]` expression is interpolated into the generated
/// `visit_fields` body, so importing `observed::Value` there would shadow a
/// consumer type of the same name and reject otherwise valid input.
#[test]
fn consumer_type_named_value_is_not_shadowed() {
    struct Value;

    impl Value {
        fn class() -> data_privacy::DataClass {
            MicrosoftEnterpriseDataTaxonomy::Eupi.data_class()
        }
    }

    #[event("hygiene.probe")]
    #[info]
    struct HygieneProbe {
        #[data_class(Value::class())]
        user: PublicString,
    }

    let engine = passthrough_redaction_engine();
    let fields = collect_fields(
        &HygieneProbe {
            user: PublicString("alice".into()),
        },
        &engine,
    );

    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].field_name, "user");
}
