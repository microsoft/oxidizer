// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the processor pipeline: interest-based lazy construction,
//! multi-processor fan-out, composable emitters, and reentrancy safety.
//!
//! Covers DESIGN.md requirements:
//! - Interest-based lazy construction (`is_interested` gates event closure)
//! - Per-processor filtering inside `process()`
//! - Composite emitters (`Sink::composite`)
//! - Zero-cost when inactive (noop sink or all-rejecting processors skip construction)
//! - Per-processor redaction (each processor gets its own `RedactionEngine`)
//! - Reentrancy safety (processor calling `emit!` doesn't deadlock)

use std::cell::Cell;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use observed::{Severity, Sink, emit, event};
use observed_testing::events::ProbeEvent;
use observed_testing::types::{PublicBool, PublicI64, PublicString};
use observed_testing::{ExpectedEvent, MockProcessor, TEST_ID};

#[event("user.login")]
#[info]
struct UserLogin {
    user_id: PublicI64,
    mfa_used: PublicBool,
}

#[event("auth.failed")]
#[warning]
struct AuthFailed {
    attempts: PublicI64,
}

// ---- Tests ----

#[test]
fn severity_filter_drops_low_severity_events() {
    #[event("system.crash")]
    #[fatal]
    struct SystemCrash {
        exit_code: PublicI64,
    }

    let processor = MockProcessor::with_filter(|desc| desc.log().is_some_and(|l| l.severity() >= Severity::Warn));
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    // Info-level -> dropped by filter
    emit!(
        sink,
        UserLogin {
            user_id: PublicI64(1),
            mfa_used: PublicBool(false)
        }
    );
    // Warn-level -> passes filter
    emit!(sink, AuthFailed { attempts: PublicI64(3) });
    // Fatal-level -> passes filter
    emit!(sink, SystemCrash { exit_code: PublicI64(1) });

    let events = processor.events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0],
        ExpectedEvent::new("auth.failed", Severity::Warn).dimension("attempts", "3")
    );
    assert_eq!(
        events[1],
        ExpectedEvent::new("system.crash", Severity::Fatal).dimension("exit_code", "1")
    );
}

#[test]
fn uninterested_processor_receives_nothing() {
    let processor = MockProcessor::with_filter(|_| false);
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    emit!(
        sink,
        UserLogin {
            user_id: PublicI64(1),
            mfa_used: PublicBool(true)
        }
    );
    emit!(sink, AuthFailed { attempts: PublicI64(1) });

    assert!(processor.is_empty());
}

#[test]
fn flushing_a_failing_processor_reports_it_and_keeps_the_events() {
    let processor = MockProcessor::with_flush_error("mock", "exporter offline");
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    emit!(sink, AuthFailed { attempts: PublicI64(3) });

    let error = sink.flush().expect_err("the mock processor always fails to flush");
    assert_eq!(error.failures().len(), 1);
    assert_eq!(error.failures()[0].processor(), "mock");
    // A failing flush must not discard what was already captured.
    assert_eq!(processor.len(), 1);
}

#[test]
fn multiple_processors_receive_events_independently() {
    let all_processor = MockProcessor::new();
    let warn_processor = MockProcessor::with_filter(|desc| desc.log().is_some_and(|l| l.severity() >= Severity::Warn));

    let sink = Sink::new(
        "test",
        vec![Arc::new(all_processor.clone()), Arc::new(warn_processor.clone())],
        tick::SimpleClock::new_frozen(),
    );

    emit!(
        sink,
        UserLogin {
            user_id: PublicI64(1),
            mfa_used: PublicBool(false)
        }
    );
    emit!(sink, AuthFailed { attempts: PublicI64(5) });

    assert_eq!(all_processor.len(), 2);
    assert_eq!(warn_processor.len(), 1);
    assert_eq!(
        warn_processor.single_event(),
        ExpectedEvent::new("auth.failed", Severity::Warn).dimension("attempts", "5")
    );
}

