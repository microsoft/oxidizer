// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for immediate per-Sink event sampling.

use std::borrow::Cow;
use std::cell::Cell;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use data_privacy::{DataClass, Sensitive};
use observed::interop::{DynEvent, emit_dyn_event};
use observed::metadata::{EventDescription, LogDescription};
use observed::processing::FieldVisitorFn;
use observed::{EventSampler, EventSamplingContext, EventSamplingDecision, Severity, Sink, SinkId, emit, event};
use observed_testing::types::{PiiString, PublicI64};
use observed_testing::{ExpectedEvent, MockProcessor, TEST_ID};

const PII: DataClass = DataClass::new("test_taxonomy", "pii");

// These atomics are observation counters only. Assertions read them after the
// synchronous emission, so no ordering relative to other memory is required.

#[event("user.action")]
#[info("User acted")]
struct UserAction {
    user: PiiString,
    #[unredacted]
    action_code: i64,
}

#[event("http.server.request")]
#[info("Request handled")]
#[histogram(duration, name = "http.server.request.duration")]
struct HttpServerRequest {
    #[unredacted]
    duration: f64,
    status: PublicI64,
}

#[event("session.opened")]
#[info("Session opened")]
struct SessionOpened<'a> {
    user: Sensitive<&'a str>,
}

struct ProbeSampler<F> {
    calls: Arc<AtomicUsize>,
    decide: F,
}

impl<F> EventSampler for ProbeSampler<F>
where
    F: for<'a> Fn(&EventSamplingContext<'a>) -> EventSamplingDecision + Send + Sync + 'static,
{
    fn sample(&self, event: &EventSamplingContext<'_>) -> EventSamplingDecision {
        _ = self.calls.fetch_add(1, Ordering::Relaxed);
        (self.decide)(event)
    }
}

fn probe_sampler<F>(decide: F) -> (impl EventSampler, Arc<AtomicUsize>)
where
    F: for<'a> Fn(&EventSamplingContext<'a>) -> EventSamplingDecision + Send + Sync + 'static,
{
    let calls = Arc::new(AtomicUsize::new(0));
    (
        ProbeSampler {
            calls: Arc::clone(&calls),
            decide,
        },
        calls,
    )
}

fn constant_sampler(decision: EventSamplingDecision) -> (impl EventSampler, Arc<AtomicUsize>) {
    probe_sampler(move |_| decision)
}

/// Builds a sink with one [`MockProcessor`] and the given sampler.
fn sampled_emitter(id: SinkId, sampler: impl Into<Box<dyn EventSampler>>) -> (Sink, MockProcessor) {
    let processor = MockProcessor::new();
    let sink = Sink::builder(id)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(processor.clone())])
        .event_sampler(sampler)
        .build();
    (sink, processor)
}

/// A sampler whose clones share one call counter, so a single sampling state
/// can span several sinks. Drops events for `drop_for` only.
#[derive(Clone)]
struct SharedSampler {
    calls: Arc<AtomicUsize>,
    drop_for: SinkId,
}

impl EventSampler for SharedSampler {
    fn sample(&self, event: &EventSamplingContext<'_>) -> EventSamplingDecision {
        _ = self.calls.fetch_add(1, Ordering::Relaxed);
        if event.sink_id() == self.drop_for {
            EventSamplingDecision::Drop
        } else {
            EventSamplingDecision::Continue
        }
    }
}

struct BridgedEvent<'a> {
    message: &'a str,
}

impl DynEvent for BridgedEvent<'_> {
    fn name(&self) -> &'static str {
        "bridge.event"
    }

    fn body(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Owned(self.message.to_owned()))
    }

    fn source_file(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed("bridge.rs"))
    }

    fn source_line(&self) -> Option<u32> {
        Some(17)
    }

    fn source_crate(&self) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed("bridge"))
    }

    fn visit_fields(&self, _visitor: &mut FieldVisitorFn<'_>) -> ControlFlow<()> {
        ControlFlow::Continue(())
    }

    fn description(&self) -> EventDescription {
        EventDescription::new(
            "bridge.event",
            None,
            Some(LogDescription::new("bridge.event", Severity::Info, None)),
            None,
            false,
            false,
        )
    }
}

