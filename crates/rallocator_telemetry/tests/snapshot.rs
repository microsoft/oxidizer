// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration and migration tests for telemetry snapshot encoding.
#![expect(
    clippy::cast_possible_truncation,
    clippy::panic,
    clippy::unwrap_used,
    reason = "Explicit wire fixtures use bounded test values and should fail immediately when malformed"
)]

use rallocator_telemetry::callers::{AddressLookup, Callers, Event, EventKind, HeapKind, ThreadLog, ThreadName};
use rallocator_telemetry::snapshot::{Domain, Estimate, Region, SizeClass, SkippedSection, Snapshot, Stats, Version};
use rallocator_telemetry::topology::{Segment, Slice, SliceKind, TopologyRegion};
use rallocator_telemetry::{decode, encode, encoded_len};
use rallocator_wire::format::{Header, Section};
use rallocator_wire::io::Writer;
use rallocator_wire::{Decode, Encode};

const SECTION_METADATA: u16 = 1;
const SECTION_STATS: u16 = 2;
const SECTION_SIZE_CLASSES: u16 = 3;
const SECTION_REGIONS: u16 = 4;
const SECTION_CALLERS: u16 = 5;
const SECTION_ADDRESSES: u16 = 6;
const SECTION_TOPOLOGY: u16 = 7;
const SECTION_DOMAINS: u16 = 8;

fn fixture() -> Snapshot {
    let mut snapshot = Snapshot::new(Version::new(0, 1, 0));
    snapshot.metadata.capture_duration_nanos = 42;
    snapshot.stats = Stats::new(0, 0, 123, 0, 4096, 0, 0, 7, 0, 2, 0, 0, 0);
    snapshot.size_classes.push(SizeClass::new(
        3,
        64,
        Estimate::new(2, 1, 3),
        Estimate::new(96, 80, 112),
        Estimate::new(128, 64, 192),
    ));
    snapshot.regions.push(Region::new(0, 1 << 30, 8, 16_376));
    snapshot
        .domains
        .push(Domain::new(1, true, 1, 1 << 30, 8, 16_376, 1, 0, 1, 6, vec![0]));
    snapshot.topology.push(TopologyRegion::new(
        0,
        0x4000_0000,
        64 * (64 << 10),
        64 << 10,
        vec![0b1_1111],
        vec![
            Slice::new(0, SliceKind::Small, 0, 0x1234, 0, 0, vec![Segment::new(0, 1, false, 7, 511, true)]),
            Slice::new(1, SliceKind::Bump, 1, 0x5678, 0, 0, Vec::new()),
            Slice::new(2, SliceKind::Unknown, 1, 0, 0, 0, Vec::new()),
            Slice::new(3, SliceKind::Medium, 2, 0x9ABC, 32 << 10, 64 << 10, Vec::new()),
            Slice::new(4, SliceKind::MediumContinuation, 0, 0x9ABC, 0, 0, Vec::new()),
        ],
    ));
    snapshot.callers = Some(Callers::new(
        9,
        1,
        0,
        vec![ThreadLog::new(1, 1, 0, vec![0, 1], vec![0, 0])],
        vec![
            Event::new(
                1,
                1,
                1,
                4,
                EventKind::Allocated,
                7,
                HeapKind::General,
                false,
                0x1234,
                64,
                8,
                vec![0xAAAA, 0xBBBB],
            ),
            Event::new(
                1,
                2,
                2,
                4,
                EventKind::Deallocated,
                7,
                HeapKind::General,
                false,
                0x1234,
                64,
                8,
                Vec::new(),
            ),
        ],
        vec![
            ThreadName::new(1, "allocator".to_owned()),
            ThreadName::new(2, "reclaimer".to_owned()),
        ],
    ));
    snapshot.addresses.push(AddressLookup::new(
        0xAAAA,
        Some("fixture::allocate".to_owned()),
        Some("src/fixture.rs".to_owned()),
        Some(42),
        Some(7),
    ));
    snapshot
}

fn current_schema() -> u16 {
    Snapshot::new(Version::new(0, 1, 0)).metadata.telemetry_schema_version
}

fn encoded(snapshot: &Snapshot) -> Vec<u8> {
    let mut bytes = vec![0; encoded_len(snapshot).unwrap()];
    encode(snapshot, &mut bytes).unwrap();
    bytes
}

