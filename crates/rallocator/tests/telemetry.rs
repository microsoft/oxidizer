// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for allocator telemetry.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::undocumented_unsafe_blocks,
    clippy::unwrap_used,
    reason = "Bounded fixtures exercise raw allocator telemetry and stop immediately on capture failures"
)]

use std::alloc::{GlobalAlloc, Layout};
#[cfg(not(miri))]
use std::hint::black_box;
use std::sync::Mutex;

use allocation_hints::heaps::{Heap, bump, thread_heap};
use allocation_hints::with_hint;
use seismograph_rallocator::callers::{EventKind, HeapKind};
use seismograph_rallocator::snapshot::Snapshot;
use seismograph_rallocator::topology::SliceKind;
use support::stats;

mod support;

fn track_callers(enabled: bool) {
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled,
            capture_backtraces: enabled,
            ..Default::default()
        },
        ..Default::default()
    });
}

static TEST_LOCK: Mutex<()> = Mutex::new(());

const OVERWRITE_TEST_ALLOCATIONS: usize = 64 * 1024;

rallocator::rallocator!();

#[test]
fn caller_tracking_is_runtime_gated() {
    let _test = test_lock();
    track_callers(false);
    let before = snapshot();
    drop(Box::new(1_u64));
    assert_eq!(snapshot().is_some(), before.is_some());

    track_callers(true);
    let value = Box::new(42_u64);
    let address = (&raw const *value).addr() as u64;
    drop(value);
    track_callers(false);

    let snapshot = decoded_snapshot();
    let callers = snapshot.callers.unwrap();
    assert_ne!(callers.session_id, 0);
    let events: Vec<_> = callers.events.iter().filter(|event| event.address == address).collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, EventKind::Allocated);
    assert_eq!(events[1].kind, EventKind::Deallocated);
    #[cfg(not(miri))]
    assert!(!events[0].call_stack.is_empty());
}

#[test]
fn passive_bump_hints_detach_and_reuse_native_state() {
    let _test = test_lock();
    track_callers(true);

    let heap = Heap::bump(bump::Options::new());
    let first = with_hint(&heap, || vec![1_u8; 1024]);
    let first_address = first.as_ptr() as u64;
    drop(first);

    let second = with_hint(&heap, || vec![2_u8; 1024]);
    let second_address = second.as_ptr() as u64;
    drop(heap);
    drop(second);

    track_callers(false);
    let snapshot = decoded_snapshot();
    let allocations = snapshot
        .callers
        .unwrap()
        .events
        .into_iter()
        .filter(|event| event.kind == EventKind::Allocated && (event.address == first_address || event.address == second_address))
        .collect::<Vec<_>>();

    assert_eq!(allocations.len(), 2);
    assert!(allocations.iter().all(|event| event.heap_kind == HeapKind::Bump));
    assert_eq!(allocations[0].heap_id, allocations[1].heap_id);
}

#[test]
fn snapshot_capture_does_not_add_allocator_mappings() {
    let _test = test_lock();
    track_callers(false);
    let before = stats().unwrap();

    let captured = snapshot().unwrap();
    let after = stats().unwrap();

    assert!(after.mapped_bytes <= before.mapped_bytes);
    assert_eq!(after.os_mappings, before.os_mappings);
    drop(captured);
}

