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

#[test]
fn container_decode_rejects_truncation_and_invalid_policies_without_panicking() {
    let fixture = DecodedSnapshot {
        events: Events {
            clock: EventClock::CURRENT,
            ..Events::default()
        },
        ..DecodedSnapshot::default()
    };
    let encoded = test_util::encode(&fixture).unwrap();
    assert_eq!(decode(encoded.as_bytes()).unwrap(), fixture);

    for length in 0..encoded.as_bytes().len() {
        assert!(decode(&encoded.as_bytes()[..length]).is_err(), "input length {length}");
    }
    for offset in [32, 40, 48, 56, 64, 72] {
        let mut bytes = encoded.as_bytes().to_vec();
        bytes[offset] = 2;

        assert!(decode(&bytes).is_err(), "policy offset {offset}");
    }
}

#[test]
fn empty_thread_names_need_no_padding_and_invalid_utf8_is_reported_as_an_error() {
    let mut fixture = DecodedSnapshot {
        events: Events {
            clock: EventClock::CURRENT,
            threads: (1..=5)
                .map(|thread| ThreadLog {
                    thread_id: ThreadId::new(thread),
                    total_events: 0,
                    lost_events: 0,
                    name: String::new(),
                })
                .collect(),
            ..Events::default()
        },
        ..DecodedSnapshot::default()
    };
    let encoded = test_util::encode(&fixture).unwrap();
    assert_eq!(decode(encoded.as_bytes()).unwrap(), fixture);

    fixture.events.threads[0].name = "unique-thread-name".into();
    let mut bytes = test_util::encode(&fixture).unwrap().as_bytes().to_vec();
    let name = bytes
        .windows(b"unique-thread-name".len())
        .position(|value| value == b"unique-thread-name")
        .unwrap();
    bytes[name] = 0xff;

    assert_eq!(
        decode(&bytes).unwrap_err().to_string(),
        "the seismograph snapshot is malformed or unsupported"
    );
}

#[test]
fn container_codec_propagates_symbol_validation_failures() {
    let mut fixture = DecodedSnapshot {
        events: Events {
            clock: EventClock::CURRENT,
            ..Events::default()
        },
        symbols: vec![Symbol {
            address: Address::new(0x1122_3344_5566_7788),
            name: None,
            filename: None,
            line: None,
            column: None,
        }],
        ..DecodedSnapshot::default()
    };
    let mut bytes = test_util::encode(&fixture).unwrap().as_bytes().to_vec();
    assert_eq!(decode(&bytes).unwrap(), fixture);
    let address = bytes
        .windows(8)
        .position(|value| value == 0x1122_3344_5566_7788_u64.to_le_bytes())
        .unwrap();
    bytes[address..address + 8].fill(0);

    assert_eq!(
        decode(&bytes).unwrap_err().to_string(),
        "the seismograph snapshot is malformed or unsupported"
    );
    fixture.symbols[0].address = Address::new(0);
    assert_eq!(
        test_util::encode(&fixture).unwrap_err().to_string(),
        "the seismograph snapshot is malformed or unsupported"
    );
}
