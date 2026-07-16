// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for `RedactedEventOtelExt::to_otel_log_record`.
//!
//! Verifies that emit events are correctly mapped to `OTel` `LogRecord`:
//! severity, event name, body, dimensions → attributes, renamed fields,
//! and enrichments → attributes.

#![cfg(not(miri))] // OTel SDK internally calls SystemTime::now(), unsupported under Miri isolation

use std::ops::ControlFlow;
use std::sync::Arc;

use data_privacy::DataClass;
use observed::enrichment::EnrichFnExt;
use observed::processing::{EventProcessor, EventView};
use observed::{Enrichment, Event, Sink, SinkId, emit};
use opentelemetry::logs::{AnyValue, LoggerProvider};
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};

const TEST_DC: DataClass = DataClass::new("test", "public");

// ---------------------------------------------------------------------------
// Event / enrichment types
// ---------------------------------------------------------------------------

#[derive(Event)]
#[event(name = "http.request")]
#[log(severity = info)]
struct HttpRequest {
    #[data_class(TEST_DC)]
    status: i64,
    #[data_class(TEST_DC)]
    retries: i64,
}

#[derive(Event)]
#[event(name = "app.warning")]
#[log(severity = warn, message = "Something went wrong")]
struct AppWarning {
    #[data_class(TEST_DC)]
    code: i64,
}

#[derive(Event)]
#[event(name = "db.error")]
#[log(severity = error)]
struct DbError {
    #[dimension(log = "db.system")]
    #[data_class(TEST_DC)]
    system_id: i64,
}

#[derive(Enrichment)]
struct TenantEnrich {
    #[data_class(TEST_DC)]
    tenant: i64,
}

#[derive(Enrichment)]
struct LibVersion {
    #[dimension(log = "lib.version")]
    #[data_class(TEST_DC)]
    version: i64,
}

#[derive(Enrichment)]
struct OptionalEnrich {
    #[data_class(TEST_DC)]
    user_agent: Option<i64>,
    #[data_class(TEST_DC)]
    region: Option<i64>,
}

#[derive(Event)]
#[event(name = "http.optional")]
#[log(severity = info)]
struct OptionalRequest {
    #[data_class(TEST_DC)]
    status: i64,
    #[data_class(TEST_DC)]
    user_agent: Option<i64>,
    #[data_class(TEST_DC)]
    region: Option<i64>,
}

#[derive(Enrichment)]
struct DroppedOptionalEnrich {
    #[data_class(TEST_DC)]
    #[if_none(drop)]
    user_agent: Option<i64>,
    #[data_class(TEST_DC)]
    #[if_none(drop)]
    region: Option<i64>,
}

#[derive(Event)]
#[event(name = "http.optional.dropped")]
#[log(severity = info)]
struct DroppedOptionalRequest {
    #[data_class(TEST_DC)]
    status: i64,
    #[data_class(TEST_DC)]
    #[if_none(drop)]
    user_agent: Option<i64>,
    #[data_class(TEST_DC)]
    #[if_none(drop)]
    region: Option<i64>,
}

// ---------------------------------------------------------------------------
// OTel test processor
// ---------------------------------------------------------------------------

struct OtelTestProcessor {
    logger: opentelemetry_sdk::logs::SdkLogger,
    redaction_engine: data_privacy::RedactionEngine,
}

impl EventProcessor for OtelTestProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        use opentelemetry::logs::{LogRecord, Logger};

        let mut record = self.logger.create_log_record();

        record.set_event_name(event.name());
        if let Some(severity) = event.severity() {
            record.set_severity_number(opentelemetry::logs::Severity::from(severity));
            record.set_severity_text(severity.as_str());
        }
        record.set_timestamp(std::time::SystemTime::now());

        if let Some(body) = event.body() {
            record.set_body(AnyValue::String(body.into_owned().into()));
        }

        let engine = &self.redaction_engine;

        let _ = event.visit_fields(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });

        let _ = event.visit_enrichments(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });

        self.logger.emit(record);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

fn otel_processor(provider: &SdkLoggerProvider) -> OtelTestProcessor {
    OtelTestProcessor {
        logger: provider.logger("test"),
        redaction_engine: data_privacy::RedactionEngine::builder()
            .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::with_mode(
                data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
            ))
            .build(),
    }
}

fn otel_emitter() -> (Sink, SdkLoggerProvider, InMemoryLogExporter) {
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();

    let sink = Sink::new("test", vec![Arc::new(otel_processor(&provider))], tick::SimpleClock::new_frozen());
    (sink, provider, exporter)
}

