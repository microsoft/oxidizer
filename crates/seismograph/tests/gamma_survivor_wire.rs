// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic snapshot decoding and metadata regressions.

use seismograph::recorder::event::{EventClock, EventKind, EventPayload, EventTimestamp};
use seismograph::recorder::io::{IoOutcome, IoResourceKind};
use seismograph::snapshot::{SourceData, SourceId, decode};

#[test]
fn source_extent_cannot_consume_the_trailing_symbol_section() {
    let mut bytes = header(10, 0, 0, 1);
    let length_offset = bytes.len() + 12;
    source(&mut bytes, 17, b"source", b"\x12\x34\x56");
    let bytes = finish(bytes);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.sources[0].name, "source");
    assert_eq!(decoded.sources[0].data, b"\x12\x34\x56");
    for advertised in [4u64, u64::MAX] {
        let mut invalid = bytes.clone();
        invalid[length_offset..length_offset + 8].copy_from_slice(&advertised.to_le_bytes());
        assert_eq!(
            decode(&invalid).unwrap_err().to_string(),
            "the seismograph snapshot is malformed or unsupported"
        );
    }
}

#[test]
fn variable_first_source_cannot_hide_a_truncated_second_header_or_name() {
    let mut bytes = header(10, 0, 0, 2);
    source(&mut bytes, 17, &[b'a'; 64], &[0x12; 64]);
    let second = bytes.len();
    source(&mut bytes, 19, b"second", &[]);
    let bytes = finish(bytes);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.sources.len(), 2);
    assert_eq!(decoded.sources[1].name, "second");
    for end in second..bytes.len() {
        assert_eq!(
            decode(&bytes[..end]).unwrap_err().to_string(),
            "the seismograph snapshot is malformed or unsupported",
            "prefix {end}"
        );
    }
}

#[test]
fn unsupported_versions_and_unknown_event_kinds_are_errors_not_panics() {
    for version in 0..10 {
        assert_eq!(
            decode(&finish(header(version, 0, 0, 0))).unwrap_err().to_string(),
            "the seismograph snapshot is malformed or unsupported"
        );
    }
    let mut bytes = event(1, EventKind::MutexAccess, [13, 0, 0, 0, 0, 0, 0, 0]);
    bytes[116] = 255;
    assert_eq!(
        decode(&bytes).unwrap_err().to_string(),
        "the seismograph snapshot is malformed or unsupported"
    );
}

#[test]
fn gamma_round2_current_events_reject_payload_class_mismatches_without_panicking() {
    let bytes = event(4, EventKind::MutexAccess, [13, 17, 19, 23, 29, 31, 0, 0]);
    assert_eq!(
        decode(&bytes).unwrap_err().to_string(),
        "the seismograph snapshot is malformed or unsupported"
    );
}

#[cfg(test)]
fn header(version: u16, threads: u32, events: u32, sources: u32) -> Vec<u8> {
    let mut bytes = b"SEISMOG\0".to_vec();
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0x0807_0605_0403_0201_u64.to_le_bytes());
    for count in [threads, events, sources] {
        bytes.extend_from_slice(&count.to_le_bytes());
    }
    for index in 0_u32..6 {
        bytes.extend_from_slice(&[u8::from(index % 2 == 0), u8::from(index % 2 != 0), 0, 0]);
        bytes.extend_from_slice(&(index + 3).to_le_bytes());
    }
    bytes.extend_from_slice(&[1, 0, 0, 0]);
    bytes.extend_from_slice(&1_000_000_000_u64.to_le_bytes());
    bytes
}

#[cfg(test)]
fn source(bytes: &mut Vec<u8>, id: u64, name: &[u8], payload: &[u8]) {
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&0x0201_u16.to_le_bytes());
    bytes.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(payload);
}

#[cfg(test)]
fn finish(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes
}

#[cfg(test)]
fn event(tag: u8, kind: EventKind, fields: [u64; 8]) -> Vec<u8> {
    let mut bytes = header(10, 0, 1, 0);
    for value in [0x102, 0x304, 0x506] {
        bytes.extend_from_slice(&u64::to_le_bytes(value));
    }
    bytes.extend_from_slice(&[wire_kind(kind), tag, 2, 0]);
    for value in fields.into_iter().chain([0x708, 0x90a]) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    finish(bytes)
}

#[expect(clippy::panic, reason = "Test fixtures must supply an independently mapped event kind")]
fn wire_kind(kind: EventKind) -> u8 {
    match kind {
        EventKind::MutexAccess => 3,
        EventKind::Allocation => 14,
        EventKind::ChannelHighWatermark => 31,
        EventKind::TaskSpawned => 39,
        EventKind::IoReadFinished => 58,
        EventKind::CacheHit => 61,
        _ => panic!("the fixture must specify the kind's independent wire value"),
    }
}

