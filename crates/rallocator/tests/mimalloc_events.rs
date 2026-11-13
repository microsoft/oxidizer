// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Allocation-event contracts for the Rust mimalloc backend.

use std::alloc::{GlobalAlloc, Layout};

use seismograph::recorder::event::{Event, EventKind};
use seismograph::recorder::{Configuration, Policy, without_recording};

rallocator::rallocator!(backend = mimalloc);

#[test]
fn moves_retained_resizes_and_failed_resizes_record_matching_lifetimes() {
    seismograph::recorder(Configuration {
        allocations: Policy::all(false),
        ..Default::default()
    });
    let layout = Layout::from_size_align(17, 8).unwrap();
    let too_large = Layout::from_size_align(1_usize << 62, layout.align()).unwrap();
    // SAFETY: The valid nonzero layout exceeds every supported address space.
    assert!(unsafe { GLOBAL.alloc(too_large) }.is_null());
    // SAFETY: The zeroed request uses the same valid nonzero layout.
    assert!(unsafe { GLOBAL.alloc_zeroed(too_large) }.is_null());
    // SAFETY: The layout is valid and nonzero.
    let original = unsafe { GLOBAL.alloc(layout) };
    assert!(!original.is_null());
    // SAFETY: This live allocation has room for all requested bytes.
    unsafe { original.write_bytes(0x5c, layout.size()) };

    // SAFETY: An unchanged valid resize keeps the same live allocation.
    assert_eq!(unsafe { GLOBAL.realloc(original, layout, layout.size()) }, original);

    // SAFETY: The original remains live for its exact layout.
    let retained = unsafe { GLOBAL.realloc(original, layout, 18) };
    assert_eq!(retained, original);
    let retained_layout = Layout::from_size_align(18, 8).unwrap();
    // SAFETY: The live block and nonzero new size satisfy the resize contract.
    let failure = unsafe { GLOBAL.realloc(retained, retained_layout, too_large.size()) };
    assert!(failure.is_null());
    // SAFETY: The failed resize leaves the initialized prefix accessible.
    assert_eq!(unsafe { retained.read() }, 0x5c);
    // SAFETY: The retained allocation is live and the requested size is valid.
    let moved = unsafe { GLOBAL.realloc(retained, retained_layout, 65537) };
    assert!(!moved.is_null());
    assert_ne!(moved, retained);
    // SAFETY: Successful resizing preserves the previously initialized prefix.
    assert_eq!(unsafe { moved.read() }, 0x5c);
    // SAFETY: The moved allocation is retired once with its current layout.
    unsafe { GLOBAL.dealloc(moved, Layout::from_size_align(65537, 8).unwrap()) };

    let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    without_recording(|| {
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        assert!(
            decoded
                .events
                .events
                .iter()
                .filter_map(Event::allocation)
                .all(|allocation| allocation.address.get() != 0)
        );
        let actual = decoded
            .events
            .events
            .iter()
            .filter_map(|event| {
                if let Some(resize) = event.reallocation() {
                    return Some((
                        event.kind,
                        resize.old_address.get(),
                        resize.new_address.get(),
                        resize.old_size,
                        resize.new_size,
                        resize.alignment,
                    ));
                }
                event
                    .allocation()
                    .filter(|allocation| {
                        allocation.address.get() == original.addr() as u64 || allocation.address.get() == moved.addr() as u64
                    })
                    .map(|allocation| {
                        (
                            event.kind,
                            allocation.address.get(),
                            allocation.address.get(),
                            allocation.size,
                            allocation.size,
                            allocation.alignment,
                        )
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                (EventKind::Allocation, original.addr() as u64, original.addr() as u64, 17, 17, 8),
                (EventKind::Reallocation, original.addr() as u64, original.addr() as u64, 17, 18, 8),
                (EventKind::Reallocation, original.addr() as u64, moved.addr() as u64, 18, 65537, 8),
                (EventKind::Deallocation, moved.addr() as u64, moved.addr() as u64, 65537, 65537, 8),
            ]
        );
        let projected = seismograph_rallocator::events::callers(&decoded.events);
        let lifetime = projected
            .events
            .iter()
            .find(|event| event.address == original.addr() as u64)
            .unwrap();
        assert_eq!(
            projected
                .events
                .iter()
                .filter(|event| event.allocation_id == lifetime.allocation_id)
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            [
                seismograph_rallocator::callers::EventKind::Allocated,
                seismograph_rallocator::callers::EventKind::Reallocated,
                seismograph_rallocator::callers::EventKind::Reallocated,
                seismograph_rallocator::callers::EventKind::Deallocated,
            ]
        );
    });
}
