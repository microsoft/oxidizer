// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Deterministic snapshot decoding and metadata regressions.

use seismograph::recorder::alloc::HeapKind;
use seismograph::recorder::event::{EventClock, EventKind, EventPayload, EventTimestamp};
use seismograph::recorder::io::{IoOutcome, IoResourceKind};
use seismograph::snapshot::{SourceData, SourceId, decode};

#[test]
fn nov6_source_extent_accepts_the_exact_remaining_payload_and_returns_errors_for_larger_lengths() {
    let mut bytes = header(8, 0, 0, 1);
    let length_offset = bytes.len() + 12;
    source(&mut bytes, 17, b"source", b"\x12\x34\x56");
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
    let mut bytes = header(8, 0, 0, 2);
    source(&mut bytes, 17, &[b'a'; 64], &[0x12; 64]);
    let second = bytes.len();
    source(&mut bytes, 19, b"second", &[]);
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
fn gamma_round2_version_zero_and_legacy_unknown_kinds_are_errors_not_panics() {
    let bytes = header(0, 0, 0, 0);
    assert_eq!(
        decode(&bytes).unwrap_err().to_string(),
        "the seismograph snapshot is malformed or unsupported"
    );
    let mut bytes = header(1, 0, 1, 0);
    for value in [13_u64, 17, 19] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&[255, 0, 0, 0]);
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
    if version >= 5 {
        let count = usize::from(version.saturating_sub(2).min(6));
        for index in 0..count {
            bytes.extend_from_slice(&[u8::from(index % 2 == 0), u8::from(index % 2 != 0), 0, 0]);
            bytes.extend_from_slice(&(u32::try_from(index).unwrap() + 3).to_le_bytes());
        }
    } else if version >= 3 {
        bytes.extend_from_slice(&7_u32.to_le_bytes());
    }
    if version >= 4 {
        bytes.extend_from_slice(&[1, 0, 0, 0]);
        bytes.extend_from_slice(&1_000_000_000_u64.to_le_bytes());
    }
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
fn event(tag: u8, kind: EventKind, fields: [u64; 8]) -> Vec<u8> {
    let mut bytes = header(8, 0, 1, 0);
    for value in [0x102, 0x304, 0x506] {
        bytes.extend_from_slice(&u64::to_le_bytes(value));
    }
    bytes.extend_from_slice(&[wire_kind(kind), tag, 2, 0]);
    for value in fields.into_iter().chain([0x708, 0x90a]) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
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
    for version in 1..=8 {
        let mut bytes = header(version, 1, 0, 2);
        for value in [0x102_u64, 29, 11] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        if version >= 2 {
            bytes.extend_from_slice(&4_u16.to_le_bytes());
            bytes.extend_from_slice("λab".as_bytes());
        }
        source(&mut bytes, 17, "α".as_bytes(), &[1, 3, 5]);
        source(&mut bytes, 23, b"second", &[2, 4]);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.capture_duration_nanos, 0x0807_0605_0403_0201);
        assert_eq!((decoded.events.total_events, decoded.events.lost_events), (29, 11));
        assert_eq!(decoded.events.threads[0].thread_id.get(), 0x102);
        assert_eq!(decoded.events.threads[0].name, if version >= 2 { "λab" } else { "" });
        assert_eq!(decoded.sources[0].id.get(), 17);
        assert_eq!(decoded.sources[0].schema_version, 0x201);
        assert_eq!(decoded.sources[0].name, "α");
        assert_eq!(decoded.sources[0].data, [1, 3, 5]);
        assert_eq!((decoded.sources[1].id.get(), decoded.sources[1].data.as_slice()), (23, &[2, 4][..]));
        assert_eq!(
            decoded.events.clock,
            if version >= 4 {
                EventClock::ProcessMonotonic
            } else {
                EventClock::Unspecified
            }
        );
        let policies = decoded.events.recording;
        assert_eq!(
            policies.allocations.event_sampling.get(),
            if version >= 5 {
                3
            } else if version >= 3 {
                7
            } else {
                1
            }
        );
        assert_eq!(policies.runtime_tasks.enabled, version < 5);
        assert!(!policies.allocations.capture_backtraces);
        assert_eq!(policies.io.enabled, version >= 7);
        assert!(!policies.cache.enabled);
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_err(), "version {version}, prefix {end}");
        }
        if matches!(version, 3 | 4) {
            for sampling in [0_u32, u32::MAX] {
                let mut invalid = bytes.clone();
                invalid[32..36].copy_from_slice(&sampling.to_le_bytes());
                decode(&invalid).unwrap_err();
            }
        }
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
    for version in 1..=8 {
        let mut thread = header(version, 1, 0, 0);
        for value in [13_u64, 17, 19] {
            thread.extend_from_slice(&value.to_le_bytes());
        }
        if version >= 2 {
            thread.extend_from_slice(&0_u16.to_le_bytes());
        }
        assert_eq!(decode(&thread).unwrap().events.threads.len(), 1);
        if version >= 2 {
            let name_len = thread.len() - 2;
            thread[name_len..].copy_from_slice(&1_u16.to_le_bytes());
            thread.push(255);
            decode(&thread).unwrap_err();
        }
        let mut bytes = header(version, 0, 0, 1);
        source(&mut bytes, 13, b"", b"");
        assert_eq!(decode(&bytes).unwrap().sources.len(), 1);
    }
}