#[test]
fn gamma_survivor_headers_and_sources_preserve_distinct_fields() {
    let mut bytes = header(10, 1, 0, 2);
    for value in [0x102_u64, 29, 11] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice("λab".as_bytes());
    source(&mut bytes, 17, "α".as_bytes(), &[1, 3, 5]);
    source(&mut bytes, 23, b"second", &[2, 4]);
    let bytes = finish(bytes);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.capture_duration_nanos, 0x0807_0605_0403_0201);
    assert_eq!((decoded.events.total_events, decoded.events.lost_events), (29, 11));
    assert_eq!(decoded.events.threads[0].thread_id.get(), 0x102);
    assert_eq!(decoded.events.threads[0].name, "λab");
    assert_eq!(decoded.sources[0].id.get(), 17);
    assert_eq!(decoded.sources[0].schema_version, 0x201);
    assert_eq!(decoded.sources[0].name, "α");
    assert_eq!(decoded.sources[0].data, [1, 3, 5]);
    assert_eq!((decoded.sources[1].id.get(), decoded.sources[1].data.as_slice()), (23, &[2, 4][..]));
    assert_eq!(decoded.events.clock, EventClock::ProcessMonotonic);
    let policies = decoded.events.recording;
    assert_eq!(policies.allocations.event_sampling.get(), 3);
    assert!(!policies.runtime_tasks.enabled);
    assert!(!policies.allocations.capture_backtraces);
    assert!(policies.io.enabled);
    assert!(!policies.cache.enabled);
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end]).is_err(), "prefix {end}");
    }
}

#[test]
fn gamma_survivor_source_debug_has_exact_type_label() {
    assert_eq!(
        format!("{:?}", SourceData::copy_from(&[1, 2, 3]).unwrap()),
        "SourceData { bytes: 3, .. }"
    );
    assert_eq!(SourceId::from_raw(0), None);
    assert_eq!(SourceId::from_raw(u64::MAX).unwrap().get(), u64::MAX);
}

#[test]
fn gamma_survivor_io_identifiers_advance_by_one() {
    use seismograph::recorder::io::IoOperationId;
    let first = IoOperationId::allocate().get();
    assert_eq!(IoOperationId::allocate().get(), first + 1);
}

#[test]
fn gamma_survivor_minimum_thread_and_source_records_are_admitted() {
    let mut thread = header(10, 1, 0, 0);
    for value in [13_u64, 17, 19] {
        thread.extend_from_slice(&value.to_le_bytes());
    }
    thread.extend_from_slice(&0_u16.to_le_bytes());
    assert_eq!(decode(&finish(thread.clone())).unwrap().events.threads.len(), 1);
    let name_len = thread.len() - 2;
    thread[name_len..].copy_from_slice(&1_u16.to_le_bytes());
    thread.push(255);
    decode(&finish(thread)).unwrap_err();
    let mut bytes = header(10, 0, 0, 1);
    source(&mut bytes, 13, b"", b"");
    assert_eq!(decode(&finish(bytes)).unwrap().sources.len(), 1);
}

#[test]
fn gamma_survivor_variable_sections_reach_each_later_truncated_record() {
    let mut sources = header(10, 0, 0, 2);
    source(&mut sources, 17, &[b'a'; 64], &[1; 64]);
    let source_end = sources.len();
    source(&mut sources, 19, b"last", &[2; 7]);
    let mut threads = header(10, 2, 0, 0);
    for value in [13_u64, 17, 19] {
        threads.extend_from_slice(&value.to_le_bytes());
    }
    threads.extend_from_slice(&128_u16.to_le_bytes());
    threads.extend_from_slice(&[b'a'; 128]);
    let thread_end = threads.len();
    for value in [23_u64, 29, 31] {
        threads.extend_from_slice(&value.to_le_bytes());
    }
    threads.extend_from_slice(&0_u16.to_le_bytes());
    let mut events = header(10, 0, 2, 0);
    let mut first = event(4, EventKind::TaskSpawned, [13, 17, 19, 23, 29, 31, 0, 0]);
    first[118] = 24;
    let symbols = first.len() - 4;
    first.splice(symbols..symbols, [0; 22 * 8]);
    events.extend_from_slice(&first[92..first.len() - 4]);
    let event_end = events.len();
    let second = event(4, EventKind::TaskSpawned, [13, 17, 19, 23, 29, 31, 0, 0]);
    events.extend_from_slice(&second[92..second.len() - 4]);
    for (bytes, start) in [(sources, source_end), (threads, thread_end), (events, event_end)] {
        let bytes = finish(bytes);
        decode(&bytes).unwrap();
        for end in start..bytes.len() {
            assert!(decode(&bytes[..end]).is_err(), "later record prefix {end}");
        }
    }
}

