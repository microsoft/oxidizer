// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for immediate per-Sink event sampling.

use std::borrow::Cow;
use std::cell::Cell;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use data_privacy::{DataClass, Sensitive};
use observed::interop::{DynEvent, emit_dyn_event};
use observed::metadata::{EventDescription, LogDescription};
use observed::processing::FieldVisitorFn;
use observed::{EventSampler, EventSamplingContext, EventSamplingDecision, Severity, Sink, SinkId, emit, event};
use observed_testing::types::{PiiString, PublicI64};
use observed_testing::{ExpectedEvent, MockProcessor, TEST_ID, test_emitter};

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
    fn sample(&self, context: &EventSamplingContext<'_>) -> EventSamplingDecision {
        _ = self.calls.fetch_add(1, Ordering::Relaxed);
        (self.decide)(context)
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

/// A processor that wants every event or none, counting how often it is asked.
fn counting_processor(wants: bool) -> (MockProcessor, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let processor = MockProcessor::with_filter(move |_| {
        _ = counter.fetch_add(1, Ordering::Relaxed);
        wants
    });
    (processor, asked)
}

fn user_action(action_code: i64) -> ExpectedEvent {
    ExpectedEvent::new("user.action", Severity::Info)
        .body("User acted")
        .dimension("action_code", action_code)
        .dimension("user", "Alice")
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
    let (sink, processor) = test_emitter(TEST_ID);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = sink.with_event_sampler(sampler);

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 42,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(processor.single_event(), user_action(42),);
}

#[test]
fn always_off_sampler_always_drops() {
    let logs = MockProcessor::with_filter(|description| description.log().is_some());
    let metrics = MockProcessor::with_filter(EventDescription::contains_metrics);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let sink = Sink::new(
        TEST_ID,
        vec![Arc::new(logs.clone()), Arc::new(metrics.clone())],
        tick::SimpleClock::new_frozen(),
    )
    .with_event_sampler(sampler);

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
fn dropped_event_is_never_built() {
    let (sink, processor) = test_emitter(TEST_ID);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let sink = sink.with_event_sampler(sampler);
    let built = Cell::new(false);

    emit!(sink, {
        built.set(true);
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    });

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(!built.get());
    assert!(processor.is_empty());
}

#[test]
fn event_sampler_is_called_without_processor_interest() {
    let processor = MockProcessor::with_filter(|_| false);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::new(TEST_ID, vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen()).with_event_sampler(sampler);

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

#[derive(Debug)]
struct SeenContext {
    name: &'static str,
    is_log: bool,
    contains_metrics: bool,
    processors_interested: bool,
}

#[test]
fn event_sampler_receives_description_and_processor_interest() {
    let processor = MockProcessor::new();
    let seen: Arc<Mutex<Option<SeenContext>>> = Arc::new(Mutex::new(None));

    let recorder = Arc::clone(&seen);
    let (sampler, calls) = probe_sampler(move |context| {
        *recorder.lock().unwrap() = Some(SeenContext {
            name: context.description().name(),
            is_log: context.description().is_log(),
            contains_metrics: context.description().contains_metrics(),
            processors_interested: context.processors_interested(),
        });
        EventSamplingDecision::Continue
    });

    let sink = Sink::new(TEST_ID, vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen()).with_event_sampler(sampler);
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
    assert!(seen.processors_interested);
    assert_eq!(processor.len(), 1);
}

#[test]
fn leaf_sampler_in_composite_decides_only_for_its_leaf() {
    let sampled_processor = MockProcessor::new();
    let other_processor = MockProcessor::new();
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);

    let sampled_sink = Sink::new(
        SinkId::new("sampled"),
        vec![Arc::new(sampled_processor.clone())],
        tick::SimpleClock::new_frozen(),
    )
    .with_event_sampler(sampler);
    let other_sink = Sink::new(
        SinkId::new("other"),
        vec![Arc::new(other_processor.clone())],
        tick::SimpleClock::new_frozen(),
    );
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
    assert_eq!(other_processor.single_event(), user_action(7));
}

#[test]
fn continued_event_reaches_only_interested_processors() {
    let interested = MockProcessor::new();
    let uninterested = MockProcessor::with_filter(|_| false);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::new(
        TEST_ID,
        vec![Arc::new(interested.clone()), Arc::new(uninterested.clone())],
        tick::SimpleClock::new_frozen(),
    )
    .with_event_sampler(sampler);

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 4,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(interested.single_event(), user_action(4));
    assert!(uninterested.is_empty());
}

#[test]
fn processors_are_asked_once_when_sampler_does_not_ask() {
    let (interested, interested_asked) = counting_processor(true);
    let (uninterested, uninterested_asked) = counting_processor(false);
    let (sampler, _calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::new(
        TEST_ID,
        vec![Arc::new(interested.clone()), Arc::new(uninterested.clone())],
        tick::SimpleClock::new_frozen(),
    )
    .with_event_sampler(sampler);

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 5,
        }
    );

    assert_eq!(interested_asked.load(Ordering::Relaxed), 1);
    assert_eq!(uninterested_asked.load(Ordering::Relaxed), 1);
    assert_eq!(interested.len(), 1);
    assert!(uninterested.is_empty());
}

#[test]
fn processors_are_asked_once_when_sampler_asks() {
    let (interested, interested_asked) = counting_processor(true);
    let (uninterested, uninterested_asked) = counting_processor(false);
    let (sampler, _calls) = probe_sampler(|context| {
        assert!(context.processors_interested());
        assert!(context.processors_interested());
        EventSamplingDecision::Continue
    });
    let sink = Sink::new(
        TEST_ID,
        vec![Arc::new(interested.clone()), Arc::new(uninterested.clone())],
        tick::SimpleClock::new_frozen(),
    )
    .with_event_sampler(sampler);

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 6,
        }
    );

    assert_eq!(interested_asked.load(Ordering::Relaxed), 1);
    assert_eq!(uninterested_asked.load(Ordering::Relaxed), 1);
    assert_eq!(interested.len(), 1);
    assert!(uninterested.is_empty());
}

