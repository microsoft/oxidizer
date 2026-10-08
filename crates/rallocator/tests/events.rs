// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Process-global event bridge regressions; each test holds the recorder lock.

#![expect(
    clippy::multiple_unsafe_ops_per_block,
    reason = "Each raw-allocation scenario keeps its checked pointer lifecycle together"
)]

use std::alloc::{GlobalAlloc, Layout};
use std::sync::Mutex;

use rallocator::Rallocator;
use seismograph::recorder::alloc::Allocation;
use seismograph::recorder::event::EventKind;
use seismograph::recorder::{Configuration, RecordingPolicy, SuppressionGuard};

rallocator::rallocator!();

static LOCK: Mutex<()> = Mutex::new(());

struct Recording;

impl Recording {
    fn start() -> Self {
        seismograph::recorder(Configuration {
            allocations: RecordingPolicy::all(true),
            ..Default::default()
        });
        Self
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        seismograph::recorder(Configuration::default());
    }
}

#[expect(clippy::unwrap_used, reason = "Test capture failures must fail the scenario")]
fn events() -> Vec<(EventKind, Allocation)> {
    let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let _suppression = SuppressionGuard::enter();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    if let Some(source) = decoded
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
    {
        assert_eq!(source.schema_version, 3);
        let allocator = seismograph_rallocator::decode(&source.data).unwrap();
        assert!(allocator.owner_count >= allocator.owners.len() as u64);
        assert!(allocator.global.local_limit_bytes > 0);
        assert!(allocator.owners.iter().all(|owner| {
            owner
                .observation
                .is_none_or(|state| state.classes.iter().all(|class| class.object_bytes > 0))
        }));
    }
    decoded
        .events
        .events
        .iter()
        .filter_map(|event| event.allocation().map(|allocation| (event.kind, allocation)))
        .collect()
}

fn at_address(events: &[(EventKind, Allocation)], pointer: *mut u8) -> Vec<(EventKind, Allocation)> {
    let _suppression = SuppressionGuard::enter();
    events
        .iter()
        .copied()
        .filter(|(_, allocation)| allocation.address.get() == pointer.addr() as u64)
        .collect()
}