#[test]
fn gamma_survivor_payloads_decode_every_nonzero_field() {
    let bytes = event(4, EventKind::TaskSpawned, [13, 17, 19, 23, 29, 31, 0, 0]);
    let decoded = decode(&bytes).unwrap();
    let value = &decoded.events.events[0];
    assert_eq!(
        (value.thread_id.get(), value.sequence.get(), value.timestamp),
        (0x102, 0x304, EventTimestamp::from_ticks(0x506))
    );
    assert_eq!(value.call_stack.iter().map(|frame| frame.get()).collect::<Vec<_>>(), [0x708, 0x90a]);
    let payload = value.runtime().unwrap();
    assert_eq!((payload.runtime_id.get(), payload.worker_id.unwrap().get()), (13, 17));
    assert_eq!(
        (payload.subject_id, payload.related_id, payload.value_0, payload.value_1),
        (19, 23, 29, 31)
    );
    let bytes = event(
        5,
        EventKind::IoReadFinished,
        [13, 17, 19, 23, 29, 0, 31, 0x25 | (1 << 32) | (2 << 40)],
    );
    let decoded = decode(&bytes).unwrap();
    let payload = decoded.events.events[0].io().unwrap();
    assert_eq!(
        (
            payload.operation_id.get(),
            payload.resource_id.get(),
            payload.buffer_id.unwrap().get()
        ),
        (13, 17, 19)
    );
    assert_eq!(
        (
            payload.requested_bytes,
            payload.completed_bytes,
            payload.buffer_len,
            payload.buffer_span_count
        ),
        (23, 29, 31, 37)
    );
    assert_eq!(payload.resource_kind, IoResourceKind::File);
    assert_eq!(payload.outcome, IoOutcome::Success);
    let bytes = event(2, EventKind::ChannelHighWatermark, [13, 17, 0, 0, 0, 0, 0, 0]);
    assert!(
        matches!(decode(&bytes).unwrap().events.events[0].payload, EventPayload::Numeric(value) if value.object_id.get() == 13 && value.value == 17)
    );
    let cache = event(2, EventKind::CacheHit, [13, 17, 0, 0, 0, 0, 0, 0]);
    assert!(matches!(decode(&cache).unwrap().events.events[0].payload, EventPayload::Numeric(value) if value.value == 17));
}

#[test]
fn gamma_survivor_validation_changes_one_valid_field_at_a_time() {
    let valid = event(4, EventKind::TaskSpawned, [13, 17, 19, 23, 29, 31, 0, 0]);
    for (offset, value) in [
        (0, 0),
        (8, 0),
        (8, 9),
        (10, 1),
        (32, 2),
        (33, 2),
        (34, 1),
        (36, 0),
        (80, 0),
        (82, 1),
        (85, 0),
        (116, 255),
        (117, 9),
        (118, 33),
        (119, 1),
        (168, 1),
        (176, 1),
    ] {
        let mut bytes = valid.clone();
        bytes[offset] = value;
        assert!(decode(&bytes).is_err(), "offset {offset}");
    }
    for offset in [20, 24, 28] {
        let mut bytes = valid.clone();
        bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        decode(&bytes).unwrap_err();
        for bit in [0, 8, 16, 24] {
            let mut empty = finish(header(10, 0, 0, 0));
            empty[offset..offset + 4].copy_from_slice(&(1_u32 << bit).to_le_bytes());
            decode(&empty).unwrap_err();
        }
    }
    let mut bytes = header(10, 0, 0, 1);
    source(&mut bytes, 17, b"ok", &[1]);
    let mut bytes = finish(bytes);
    for offset in [92, 112, 116] {
        let mut invalid = bytes.clone();
        invalid[offset] = if offset == 92 { 0 } else { 255 };
        decode(&invalid).unwrap_err();
    }
    let mut duplicate = header(10, 0, 0, 2);
    source(&mut duplicate, 17, b"a", &[1]);
    source(&mut duplicate, 17, b"b", &[2]);
    decode(&finish(duplicate)).unwrap_err();
    bytes.push(0);
    decode(&bytes).unwrap_err();
    let io = event(
        5,
        EventKind::IoReadFinished,
        [13, 17, 19, 23, 29, 0, 31, 0x25 | (1 << 32) | (2 << 40)],
    );
    for offset in [120, 128, 180, 181] {
        let mut invalid = io.clone();
        invalid[offset] = 0;
        assert!(decode(&invalid).is_err(), "I/O identity or enum offset {offset}");
    }
}