#[test]
fn dropping_sampler_that_does_not_ask_skips_processors() {
    let (processor, asked) = counting_processor(true);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let sink = Sink::new(TEST_ID, vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen()).with_event_sampler(sampler);

    emit!(
        sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(asked.load(Ordering::Relaxed), 0);
    assert!(processor.is_empty());
}

#[test]
fn continued_event_no_processor_wants_is_never_built() {
    let processor = MockProcessor::with_filter(|_| false);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::new(TEST_ID, vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen()).with_event_sampler(sampler);
    let built = Cell::new(false);

    emit!(sink, {
        built.set(true);
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    });

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(!built.get());
    assert!(processor.is_empty());
}

#[test]
fn sampled_sink_accepts_stack_borrowing_event() {
    let processor = MockProcessor::new();
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::new(TEST_ID, vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen()).with_event_sampler(sampler);

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
    let (sampler, calls) = probe_sampler(|context| {
        assert_eq!(context.description().name(), "bridge.event");
        EventSamplingDecision::Continue
    });
    let sink = Sink::new(TEST_ID, vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen()).with_event_sampler(sampler);

    let message = String::from("bridged message");
    emit_dyn_event(&sink, &BridgedEvent { message: &message });

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let captured = processor.single_event();
    assert_eq!(captured, ExpectedEvent::new("bridge.event", Severity::Info).body("bridged message"),);
    assert_eq!(captured.source_file(), Some("bridge.rs"));
    assert_eq!(captured.source_line(), Some(17));
}

#[test]
fn clone_made_before_attachment_remains_unsampled() {
    let (sink, processor) = test_emitter(TEST_ID);
    let unsampled = sink.clone();
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let gated_sink = sink.with_event_sampler(sampler);

    emit!(
        unsampled,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );
    emit!(
        gated_sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 2,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(processor.len(), 1);
    assert_eq!(processor.single_event(), user_action(1),);
}

#[test]
fn clone_made_after_attachment_shares_the_sampler() {
    let (sink, processor) = test_emitter(TEST_ID);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let gated_sink = sink.with_event_sampler(sampler);
    let clone = gated_sink.clone();

    emit!(
        clone,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );
    emit!(
        gated_sink,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 2,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert!(processor.is_empty());
}

#[test]
fn second_event_sampler_attachment_replaces_first() {
    let (sink, processor) = test_emitter(TEST_ID);
    let (first, first_calls) = constant_sampler(EventSamplingDecision::Continue);
    let (second, second_calls) = constant_sampler(EventSamplingDecision::Drop);
    let sink = sink.with_event_sampler(first).with_event_sampler(second);

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
    let (sink, processor) = test_emitter(TEST_ID);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let sink = sink.with_event_sampler(Box::new(sampler) as Box<dyn EventSampler>);

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

#[test]
fn composite_attachment_replaces_leaf_samplers() {
    let (first, first_processor) = test_emitter(SinkId::new("first"));
    let (second, second_processor) = test_emitter(SinkId::new("second"));
    let (replaced, replaced_calls) = constant_sampler(EventSamplingDecision::Continue);
    let first = first.with_event_sampler(replaced);
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let composite = Sink::composite([first, second]).with_event_sampler(sampler);

    emit!(
        composite,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 9,
        }
    );

    assert_eq!(replaced_calls.load(Ordering::Relaxed), 0);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert!(first_processor.is_empty());
    assert!(second_processor.is_empty());
}

#[test]
fn nested_composite_keeps_its_sampler_for_its_own_leaves() {
    let (first, first_processor) = test_emitter(SinkId::new("first"));
    let (second, second_processor) = test_emitter(SinkId::new("second"));
    let (outside, outside_processor) = test_emitter(SinkId::new("outside"));
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Drop);
    let nested = Sink::composite([first, second]).with_event_sampler(sampler);
    let composite = Sink::composite([nested, outside]);

    emit!(
        composite,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 2,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 2);
    assert!(first_processor.is_empty());
    assert!(second_processor.is_empty());
    assert_eq!(outside_processor.single_event(), user_action(2));
}

#[test]
fn noop_attachment_is_accepted_and_never_samples() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::noop().with_event_sampler(sampler);
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

#[test]
fn empty_composite_attachment_is_accepted_and_never_samples() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::composite(std::iter::empty()).with_event_sampler(sampler);
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

#[test]
fn processorless_leaf_attachment_is_accepted_and_never_samples() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let sink = Sink::new(TEST_ID, Vec::new(), tick::SimpleClock::new_frozen()).with_event_sampler(sampler);
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

#[test]
fn processorless_leaf_in_composite_never_samples() {
    let (sampler, calls) = constant_sampler(EventSamplingDecision::Continue);
    let processorless = Sink::new("processorless", Vec::new(), tick::SimpleClock::new_frozen()).with_event_sampler(sampler);
    let (sink, processor) = test_emitter(TEST_ID);
    let composite = Sink::composite([processorless, sink]);

    emit!(
        composite,
        UserAction {
            user: PiiString("Alice".into()),
            action_code: 1,
        }
    );

    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(processor.len(), 1);
}

#[test]
#[should_panic(expected = "received the same leaf twice")]
fn composite_rejects_same_sink_before_and_after_attachment() {
    let (sink, _processor) = test_emitter(TEST_ID);
    let unsampled = sink.clone();
    let (sampler, _calls) = constant_sampler(EventSamplingDecision::Drop);
    let gated_sink = sink.with_event_sampler(sampler);

    _ = Sink::composite([unsampled, gated_sink]);
}
