// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coverage-focused tests for the type-erased (`DynEvent`) dispatch path,
//! synthetic event views, composite/no-op sink behavior, and the small
//! accessor / conversion / `Debug` surfaces that the feature-level tests do
//! not otherwise exercise.

use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use data_privacy::RedactionEngine;
use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
use observed::__private::EnrichmentEntry;
use observed::enrichment::{EnrichFutureExt, Enrichment};
use observed::interop::{DynEvent, emit_dyn_event};
use observed::metadata::{EventDescription, FieldDescriptor, InstrumentKind, LogDescription, MetricDescription};
use observed::processing::{EventProcessor, EventView, FieldVisitorFn};
use observed::{Severity, Sink, SinkId, Value, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::types::PublicString;
use observed_testing::{ExpectedEvent, MockProcessor};

/// A foreign event type dispatched through the type-erased pipeline. It states
/// a log signal explicitly, as every adaptor must.
struct DynProbe;

impl DynEvent for DynProbe {
    fn name(&self) -> &'static str {
        "dyn.probe"
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

    fn description(&self) -> EventDescription {
        EventDescription::new(
            "dyn.probe",
            None,
            Some(LogDescription::new("dyn.probe", Severity::Info, None)),
            None,
            false,
            false,
        )
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

    fn flush(&self) -> Result<(), observed::FlushError> {
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

    fn flush(&self) -> Result<(), observed::FlushError> {
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

    fn flush(&self) -> Result<(), observed::FlushError> {
        Err(observed::FlushError::new("failing-processor", "flush boom"))
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

/// An adaptor that describes a log signal reaches a logs-only processor - the
/// shape used by real destinations - rather than being dropped during the
/// interest pass.
#[test]
fn dyn_event_with_log_signal_reaches_logs_only_processor() {
    let logs_only = MockProcessor::with_filter(EventDescription::is_log);
    let sink = Sink::new("dyn_logs_only", vec![Arc::new(logs_only.clone())], tick::SimpleClock::new_frozen());

    emit_dyn_event(&sink, &DynProbe);

    let captured = logs_only.single_event();
    assert_eq!(captured.name(), "dyn.probe");
}

/// Signal routing follows the description alone, and the severity a destination
/// exports is read from that same description. An adaptor that describes no log
/// signal therefore has no severity to report either, and is filtered out.
/// Nothing infers the signal on its behalf, which is what keeps `body()` -
/// exported without redaction - from reaching a log destination the adaptor
/// never asked for.
#[test]
fn a_dyn_event_without_a_log_signal_has_no_severity_and_never_reaches_logs() {
    /// Reports a body, but describes no signal.
    struct UndeclaredProbe;

    impl DynEvent for UndeclaredProbe {
        fn name(&self) -> &'static str {
            "dyn.undeclared"
        }

        fn body(&self) -> Option<Cow<'static, str>> {
            Some(Cow::Borrowed("unsanitized runtime text"))
        }

        fn source_file(&self) -> Option<Cow<'static, str>> {
            None
        }

        fn source_line(&self) -> Option<u32> {
            None
        }

        fn source_crate(&self) -> Option<Cow<'static, str>> {
            None
        }

        fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            ControlFlow::Continue(())
        }

        fn description(&self) -> EventDescription {
            EventDescription::new("dyn.undeclared", None, None, None, false, false)
        }
    }

    assert!(!UndeclaredProbe.description().is_log());

    // The description is the only source of severity, so an adaptor cannot
    // state a level for an event it routed nowhere.
    let view = EventView::new_synthetic(&UndeclaredProbe, SystemTime::UNIX_EPOCH);
    assert_eq!(view.severity(), None);

    let logs_only = MockProcessor::with_filter(EventDescription::is_log);
    let sink = Sink::new("dyn_undeclared", vec![Arc::new(logs_only.clone())], tick::SimpleClock::new_frozen());

    emit_dyn_event(&sink, &UndeclaredProbe);

    assert!(logs_only.events().is_empty());
}

/// The severity a destination exports comes from the log description, so an
/// adaptor states it once and routing and export cannot disagree.
#[test]
fn dyn_event_severity_is_read_from_the_log_description() {
    let view = EventView::new_synthetic(&DynProbe, SystemTime::UNIX_EPOCH);

    assert_eq!(view.severity(), Some(Severity::Info));
}

/// Pins the documented dynamic-body contract: field values pass through the
/// processor's redaction engine, but the body does not. An adaptor must
/// therefore report untrusted runtime input as a field, never as the body.
#[test]
fn dyn_event_body_bypasses_redaction_while_fields_do_not() {
    /// A foreign event carrying one classified field alongside its body.
    struct FieldProbe;

    impl DynEvent for FieldProbe {
        fn name(&self) -> &'static str {
            "dyn.redaction"
        }

        fn body(&self) -> Option<Cow<'static, str>> {
            Some(Cow::Owned("body-secret".to_owned()))
        }

        fn source_file(&self) -> Option<Cow<'static, str>> {
            None
        }

        fn source_line(&self) -> Option<u32> {
            None
        }

        fn source_crate(&self) -> Option<Cow<'static, str>> {
            None
        }

        fn visit_fields(&self, visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
            visitor(&FieldDescriptor::log_only("detail"), &|engine| {
                observed::Value::from(observed::__private::RedactedToString::to_redacted_string(
                    &PublicString("field-secret".to_owned()),
                    engine,
                ))
            })
        }

        fn description(&self) -> EventDescription {
            EventDescription::new(
                "dyn.redaction",
                None,
                Some(LogDescription::new("dyn.redaction", Severity::Info, None)),
                None,
                false,
                false,
            )
        }
    }

    let erasing = MockProcessor::with_redaction_engine(
        RedactionEngine::builder()
            .set_fallback_redactor(SimpleRedactor::with_mode(SimpleRedactorMode::Erase))
            .build(),
    );
    let sink = Sink::new("dyn_redaction", vec![Arc::new(erasing.clone())], tick::SimpleClock::new_frozen());

    emit_dyn_event(&sink, &FieldProbe);

    // The classified field went through the engine and was erased; the body did
    // not - adaptors own its sanitization.
    assert_eq!(
        erasing.single_event(),
        ExpectedEvent::new("dyn.redaction", Severity::Info)
            .body("body-secret")
            .dimension("detail", ""),
    );
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
fn flush_reports_every_processor_error() {
    let single = Sink::new("failing", vec![Arc::new(FailingProcessor)], tick::SimpleClock::new_frozen());
    let err = single.flush().expect_err("the only processor fails to flush");
    assert_eq!(err.failures().len(), 1);

    // A composite keeps flushing after a leaf fails, so both are reported.
    let composite = Sink::composite([
        Sink::new("f1", vec![Arc::new(FailingProcessor)], tick::SimpleClock::new_frozen()),
        Sink::new("f2", vec![Arc::new(FailingProcessor)], tick::SimpleClock::new_frozen()),
    ]);
    let err = composite.flush().expect_err("both leaves fail to flush");
    assert_eq!(err.failures().len(), 2);
}

#[test]
fn event_processor_flush_through_arc() {
    let processor: Arc<dyn EventProcessor> = Arc::new(FailingProcessor);
    assert!(processor.flush().is_err());
}

#[test]
fn value_conversions_and_accessors() {
    assert_eq!(Value::from("hello"), Value::String("hello".into()));

    let numeric = Value::from(3_i64);
    assert_eq!(observed_utils::metric_number_of(&numeric), Some(3.0));

    let float = Value::from(1.5_f64);
    assert_eq!(observed_utils::metric_number_of(&float), Some(1.5));

    // Non-numeric values yield no metric number.
    let boolean = Value::from(true);
    assert_eq!(observed_utils::metric_number_of(&boolean), None);
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
