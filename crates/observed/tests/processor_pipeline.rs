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

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use observed::{Event, Severity, Sink, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::types::{PublicBool, PublicI64};
use observed_testing::{ExpectedEvent, MockProcessor, TEST_ID};

#[derive(Debug, Event)]
#[event(name = "user.login")]
#[log(severity = info)]
struct UserLogin {
    user_id: PublicI64,
    mfa_used: PublicBool,
}

#[derive(Debug, Event)]
#[event(name = "auth.failed")]
#[log(severity = warn)]
struct AuthFailed {
    attempts: PublicI64,
}

// ---- Tests ----

#[test]
fn severity_filter_drops_low_severity_events() {
    #[derive(Debug, Event)]
    #[event(name = "system.crash")]
    #[log(severity = fatal)]
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
        ExpectedEvent::new("auth.failed", Severity::Warn).dimension("attempts", "3").log()
    );
    assert_eq!(
        events[1],
        ExpectedEvent::new("system.crash", Severity::Fatal)
            .dimension("exit_code", "1")
            .log()
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
        ExpectedEvent::new("auth.failed", Severity::Warn).dimension("attempts", "5").log()
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
            .log()
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
            .log()
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

    #[derive(Debug, Event)]
    #[event(name = "internal.heartbeat")]
    #[log(severity = trace)]
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
            self.events.lock().unwrap().push(event.name().to_owned());

            // Attempt reentrant emission - this should be silently dropped
            emit!(self.inner_emitter, Heartbeat);
        }

        fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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

    let captured = processor.events.lock().unwrap();
    assert_eq!(
        captured.len(),
        1,
        "only the outer event should be captured; reentrant event is dropped"
    );
    assert_eq!(captured[0], "test.probe");
    assert!(inner_processor.is_empty(), "reentrant event should be dropped, not forwarded");
}