fn section(bytes: &[u8], wanted: u16) -> (usize, usize) {
    let mut offset = Header::encoded_len();
    while offset < bytes.len() {
        let id = u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
        let length = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        if id == wanted {
            return (offset, offset + Section::encoded_len(0));
        }
        offset += Section::encoded_len(length);
    }
    panic!("section {wanted} not found");
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn snapshot_round_trips() {
    let expected = fixture();
    let mut bytes = encoded(&expected);
    assert_eq!(encode(&expected, &mut bytes).unwrap(), bytes.len());
    assert_eq!(decode(&bytes).unwrap(), expected);
}

#[test]
fn duplicate_known_sections_are_rejected() {
    let mut bytes = encoded(&fixture());
    let (header, payload) = section(&bytes, SECTION_METADATA);
    let payload_len = u32::from_le_bytes(bytes[header + 4..header + 8].try_into().unwrap()) as usize;
    let duplicate = bytes[header..payload + payload_len].to_vec();
    bytes.extend_from_slice(&duplicate);
    decode(&bytes).unwrap_err();
}

#[test]
fn noncanonical_booleans_are_rejected() {
    let mut bytes = encoded(&fixture());
    let (_, domains) = section(&bytes, SECTION_DOMAINS);
    bytes[domains + 4 + 8] = 2;
    decode(&bytes).unwrap_err();
}

#[test]
fn wire_traits_delegate_to_snapshot_encoding() {
    let expected = fixture();
    let length = Encode::encoded_len(&expected).unwrap();
    let mut bytes = vec![0; length];
    assert_eq!(Encode::encode(&expected, &mut bytes).unwrap(), length);
    assert_eq!(<Snapshot as Decode>::decode(&bytes).unwrap(), expected);
}

#[test]
fn empty_optional_data_round_trips() {
    let mut expected = Snapshot::new(Version::new(1, 2, 3));
    expected.addresses.push(AddressLookup::default());
    assert_eq!(decode(&encoded(&expected)).unwrap(), expected);
}

#[test]
fn encode_requires_exact_output_length() {
    let snapshot = fixture();
    let length = encoded_len(&snapshot).unwrap();
    encode(&snapshot, &mut vec![0; length - 1]).unwrap_err();
    encode(&snapshot, &mut vec![0; length + 1]).unwrap_err();
}

#[test]
fn unknown_sections_are_skipped() {
    let mut expected = fixture();
    expected.skipped_sections.push(SkippedSection::new(999, 1));
    let original_len = encoded_len(&fixture()).unwrap();
    let mut bytes = vec![0; original_len + Section::encoded_len(3)];
    let mut original = vec![0; original_len];
    encode(&fixture(), &mut original).unwrap();
    bytes[..original_len].copy_from_slice(&original);
    let mut writer = Writer::new(&mut bytes[original_len..]);
    writer.begin_section(999, 1, 3).unwrap();
    writer.write_bytes(&[1, 2, 3]).unwrap();
    writer.finish().unwrap();
    assert_eq!(decode(&bytes).unwrap(), expected);
}

#[test]
fn truncated_payload_is_rejected() {
    let expected = fixture();
    let mut bytes = vec![0; encoded_len(&expected).unwrap()];
    encode(&expected, &mut bytes).unwrap();
    bytes.pop();
    decode(&bytes).unwrap_err();
}

#[test]
fn required_sections_must_be_present() {
    let mut bytes = vec![0; Header::encoded_len()];
    Writer::new(&mut bytes)
        .write_header(Header::new(current_schema(), Version::new(0, 1, 0)))
        .unwrap();
    decode(&bytes).unwrap_err();

    let mut bytes = encoded(&fixture());
    let (stats, _) = section(&bytes, SECTION_STATS);
    bytes[stats..stats + 2].copy_from_slice(&999_u16.to_le_bytes());
    decode(&bytes).unwrap_err();
}

#[test]
fn unsupported_schemas_and_section_versions_are_handled() {
    for schema in [0, current_schema() + 1] {
        let mut bytes = encoded(&fixture());
        bytes[10..12].copy_from_slice(&schema.to_le_bytes());
        decode(&bytes).unwrap_err();
    }

    let mut bytes = encoded(&fixture());
    let (topology, _) = section(&bytes, SECTION_TOPOLOGY);
    bytes[topology + 2..topology + 4].copy_from_slice(&3_u16.to_le_bytes());
    let decoded = decode(&bytes).unwrap();
    assert!(decoded.topology.is_empty());
    assert_eq!(decoded.skipped_sections, vec![SkippedSection::new(SECTION_TOPOLOGY, 3)]);

    let mut bytes = encoded(&fixture());
    let (domains, _) = section(&bytes, SECTION_DOMAINS);
    bytes[domains + 2..domains + 4].copy_from_slice(&0_u16.to_le_bytes());
    let decoded = decode(&bytes).unwrap();
    assert!(decoded.domains.is_empty());
    assert_eq!(decoded.skipped_sections, vec![SkippedSection::new(SECTION_DOMAINS, 0)]);

    let mut bytes = encoded(&fixture());
    let (callers, _) = section(&bytes, SECTION_CALLERS);
    bytes[callers + 2..callers + 4].copy_from_slice(&4_u16.to_le_bytes());
    let decoded = decode(&bytes).unwrap();
    assert!(decoded.callers.is_none());
    assert_eq!(decoded.skipped_sections, vec![SkippedSection::new(SECTION_CALLERS, 4)]);
}

#[test]
fn legacy_caller_events_decode_with_compatible_defaults() {
    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    let mut payload = Vec::new();
    payload.push(1);
    push_u64(&mut payload, 9);
    push_u64(&mut payload, 2);
    push_u64(&mut payload, 0);
    push_u32(&mut payload, 1);
    push_u64(&mut payload, 1);
    push_u64(&mut payload, 2);
    push_u64(&mut payload, 0);
    push_u32(&mut payload, 2);
    for (sequence, kind, frames) in [(1_u64, 1_u8, &[0xAAAA_u64][..]), (2, 2, &[][..])] {
        push_u64(&mut payload, 1);
        push_u64(&mut payload, sequence);
        push_u64(&mut payload, 4);
        payload.push(kind);
        push_u64(&mut payload, 0x1234);
        push_u64(&mut payload, 64);
        push_u64(&mut payload, 8);
        push_u32(&mut payload, frames.len() as u32);
        for &frame in frames {
            push_u64(&mut payload, frame);
        }
    }

    let mut bytes = encoded(&fixture());
    let (header, section_payload) = section(&bytes, SECTION_CALLERS);
    let old_length = u32::from_le_bytes(bytes[header + 4..header + 8].try_into().unwrap()) as usize;
    let mut replacement = Vec::with_capacity(Section::encoded_len(payload.len()));
    replacement.extend_from_slice(&SECTION_CALLERS.to_le_bytes());
    replacement.extend_from_slice(&1_u16.to_le_bytes());
    replacement.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    replacement.extend_from_slice(&payload);
    bytes.splice(header..section_payload + old_length, replacement);

    let callers = decode(&bytes).unwrap().callers.unwrap();
    assert!(callers.threads[0].allocated_histogram.is_empty());
    assert!(callers.threads[0].live_histogram.is_empty());
    assert_eq!(callers.events[0].event_thread_id, 1);
    assert_eq!(callers.events[0].heap_id, 0);
    assert_eq!(callers.events[0].heap_kind, HeapKind::General);
    assert!(!callers.events[0].freed_after_heap_release);
    assert_eq!(callers.events[0].call_stack, vec![0xAAAA]);
}

#[test]
fn callers_v2_diagnostics_decode_without_thread_names() {
    let mut expected = fixture();
    expected.callers.as_mut().unwrap().thread_names.clear();
    let mut bytes = encoded(&expected);
    let (header, payload) = section(&bytes, SECTION_CALLERS);
    let old_length = u32::from_le_bytes(bytes[header + 4..header + 8].try_into().unwrap()) as usize;
    bytes[header + 2..header + 4].copy_from_slice(&2_u16.to_le_bytes());
    bytes.drain(payload + old_length - 4..payload + old_length);
    write_u32(&mut bytes, header + 4, (old_length - 4) as u32);

    assert_eq!(decode(&bytes).unwrap().callers, expected.callers);
}

#[test]
fn trailing_known_section_payload_is_rejected() {
    let mut bytes = encoded(&fixture());
    let (metadata, _) = section(&bytes, SECTION_METADATA);
    write_u32(&mut bytes, metadata + 4, 9);
    decode(&bytes).unwrap_err();
}

#[test]
fn malformed_topology_dimensions_and_counts_are_rejected() {
    for (description, relative_offset, value) in [
        ("zero region size", 16, 0_u32),
        ("bitmap word count", 32, 2),
        ("detailed slice count", 44, 4),
        ("slice outside region", 48, u32::MAX),
        ("duplicate detailed slice", 97, 0),
    ] {
        let mut bytes = encoded(&fixture());
        let (_, payload) = section(&bytes, SECTION_TOPOLOGY);
        write_u32(&mut bytes, payload + relative_offset, value);
        assert!(decode(&bytes).is_err(), "{description}");
    }

    let mut bytes = encoded(&fixture());
    let (_, payload) = section(&bytes, SECTION_TOPOLOGY);
    bytes[payload + 16..payload + 24].copy_from_slice(&(2_u64 * (64 << 10)).to_le_bytes());
    assert!(decode(&bytes).is_err(), "bitmap bits beyond the declared region");
}

#[test]
fn malformed_collection_counts_are_rejected() {
    for (section_id, relative_offset) in [
        (SECTION_SIZE_CLASSES, 0),
        (SECTION_REGIONS, 0),
        (SECTION_DOMAINS, 0),
        (SECTION_DOMAINS, 77),
        (SECTION_TOPOLOGY, 0),
        (SECTION_TOPOLOGY, 32),
        (SECTION_TOPOLOGY, 44),
        (SECTION_TOPOLOGY, 81),
        (SECTION_CALLERS, 25),
        (SECTION_CALLERS, 53),
        (SECTION_CALLERS, 73),
        (SECTION_CALLERS, 93),
        (SECTION_CALLERS, 164),
        (SECTION_ADDRESSES, 0),
    ] {
        let mut bytes = encoded(&fixture());
        let (_, payload) = section(&bytes, section_id);
        if section_id == SECTION_TOPOLOGY && relative_offset == 81 {
            bytes[payload + relative_offset] = u8::MAX;
        } else {
            write_u32(&mut bytes, payload + relative_offset, u32::MAX);
        }
        assert!(decode(&bytes).is_err(), "section {section_id}, offset {relative_offset}");
    }
}

#[test]
fn invalid_enum_discriminants_and_address_data_are_rejected() {
    let mut bytes = encoded(&fixture());
    let (_, topology) = section(&bytes, SECTION_TOPOLOGY);
    bytes[topology + 52] = u8::MAX;
    decode(&bytes).unwrap_err();

    let mut bytes = encoded(&fixture());
    let (_, callers) = section(&bytes, SECTION_CALLERS);
    bytes[callers] = 2;
    decode(&bytes).unwrap_err();

    let mut bytes = encoded(&fixture());
    let (_, callers) = section(&bytes, SECTION_CALLERS);
    bytes[callers + 129] = 3;
    decode(&bytes).unwrap_err();

    let mut bytes = encoded(&fixture());
    let (_, callers) = section(&bytes, SECTION_CALLERS);
    bytes[callers + 138] = u8::MAX;
    decode(&bytes).unwrap_err();

    let mut bytes = encoded(&fixture());
    let (_, addresses) = section(&bytes, SECTION_ADDRESSES);
    bytes[addresses + 12] = 0x10;
    decode(&bytes).unwrap_err();

    let mut bytes = encoded(&fixture());
    let (_, addresses) = section(&bytes, SECTION_ADDRESSES);
    bytes[addresses + 17] = 0xFF;
    decode(&bytes).unwrap_err();
}

#[test]
fn legacy_topology_segments_decode_without_utilization() {
    let mut bytes = encoded(&fixture());
    let (topology, payload) = section(&bytes, SECTION_TOPOLOGY);
    bytes[topology + 2..topology + 4].copy_from_slice(&1_u16.to_le_bytes());
    let old_length = u32::from_le_bytes(bytes[topology + 4..topology + 8].try_into().unwrap()) as usize;
    bytes.drain(payload + 88..payload + 97);
    write_u32(&mut bytes, topology + 4, (old_length - 9) as u32);

    let decoded = decode(&bytes).unwrap();
    let segment = decoded.topology[0].slices[0].segments[0];
    assert_eq!(segment.live_blocks, 0);
    assert_eq!(segment.usable_blocks, 0);
    assert!(!segment.utilization_tracked);
}