#[test]
fn event_sampler_continues_borrowed_event() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let (sink, processor) = sampled_emitter(TEST_ID, sampler);

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 42,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .body("User acted")
            .dimension("action_code", 42i64)
            .dimension("user", "Alice"),
    );
}

#[test]
fn always_off_sampler_always_drops() {
    let logs = MockProcessor::with_filter(|description| description.log().is_some());
    let metrics = MockProcessor::with_filter(EventDescription::contains_metrics);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let sink = Sink::builder(TEST_ID)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(logs.clone()), Arc::new(metrics.clone())])
        .event_sampler(sampler)
        .build();

    emit!(
        sink,
        HttpServerRequest {
            duration: 0.042,
            status: PublicI64(200),
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(logs.is_empty());
    assert!(metrics.is_empty());
}

#[test]
fn event_sampler_is_not_called_without_processor_interest() {
    let processor = MockProcessor::with_filter(|_| false);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::builder(TEST_ID)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(processor.clone())])
        .event_sampler(sampler)
        .build();

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(processor.is_empty());
}

#[derive(Debug)]
struct SeenContext {
    name: &'static str,
    is_log: bool,
    contains_metrics: bool,
    sink_id: SinkId,
    timestamp: SystemTime,
}

#[test]
fn event_sampler_receives_description_sink_id_and_timestamp() {
    let id = SinkId::new("context");
    let clock = tick::SimpleClock::new_frozen();
    let processor = MockProcessor::new();
    let seen: Arc<Mutex<Option<SeenContext>>> = Arc::new(Mutex::new(None));

    let recorder = Arc::clone(&seen);
    let (sampler, calls) = probe_sampler(move |event| {
        *recorder.lock().unwrap() = Some(SeenContext {
            name: event.description().name(),
            is_log: event.description().is_log(),
            contains_metrics: event.description().contains_metrics(),
            sink_id: event.sink_id(),
            timestamp: event.timestamp(),
        });
        EventSamplingDecision::Continue
    });

    let sink = Sink::builder(id)
        .clock(clock.clone())
        .processors(vec![Arc::new(processor.clone())])
        .event_sampler(sampler)
        .build();
    emit!(
        sink,
        HttpServerRequest {
            duration: 1.5,
            status: PublicI64(200),
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let seen = seen.lock().unwrap().take().unwrap();
    assert_eq!(seen.name, "http.server.request");
    assert!(seen.is_log);
    assert!(seen.contains_metrics);
    assert_eq!(seen.sink_id, id);
    assert_eq!(seen.timestamp, clock.system_time());
    assert_eq!(processor.len(), 1);
}

#[test]
fn composite_leaves_decide_independently() {
    let sampled_id = SinkId::new("sampled");
    let other_id = SinkId::new("other");
    let sampled_processor = MockProcessor::new();
    let other_processor = MockProcessor::new();
    let (sampler, calls) = probe_sampler(move |event| {
        assert_eq!(event.sink_id(), sampled_id);
        EventSamplingDecision::Drop
    });

    let sampled_sink = Sink::builder(sampled_id)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(sampled_processor.clone())])
        .event_sampler(sampler)
        .build();
    let other_sink = Sink::builder(other_id)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(other_processor.clone())])
        .build();
    let composite = Sink::composite([sampled_sink, other_sink]);

    emit!(
        composite,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 7,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(sampled_processor.is_empty());
    assert_eq!(
        other_processor.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .body("User acted")
            .dimension("action_code", 7i64)
            .dimension("user", "Alice"),
    );
}

#[test]
fn composite_does_not_sample_uninterested_leaf() {
    let interested = MockProcessor::new();
    let uninterested = MockProcessor::with_filter(|_| false);
    let (interested_sampler, interested_calls) = constant_sampler(EventSamplingDecision::Continue);
    let (uninterested_sampler, uninterested_calls) = constant_sampler(EventSamplingDecision::Continue);

    let composite = Sink::composite([
        Sink::builder("interested")
            .clock(tick::SimpleClock::new_frozen())
            .processors(vec![Arc::new(interested.clone())])
            .event_sampler(interested_sampler)
            .build(),
        Sink::builder("uninterested")
            .clock(tick::SimpleClock::new_frozen())
            .processors(vec![Arc::new(uninterested.clone())])
            .event_sampler(uninterested_sampler)
            .build(),
    ]);

    emit!(
        composite,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 3,
        }
    );

    assert_eq!(interested_calls.load(Ordering::Relaxed), 1);
    assert_eq!(uninterested_calls.load(Ordering::Relaxed), 0);
    assert_eq!(interested.len(), 1);
    assert!(uninterested.is_empty());
}

