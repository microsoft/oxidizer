// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Routing and context contracts for already-constructed dynamic events.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::borrow::Cow;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use data_privacy::RedactionEngine;
use observed::enrichment::EnrichFnExt;
use observed::interop::{DynEvent, emit_dyn_event};
use observed::metadata::EventDescription;
use observed::processing::{EventProcessor, EventView, FieldVisitorFn};
use observed::{Enrichment, EventSampler, EventSamplingContext, EventSamplingDecision, FlushError, Sink, SinkId, Value, emit, event};
use tick::{ClockControl, SimpleClock};

/// Supplies a dynamic description that can be initialized during delivery.
#[derive(Default)]
struct DynamicEvent {
    initialized_name: Arc<OnceLock<&'static str>>,
    descriptions: AtomicUsize,
}

#[mutants::skip]
impl DynEvent for DynamicEvent {
    fn name(&self) -> &'static str {
        self.initialized_name.get().copied().unwrap_or("dynamic.event")
    }

    fn body(&self) -> Option<Cow<'static, str>> {
        None
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
        self.descriptions.fetch_add(1, Ordering::Relaxed);
        EventDescription::new(self.name(), None, None, None, false, false)
    }
}

/// Configures deterministic interest and delivery effects without an exporter.
struct Processor<I, P> {
    interested: I,
    process: P,
}

