// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Enrichment state must be scoped to each poll, not to the future lifetime.

use std::future::poll_fn;
use std::ops::ControlFlow;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use future_assert::FutureTestExt;
use observed::enrichment::{EnrichFnExt, EnrichFutureExt, Enrichment};
use observed::processing::{EventProcessor, EventView};
use observed::{Sink, SinkId, Value, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::{ExpectedEvent, MockProcessor, TEST_ID, test_emitter};

#[test]
fn gamma_survivor_composite_uninterested_child_does_not_sample_its_auto_advancing_clock() {
    use std::time::{Duration, SystemTime};

    use observed::metadata::EventDescription;

    struct Uninterested;
    impl EventProcessor for Uninterested {
        fn is_interested(&self, _: &EventDescription) -> bool {
            false
        }
        fn process(&self, _: &EventView<'_>) {
            panic!("an uninterested child must not receive an event");
        }
        fn flush(&self) -> Result<(), observed::FlushError> {
            Ok(())
        }
    }
    let clock = tick::ClockControl::new().auto_advance(Duration::from_millis(1)).to_clock();
    let ignored = Sink::builder("ignored")
        .clock(clock.clone())
        .processors(vec![Arc::new(Uninterested)])
        .build();
    let (interested, processor) = test_emitter(TEST_ID);
    let composite = Sink::composite([ignored, interested]);
    emit!(composite, ProbeEvent::new(7));
    assert_eq!(processor.events().len(), 1);
    assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH);
    assert_eq!(clock.system_time(), SystemTime::UNIX_EPOCH + Duration::from_millis(1));
}

struct Layer(&'static str, i64);

impl Enrichment for Layer {
    fn into_entries(self) -> Vec<observed::enrichment::EnrichmentEntry> {
        vec![observed::enrichment::EnrichmentEntry::unclassified(self.0, self.1)]
    }
}

#[test]
fn gamma_survivor_pending_enrichment_restores_parent_and_reuses_new_parent_on_repoll() {
    let (sink, processor) = test_emitter(TEST_ID);
    let mut future = pin!(
        poll_fn(|_| {
            emit!(sink, ProbeEvent::new(7));
            Poll::<()>::Pending
        })
        .enrich(&sink, Layer("child", 2))
    );
    (|| future.as_mut().unwrap_pending()).enrich(&sink, Layer("parent", 1))();
    emit!(sink, ProbeEvent::new(8));
    (|| future.as_mut().unwrap_pending()).enrich(&sink, Layer("parent", 3))();
    emit!(sink, ProbeEvent::new(9));
    let events = processor.events();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("value", "7")
            .dimension("child", 2_i64)
            .dimension("parent", 1_i64)
    );
    assert_eq!(
        events[1],
        ExpectedEvent::new("test.probe", observed::Severity::Info).dimension("value", "8")
    );
    assert_eq!(
        events[2],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("value", "7")
            .dimension("child", 2_i64)
            .dimension("parent", 3_i64)
    );
    assert_eq!(
        events[3],
        ExpectedEvent::new("test.probe", observed::Severity::Info).dimension("value", "9")
    );
}

#[derive(Default)]
struct OrderProcessor(Mutex<Vec<(String, Value)>>);

#[cfg(test)]
impl EventProcessor for OrderProcessor {
    fn is_interested(&self, _: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let engine = data_privacy::RedactionEngine::builder().build();
        let result = event.visit_enrichments(&mut |descriptor, getter| {
            self.0.lock().unwrap().push((descriptor.field_name().to_owned(), getter(&engine)));
            ControlFlow::Continue(())
        });
        assert_eq!(result, ControlFlow::Continue(()));
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

#[test]
fn gamma_survivor_target_filter_skips_one_layer_and_preserves_oldest_first_order() {
    let processor = Arc::new(OrderProcessor::default());
    let sink = Sink::builder("target")
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::<OrderProcessor>::clone(&processor)])
        .build();
    (|| {
        (|| {
            (|| emit!(sink, ProbeEvent::new(1))).enrich(&sink, Layer("last", 3))();
        })
        .enrich_for(&sink, SinkId::new("different"), Layer("skip", 2))();
    })
    .enrich(&sink, Layer("first", 1))();
    assert_eq!(
        *processor.0.lock().unwrap(),
        [("first".to_owned(), Value::from(1_i64)), ("last".to_owned(), Value::from(3_i64))]
    );
}