#[test]
#[cfg_attr(miri, ignore = "cross-thread tracking-log/TLS lifecycle coverage is exercised by native tests")]
fn collection_includes_every_participating_thread_log() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);

    let threads: Vec<_> = (0..4)
        .map(|value| {
            std::thread::spawn(move || {
                drop(std::hint::black_box(Box::new(value)));
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }

    track_callers(false);

    let snapshot = decoded_snapshot();
    let callers = snapshot.callers.unwrap();
    assert!(callers.threads.len() >= 4);
    assert!(
        callers
            .events
            .iter()
            .filter(|event| { event.kind == EventKind::Allocated && event.size == size_of::<i32>() as u64 })
            .count()
            >= 4
    );
}

#[test]
#[cfg_attr(miri, ignore = "cross-thread tracking-log/TLS lifecycle coverage is exercised by native tests")]
fn remote_thread_heap_allocations_keep_caller_tracking() {
    let _test = test_lock();
    track_callers(false);
    let heap = thread_heap();
    track_callers(true);

    let value = std::thread::spawn(move || with_hint(&heap, || Box::new([3_u8; 64])))
        .join()
        .unwrap();
    let address = value.as_ptr() as u64;
    drop(value);
    track_callers(false);

    let snapshot = decoded_snapshot();
    assert!(snapshot.callers.unwrap().events.iter().any(|event| {
        event.kind == EventKind::Allocated
            && event.address == address
            && event.heap_kind == HeapKind::Thread
            && !event.call_stack.is_empty()
    }));
}

#[test]
fn one_logical_heap_gets_independent_native_realizations_per_thread() {
    let _test = test_lock();
    track_callers(false);
    let heap = Heap::bump(bump::Options::new());
    track_callers(true);

    let first_heap = heap.clone();
    let second_heap = heap;
    let first = std::thread::spawn(move || with_hint(&first_heap, || Box::new([1_u8; 80])));
    let second = std::thread::spawn(move || with_hint(&second_heap, || Box::new([2_u8; 80])));
    let first = first.join().unwrap();
    let second = second.join().unwrap();
    let addresses = [first.as_ptr() as u64, second.as_ptr() as u64];
    drop((first, second));
    track_callers(false);

    let callers = decoded_snapshot().callers.unwrap();
    let heap_ids = addresses.map(|address| {
        callers
            .events
            .iter()
            .find(|event| event.kind == EventKind::Allocated && event.address == address)
            .unwrap()
            .heap_id
    });
    assert_ne!(heap_ids[0], heap_ids[1]);
}

#[test]
fn thread_heap_hint_remains_usable_after_its_owner_exits() {
    let _test = test_lock();
    track_callers(false);
    let heap = std::thread::spawn(thread_heap).join().unwrap();
    track_callers(true);

    let value = with_hint(&heap, || Box::new([4_u8; 96]));
    let address = value.as_ptr() as u64;
    drop(value);
    track_callers(false);

    assert!(
        decoded_snapshot()
            .callers
            .unwrap()
            .events
            .iter()
            .any(|event| event.kind == EventKind::Allocated && event.address == address && event.heap_kind == HeapKind::Thread)
    );
}

#[test]
fn snapshot_encodes_allocations_by_stack() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);
    for value in 0..4 {
        drop(Box::new([value as u8; 777]));
    }
    track_callers(false);

    let snapshot = decoded_snapshot();
    let callers = snapshot.callers.unwrap();
    let target_stack = &callers
        .events
        .iter()
        .find(|event| event.kind == EventKind::Allocated && event.size == 777)
        .unwrap()
        .call_stack;
    let allocation_count = callers
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Allocated && event.call_stack == *target_stack)
        .count();
    assert_eq!(allocation_count, 4);
}

#[test]
#[cfg_attr(miri, ignore = "cross-thread tracking-log/TLS lifecycle coverage is exercised by native tests")]
fn tracked_allocation_can_be_freed_on_another_thread() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);
    let address = Box::into_raw(Box::new([0xA5_u8; 128])) as usize;

    std::thread::Builder::new()
        .name("telemetry-reclaimer".to_owned())
        .spawn(move || unsafe {
            drop(Box::from_raw(address as *mut [u8; 128]));
        })
        .unwrap()
        .join()
        .unwrap();
    track_callers(false);

    let snapshot = decoded_snapshot();
    let callers = snapshot.callers.unwrap();
    let allocated = callers
        .events
        .iter()
        .find(|event| event.kind == EventKind::Allocated && event.address == address as u64 && event.size == 128)
        .unwrap();
    let deallocated = callers
        .events
        .iter()
        .find(|event| {
            event.kind == EventKind::Deallocated
                && event.thread_log_id == allocated.thread_log_id
                && event.allocation_id == allocated.allocation_id
        })
        .unwrap();
    assert_eq!(allocated.allocation_id, deallocated.allocation_id);
    assert_eq!(allocated.thread_log_id, deallocated.thread_log_id);
    assert_ne!(allocated.event_thread_id, deallocated.event_thread_id);
    assert!(!deallocated.call_stack.is_empty());
    assert!(
        callers
            .thread_names
            .iter()
            .any(|thread| { thread.thread_id == deallocated.event_thread_id && thread.name == "telemetry-reclaimer" })
    );
}

