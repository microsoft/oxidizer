// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public sink interest contracts.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView};
use observed::{Event, EventSampler, EventSamplingContext, EventSamplingDecision, FlushError, Sink, emit, event};
use tick::{ClockControl, SimpleClock};

/// Event used to compare interest queries with actual construction and delivery.
#[event("interest.accepted")]
struct AcceptedEvent;

/// Selects an event by name and one-time initialization while recording delivery.
struct ProbeProcessor {
    initial_interest: bool,
    initialized_interest: OnceLock<bool>,
    processed: AtomicUsize,
    flushed: AtomicUsize,
}

#[mutants::skip]
impl ProbeProcessor {
    fn new(initial_interest: bool) -> Self {
        Self {
            initial_interest,
            initialized_interest: OnceLock::new(),
            processed: AtomicUsize::new(0),
            flushed: AtomicUsize::new(0),
        }
    }

    fn assert_untouched(&self) {
        assert_eq!(self.processed.load(Ordering::Relaxed), 0);
        assert_eq!(self.flushed.load(Ordering::Relaxed), 0);
    }
}

#[mutants::skip]
impl EventProcessor for ProbeProcessor {
    fn is_interested(&self, description: &EventDescription) -> bool {
        description.name() == AcceptedEvent::DESCRIPTION.name() && *self.initialized_interest.get().unwrap_or(&self.initial_interest)
    }

    fn process(&self, _event: &EventView<'_>) {
        self.processed.fetch_add(1, Ordering::Relaxed);
    }

