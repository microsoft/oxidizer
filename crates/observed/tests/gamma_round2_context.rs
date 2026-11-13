// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment, transfer, processor, and diagnostic contracts in Observed's own test target.

use std::future::{poll_fn, ready};
use std::ops::ControlFlow;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::SystemTime;

use observed::enrichment::{EnrichFnExt, EnrichFutureExt, Enrichment, EnrichmentEntry};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{EventSampler, EventSamplingContext, EventSamplingDecision, FlushError, Sink, Value, emit, event};

#[event("round2.event")]
#[info("round2 body")]
struct Probe;

#[event("round2.unwanted")]
#[info("must not reach Capture")]
struct Unwanted;

struct Layer(&'static str, i64);

impl Enrichment for Layer {
    fn into_entries(self) -> Vec<EnrichmentEntry> {
        vec![EnrichmentEntry::unclassified(self.0, self.1)]
    }
}

#[derive(Default)]
struct Capture {
    fields: Mutex<Vec<Vec<(String, Value)>>>,
    descriptions: Mutex<Vec<String>>,
}

impl EventProcessor for Capture {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == "round2.event"
    }

    fn process(&self, event: &EventView<'_>) {
        self.descriptions
            .lock()
            .expect("the capture mutex is only held during infallible vector updates")
            .push(format!("{event:?}"));
        let mut fields = Vec::new();
        let result =
            event.visit_enrichments(&mut |descriptor: &observed::metadata::FieldDescriptor,
                                          getter: &observed::processing::FieldValueFn<'_>| {
                fields.push((
                    descriptor.field_name().to_owned(),
                    getter(&data_privacy::simple_redactor::SimpleRedactor::with_mode(
                        data_privacy::simple_redactor::SimpleRedactorMode::Passthrough,
                    )),
                ));
                ControlFlow::Continue(())
            });
        assert_eq!(result, ControlFlow::Continue(()));
        self.fields
            .lock()
            .expect("the capture mutex is only held during infallible vector updates")
            .push(fields);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

fn sink(capture: &Arc<Capture>) -> Sink {
    Sink::builder("round2.sink")
        .clock(tick::SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH))
        .processors(vec![Arc::new(Arc::clone(capture))])
        .build()
}

#[test]
fn gamma_round2_future_enrichment_keeps_values_order_and_restores_each_poll() {
    let capture = Arc::new(Capture::default());
    let sink = sink(&capture);
    emit!(sink, Unwanted);
    assert!(capture.fields.lock().unwrap().is_empty());
    let mut child = pin!(
        poll_fn(|_| {
            assert_eq!(
                sink.current_enrichments()
                    .iter()
                    .map(|entry| entry.key().to_string())
                    .collect::<Vec<_>>(),
                ["parent", "child"]
            );
            emit!(sink, Probe);
            Poll::<()>::Pending
        })
        .enrich(&sink, Layer("child", 2))
    );
    for parent in [1, 3] {
        (|| {
            assert!(child.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        })
        .enrich(&sink, Layer("parent", parent))();
        assert!(sink.current_enrichments().is_empty());
    }
    assert_eq!(
        *capture.fields.lock().unwrap(),
        [
            vec![("parent".to_owned(), Value::from(1_i64)), ("child".to_owned(), Value::from(2_i64))],
            vec![("parent".to_owned(), Value::from(3_i64)), ("child".to_owned(), Value::from(2_i64))],
        ]
    );
}

#[test]
fn gamma_round2_composite_transfer_restores_each_childs_independent_chain() {
    let first_capture = Arc::new(Capture::default());
    let second_capture = Arc::new(Capture::default());
    let first = sink(&first_capture);
    let second = sink(&second_capture);
    let composite = Sink::composite([first.clone(), second.clone()]);
    let transfer = (|| (|| composite.transfer_context()).enrich(&second, Layer("second", 2))()).enrich(&first, Layer("first", 1))();
    assert!(first.current_enrichments().is_empty());
    assert!(second.current_enrichments().is_empty());
    let applied = transfer.apply_current_thread();
    emit!(composite, Probe);
    drop(applied);
    assert_eq!(
        *first_capture.fields.lock().unwrap(),
        [vec![("first".to_owned(), Value::from(1_i64))]]
    );
    assert_eq!(
        *second_capture.fields.lock().unwrap(),
        [vec![("second".to_owned(), Value::from(2_i64))]]
    );
    assert!(first.current_enrichments().is_empty());
    assert!(second.current_enrichments().is_empty());
}

#[test]
fn gamma_round2_targeted_future_and_transfer_additions_reach_the_selected_sink() {
    let capture = Arc::new(Capture::default());
    let sink = sink(&capture);
    let mut future = pin!((async { emit!(sink, Probe) }).enrich_for(&sink, sink.id(), Layer("target", 5)));
    assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_ready());
    let transfer = sink.transfer_context().with_enrichment(Layer("broadcast", 7));
    let applied = transfer.apply_current_thread();
    emit!(sink, Probe);
    drop(applied);
    let transfer = sink.transfer_context().with_enrichment_for(sink.id(), Layer("target", 11));
    let applied = transfer.apply_current_thread();
    emit!(sink, Probe);
    drop(applied);
    assert_eq!(
        *capture.fields.lock().unwrap(),
        [
            vec![("target".to_owned(), Value::from(5_i64))],
            vec![("broadcast".to_owned(), Value::from(7_i64))],
            vec![("target".to_owned(), Value::from(11_i64))],
        ]
    );
    assert!(sink.current_enrichments().is_empty());
}