fn find_attr<'a>(attrs: &'a [(opentelemetry::Key, AnyValue)], name: &str) -> Option<&'a AnyValue> {
    attrs.iter().find(|(k, _)| k.as_str() == name).map(|(_, v)| v)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn maps_severity_name_and_dimensions() {
    let (sink, provider, exporter) = otel_emitter();

    emit!(sink, HttpRequest { status: 200, retries: 3 });
    // flush before shutdown - OTel SDK 0.32 resets the in-memory exporter on shutdown
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    assert_eq!(logs.len(), 1);
    let _ = provider.shutdown();

    let record = &logs[0].record;
    assert_eq!(record.severity_number(), Some(opentelemetry::logs::Severity::Info),);
    assert_eq!(record.severity_text(), Some("INFO"));
    assert_eq!(record.event_name(), Some("http.request"));

    let attrs: Vec<_> = record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert!(matches!(
        find_attr(&attrs, "status"),
        Some(AnyValue::String(s)) if s.as_ref() == "200"
    ));
    assert!(matches!(
        find_attr(&attrs, "retries"),
        Some(AnyValue::String(s)) if s.as_ref() == "3"
    ));
}

#[test]
fn maps_body() {
    let (sink, provider, exporter) = otel_emitter();

    emit!(sink, AppWarning { code: 42 });
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let record = &logs[0].record;
    let _ = provider.shutdown();
    assert!(matches!(record.body(), Some(AnyValue::String(s)) if s.as_ref() == "Something went wrong"),);
}

#[test]
fn maps_renamed_field() {
    let (sink, provider, exporter) = otel_emitter();

    emit!(sink, DbError { system_id: 5 });
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert!(matches!(
        find_attr(&attrs, "db.system"),
        Some(AnyValue::String(s)) if s.as_ref() == "5"
    ));
}

#[test]
fn maps_enrichments_to_attributes() {
    let (sink, provider, exporter) = otel_emitter();

    (|| {
        emit!(sink, HttpRequest { status: 201, retries: 0 });
    })
    .enrich(&sink, TenantEnrich { tenant: 1 })();
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    assert!(matches!(
        find_attr(&attrs, "tenant"),
        Some(AnyValue::String(s)) if s.as_ref() == "1"
    ));
    assert!(matches!(
        find_attr(&attrs, "status"),
        Some(AnyValue::String(s)) if s.as_ref() == "201"
    ));
}

#[test]
fn enrichment_option_none_is_filled_by_default() {
    let (sink, provider, exporter) = otel_emitter();

    (|| {
        emit!(sink, HttpRequest { status: 200, retries: 0 });
    })
    .enrich(
        &sink,
        OptionalEnrich {
            user_agent: None,
            region: None,
        },
    )();
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    // `None` enrichment entries are filled with the default `"n/a"` placeholder.
    assert!(matches!(
        find_attr(&attrs, "user_agent"),
        Some(AnyValue::String(s)) if s.as_ref() == "n/a"
    ));
    assert!(matches!(
        find_attr(&attrs, "region"),
        Some(AnyValue::String(s)) if s.as_ref() == "n/a"
    ));
}

#[test]
fn enrichment_option_none_is_dropped_with_drop_behavior() {
    let (sink, provider, exporter) = otel_emitter();

    (|| {
        emit!(sink, HttpRequest { status: 200, retries: 0 });
    })
    .enrich(
        &sink,
        DroppedOptionalEnrich {
            user_agent: None,
            region: None,
        },
    )();
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    // `#[if_none(drop)]` omits `None` enrichment entries entirely.
    assert!(find_attr(&attrs, "user_agent").is_none());
    assert!(find_attr(&attrs, "region").is_none());
}

#[test]
fn option_none_is_omitted_some_is_present() {
    let (sink, provider, exporter) = otel_emitter();

    emit!(
        sink,
        OptionalRequest {
            status: 200,
            user_agent: Some(7),
            region: Some(42),
        }
    );
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    // Present `Some` values are recorded.
    assert!(matches!(
        find_attr(&attrs, "user_agent"),
        Some(AnyValue::String(s)) if s.as_ref() == "7"
    ));
    assert!(matches!(
        find_attr(&attrs, "region"),
        Some(AnyValue::String(s)) if s.as_ref() == "42"
    ));
}

