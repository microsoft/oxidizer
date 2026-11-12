// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! First-class successful resize recording and current-format validation.

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind, Reallocation};
use seismograph::recorder::event::{Address, EventClass, EventKind, Record};
use seismograph::recorder::{Configuration, EventBufferCapacity, EventSampling, Policy};
use seismograph::snapshot::{DecodedSnapshot, EventBufferDisposition, SnapshotOptions};

#[test]
fn successful_resizes_keep_one_record_with_both_storage_endpoints() {
    seismograph::recorder(Configuration {
        allocations: Policy::all(false),
        event_capacity_per_thread: EventBufferCapacity::new(64),
        ..Configuration::default()
    });
    let allocation = Allocation {
        allocation_id: AllocationId::new(0x1000),
        event_thread_id: EventThreadId::new(0),
        heap_id: HeapId::new(3),
        heap_kind: HeapKind::General,
        freed_after_heap_release: false,
        address: Address::new(0x1000),
        size: 64,
        alignment: 16,
    };
    let in_place = Reallocation {
        allocation_id: allocation.allocation_id,
        heap_id: allocation.heap_id,
        heap_kind: HeapKind::General,
        old_address: allocation.address,
        new_address: allocation.address,
        old_size: 64,
        new_size: 96,
        alignment: 16,
    };
    let moved = Reallocation {
        old_size: 96,
        new_size: 4096,
        new_address: Address::new(0x2000),
        ..in_place
    };
    seismograph::record(EventClass::Allocation, || Some(Record::allocation(allocation)));
    seismograph::record(EventClass::Allocation, || Some(Record::reallocation(in_place)));
    seismograph::record(EventClass::Allocation, || Some(Record::reallocation(moved)));
    seismograph::record(EventClass::Allocation, || {
        Some(Record::deallocation(Allocation {
            allocation_id: AllocationId::new(0x2000),
            address: moved.new_address,
            size: moved.new_size,
            ..allocation
        }))
    });

    let capture = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    let decoded = seismograph::snapshot::decode(capture.as_bytes()).unwrap();

    assert_eq!(
        decoded.events.events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            EventKind::Allocation,
            EventKind::Reallocation,
            EventKind::Reallocation,
            EventKind::Deallocation,
        ]
    );
    assert_eq!(
        decoded
            .events
            .events
            .iter()
            .filter_map(seismograph::recorder::event::Event::reallocation)
            .collect::<Vec<_>>(),
        [in_place, moved]
    );
    assert_eq!(EventKind::Reallocation.class(), EventClass::Allocation);
    assert_eq!(decoded.events.events[1].object_id().unwrap().get(), allocation.allocation_id.get());
    assert!(decoded.events.events[1].allocation().is_none());
    assert!(decoded.events.events[0].reallocation().is_none());
    assert_malformed_resize_rejected(capture.as_bytes(), &decoded);
    let mut incorrectly_versioned = capture.as_bytes().to_vec();
    incorrectly_versioned[8..10].copy_from_slice(&9_u16.to_le_bytes());
    assert_invalid_format(&incorrectly_versioned);
    assert_stable_identity_sampling(allocation, moved);
}

#[expect(clippy::unwrap_used, reason = "The valid-payload control must decode successfully")]
fn assert_malformed_resize_rejected(bytes: &[u8], decoded: &DecodedSnapshot) {
    // Format 10 uses a 92-byte header, named thread rows, and 92-byte event rows.
    let threads_len: usize = decoded.events.threads.iter().map(|thread| 26 + thread.name.len()).sum();
    let allocation_start = 92 + threads_len;
    let resize_start = allocation_start + 92;
    for (offset, replacement) in [(24, 14), (27, 1)] {
        let mut malformed = bytes.to_vec();
        malformed[resize_start + offset] = replacement;
        assert_invalid_format(&malformed);
    }
    let mut mismatched = bytes.to_vec();
    mismatched[resize_start + 25] = 3;
    mismatched.copy_within(allocation_start + 28..allocation_start + 92, resize_start + 28);
    assert_invalid_format(&mismatched);
    mismatched[resize_start + 24] = bytes[allocation_start + 24];
    let valid = seismograph::snapshot::decode(&mismatched).unwrap();
    assert_eq!(valid.events.events[1].allocation(), decoded.events.events[0].allocation());
    for invalid_heap_kind in [255_u64, 256, u64::MAX] {
        let mut malformed = bytes.to_vec();
        malformed[resize_start + 84..resize_start + 92].copy_from_slice(&invalid_heap_kind.to_le_bytes());
        assert_invalid_format(&malformed);
    }
}

#[expect(clippy::unwrap_used, reason = "Malformed fixtures must fail decoding")]
fn assert_invalid_format(bytes: &[u8]) {
    let error = seismograph::snapshot::decode(bytes).unwrap_err();
    assert_eq!(error.to_string(), "the seismograph snapshot is malformed or unsupported");
}

#[expect(clippy::unwrap_used, reason = "Test configuration and captures must be valid")]
fn assert_stable_identity_sampling(allocation: Allocation, resize: Reallocation) {
    seismograph::recorder(Configuration {
        allocations: Policy {
            event_sampling: EventSampling::one_in(2).unwrap(),
            ..Policy::all(false)
        },
        event_capacity_per_thread: EventBufferCapacity::new(if cfg!(miri) { 64 } else { 512 }),
        ..Configuration::default()
    });
    let population = if cfg!(miri) { 16 } else { 128 };
    for id in 1..=population {
        let allocation_id = AllocationId::new(id);
        let allocation = Allocation {
            allocation_id,
            address: Address::new(0x1000 + id * 64),
            ..allocation
        };
        let resize = Reallocation {
            allocation_id,
            old_address: allocation.address,
            new_address: Address::new(0x0020_0000 + id * 4096),
            old_size: allocation.size,
            ..resize
        };
        seismograph::record(EventClass::Allocation, || Some(Record::allocation(allocation)));
        seismograph::record(EventClass::Allocation, || Some(Record::reallocation(resize)));
        seismograph::record(EventClass::Allocation, || {
            Some(Record::deallocation(Allocation {
                address: resize.new_address,
                size: resize.new_size,
                ..allocation
            }))
        });
    }
    let capture = seismograph::snapshot(SnapshotOptions {
        event_buffers: EventBufferDisposition::Stop,
    })
    .unwrap();
    let events = seismograph::snapshot::decode(capture.as_bytes()).unwrap().events.events;
    assert!(!events.is_empty());
    for id in 1..=population {
        let kinds = events
            .iter()
            .filter(|event| event.object_id().unwrap().get() == id)
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert!(
            kinds.is_empty() || kinds == [EventKind::Allocation, EventKind::Reallocation, EventKind::Deallocation],
            "sampling must use the allocation ID for each retained lifecycle operation"
        );
    }
}