#[test]
fn sampled_sink_accepts_stack_borrowing_event() {
    let processor = MockProcessor::new();
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::builder(TEST_ID)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(processor.clone())])
        .event_sampler(sampler)
        .build();

    let user = String::from("borrowed-user");
    emit!(
        sink,
        SessionOpened {
            user: Sensitive::new(user.as_str(), PII),
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("session.opened", Severity::Info)
            .body("Session opened")
            .dimension("user", "borrowed-user"),
    );
}

#[test]
fn dynamic_borrowed_event_reaches_sampler() {
    let processor = MockProcessor::new();
    let (sampler, calls) = probe_sampler(|event| {
        assert_eq!(event.description().name(), "bridge.event");
        assert_eq!(event.sink_id(), TEST_ID);
        EventSamplingDecision::Continue
    });
    let sink = Sink::builder(TEST_ID)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(processor.clone())])
        .event_sampler(sampler)
        .build();

    let message = String::from("bridged message");
    emit_dyn_event(&sink, &BridgedEvent { message: &message });

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let captured = processor.single_event();
    assert_eq!(captured, ExpectedEvent::new("bridge.event", Severity::Info).body("bridged message"),);
    assert_eq!(captured.source_file(), Some("bridge.rs"));
    assert_eq!(captured.source_line(), Some(17));
}

#[test]
fn clones_share_the_sampler() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let (sink, processor) = sampled_emitter(TEST_ID, sampler);
    let clone = sink.clone();

    emit!(
        clone,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );
    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 2,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert!(processor.is_empty());
}

#[test]
fn later_event_sampler_replaces_earlier_one() {
    let processor = MockProcessor::new();
    let (first, first_calls) = constant_sampler(EventSamplingDecision::Continue);
    let (second, second_calls) = constant_sampler(EventSamplingDecision::Drop);
    let sink = Sink::builder(TEST_ID)
        .clock(tick::SimpleClock::new_frozen())
        .processors(vec![Arc::new(processor.clone())])
        .event_sampler(first)
        .event_sampler(second)
        .build();

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );

    assert_eq!(first_calls.load(Ordering::Relaxed), 0);
    assert_eq!(second_calls.load(Ordering::Relaxed), 1);
    assert!(processor.is_empty());
}

#[test]
fn boxed_sampler_decides_for_sink() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let (sink, processor) = sampled_emitter(TEST_ID, Box::new(sampler) as Box<dyn EventSampler>);

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(processor.is_empty());
}

/// A composite has no sampler of its own. Sampling state spans the sinks in it
/// by attaching clones of one sampler to each sink, which tells them apart by
/// sink id.
#[test]
fn shared_sampler_state_spans_composite_leaves() {
    let first_id = SinkId::new("first");
    let second_id = SinkId::new("second");
    let calls = Arc::new(AtomicUsize::new(0));
    let sampler = SharedSampler {
        calls: Arc::clone(&calls),
        drop_for: first_id,
    };
    let (first, first_processor) = sampled_emitter(first_id, sampler.clone());
    let (second, second_processor) = sampled_emitter(second_id, sampler);
    let composite = Sink::composite([first, second]);

    emit!(
        composite,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 9,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert!(first_processor.is_empty());
    assert_eq!(
        second_processor.single_event(),
        ExpectedEvent::new("user.action", Severity::Info)
            .body("User acted")
            .dimension("action_code", 9i64)
            .dimension("user", "Alice"),
    );
}

#[test]
fn processorless_leaf_never_calls_its_sampler() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::builder(TEST_ID)
        .clock(tick::SimpleClock::new_frozen())
        .event_sampler(sampler)
        .build();
    let built = Cell::new(false);

    emit!(sink, {
        built.set(true);
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    });

    assert!(!built.get());
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}