#[test]
fn zeroed_allocation_and_free_match_with_backtraces() {
    let _lock = LOCK.lock().unwrap();
    let _recording = Recording::start();
    let layout = Layout::from_size_align(73, 64).unwrap();
    // SAFETY: The layout is nonzero and the allocation is checked and freed once.
    unsafe {
        let pointer = Rallocator.alloc_zeroed(layout);
        assert!(!pointer.is_null());
        assert_eq!(std::slice::from_raw_parts(pointer, 73), &[0; 73]);
        Rallocator.dealloc(pointer, layout);
        let records = at_address(&events(), pointer);
        assert_eq!(records.len(), 2);
        assert_eq!((records[0].0, records[1].0), (EventKind::Allocation, EventKind::Deallocation));
        assert_eq!(records[0].1, records[1].1);
        assert_eq!((records[0].1.size, records[0].1.alignment), (73, 64));
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let _suppression = SuppressionGuard::enter();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let retained = decoded
            .events
            .events
            .iter()
            .filter(|event| {
                event
                    .allocation()
                    .is_some_and(|allocation| allocation.allocation_id == records[0].1.allocation_id)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            retained.iter().map(|event| event.kind).collect::<Vec<_>>(),
            [EventKind::Allocation, EventKind::Deallocation]
        );
        assert!(retained.iter().all(|event| !event.call_stack.is_empty()));
    }
}

#[test]
fn remote_free_after_owner_exit_records_address_and_container_actor() {
    let _lock = LOCK.lock().unwrap();
    let _recording = Recording::start();
    let layout = Layout::from_size_align(17_123, 128).unwrap();
    let address = std::thread::spawn(move || {
        // SAFETY: Valid nonzero layout; ownership of the returned allocation is transferred.
        let pointer = unsafe { Rallocator.alloc(layout) };
        assert!(!pointer.is_null());
        pointer.expose_provenance()
    })
    .join()
    .unwrap();
    let pointer = std::ptr::with_exposed_provenance_mut::<u8>(address);
    // SAFETY: The allocating thread transferred its still-live allocation.
    unsafe { Rallocator.dealloc(pointer, layout) };
    // Retain captures consume exited-thread buffers, so inspect both actors
    // and payloads from the same capture.
    let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    let _suppression = SuppressionGuard::enter();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let matching = decoded
        .events
        .events
        .iter()
        .filter(|event| {
            event
                .allocation()
                .is_some_and(|allocation| allocation.address.get() == address as u64)
        })
        .collect::<Vec<_>>();
    let records = matching
        .iter()
        .map(|event| (event.kind, event.allocation().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].1.allocation_id, records[1].1.allocation_id);
    assert_eq!(records[0].1.allocation_id.get(), address as u64);
    assert!(
        records
            .iter()
            .all(|(_, allocation)| { allocation.heap_id.get() == 0 && allocation.event_thread_id.get() == 0 })
    );
    assert_ne!(matching[0].thread_id, matching[1].thread_id);
    let callers = seismograph_rallocator::events::callers(&decoded.events);
    let projected = callers
        .events
        .iter()
        .filter(|event| event.address == address as u64)
        .collect::<Vec<_>>();
    assert_eq!(projected.len(), 2);
    assert_eq!(projected[0].allocation_id, projected[1].allocation_id);
    assert!(projected.iter().all(|event| event.allocation_recorded));
    let allocating_thread = matching
        .iter()
        .find(|event| event.kind == EventKind::Allocation)
        .unwrap()
        .thread_id
        .get();
    assert!(projected.iter().all(|event| event.thread_log_id == allocating_thread));
    assert!(
        !records[1].1.freed_after_heap_release,
        "general owners do not masquerade as released bump heaps"
    );
}

#[test]
fn recording_bridge_preserves_relaxed_free_only_pointer_handoffs() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let _lock = LOCK.lock().unwrap();
    let _recording = Recording::start();
    let mailbox = AtomicUsize::new(0);
    let layout = Layout::from_size_align(509, 16).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..64 {
                let address = loop {
                    let address = mailbox.swap(0, Ordering::Relaxed);
                    if address != 0 {
                        break address;
                    }
                    std::hint::spin_loop();
                };
                // SAFETY: The producer transferred the live allocation for
                // free-only ownership; no application payload is accessed.
                unsafe { Rallocator.dealloc(std::ptr::with_exposed_provenance_mut(address), layout) };
            }
        });
        for _ in 0..64 {
            // SAFETY: The valid allocation transfers once through the mailbox.
            let pointer = unsafe { Rallocator.alloc(layout) };
            assert!(!pointer.is_null());
            while mailbox.load(Ordering::Relaxed) != 0 {
                std::hint::spin_loop();
            }
            mailbox.store(pointer.expose_provenance(), Ordering::Relaxed);
        }
    });
    let _suppression = SuppressionGuard::enter();
    let mut identities = std::collections::HashMap::<_, Vec<_>>::new();
    for (kind, allocation) in events()
        .into_iter()
        .filter(|(_, allocation)| allocation.size == 509 && allocation.alignment == 16)
    {
        identities.entry(allocation.allocation_id).or_default().push(kind);
    }
    assert_eq!(identities.values().map(Vec::len).sum::<usize>(), 128);
    assert!(identities.values().all(|kinds| {
        kinds.iter().filter(|kind| **kind == EventKind::Allocation).count()
            == kinds.iter().filter(|kind| **kind == EventKind::Deallocation).count()
    }));
}

