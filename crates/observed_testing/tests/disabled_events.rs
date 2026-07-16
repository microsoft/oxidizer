// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for the disabled event attribute and runtime event configuration.
//!
//! Covers DESIGN.md requirements:
//! - Events can be disabled by default (`#[event(name = "...", disabled)]`)
//! - Disabled events are not emitted unless a processor explicitly opts in

use std::sync::Arc;

use observed::{Event, Severity, Sink, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::types::PublicI64;
use observed_testing::{ExpectedEvent, MockProcessor, TEST_ID};

#[derive(Debug, Event)]
#[event(name = "internal.trace_detail", disabled)]
#[log(severity = debug)]
struct DisabledEvent {
    detail: PublicI64,
}

#[test]
fn disabled_event_captured_by_default_processor_with_flag() {
    let (sink, processor) = observed_testing::test_emitter(TEST_ID);

    emit!(sink, DisabledEvent { detail: PublicI64(42) });

    // The MockProcessor accepts all events by default (no filter),
    // so it WILL receive the disabled event - the "disabled" flag is a hint
    // for processors, not a hard block.
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("internal.trace_detail", Severity::Debug)
            .dimension("detail", "42")
            .log()
            .disabled(),
    );
}

#[test]
fn disabled_event_filtered_by_log_and_metric_proc() {
    let processor = MockProcessor::with_filter(|desc| !desc.is_disabled() && (desc.is_log() || desc.contains_metrics()));
    let sink = Sink::new("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    emit!(sink, DisabledEvent { detail: PublicI64(1) });
    emit!(sink, ProbeEvent::new(1));

    let events = processor.events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0],
        ExpectedEvent::new("test.probe", Severity::Info).dimension("value", "1").log()
    );
}

#[test]
fn disabled_event_description_has_disabled_flag() {
    assert!(DisabledEvent::DESCRIPTION.is_disabled());
    assert!(!ProbeEvent::DESCRIPTION.is_disabled());
}