#[cfg(not(miri))]
#[test]
fn bounded_per_thread_logs_report_overwritten_events() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);
    for value in 0..=OVERWRITE_TEST_ALLOCATIONS {
        black_box(Box::new(value));
    }
    track_callers(false);

    let snapshot = decoded_snapshot();
    let callers = snapshot.callers.unwrap();
    assert!(callers.total_events >= 5_000);
    assert!(callers.lost_events > 0);
    assert!(
        callers
            .threads
            .iter()
            .all(|thread| thread.total_events - thread.lost_events <= 131_072)
    );
    assert_eq!(callers.lost_events, callers.total_events - callers.events.len() as u64);
}

#[test]
#[cfg_attr(miri, ignore = "cross-thread tracking-log/TLS lifecycle coverage is exercised by native tests")]
fn collection_is_safe_while_a_thread_updates_its_log() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);

    let worker = std::thread::spawn(|| {
        let allocation_count = if cfg!(miri) { 32 } else { 2_000 };
        for value in 0..allocation_count {
            drop(Box::new([value as u8; 96]));
        }
    });
    while !worker.is_finished() {
        let snapshot = decoded_snapshot();
        let callers = snapshot.callers.unwrap();
        assert!(
            callers
                .events
                .iter()
                .all(|event| { event.kind == EventKind::Deallocated || !event.call_stack.is_empty() })
        );
    }
    worker.join().unwrap();
    track_callers(false);

    let snapshot = decoded_snapshot();
    let callers = snapshot.callers.unwrap();
    assert!(
        callers
            .events
            .iter()
            .all(|event| { event.kind == EventKind::Deallocated || !event.call_stack.is_empty() })
    );
}

#[test]
#[cfg(not(miri))]
fn retained_call_stacks_are_encoded() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);
    drop(Box::new(7_u64));
    track_callers(false);

    let snapshot = decoded_snapshot();
    let callers = snapshot.callers.unwrap();
    let event = callers.events.iter().find(|event| event.kind == EventKind::Allocated).unwrap();
    assert!(!event.call_stack.is_empty());
    assert!(event.call_stack.iter().all(|address| *address != 0));
    assert!(
        event
            .call_stack
            .iter()
            .all(|address| { snapshot.addresses.iter().any(|lookup| lookup.address == *address) })
    );
}

#[test]
fn tracked_larger_small_allocations_reuse_context_slabs() {
    let _test = test_lock();
    track_callers(false);
    let allocator = &GLOBAL;
    let layout = Layout::from_size_align(4_096, 16).unwrap();
    track_callers(true);

    let first = unsafe { allocator.alloc(layout) };
    assert!(!first.is_null());
    unsafe { allocator.dealloc(first, layout) };
    let mappings = stats().unwrap().os_mappings;

    let second = unsafe { allocator.alloc(layout) };
    assert!(!second.is_null());
    unsafe { allocator.dealloc(second, layout) };
    track_callers(false);

    assert_eq!(stats().unwrap().os_mappings, mappings);
}

#[test]
fn bump_heap_allocations_are_still_fully_tracked() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);
    let heap = Heap::bump(bump::Options::new());
    let value = with_hint(&heap, || Box::new([7_u8; 256]));
    let address = value.as_ptr() as usize;
    drop(value);
    track_callers(false);

    let snapshot = decoded_snapshot();
    assert!(
        snapshot
            .topology
            .iter()
            .flat_map(|region| &region.slices)
            .any(|slice| slice.kind == SliceKind::Bump)
    );
    assert!(snapshot.callers.unwrap().events.iter().any(|event| {
        event.kind == EventKind::Allocated
            && event.address == address as u64
            && event.size == 256
            && event.heap_kind == HeapKind::Bump
            && event.heap_id != 0
    }));
}

#[test]
fn escaped_bump_free_uses_the_cached_attachment_and_records_its_stack() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);
    let heap = Heap::bump(bump::Options::new());
    let value = with_hint(&heap, || Box::new([0x5A_u8; 384]));
    let address = value.as_ptr() as usize;
    drop(heap);
    drop(value);
    track_callers(false);

    let callers = decoded_snapshot().callers.unwrap();
    let allocation = callers
        .events
        .iter()
        .find(|event| event.kind == EventKind::Allocated && event.address == address as u64)
        .unwrap();
    let deallocation = callers
        .events
        .iter()
        .find(|event| {
            event.kind == EventKind::Deallocated
                && event.thread_log_id == allocation.thread_log_id
                && event.allocation_id == allocation.allocation_id
        })
        .unwrap();
    assert_eq!(allocation.heap_kind, HeapKind::Bump);
    assert_eq!(allocation.heap_id, deallocation.heap_id);
    assert!(!deallocation.freed_after_heap_release);
    #[cfg(not(miri))]
    assert!(!deallocation.call_stack.is_empty());
}