#[test]
fn recording_restart_emits_frees_of_paused_and_suppressed_allocations() {
    let _lock = LOCK.lock().unwrap();
    let recording = Recording::start();
    let layout = Layout::from_size_align(109, 16).unwrap();
    // SAFETY: All pointers are checked and freed once with their original layout.
    unsafe {
        let tracked = Rallocator.alloc(layout);
        assert!(!tracked.is_null());
        let first = at_address(&events(), tracked);
        assert_eq!(first.len(), 1);
        drop(recording);
        let paused = Rallocator.alloc(layout);
        assert!(!paused.is_null());
        let _recording = Recording::start();
        let suppressed = {
            let _suppression = SuppressionGuard::enter();
            Rallocator.alloc(layout)
        };
        assert!(!suppressed.is_null());
        Rallocator.dealloc(tracked, layout);
        Rallocator.dealloc(paused, layout);
        Rallocator.dealloc(suppressed, layout);
        let second = events();
        let freed = at_address(&second, tracked);
        assert_eq!(freed.len(), 1);
        assert_eq!(freed[0].0, EventKind::Deallocation);
        assert_eq!(first[0].1.allocation_id, freed[0].1.allocation_id);
        for pointer in [paused, suppressed] {
            let records = at_address(&second, pointer);
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].0, EventKind::Deallocation);
            assert_eq!(records[0].1.allocation_id.get(), pointer.addr() as u64);
            assert_eq!((records[0].1.size, records[0].1.alignment), (109, 16));
        }
    }
}

#[test]
fn stopped_frees_emit_nothing_and_restart_uses_only_current_operation_data() {
    let _lock = LOCK.lock().unwrap();
    let recording = Recording::start();
    let layout = Layout::from_size_align(391, 16).unwrap();
    // SAFETY: Requests are nonzero and each allocation is freed exactly once.
    unsafe {
        let original = Rallocator.alloc(layout);
        assert!(!original.is_null());
        let first = at_address(&events(), original);
        assert_eq!(first.len(), 1);
        drop(recording);
        Rallocator.dealloc(original, layout);
        let paused = Rallocator.alloc(layout);
        assert!(!paused.is_null());
        let _recording = Recording::start();
        Rallocator.dealloc(paused, layout);
        let current = Rallocator.alloc(layout);
        assert!(!current.is_null());
        let before_free = events();
        let records = at_address(&before_free, current);
        assert_eq!(records.iter().filter(|(kind, _)| *kind == EventKind::Allocation).count(), 1);
        assert_eq!(
            records.iter().filter(|(kind, _)| *kind == EventKind::Deallocation).count(),
            usize::from(paused == current)
        );
        assert!(
            records
                .iter()
                .all(|(_, allocation)| allocation.allocation_id.get() == current.addr() as u64)
        );
        Rallocator.dealloc(current, layout);
        assert_eq!(at_address(&events(), current).len(), records.len() + 1);
    }
}

#[test]
fn realloc_preserves_native_pointer_rules_and_records_successful_layout_changes() {
    let _lock = LOCK.lock().unwrap();
    let _recording = Recording::start();
    let layout = Layout::from_size_align(17, 8).unwrap();
    // SAFETY: Realloc sizes fit isize and are nonzero; copies and frees use live layouts.
    unsafe {
        let original = Rallocator.alloc(layout);
        assert!(!original.is_null());
        original.write_bytes(42, 17);
        let replacement = Rallocator.realloc(original, layout, 18);
        assert!(!replacement.is_null());
        assert_eq!(replacement, original);
        assert_eq!(std::slice::from_raw_parts(replacement, 17), &[42; 17]);
        let grown_layout = Layout::from_size_align(18, 8).unwrap();
        let failed = Rallocator.realloc(replacement, grown_layout, (1_usize << 46) + 1);
        assert!(failed.is_null());
        assert_eq!(std::slice::from_raw_parts(replacement, 17), &[42; 17]);
        let before_free = events();
        let records = at_address(&before_free, original);
        assert_eq!(records.len(), 3);
        assert_eq!(
            records
                .iter()
                .map(|(kind, allocation)| (*kind, allocation.size))
                .collect::<Vec<_>>(),
            vec![
                (EventKind::Allocation, 17),
                (EventKind::Deallocation, 17),
                (EventKind::Allocation, 18)
            ]
        );
        let moved = Rallocator.realloc(replacement, grown_layout, 4097);
        assert!(!moved.is_null());
        assert_ne!(moved, original);
        assert_eq!(std::slice::from_raw_parts(moved, 17), &[42; 17]);
        Rallocator.dealloc(moved, Layout::from_size_align(4097, 8).unwrap());
        let final_events = events();
        let old_records = at_address(&final_events, original);
        assert_eq!(old_records.len(), 4);
        assert_eq!((old_records[3].0, old_records[3].1.size), (EventKind::Deallocation, 18));
        let new_records = at_address(&final_events, moved);
        assert_eq!(new_records.len(), 2);
        assert_eq!(new_records[0].1, new_records[1].1);
        assert_eq!(new_records[0].1.size, 4097);
    }
}