#[test]
fn composite_fans_out_to_each_child() {
    let base_processor = MockProcessor::new();
    let extra_processor = MockProcessor::new();

    let base = Sink::new("test", vec![Arc::new(base_processor.clone())], tick::SimpleClock::new_frozen());

    let extra = Sink::new("test", vec![Arc::new(extra_processor.clone())], tick::SimpleClock::new_frozen());

    let composed = Sink::composite([base.clone(), extra]);

    emit!(
        composed,
        UserLogin {
            user_id: PublicI64(42),
            mfa_used: PublicBool(true)
        }
    );

    // Both processors receive the event: the composite dispatches through
    // each child in turn, and each child's own processors see it.
    assert_eq!(base_processor.len(), 1);
    assert_eq!(extra_processor.len(), 1);

    // Emitting through `base` alone only reaches base's processor.
    emit!(
        base,
        UserLogin {
            user_id: PublicI64(1),
            mfa_used: PublicBool(false)
        }
    );
    assert_eq!(base_processor.len(), 2);
    assert_eq!(extra_processor.len(), 1);

    let event = &base_processor.events()[1];
    assert_eq!(
        *event,
        ExpectedEvent::new("user.login", Severity::Info)
            .dimension("mfa_used", "false")
            .dimension("user_id", "1")
    );
}

#[test]
#[expect(clippy::redundant_clone, reason = "Testing")]
fn emitter_clone_shares_processors() {
    let (sink, processor) = observed_testing::test_emitter(TEST_ID);

    let cloned = sink.clone();
    emit!(
        cloned,
        UserLogin {
            user_id: PublicI64(99),
            mfa_used: PublicBool(true)
        }
    );
    assert_eq!(processor.len(), 1);
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("user.login", Severity::Info)
            .dimension("mfa_used", "true")
            .dimension("user_id", "99")
    );
}

// ---------------------------------------------------------------------------
// Lazy construction - zero-cost when inactive
// ---------------------------------------------------------------------------

#[test]
fn lazy_construction() {
    static CONSTRUCTION_COUNT: AtomicU32 = AtomicU32::new(0);

    // All processors reject via is_interested - closure should NOT be called.
    let rejected = MockProcessor::with_filter(|_| false);
    let sink = Sink::new(
        "test",
        vec![Arc::new(rejected.clone()), Arc::new(rejected.clone())],
        tick::SimpleClock::new_frozen(),
    );

    emit!(
        sink,
        ProbeEvent {
            value: {
                CONSTRUCTION_COUNT.fetch_add(1, Ordering::SeqCst);
                PublicI64(42)
            },
        }
    );

    // it should always be zero, otherwise optimization is broken and has to be fixed.
    assert_eq!(CONSTRUCTION_COUNT.load(Ordering::SeqCst), 0);
    assert!(rejected.is_empty());

    // Multiple interested processors - closure called exactly once, both receive event.
    let processor_a = MockProcessor::new();
    let processor_b = MockProcessor::new();
    let sink = Sink::new(
        "test",
        vec![Arc::new(processor_a.clone()), Arc::new(processor_b.clone())],
        tick::SimpleClock::new_frozen(),
    );

    emit!(
        sink,
        ProbeEvent {
            value: {
                CONSTRUCTION_COUNT.fetch_add(1, Ordering::SeqCst);
                PublicI64(42)
            },
        }
    );

    assert_eq!(CONSTRUCTION_COUNT.load(Ordering::SeqCst), 1);
    assert_eq!(processor_a.len(), 1);
    assert_eq!(processor_b.len(), 1);
}

// ---------------------------------------------------------------------------
// Reentrancy safety - processor calling emit! must not deadlock
// ---------------------------------------------------------------------------

#[test]
fn reentrant_emit_from_processor_push_does_not_deadlock() {
    use std::sync::{Arc, Mutex};

    #[event("internal.heartbeat")]
    #[trace]
    struct Heartbeat;

    /// A processor that tries to `emit!` during `process()`.
    /// The reentrancy guard should silently drop the recursive event.
    struct ReentrantProcessor {
        inner_emitter: Sink,
        events: Mutex<Vec<String>>,
    }

    impl observed::processing::EventProcessor for ReentrantProcessor {
        fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
            true
        }

        fn process(&self, event: &observed::processing::EventView<'_>) {
            self.events.lock().expect("lock is not poisoned").push(event.name().to_owned());

            // Attempt reentrant emission - this should be silently dropped
            emit!(self.inner_emitter, Heartbeat);
        }

        fn flush(&self) -> Result<(), observed::FlushError> {
            Ok(())
        }
    }

    let (inner_emitter, inner_processor) = observed_testing::test_emitter(TEST_ID);

    let processor = Arc::new(ReentrantProcessor {
        inner_emitter,
        events: Mutex::new(Vec::new()),
    });

    let sink = Sink::new(
        "test",
        vec![Arc::clone(&processor) as Arc<dyn observed::processing::EventProcessor>],
        tick::SimpleClock::new_frozen(),
    );

    // This should complete without deadlock. The inner Heartbeat should be silently dropped.
    emit!(sink, ProbeEvent::new(1));

    let captured = processor.events.lock().expect("lock is not poisoned");
    assert_eq!(
        captured.len(),
        1,
        "only the outer event should be captured; reentrant event is dropped"
    );
    assert_eq!(captured[0], "test.probe");
    assert!(inner_processor.is_empty(), "reentrant event should be dropped, not forwarded");
}