    fn flush(&self) -> Result<(), FlushError> {
        self.flushed.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Shares a sampling count across clones and drops deliveries despite interest.
#[derive(Clone, Default)]
struct DropSampler {
    sampled: Arc<AtomicUsize>,
}

#[mutants::skip]
impl EventSampler for DropSampler {
    fn sample(&self, _event: &EventSamplingContext<'_>) -> EventSamplingDecision {
        self.sampled.fetch_add(1, Ordering::Relaxed);
        EventSamplingDecision::Drop
    }
}

#[mutants::skip]
fn leaf(processor: &Arc<ProbeProcessor>) -> Sink {
    Sink::builder("interest")
        .processors(vec![Arc::clone(processor) as Arc<dyn EventProcessor>])
        .clock(SimpleClock::new_frozen())
        .build()
}

#[test]
fn noop_and_empty_sinks_are_uninterested() {
    let sinks = [
        Sink::noop(),
        Sink::builder("empty").clock(SimpleClock::new_frozen()).build(),
        Sink::composite([]),
        Sink::composite([Sink::noop(), Sink::builder("empty-child").clock(SimpleClock::new_frozen()).build()]),
    ];

    for sink in sinks {
        assert!(!sink.is_interested(&AcceptedEvent::DESCRIPTION));
    }
}

#[test]
fn leaf_interest_accepts_any_processor_and_preserves_metadata() {
    for (first, second, expected) in [(false, false, false), (false, true, true), (true, false, true), (true, true, true)] {
        let first = Arc::new(ProbeProcessor::new(first));
        let second = Arc::new(ProbeProcessor::new(second));
        let sink = Sink::builder("mixed")
            .processors(vec![
                Arc::clone(&first) as Arc<dyn EventProcessor>,
                Arc::clone(&second) as Arc<dyn EventProcessor>,
            ])
            .clock(SimpleClock::new_frozen())
            .build();

        assert_eq!(sink.is_interested(&AcceptedEvent::DESCRIPTION), expected);
        assert!(!sink.is_interested(&EventDescription::new("interest.rejected", None, None, None, false, false)));
        first.assert_untouched();
        second.assert_untouched();
    }
}

#[test]
fn composite_interest_accepts_any_child() {
    for (first, second, expected) in [(false, false, false), (false, true, true), (true, false, true), (true, true, true)] {
        let first = Arc::new(ProbeProcessor::new(first));
        let second = Arc::new(ProbeProcessor::new(second));
        // Independent leaves may share an id but must have distinct enrichment slots.
        let sink = Sink::composite([leaf(&first), Sink::composite([Sink::noop(), leaf(&second)])]);

        assert_eq!(sink.is_interested(&AcceptedEvent::DESCRIPTION), expected);
        assert!(!sink.is_interested(&EventDescription::new("interest.rejected", None, None, None, false, false)));
        first.assert_untouched();
        second.assert_untouched();
    }
}

#[test]
fn initialization_updates_interest_in_both_directions() {
    for (before, after) in [(false, true), (true, false)] {
        let processor = Arc::new(ProbeProcessor::new(before));
        let sink = leaf(&processor);
        let uninterested = Arc::new(ProbeProcessor::new(false));
        let composite = Sink::composite([leaf(&uninterested), sink.clone()]);

        assert_eq!(sink.is_interested(&AcceptedEvent::DESCRIPTION), before);
        assert_eq!(composite.is_interested(&AcceptedEvent::DESCRIPTION), before);

        processor.initialized_interest.set(after).unwrap();

        assert_eq!(sink.is_interested(&AcceptedEvent::DESCRIPTION), after);
        assert_eq!(composite.is_interested(&AcceptedEvent::DESCRIPTION), after);
        processor.assert_untouched();
        uninterested.assert_untouched();

        emit!(&sink, AcceptedEvent);
        assert_eq!(processor.processed.load(Ordering::Relaxed), usize::from(after));
    }
}

#[test]
fn interest_queries_do_not_read_leaf_clocks() {
    for (first_interest, second_interest) in [(false, false), (false, true), (true, false), (true, true)] {
        // Nonzero increments make any clock read observable without using real time.
        let first_control = ClockControl::new().auto_advance(Duration::from_secs(1));
        let second_control = ClockControl::new().auto_advance(Duration::from_secs(1));
        let first = Sink::builder("first")
            .processors(vec![Arc::new(ProbeProcessor::new(first_interest))])
            .clock(first_control.to_simple_clock())
            .build();
        let second = Sink::builder("second")
            .processors(vec![Arc::new(ProbeProcessor::new(second_interest))])
            .clock(second_control.to_simple_clock())
            .build();
        let composite = Sink::composite([first.clone(), second.clone()]);

        assert_eq!(first.is_interested(&AcceptedEvent::DESCRIPTION), first_interest);
        assert_eq!(second.is_interested(&AcceptedEvent::DESCRIPTION), second_interest);
        assert_eq!(
            composite.is_interested(&AcceptedEvent::DESCRIPTION),
            first_interest || second_interest
        );

        assert_eq!(first_control.to_simple_clock().system_time(), SystemTime::UNIX_EPOCH);
        assert_eq!(second_control.to_simple_clock().system_time(), SystemTime::UNIX_EPOCH);
    }
}

#[test]
fn uninterested_queries_do_not_construct_or_sample() {
    let processor = Arc::new(ProbeProcessor::new(false));
    let sampler = DropSampler::default();
    let sink = Sink::builder("interest")
        .processors(vec![Arc::clone(&processor) as Arc<dyn EventProcessor>])
        .clock(SimpleClock::new_frozen())
        .event_sampler(sampler.clone())
        .build();
    let composite = Sink::composite([Sink::noop(), sink.clone()]);
    for sink in [sink, composite] {
        assert!(!sink.is_interested(&AcceptedEvent::DESCRIPTION));
    }

    assert_eq!(sampler.sampled.load(Ordering::Relaxed), 0);
    processor.assert_untouched();
}

#[test]
fn queries_do_not_sample_and_interest_does_not_guarantee_delivery() {
    let processor = Arc::new(ProbeProcessor::new(true));
    let sampler = DropSampler::default();
    let sink = Sink::builder("interest")
        .processors(vec![Arc::clone(&processor) as Arc<dyn EventProcessor>])
        .clock(SimpleClock::new_frozen())
        .event_sampler(sampler.clone())
        .build();
    let composite = Sink::composite([Sink::noop(), sink.clone()]);

    assert!(sink.is_interested(&AcceptedEvent::DESCRIPTION));
    assert!(composite.is_interested(&AcceptedEvent::DESCRIPTION));
    assert_eq!(sampler.sampled.load(Ordering::Relaxed), 0);
    processor.assert_untouched();

    let constructed = Cell::new(0);
    emit!(&composite, {
        constructed.set(constructed.get() + 1);
        AcceptedEvent
    });

    assert_eq!(constructed.get(), 0);
    assert_eq!(sampler.sampled.load(Ordering::Relaxed), 1);
    processor.assert_untouched();
    assert!(sink.is_interested(&AcceptedEvent::DESCRIPTION));
    assert_eq!(sampler.sampled.load(Ordering::Relaxed), 1);
}