#[test]
fn gamma_survivor_composite_transfer_captures_each_child_and_additional_targets() {
    let first = MockProcessor::new();
    let second = MockProcessor::new();
    let one = Sink::builder("one")
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(first.clone())])
        .build();
    let two = Sink::builder("two")
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(second.clone())])
        .build();
    let composite = Sink::composite([one.clone(), two.clone()]);
    let transfer = (|| (|| composite.transfer_context()).enrich(&two, Layer("two_only", 2))()).enrich(&one, Layer("one_only", 1))()
        .with_enrichment(Layer("shared", 3))
        .with_enrichment_for(SinkId::new("two"), Layer("target", 4));
    {
        let _guard = transfer.apply_current_thread();
        emit!(composite, ProbeEvent::new(7));
    }
    emit!(composite, ProbeEvent::new(8));
    assert_eq!(
        first.events()[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("value", "7")
            .dimension("one_only", 1_i64)
            .dimension("shared", 3_i64)
    );
    assert_eq!(
        second.events()[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("value", "7")
            .dimension("two_only", 2_i64)
            .dimension("shared", 3_i64)
            .dimension("target", 4_i64)
    );
    for processor in [first, second] {
        assert_eq!(
            processor.events()[1],
            ExpectedEvent::new("test.probe", observed::Severity::Info).dimension("value", "8")
        );
    }
}

struct CountingProcessor {
    interested: bool,
    counts: Mutex<(usize, usize)>,
}

#[cfg(test)]
impl EventProcessor for CountingProcessor {
    fn is_interested(&self, _: &observed::metadata::EventDescription) -> bool {
        let mut counts = self.counts.lock().unwrap();
        counts.0 += 1;
        assert_eq!(counts.0, 1);
        self.interested
    }

    fn process(&self, event: &EventView<'_>) {
        self.counts.lock().unwrap().1 += 1;
        let mut visited = 0;
        assert_eq!(
            event.visit_fields(&mut |_, _| {
                visited += 1;
                ControlFlow::Break(())
            }),
            ControlFlow::Break(())
        );
        assert_eq!(visited, 1);
        let mut enrichments = 0;
        assert_eq!(
            event.visit_enrichments(&mut |_, _| {
                enrichments += 1;
                ControlFlow::Break(())
            }),
            ControlFlow::Break(())
        );
        assert_eq!(enrichments, 1);
        let debug = format!("{event:?}");
        assert!(debug.contains("name: \"test.probe\""), "{debug}");
        assert!(debug.contains("severity: Some(Info)"), "{debug}");
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

struct AskTwice;

#[cfg(test)]
impl observed::EventSampler for AskTwice {
    fn sample(&self, context: &observed::EventSamplingContext<'_>) -> observed::EventSamplingDecision {
        assert!(format!("{context:?}").contains("description: "));
        assert!(context.processors_interested());
        assert!(context.processors_interested());
        observed::EventSamplingDecision::Continue
    }
}

#[test]
fn gamma_survivor_arc_processor_forwards_interest_and_delivery_and_sampling_asks_each_once() {
    let processors: Vec<_> = [false, true, false, true]
        .into_iter()
        .map(|interested| {
            Arc::new(CountingProcessor {
                interested,
                counts: Mutex::new((0, 0)),
            })
        })
        .collect();
    let sink = Sink::builder("interest")
        .clock(tick::SimpleClock::new_frozen())
        .processors(
            processors
                .iter()
                .map(|processor| Arc::new(Arc::clone(processor)) as Arc<dyn EventProcessor>)
                .collect::<Vec<_>>(),
        )
        .event_sampler(AskTwice)
        .build();
    (|| (|| emit!(sink, ProbeEvent::new(7))).enrich(&sink, Layer("second", 2))()).enrich(&sink, Layer("first", 1))();
    for (index, processor) in processors.iter().enumerate() {
        assert_eq!(*processor.counts.lock().unwrap(), (1, usize::from(index % 2 == 1)));
    }
}

#[test]
fn gamma_survivor_enriched_and_transferred_debug_keep_inner_values_and_empty_enrichments() {
    struct Empty;
    impl Enrichment for Empty {
        fn into_entries(self) -> Vec<observed::enrichment::EnrichmentEntry> {
            Vec::new()
        }
    }

    let (sink, processor) = test_emitter(TEST_ID);
    let future = std::future::ready(42).enrich(&sink, Layer("value", 9));
    let debug = format!("{future:?}");
    assert!(debug.contains("Enriched"), "{debug}");
    assert!(debug.contains("inner: "), "{debug}");
    assert!(debug.contains("42"), "{debug}");
    let transfer = sink.transfer_context();
    assert!(format!("{transfer:?}").contains("observed::context::transfer::Transfer"));
    let transferred = future.attach(transfer);
    let debug = format!("{transferred:?}");
    assert!(debug.contains("Transferred"), "{debug}");
    assert!(debug.contains("inner: "), "{debug}");
    assert!(debug.contains("42"), "{debug}");
    (|| emit!(sink, ProbeEvent::new(7))).enrich(&sink, Empty)();
    assert_eq!(
        processor.events()[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info).dimension("value", "7")
    );
}

#[test]
fn gamma_survivor_enrichment_debug_distinguishes_primitive_from_classified_without_leaking() {
    let primitive = observed::enrichment::EnrichmentEntry::unclassified("number", 42_i64);
    let classified = observed::enrichment::EnrichmentEntry::new(
        "secret",
        data_privacy::Sensitive::new("do-not-display", data_privacy::DataClass::new("gamma", "secret")),
    );
    assert!(format!("{primitive:?}").contains("stored: Primitive(I64(42))"));
    let debug = format!("{classified:?}");
    assert!(debug.contains("stored: Unredacted(<classified>)"), "{debug}");
    assert!(!debug.contains("do-not-display"));
}

#[test]
fn gamma_survivor_isolated_visitor_skips_global_parent_without_losing_targeted_child() {
    let processor = Arc::new(OrderProcessor::default());
    let sink = Sink::builder("isolated")
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::<OrderProcessor>::clone(&processor)])
        .isolated_enrichment()
        .build();
    (|| {
        (|| {
            let entries = sink.current_enrichments();
            assert_eq!(
                entries.iter().map(|entry| entry.key().as_str()).collect::<Vec<_>>(),
                ["global", "target"]
            );
            emit!(sink, ProbeEvent::new(7));
        })
        .enrich_for(&sink, sink.id(), Layer("target", 2))();
    })
    .enrich(&sink, Layer("global", 1))();
    assert_eq!(*processor.0.lock().unwrap(), [("target".to_owned(), Value::from(2_i64))]);
    assert!(sink.current_enrichments().is_empty());
}