// ---------------------------------------------------------------------------
// Cost of emitting: what is deferred, and what is not
// ---------------------------------------------------------------------------

// Counts how often the log-only field's initializer actually runs.
//
// Thread-local rather than a `static`: the test harness gives each test its own
// thread and emission is synchronous, so this keeps the two tests below from
// racing on a shared counter when they run in parallel.
thread_local! {
    static DETAIL_INITS: Cell<u32> = const { Cell::new(0) };
}

/// Stands in for expensive or sensitive work that only the log signal needs.
fn expensive_detail() -> PublicString {
    DETAIL_INITS.with(|c| c.set(c.get() + 1));
    PublicString("profile".to_owned())
}

/// Returns how many times `expensive_detail` ran on this thread.
fn detail_inits() -> u32 {
    DETAIL_INITS.with(Cell::get)
}

/// An event with an event-level metric plus a log-only field.
#[event("cost.probe")]
#[info]
#[counter(name = "cost.probe.count")]
struct CostProbe {
    #[dimension(metric)]
    kind: PublicI64,
    detail: PublicString,
}

/// A processor that pulls only metric-routed fields, counting what it extracted.
struct MetricOnlyProcessor {
    interested: bool,
    extracted: Arc<Mutex<Vec<&'static str>>>,
    engine: data_privacy::RedactionEngine,
}

impl observed::processing::EventProcessor for MetricOnlyProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        self.interested
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        let _ = event.visit_fields(&mut |descriptor, get_value| {
            if descriptor.metric().is_some() {
                let _ = get_value(&self.engine);
                self.extracted.lock().expect("lock is not poisoned").push(descriptor.field_name());
            }
            ControlFlow::Continue(())
        });
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

fn metric_only_sink(interested: bool) -> (Sink, Arc<Mutex<Vec<&'static str>>>) {
    let extracted = Arc::new(Mutex::new(Vec::new()));
    let processor = MetricOnlyProcessor {
        interested,
        extracted: Arc::clone(&extracted),
        engine: observed_testing::passthrough_redaction_engine(),
    };
    let sink = Sink::new(
        "cost",
        vec![Arc::new(processor) as Arc<dyn observed::processing::EventProcessor>],
        tick::SimpleClock::new_frozen(),
    );
    (sink, extracted)
}

/// When no processor is interested the event expression never runs at all, so
/// not even a log-only field's initializer is evaluated.
#[test]
fn uninterested_sink_never_evaluates_the_event_expression() {
    let (sink, extracted) = metric_only_sink(false);

    emit!(
        sink,
        CostProbe {
            kind: PublicI64(1),
            detail: expensive_detail(),
        }
    );

    assert_eq!(detail_inits(), 0);
    assert!(extracted.lock().expect("lock is not poisoned").is_empty());
}

/// Pins the actual signal-level contract: laziness past construction is per
/// field, not per signal. A metric-only processor skips *extracting* the
/// log-only field - no clone, no redaction call - but the expression that
/// initialized that field has already run, because interest from any processor
/// evaluates the whole event expression.
#[test]
fn interested_sink_evaluates_every_field_but_extracts_only_what_it_pulls() {
    let (sink, extracted) = metric_only_sink(true);

    emit!(
        sink,
        CostProbe {
            kind: PublicI64(1),
            detail: expensive_detail(),
        }
    );

    // Construction is not signal-aware: the log-only initializer ran.
    assert_eq!(detail_inits(), 1);
    // Extraction is: only the metric dimension was pulled through the engine.
    assert_eq!(*extracted.lock().expect("lock is not poisoned"), vec!["kind"]);
}