#[test]
fn option_none_is_filled_by_default() {
    let (sink, provider, exporter) = otel_emitter();

    emit!(
        sink,
        OptionalRequest {
            status: 200,
            user_agent: None,
            region: None,
        }
    );
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    // `None` optional fields are filled with the default `"n/a"` placeholder.
    assert!(matches!(
        find_attr(&attrs, "user_agent"),
        Some(AnyValue::String(s)) if s.as_ref() == "n/a"
    ));
    assert!(matches!(
        find_attr(&attrs, "region"),
        Some(AnyValue::String(s)) if s.as_ref() == "n/a"
    ));
    // Required field is unaffected.
    assert!(matches!(
        find_attr(&attrs, "status"),
        Some(AnyValue::String(s)) if s.as_ref() == "200"
    ));
}

#[test]
fn option_none_is_dropped_with_drop_behavior() {
    let (sink, provider, exporter) = otel_emitter();

    emit!(
        sink,
        DroppedOptionalRequest {
            status: 200,
            user_agent: None,
            region: None,
        }
    );
    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    // `#[if_none(drop)]` omits `None` optional fields entirely.
    assert!(find_attr(&attrs, "user_agent").is_none());
    assert!(find_attr(&attrs, "region").is_none());
    // Required field is unaffected.
    assert!(matches!(
        find_attr(&attrs, "status"),
        Some(AnyValue::String(s)) if s.as_ref() == "200"
    ));
}

#[test]
fn isolated_sink_excludes_globals_keeps_targeted() {
    static LIB: SinkId = SinkId::new("lib");

    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder().with_simple_exporter(exporter.clone()).build();
    let sink = Sink::new_isolated(LIB, vec![Arc::new(otel_processor(&provider))], tick::SimpleClock::new_frozen());

    (|| {
        emit!(sink, HttpRequest { status: 200, retries: 0 });
    })
    // Untargeted (global) enrichment: dropped by an isolated sink.
    .enrich(&sink, TenantEnrich { tenant: 1 })
    // Targeted at this sink's id: kept.
    .enrich_for(&sink, LIB, LibVersion { version: 2 })();

    let _ = provider.force_flush();
    let logs = exporter.get_emitted_logs().expect("should get logs");
    let _ = provider.shutdown();
    let attrs: Vec<_> = logs[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    // Global enrichment is excluded.
    assert!(find_attr(&attrs, "tenant").is_none());
    // Targeted enrichment survives the isolation filter.
    assert!(matches!(
        find_attr(&attrs, "lib.version"),
        Some(AnyValue::String(s)) if s.as_ref() == "2"
    ));
}

#[test]
fn enrich_for_targets_only_matching_child_at_dispatch() {
    static A: SinkId = SinkId::new("child_a");
    static B: SinkId = SinkId::new("child_b");

    let exporter_a = InMemoryLogExporter::default();
    let provider_a = SdkLoggerProvider::builder().with_simple_exporter(exporter_a.clone()).build();
    let exporter_b = InMemoryLogExporter::default();
    let provider_b = SdkLoggerProvider::builder().with_simple_exporter(exporter_b.clone()).build();

    let a = Sink::new(A, vec![Arc::new(otel_processor(&provider_a))], tick::SimpleClock::new_frozen());
    let b = Sink::new(B, vec![Arc::new(otel_processor(&provider_b))], tick::SimpleClock::new_frozen());
    let composite = Sink::composite([a, b]);

    // Broadcast a push targeted at `A`; only child `A` should resolve it.
    (|| {
        emit!(composite, HttpRequest { status: 200, retries: 0 });
    })
    .enrich_for(&composite, A, LibVersion { version: 7 })();

    let _ = provider_a.force_flush();
    let _ = provider_b.force_flush();
    let logs_a = exporter_a.get_emitted_logs().expect("should get logs a");
    let logs_b = exporter_b.get_emitted_logs().expect("should get logs b");
    let _ = provider_a.shutdown();
    let _ = provider_b.shutdown();

    let attrs_a: Vec<_> = logs_a[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let attrs_b: Vec<_> = logs_b[0].record.attributes_iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    // Child `A` (the target) sees the entry.
    assert!(matches!(
        find_attr(&attrs_a, "lib.version"),
        Some(AnyValue::String(s)) if s.as_ref() == "7"
    ));
    // Child `B` (not the target) does not.
    assert!(find_attr(&attrs_b, "lib.version").is_none());
}