#[test]
fn gamma_survivor_variable_sections_reach_each_later_truncated_record() {
    let mut sources = header(8, 0, 0, 2);
    source(&mut sources, 17, &[b'a'; 64], &[1; 64]);
    let source_end = sources.len();
    source(&mut sources, 19, b"last", &[2; 7]);
    let mut threads = header(8, 2, 0, 0);
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
    let mut events = header(8, 0, 2, 0);
    let mut first = event(4, EventKind::TaskSpawned, [13, 17, 19, 23, 29, 31, 0, 0]);
    first[118] = 24;
    first.extend_from_slice(&[0; 22 * 8]);
    events.extend_from_slice(&first[92..]);
    let event_end = events.len();
    events.extend_from_slice(&event(4, EventKind::TaskSpawned, [13, 17, 19, 23, 29, 31, 0, 0])[92..]);
    for (bytes, start) in [(sources, source_end), (threads, thread_end), (events, event_end)] {
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
}

#[test]
fn gamma_survivor_legacy_events_and_version_four_payload_boundary() {
    for version in 1..=3 {
        let kind = if version == 1 {
            EventKind::MutexAccess
        } else {
            EventKind::Allocation
        };
        let mut bytes = header(version, 0, 1, 0);
        for value in [13_u64, 17, 19] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[wire_kind(kind), 2, 0, 0]);
        if version >= 2 {
            for value in [23_u64, 29, 31, 37, 64, 258] {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        for value in [41_u64, 43] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        let decoded = decode(&bytes).unwrap();
        let value = &decoded.events.events[0];
        assert_eq!((value.thread_id.get(), value.sequence.get(), value.timestamp.ticks()), (13, 17, 0));
        assert_eq!(value.call_stack.iter().map(|frame| frame.get()).collect::<Vec<_>>(), [41, 43]);
        if version == 1 {
            assert_eq!(value.payload, EventPayload::Object(seismograph::recorder::event::ObjectId::new(19)));
        } else {
            let allocation = value.allocation().unwrap();
            assert_eq!(
                (
                    allocation.allocation_id.get(),
                    allocation.event_thread_id.get(),
                    allocation.heap_id.get()
                ),
                (19, 23, 29)
            );
            assert_eq!((allocation.address.get(), allocation.size, allocation.alignment), (31, 37, 64));
            assert_eq!((allocation.heap_kind, allocation.freed_after_heap_release), (HeapKind::Bump, true));
            let mut invalid = bytes.clone();
            invalid[header(version, 0, 1, 0).len() + 68] = 0;
            decode(&invalid).unwrap_err();
        }
        let header_len = header(version, 0, 1, 0).len();
        let mut records = bytes.clone();
        records[24..28].copy_from_slice(&2_u32.to_le_bytes());
        records[header_len + 25] = 24;
        records.extend_from_slice(&[0; 22 * 8]);
        let second_start = records.len();
        records.extend_from_slice(&bytes[header_len..]);
        decode(&records).unwrap();
        for end in second_start..records.len() {
            decode(&records[..end]).unwrap_err();
        }
        bytes[header_len + 25] = 25;
        bytes.extend_from_slice(&[0; 23 * 8]);
        decode(&bytes).unwrap_err();
    }
    let mut version_four = header(4, 0, 1, 0);
    version_four.extend_from_slice(&event(4, EventKind::TaskSpawned, [13, 17, 19, 23, 29, 31, 0, 0])[92..]);
    assert_eq!(decode(&version_four).unwrap().events.events[0].runtime().unwrap().subject_id, 19);
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
            let mut empty = header(8, 0, 0, 0);
            empty[offset..offset + 4].copy_from_slice(&(1_u32 << bit).to_le_bytes());
            decode(&empty).unwrap_err();
        }
    }
    let mut bytes = header(8, 0, 0, 1);
    source(&mut bytes, 17, b"ok", &[1]);
    for offset in [92, 112, 116] {
        let mut invalid = bytes.clone();
        invalid[offset] = if offset == 92 { 0 } else { 255 };
        decode(&invalid).unwrap_err();
    }
    let mut duplicate = header(8, 0, 0, 2);
    source(&mut duplicate, 17, b"a", &[1]);
    source(&mut duplicate, 17, b"b", &[2]);
    decode(&duplicate).unwrap_err();
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
