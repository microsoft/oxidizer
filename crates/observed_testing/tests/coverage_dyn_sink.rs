// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coverage-focused tests for the type-erased (`DynEvent`) dispatch path,
//! synthetic event views, composite/no-op sink behaviour, and the small
//! accessor / conversion / `Debug` surfaces that the feature-level tests do
//! not otherwise exercise.

use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use observed::__private::EnrichmentEntry;
use observed::enrichment::{EnrichFutureExt, Enrichment};
use observed::interop::{DynEvent, emit_dyn_event};
use observed::metadata::{FieldDescriptor, InstrumentKind, MetricDescription};
use observed::processing::{EventProcessor, EventView, FieldVisitorFn};
use observed::{Severity, Sink, SinkId, Value, emit};
use observed_testing::MockProcessor;
use observed_testing::events::ProbeEvent;
use observed_testing::types::PublicString;

/// A foreign event type dispatched through the type-erased pipeline. It does
/// not override `description`, so the default trait method is exercised too.
struct DynProbe;

impl DynEvent for DynProbe {
    fn name(&self) -> &'static str {
        "dyn.probe"
    }

    fn severity(&self) -> Option<Severity> {
        Some(Severity::Info)
    }

    fn body(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed("probe body"))
    }

    fn source_file(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed("probe.rs"))
    }

    fn source_line(&self) -> Option<u32> {
        Some(7)
    }

    fn source_crate(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed("probe_crate"))
    }

    fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }
}

/// Reads every `EventView` accessor so the delegating branches are covered.
struct ReadAllProcessor {
    saw: Arc<AtomicBool>,
}

impl EventProcessor for ReadAllProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let _ = event.source_crate();
        let _ = event.description();
        let _ = event.timestamp();
        let _ = format!("{event:?}");
        let _ = event.visit_fields(&mut |_d, _g| ControlFlow::Continue(()));
        let _ = event.visit_enrichments(&mut |_d, _g| ControlFlow::Continue(()));
        self.saw.store(true, Ordering::SeqCst);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// A processor that captures the event's `source_crate` for later assertion.
struct CaptureCrate {
    captured: Arc<Mutex<Option<String>>>,
}

impl EventProcessor for CaptureCrate {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        *self.captured.lock().expect("lock poisoned") = event.source_crate().map(Cow::into_owned);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

/// A processor whose `flush` always fails, to cover the error-propagation path.
struct FailingProcessor;

impl EventProcessor for FailingProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, _event: &EventView<'_>) {}

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("flush boom".into())
    }
}

/// An empty enrichment used only to drive the future-wrapping helpers.
struct EmptyEnrichment;

impl Enrichment for EmptyEnrichment {
    fn into_entries(self) -> Vec<EnrichmentEntry> {
        Vec::new()
    }
}

#[test]
fn dyn_event_dispatch_reads_every_accessor() {
    let saw = Arc::new(AtomicBool::new(false));
    let mock = MockProcessor::new();
    let read_all = ReadAllProcessor { saw: Arc::clone(&saw) };

    let sink = Sink::new(
        "dyn",
        vec![Arc::new(mock.clone()), Arc::new(read_all)],
        tick::SimpleClock::new_frozen(),
    );

    emit_dyn_event(&sink, &DynProbe);

    assert!(saw.load(Ordering::SeqCst), "processor should have seen the event");
    let captured = mock.single_event();
    assert_eq!(captured.name(), "dyn.probe");
}

#[test]
fn typed_event_source_crate_is_read() {
    // Reading `source_crate` on a *typed* (compile-time) event exercises the
    // typed arm that the dyn path does not. Assert the captured crate name so a
    // mutated `source_crate` implementation is caught.
    let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let sink = Sink::new(
        "typed",
        vec![Arc::new(CaptureCrate {
            captured: Arc::clone(&captured),
        })],
        tick::SimpleClock::new_frozen(),
    );

    emit!(sink, ProbeEvent::new(1));

    assert_eq!(captured.lock().expect("lock poisoned").as_deref(), Some(env!("CARGO_PKG_NAME")));
}

#[test]
fn synthetic_event_view_exposes_accessors() {
    let view = EventView::new_synthetic(&DynProbe, SystemTime::UNIX_EPOCH);

    assert_eq!(view.source_crate().as_deref(), Some("probe_crate"));
    assert_eq!(view.description().name(), "dyn.probe");
    assert_eq!(view.timestamp(), SystemTime::UNIX_EPOCH);
    assert!(format!("{view:?}").contains("dyn.probe"));

    let _ = view.visit_fields(&mut |_d, _g| ControlFlow::Continue(()));
    let _ = view.visit_enrichments(&mut |_d, _g| ControlFlow::Continue(()));
}