#[test]
fn gamma_round2_wrapper_and_entry_debug_preserve_labels_without_disclosing_secrets() {
    let capture = Arc::new(Capture::default());
    let sink = sink(&capture);
    let enriched = ready(42).enrich(&sink, Layer("value", 1));
    let enriched_debug = format!("{enriched:?}");
    assert!(enriched_debug.starts_with(std::any::type_name_of_val(&enriched)));
    assert!(enriched_debug.contains(" { inner: Ready("), "{enriched_debug}");
    let transfer = sink.transfer_context();
    assert_eq!(format!("{transfer:?}"), "observed::context::transfer::Transfer { .. }");
    let transferred = enriched.attach(transfer);
    let transferred_debug = format!("{transferred:?}");
    assert!(transferred_debug.starts_with(std::any::type_name_of_val(&transferred)));
    assert!(transferred_debug.contains(" { inner: observed::"), "{transferred_debug}");
    let primitive = EnrichmentEntry::unclassified("value", 17_i64);
    assert!(format!("{primitive:?}").contains("stored: Primitive(I64(17))"));
    let classified = EnrichmentEntry::new(
        "secret",
        data_privacy::Sensitive::new("round2 secret", data_privacy::DataClass::new("round2", "secret")),
    );
    let debug = format!("{classified:?}");
    assert!(debug.contains("stored: Unredacted(<classified>)"), "{debug}");
    assert!(!debug.contains("round2 secret"));
    emit!(sink, Probe);
    assert_eq!(
        capture.descriptions.lock().unwrap().as_slice(),
        ["observed::processing::event_view::EventView<'_> { name: \"round2.event\", severity: Some(Info), .. }"]
    );
}

struct DebugSampler;

impl EventSampler for DebugSampler {
    fn sample(&self, context: &EventSamplingContext<'_>) -> EventSamplingDecision {
        let debug = format!("{context:?}");
        assert!(
            debug.starts_with("observed::sampling::context::EventSamplingContext<'_> { description: "),
            "{debug}"
        );
        assert!(context.processors_interested());
        EventSamplingDecision::Continue
    }
}

#[test]
fn gamma_round2_sampling_context_debug_labels_its_description() {
    let capture = Arc::new(Capture::default());
    let sink = Sink::builder("round2.sampling")
        .clock(tick::SimpleClock::new_frozen_at(SystemTime::UNIX_EPOCH))
        .processors(vec![Arc::new(Arc::clone(&capture))])
        .event_sampler(DebugSampler)
        .build();
    emit!(sink, Probe);
    assert_eq!(capture.fields.lock().unwrap().len(), 1);
}