#[test]
fn active_recording_tracks_per_thread_histograms() {
    let _test = test_lock();
    track_callers(false);
    track_callers(true);
    let value = Box::new([0xC3_u8; 512]);
    let address = value.as_ptr() as usize;
    drop(value);
    track_callers(false);

    let snapshot = decoded_snapshot();
    let bucket = usize::BITS as usize - 512_usize.leading_zeros() as usize;
    assert!(snapshot.histograms.allocated.is_empty());
    assert!(snapshot.histograms.live.is_empty());
    let callers = snapshot.callers.unwrap();
    let thread = callers
        .threads
        .iter()
        .find(|thread| {
            callers.events.iter().any(|event| {
                event.thread_log_id == thread.thread_log_id && event.kind == EventKind::Allocated && event.address == address as u64
            })
        })
        .unwrap();
    assert!(thread.allocated_histogram[bucket] >= 1);
    assert_eq!(thread.live_histogram[bucket], 0);
}

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn snapshot_reports_process_totals_and_region_topology() {
    let _test = test_lock();
    track_callers(false);
    let value = Box::new([7_u8; 64]);
    let snapshot = decoded_snapshot();
    assert!(snapshot.stats.live_bytes >= 64);
    assert!(snapshot.stats.mapped_bytes >= snapshot.stats.live_bytes);

    let size_class = snapshot.size_classes.iter().find(|class| class.block_bytes == 64).unwrap();
    assert!(size_class.live_allocations.value >= 1);
    assert!(size_class.requested_bytes.value >= 64);
    assert!(size_class.usable_bytes.value >= size_class.requested_bytes.value);
    assert!(!snapshot.regions.is_empty());
    let default_domain = snapshot
        .domains
        .iter()
        .find(|domain| domain.is_default)
        .expect("default domain telemetry");
    assert!(!default_domain.region_indices.is_empty());
    assert!(default_domain.small_slices >= 1);
    let segments = snapshot
        .topology
        .iter()
        .flat_map(|region| &region.slices)
        .filter(|slice| slice.kind == SliceKind::Small)
        .flat_map(|slice| &slice.segments)
        .collect::<Vec<_>>();
    assert!(!segments.is_empty());
    assert!(segments.iter().all(|segment| !segment.utilization_tracked));

    drop(value);
}

#[test]
fn opaque_snapshot_suppresses_allocator_operations() {
    let _test = test_lock();
    #[cfg(not(miri))]
    let path = format!("opaque-snapshot-{}.bin", std::process::id());
    track_callers(false);
    let encoded = (0..32)
        .find_map(|_| {
            let before = stats().unwrap();
            let encoded = snapshot().unwrap();
            let after = stats().unwrap();
            (after.allocated_bytes == before.allocated_bytes
                && after.deallocated_bytes == before.deallocated_bytes
                && after.live_bytes == before.live_bytes
                && after.allocations == before.allocations
                && after.deallocations == before.deallocations)
                .then_some(encoded)
        })
        .expect("test-harness allocation activity did not settle");
    assert!(!encoded.as_bytes().is_empty());
    #[cfg(not(miri))]
    encoded.write_file(&path).unwrap();
    let before_drop = stats().unwrap();
    drop(encoded);
    let after_drop = stats().unwrap();
    assert_eq!(after_drop.allocated_bytes, before_drop.allocated_bytes);
    assert_eq!(after_drop.deallocated_bytes, before_drop.deallocated_bytes);
    assert_eq!(after_drop.live_bytes, before_drop.live_bytes);
    assert_eq!(after_drop.allocations, before_drop.allocations);
    assert_eq!(after_drop.deallocations, before_drop.deallocations);
    #[cfg(not(miri))]
    std::fs::remove_file(path).unwrap();
}

fn decoded_snapshot() -> Snapshot {
    let snapshot = snapshot().unwrap();
    let snapshot = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
    let source = snapshot
        .sources
        .iter()
        .find(|source| source.id == seismograph_rallocator::source::ID)
        .unwrap();
    seismograph_rallocator::decode(&source.data).unwrap()
}

fn snapshot() -> Option<seismograph::snapshot::Snapshot> {
    seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).ok()
}