#[test]
fn disabled_and_failed_operations_emit_nothing_but_enabled_resizes_and_frees_do() {
    let _lock = LOCK.lock().unwrap();
    seismograph::recorder(Configuration::default());
    let layout = Layout::from_size_align(17, 8).unwrap();
    // SAFETY: Nonzero valid requests; failed allocations are never freed.
    unsafe {
        let pointer = Rallocator.alloc(layout);
        assert!(!pointer.is_null());
        let _recording = Recording::start();
        assert_eq!(Rallocator.realloc(pointer, layout, 17), pointer);
        assert!(at_address(&events(), pointer).is_empty());
        let resized = Rallocator.realloc(pointer, layout, 18);
        assert_eq!(resized, pointer);
        Rallocator.dealloc(resized, Layout::from_size_align(18, 8).unwrap());
        let too_large = Layout::from_size_align((1_usize << 46) + 1, 8).unwrap();
        assert!(Rallocator.alloc(too_large).is_null());
        let records = at_address(&events(), pointer);
        assert_eq!(records.len(), 3);
        assert_eq!(
            records
                .iter()
                .map(|(kind, allocation)| (*kind, allocation.size))
                .collect::<Vec<_>>(),
            vec![
                (EventKind::Deallocation, 17),
                (EventKind::Allocation, 18),
                (EventKind::Deallocation, 18)
            ]
        );
    }
}

#[test]
fn snapshot_source_allocations_do_not_recurse_and_owned_values_outlive_capture() {
    static POINTER: std::sync::atomic::AtomicPtr<u8> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
    static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
        seismograph::snapshot::SourceId::new(0x1234_5678),
        "allocator-recursion-test",
        1,
        |_context| {
            if !ACTIVE.load(std::sync::atomic::Ordering::Acquire) {
                return seismograph::snapshot::SourceData::copy_from(b"inactive");
            }
            let value = Box::new([19_u8; 397]);
            let pointer = Box::into_raw(value).cast::<u8>();
            POINTER.store(pointer, std::sync::atomic::Ordering::Release);
            seismograph::snapshot::SourceData::copy_from(b"source")
        },
    );
    let _lock = LOCK.lock().unwrap();
    let _recording = Recording::start();
    seismograph::snapshot::register_source(&SOURCE);
    ACTIVE.store(true, std::sync::atomic::Ordering::Release);
    let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
    ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    let pointer = POINTER.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::AcqRel);
    assert!(!pointer.is_null());
    let address = pointer.expose_provenance();
    std::thread::spawn(move || {
        // SAFETY: The source transferred its uniquely owned Box; storage outlives capture.
        let value = unsafe { Box::from_raw(std::ptr::with_exposed_provenance_mut::<[u8; 397]>(address)) };
        assert_eq!(*value, [19; 397]);
        drop(value);
    })
    .join()
    .unwrap();
    let _suppression = SuppressionGuard::enter();
    let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    assert!(decoded.events.events.iter().all(|event| {
        event
            .allocation()
            .is_none_or(|allocation| allocation.address.get() != address as u64)
    }));
    let freed = at_address(&events(), pointer);
    assert_eq!(freed.len(), 1);
    assert_eq!(freed[0].0, EventKind::Deallocation);
}
