// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Controlled event symbols survive the public snapshot codec without sources.

use seismograph::recorder::event::{Address, Event, EventClock, EventKind, EventPayload, EventSequence, EventTimestamp, Events, ObjectId};
use seismograph::recorder::thread::{ThreadId, ThreadLog};
use seismograph::snapshot::{DecodedSnapshot, Symbol, decode, test_util};

#[test]
fn snapshots_preserve_event_symbols_without_an_allocator_or_runtime_source() {
    let events = Events {
        clock: EventClock::CURRENT,
        total_events: 1,
        threads: vec![ThreadLog {
            thread_id: ThreadId::new(1),
            total_events: 1,
            lost_events: 0,
            name: "producer".into(),
        }],
        events: vec![Event {
            thread_id: ThreadId::new(1),
            sequence: EventSequence::new(1),
            timestamp: EventTimestamp::from_ticks(1),
            kind: EventKind::MutexAccess,
            payload: EventPayload::Object(ObjectId::new(17)),
            call_stack: [32, 16, 16, 0].map(Address::new).to_vec(),
        }],
        ..Events::default()
    };
    let mut resolved = Vec::new();
    let encoded = test_util::capture(
        DecodedSnapshot {
            events: events.clone(),
            ..DecodedSnapshot::default()
        },
        |symbol| {
            resolved.push(symbol.address);
            if symbol.address == Address::new(16) {
                symbol.name = Some("producer::allocate".into());
                symbol.filename = Some("producer.rs".into());
                symbol.line = Some(42);
                symbol.column = Some(7);
            }
        },
    )
    .unwrap();

    let decoded = decode(encoded.as_bytes()).unwrap();

    assert_eq!(resolved, [Address::new(16), Address::new(32)]);
    assert_eq!(decoded.events, events);
    assert_eq!(decoded.sources, []);
    assert_eq!(
        decoded.symbols,
        [
            Symbol {
                address: Address::new(16),
                name: Some("producer::allocate".into()),
                filename: Some("producer.rs".into()),
                line: Some(42),
                column: Some(7),
            },
            Symbol {
                address: Address::new(32),
                name: None,
                filename: None,
                line: None,
                column: None,
            },
        ]
    );
}