#[mutants::skip]
impl<I, P> EventProcessor for Processor<I, P>
where
    I: Fn(&EventDescription) -> bool + Send + Sync,
    P: Fn(&EventView<'_>) + Send + Sync,
{
    fn is_interested(&self, description: &EventDescription) -> bool {
        (self.interested)(description)
    }

    fn process(&self, event: &EventView<'_>) {
        (self.process)(event);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

#[mutants::skip]
fn processor(
    interested: impl Fn(&EventDescription) -> bool + Send + Sync + 'static,
    process: impl Fn(&EventView<'_>) + Send + Sync + 'static,
) -> Arc<dyn EventProcessor> {
    Arc::new(Processor { interested, process })
}

/// Runs a synchronous sampling effect before returning its decision.
struct Sampler<F>(F);

#[mutants::skip]
impl<F> EventSampler for Sampler<F>
where
    F: Fn(&EventSamplingContext<'_>) -> EventSamplingDecision + Send + Sync + 'static,
{
    fn sample(&self, event: &EventSamplingContext<'_>) -> EventSamplingDecision {
        (self.0)(event)
    }
}

#[mutants::skip]
fn leaf(processors: Vec<Arc<dyn EventProcessor>>) -> Sink {
    Sink::new("dynamic", processors, SimpleClock::new_frozen())
}

/// Records one processor's view independently of subsequent context changes.
#[derive(Debug, PartialEq)]
struct Record {
    recipient: &'static str,
    timestamp: SystemTime,
    enrichments: Vec<(&'static str, Value)>,
}

#[mutants::skip]
impl Record {
    fn capture(recipient: &'static str, event: &EventView<'_>) -> Self {
        let mut enrichments = Vec::new();
        let redactor = RedactionEngine::default();
        let _ = event.visit_enrichments(&mut |description, value| {
            enrichments.push((description.field_name(), value(&redactor)));
            ControlFlow::Continue(())
        });
        Self {
            recipient,
            timestamp: event.timestamp(),
            enrichments,
        }
    }
}

/// Distinguishes leaf-local enrichment and isolation in composite dispatch.
#[derive(Enrichment)]
struct Scope {
    #[unredacted]
    scope: &'static str,
}

/// Exercises typed emissions nested inside dynamic processor callbacks.
#[event("nested.typed")]
struct TypedEvent;

#[test]
fn noop_and_empty_sinks_do_not_inspect_dynamic_events() {
    let event = DynamicEvent::default();
    for sink in [Sink::noop(), leaf(vec![]), Sink::composite([]), Sink::composite([leaf(vec![])])] {
        emit_dyn_event(&sink, &event);
    }
    assert_eq!(event.descriptions.load(Ordering::Relaxed), 0);
}

#[test]
fn dynamic_events_route_to_interested_processors_in_order() {
    for interests in [[true, true, true, true], [false, true, false, true], [false; 4]] {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let processors = interests
            .iter()
            .copied()
            .enumerate()
            .map(|(index, interested)| {
                processor(move |_| interested, {
                    let seen = Arc::clone(&seen);
                    move |_| seen.lock().unwrap().push(index)
                })
            })
            .collect();
        let sink = leaf(processors);

        emit_dyn_event(&sink, &DynamicEvent::default());

        let expected: Vec<_> = interests
            .iter()
            .enumerate()
            .filter_map(|(i, interested)| interested.then_some(i))
            .collect();
        assert_eq!(*seen.lock().unwrap(), expected);
    }
}

#[test]
fn an_earlier_query_does_not_reserve_dynamic_delivery() {
    for (before, after) in [(false, true), (true, false)] {
        let initialized = Arc::new(OnceLock::new());
        let delivered = Arc::new(AtomicUsize::new(0));
        let sink = leaf(vec![processor(
            {
                let initialized = Arc::clone(&initialized);
                move |_| *initialized.get().unwrap_or(&before)
            },
            {
                let delivered = Arc::clone(&delivered);
                move |_| {
                    delivered.fetch_add(1, Ordering::Relaxed);
                }
            },
        )]);
        let event = DynamicEvent::default();

        assert_eq!(sink.is_interested(&event.description()), before);
        initialized.set(after).unwrap();
        emit_dyn_event(&sink, &event);

        assert_eq!(delivered.load(Ordering::Relaxed), usize::from(after));
    }
}

#[test]
fn later_recipients_observe_initialization_from_earlier_processors() {
    for composite in [false, true] {
        for (before, after) in [(false, true), (true, false)] {
            let initialized = Arc::new(OnceLock::new());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let first = processor(|_| true, {
                let initialized = Arc::clone(&initialized);
                let seen = Arc::clone(&seen);
                move |_| {
                    seen.lock().unwrap().push("first");
                    initialized.set(after).unwrap();
                }
            });
            let second = processor(
                {
                    let initialized = Arc::clone(&initialized);
                    move |_| *initialized.get().unwrap_or(&before)
                },
                {
                    let seen = Arc::clone(&seen);
                    move |_| seen.lock().unwrap().push("second")
                },
            );
            let sink = if composite {
                Sink::composite([leaf(vec![first]), leaf(vec![second])])
            } else {
                leaf(vec![first, second])
            };

            emit_dyn_event(&sink, &DynamicEvent::default());

            let expected = if after { vec!["first", "second"] } else { vec!["first"] };
            assert_eq!(*seen.lock().unwrap(), expected);
        }
    }
}

#[test]
fn routing_uses_one_description_snapshot_across_recipients() {
    for composite in [false, true] {
        let event = DynamicEvent::default();
        let seen = Arc::new(AtomicUsize::new(0));
        let first = processor(|_| true, {
            let name = Arc::clone(&event.initialized_name);
            move |_| name.set("initialized.event").unwrap()
        });
        let second = processor(|description| description.name() == "dynamic.event", {
            let seen = Arc::clone(&seen);
            move |_| {
                seen.fetch_add(1, Ordering::Relaxed);
            }
        });
        let sink = if composite {
            Sink::composite([leaf(vec![first]), leaf(vec![second])])
        } else {
            leaf(vec![first, second])
        };

        emit_dyn_event(&sink, &event);

        assert_eq!(seen.load(Ordering::Relaxed), 1);
        assert_eq!(event.name(), "initialized.event");
    }
}

#[test]
fn sampled_dynamic_events_recheck_every_recipient_after_initialization() {
    for decision in [EventSamplingDecision::Continue, EventSamplingDecision::Drop] {
        let initialized = Arc::new(OnceLock::new());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let samples = Arc::new(AtomicUsize::new(0));
        let processors = [false, true]
            .into_iter()
            .map(|before| {
                processor(
                    {
                        let initialized = Arc::clone(&initialized);
                        move |_| if initialized.get().is_some() { !before } else { before }
                    },
                    {
                        let seen = Arc::clone(&seen);
                        move |_| seen.lock().unwrap().push(before)
                    },
                )
            })
            .collect();
        let sink = leaf(processors).with_event_sampler(Arc::new(Sampler({
            let samples = Arc::clone(&samples);
            move |_: &EventSamplingContext<'_>| {
                samples.fetch_add(1, Ordering::Relaxed);
                initialized.set(()).unwrap();
                decision
            }
        })));

        emit_dyn_event(&sink, &DynamicEvent::default());

        assert_eq!(samples.load(Ordering::Relaxed), 1);
        let expected = if decision == EventSamplingDecision::Continue {
            vec![false]
        } else {
            vec![]
        };
        assert_eq!(*seen.lock().unwrap(), expected);
    }
}

#[test]
fn uninterested_dynamic_leaves_do_not_sample() {
    let samples = Arc::new(AtomicUsize::new(0));
    let delivered = Arc::new(AtomicUsize::new(0));
    let uninterested = leaf(vec![processor(|_| false, |_| panic!("uninterested processor was dispatched"))]);
    let interested = leaf(vec![processor(|_| true, {
        let delivered = Arc::clone(&delivered);
        move |_| {
            delivered.fetch_add(1, Ordering::Relaxed);
        }
    })]);
    let composite = Sink::composite([uninterested.clone(), interested, leaf(vec![])]).with_event_sampler(Arc::new(Sampler({
        let samples = Arc::clone(&samples);
        move |_: &EventSamplingContext<'_>| {
            samples.fetch_add(1, Ordering::Relaxed);
            EventSamplingDecision::Continue
        }
    })));
    let single = uninterested.with_event_sampler(Arc::new(Sampler({
        let samples = Arc::clone(&samples);
        move |_: &EventSamplingContext<'_>| {
            samples.fetch_add(1, Ordering::Relaxed);
            EventSamplingDecision::Continue
        }
    })));

    emit_dyn_event(&single, &DynamicEvent::default());
    assert_eq!(samples.load(Ordering::Relaxed), 0);
    emit_dyn_event(&composite, &DynamicEvent::default());
    assert_eq!(samples.load(Ordering::Relaxed), 1);
    assert_eq!(delivered.load(Ordering::Relaxed), 1);
}

#[test]
fn uninterested_dynamic_leaf_does_not_read_its_clock() {
    // Advancing on reads makes clock access observable without elapsed real time.
    let control = ClockControl::new().auto_advance(Duration::from_secs(1));
    let sink = Sink::new(
        "uninterested",
        vec![processor(|_| false, |_| panic!("uninterested processor was dispatched"))],
        control.to_simple_clock(),
    );

    emit_dyn_event(&sink, &DynamicEvent::default());

    assert_eq!(control.to_simple_clock().system_time(), SystemTime::UNIX_EPOCH);
}

#[test]
fn a_sampled_leaf_does_not_suppress_an_unsampled_sibling() {
    let delivered = Arc::new(AtomicUsize::new(0));
    let sampled = leaf(vec![processor(|_| true, |_| panic!("sampled-out processor was dispatched"))])
        .with_event_sampler(Arc::new(Sampler(|_: &EventSamplingContext<'_>| EventSamplingDecision::Drop)));
    let unsampled = leaf(vec![processor(|_| true, {
        let delivered = Arc::clone(&delivered);
        move |_| {
            delivered.fetch_add(1, Ordering::Relaxed);
        }
    })]);
    let sink = Sink::composite([sampled, unsampled]);

    emit_dyn_event(&sink, &DynamicEvent::default());

    assert_eq!(delivered.load(Ordering::Relaxed), 1);
}

#[test]
fn initial_interest_initialization_can_emit_to_another_sink() {
    for dynamic in [false, true] {
        for interested in [false, true] {
            for sampled in [false, true] {
                let nested_count = Arc::new(AtomicUsize::new(0));
                let nested = leaf(vec![processor(|_| true, {
                    let nested_count = Arc::clone(&nested_count);
                    move |_| {
                        nested_count.fetch_add(1, Ordering::Relaxed);
                    }
                })]);
                let initialized = OnceLock::new();
                let delivered = Arc::new(AtomicUsize::new(0));
                let sink = leaf(vec![processor(
                    move |_| {
                        initialized.get_or_init(|| {
                            emit_dyn_event(&nested, &DynamicEvent::default());
                            emit!(&nested, TypedEvent);
                        });
                        interested
                    },
                    {
                        let delivered = Arc::clone(&delivered);
                        move |_| {
                            delivered.fetch_add(1, Ordering::Relaxed);
                        }
                    },
                )]);
                let sink = if sampled {
                    sink.with_event_sampler(Arc::new(Sampler(|_: &EventSamplingContext<'_>| EventSamplingDecision::Continue)))
                } else {
                    sink
                };
                let sink = Sink::composite([leaf(vec![]), sink]);

                // Typed admission provides the same unguarded initialization boundary.
                if dynamic {
                    emit_dyn_event(&sink, &DynamicEvent::default());
                } else {
                    emit!(&sink, TypedEvent);
                }

                assert_eq!(nested_count.load(Ordering::Relaxed), 2);
                assert_eq!(delivered.load(Ordering::Relaxed), usize::from(interested));
            }
        }
    }
}

#[test]
fn composite_leaves_keep_distinct_context_and_share_one_view_per_leaf() {
    // Distinct fixed times identify which leaf clock supplied each timestamp.
    let first_time = SystemTime::UNIX_EPOCH;
    let second_time = first_time + Duration::from_secs(1);
    let control = ClockControl::new_at(first_time);
    let records = Arc::new(Mutex::new(Vec::new()));
    let first = Sink::new(
        "first",
        vec![
            processor(|_| true, {
                let records = Arc::clone(&records);
                let control = control.clone();
                move |event| {
                    let record = Record::capture("first-a", event);
                    records.lock().unwrap().push(record);
                    // A later processor must retain this emission's captured timestamp.
                    control.advance(Duration::from_secs(1));
                }
            }),
            processor(|_| true, {
                let records = Arc::clone(&records);
                move |event| {
                    let record = Record::capture("first-b", event);
                    records.lock().unwrap().push(record);
                }
            }),
        ],
        control.to_simple_clock(),
    );
    let second = Sink::new_isolated(
        "second",
        vec![processor(|_| true, {
            let records = Arc::clone(&records);
            move |event| {
                let record = Record::capture("second", event);
                records.lock().unwrap().push(record);
            }
        })],
        SimpleClock::new_frozen_at(second_time),
    );
    let composite = Sink::composite([first.clone(), Sink::noop(), leaf(vec![]), second.clone()]);

    (|| emit_dyn_event(&composite, &DynamicEvent::default()))
        .enrich(&first, Scope { scope: "first" })
        .enrich(&second, Scope { scope: "ignored" })
        .enrich_for(&second, SinkId::new("second"), Scope { scope: "second" })();

    assert_eq!(
        *records.lock().unwrap(),
        [
            Record {
                recipient: "first-a",
                timestamp: first_time,
                enrichments: vec![("scope", Value::from("first"))],
            },
            Record {
                recipient: "first-b",
                timestamp: first_time,
                enrichments: vec![("scope", Value::from("first"))],
            },
            Record {
                recipient: "second",
                timestamp: second_time,
                enrichments: vec![("scope", Value::from("second"))],
            },
        ]
    );
}

#[test]
fn recursion_guard_covers_processors_and_samplers_across_composite_leaves() {
    for sampled in [false, true] {
        let nested_count = Arc::new(AtomicUsize::new(0));
        let nested = leaf(vec![processor(|_| true, {
            let nested_count = Arc::clone(&nested_count);
            move |_| {
                nested_count.fetch_add(1, Ordering::Relaxed);
            }
        })]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let first = leaf(vec![processor(|_| true, {
            let nested = nested.clone();
            let seen = Arc::clone(&seen);
            move |_| {
                seen.lock().unwrap().push("first");
                emit_dyn_event(&nested, &DynamicEvent::default());
                emit!(&nested, TypedEvent);
            }
        })]);
        let second = leaf(vec![processor(|_| true, {
            let seen = Arc::clone(&seen);
            move |_| seen.lock().unwrap().push("second")
        })]);
        let sink = Sink::composite([first, second]);
        let sink = if sampled {
            sink.with_event_sampler(Arc::new(Sampler({
                let nested = nested.clone();
                move |_: &EventSamplingContext<'_>| {
                    emit_dyn_event(&nested, &DynamicEvent::default());
                    emit!(&nested, TypedEvent);
                    EventSamplingDecision::Continue
                }
            })))
        } else {
            sink
        };

        emit_dyn_event(&sink, &DynamicEvent::default());

        assert_eq!(*seen.lock().unwrap(), ["first", "second"]);
        assert_eq!(nested_count.load(Ordering::Relaxed), 0);
        emit_dyn_event(&nested, &DynamicEvent::default());
        assert_eq!(nested_count.load(Ordering::Relaxed), 1);
    }
}