#[test]
fn sink_variants_debug_id_flush_and_noop() {
    let single = Sink::new("single", vec![Arc::new(MockProcessor::new())], tick::SimpleClock::new_frozen());
    let noop = Sink::noop();
    let composite = Sink::composite([single.clone(), noop.clone()]);

    // Debug for each variant.
    assert!(format!("{single:?}").contains("Single"));
    assert!(format!("{composite:?}").contains("Composite"));
    assert!(format!("{noop:?}").contains("Noop"));

    // `id()` sentinels.
    assert_eq!(single.id(), SinkId::new("single"));
    assert_eq!(composite.id(), SinkId::new("<composite>"));
    assert_eq!(noop.id(), SinkId::new("noop"));

    // `is_noop` across variants.
    assert!(!single.is_noop());
    assert!(noop.is_noop());
    // The composite has a live-processor child, so it is not a no-op; this still
    // exercises the composite arm of `is_noop`.
    assert!(!composite.is_noop());

    // A successful flush over a composite.
    composite.flush().expect("composite flush should succeed");
}

#[test]
fn flush_propagates_first_processor_error() {
    let single = Sink::new("failing", vec![Arc::new(FailingProcessor)], tick::SimpleClock::new_frozen());
    assert!(single.flush().is_err());

    // Composite over failing leaves surfaces the first error too.
    let composite = Sink::composite([
        Sink::new("f1", vec![Arc::new(FailingProcessor)], tick::SimpleClock::new_frozen()),
        Sink::new("f2", vec![Arc::new(FailingProcessor)], tick::SimpleClock::new_frozen()),
    ]);
    assert!(composite.flush().is_err());
}

#[test]
fn event_processor_flush_through_arc() {
    let processor: Arc<dyn EventProcessor> = Arc::new(FailingProcessor);
    assert!(processor.flush().is_err());
}

#[test]
fn value_conversions_and_accessors() {
    let raw = Value::from(opentelemetry::StringValue::from("hello"));
    assert_eq!(raw.as_str(), Some("hello"));

    // `as_str` on a non-string value yields `None`.
    let numeric = Value::from_raw(opentelemetry::Value::I64(3));
    assert_eq!(numeric.as_str(), None);
    assert_eq!(numeric.to_number(), Some(3.0));

    let float = Value::from_raw(opentelemetry::Value::F64(1.5));
    assert_eq!(float.to_number(), Some(1.5));

    let boolean = Value::from_raw(opentelemetry::Value::Bool(true));
    assert_eq!(boolean.to_number(), None);

    // `into_inner` returns the underlying OTel value.
    assert!(matches!(raw.into_inner(), opentelemetry::Value::String(_)));
}

#[test]
fn metadata_accessors() {
    let field = FieldDescriptor::log_only("count");
    assert_eq!(field.field_name(), "count");

    let metric = MetricDescription::new("http.requests", InstrumentKind::Counter, "request count", "{request}");
    assert_eq!(metric.description(), "request count");
    assert_eq!(metric.unit(), "{request}");
}

#[test]
fn severity_converts_to_otel() {
    use opentelemetry::logs::Severity as OtelSeverity;

    assert_eq!(OtelSeverity::from(Severity::Trace), OtelSeverity::Trace);
    assert_eq!(OtelSeverity::from(Severity::Debug), OtelSeverity::Debug);
    assert_eq!(OtelSeverity::from(Severity::Fatal), OtelSeverity::Fatal);
}

#[test]
fn context_and_future_wrappers_debug() {
    let sink = Sink::new("ctx", vec![Arc::new(MockProcessor::new())], tick::SimpleClock::new_frozen());

    let transfer = sink.transfer_context();
    assert!(format!("{transfer:?}").contains("Transfer"));

    // `Transferred<T>` Debug (T: Debug).
    let transferred = std::future::ready(()).attach(sink.transfer_context());
    assert!(format!("{transferred:?}").contains("Transferred"));

    // `Enriched<T>` Debug + the targeted `enrich_for` constructor.
    let enriched = std::future::ready(()).enrich(&sink, EmptyEnrichment);
    assert!(format!("{enriched:?}").contains("Enriched"));

    let _enriched_for = std::future::ready(()).enrich_for(&sink, SinkId::new("target"), EmptyEnrichment);
}

#[test]
fn enrichment_entry_debug_covers_both_stored_variants() {
    // Primitive (unclassified) variant.
    let primitive = EnrichmentEntry::unclassified("count", 1i64);
    assert!(format!("{primitive:?}").contains("Primitive"));

    // Classified (deferred-redaction) variant.
    let classified = EnrichmentEntry::new("user", PublicString("alice".to_owned()));
    assert!(format!("{classified:?}").contains("Unredacted"));
}
