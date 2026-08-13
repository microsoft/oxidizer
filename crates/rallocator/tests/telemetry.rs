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

use allocation_hints::domain::Domain;
use allocation_hints::heap::{Heap, Options, bump, thread_heap};
use allocation_hints::{Hint, with_hint};
#[cfg(not(miri))]
use rallocator::config::Config;
use rallocator::telemetry::stats::{Sampler, Session};
use rallocator::telemetry::{snapshot, stats, track_callers};
use rallocator_telemetry::callers::{EventKind, HeapKind};
use rallocator_telemetry::snapshot::Snapshot;
use rallocator_telemetry::topology::SliceKind;

static TEST_LOCK: Mutex<()> = Mutex::new(());

const TEST_CALLER_EVENT_CAPACITY: usize = if cfg!(miri) { 128 } else { 128 * 1024 };

rallocator::config!(TelemetryConfig {
    track_aggregates: true,
    track_callers: true,
    caller_event_capacity: TEST_CALLER_EVENT_CAPACITY,
});

rallocator::config!(DefaultCallerConfig { track_callers: true });

rallocator::config!(CustomCallerConfig {
    track_callers: true,
    caller_event_capacity: 8,
    caller_allocation_stack_frames: 3,
    caller_deallocation_stack_frames: 5,
    caller_track_threads: false,
    caller_track_heap_lifetimes: false,
});

rallocator::rallocator!(TelemetryConfig);

#[test]
fn caller_diagnostic_configuration_has_stable_defaults_and_overrides() {
    use rallocator::config::Config;

    rallocator::initialize();

    const {
        assert!(DefaultCallerConfig::CALLER_EVENT_CAPACITY == 128 * 1024);
        assert!(DefaultCallerConfig::CALLER_ALLOCATION_STACK_FRAMES == 16);
        assert!(DefaultCallerConfig::CALLER_DEALLOCATION_STACK_FRAMES == 16);
        assert!(DefaultCallerConfig::CALLER_TRACK_THREADS);
        assert!(DefaultCallerConfig::CALLER_TRACK_HEAP_LIFETIMES);

        assert!(CustomCallerConfig::CALLER_EVENT_CAPACITY == 8);
        assert!(CustomCallerConfig::CALLER_ALLOCATION_STACK_FRAMES == 3);
        assert!(CustomCallerConfig::CALLER_DEALLOCATION_STACK_FRAMES == 5);
        assert!(!CustomCallerConfig::CALLER_TRACK_THREADS);
        assert!(!CustomCallerConfig::CALLER_TRACK_HEAP_LIFETIMES);
    }
}

#[test]
fn tracking_is_off_by_default_and_process_wide_when_enabled() {
    let _test = test_lock();
    rallocator::initialize();
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
#[cfg_attr(miri, ignore = "cross-thread tracking-log/TLS lifecycle coverage is exercised by native tests")]
fn collection_includes_every_participating_thread_log() {
    let _test = test_lock();
    rallocator::initialize();
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
    rallocator::initialize();
    track_callers(false);
    let heap = thread_heap().unwrap();
    track_callers(true);

    let value = std::thread::spawn(move || with_hint(Hint::new().with_heap(&heap), || Box::new([3_u8; 64])))
        .join()
        .unwrap();
    drop(value);
    track_callers(false);

    let snapshot = decoded_snapshot();
    assert!(
        snapshot
            .callers
            .unwrap()
            .events
            .iter()
            .any(|event| { event.kind == EventKind::Allocated && event.size == 64 && !event.call_stack.is_empty() })
    );
}

#[test]
fn snapshot_encodes_allocations_by_stack() {
    let _test = test_lock();
    rallocator::initialize();
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
    rallocator::initialize();
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
    rallocator::initialize();
    track_callers(false);
    track_callers(true);
    for value in 0..=(TelemetryConfig::CALLER_EVENT_CAPACITY / 2) {
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
    rallocator::initialize();
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
    rallocator::initialize();
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
    rallocator::initialize();
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
    rallocator::initialize();
    track_callers(false);
    track_callers(true);
    let heap = Heap::with_options(Options::bump(bump::Options::new()));
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new([7_u8; 256]));
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
fn escaped_bump_free_records_heap_lifetime_and_free_stack() {
    let _test = test_lock();
    rallocator::initialize();
    track_callers(false);
    track_callers(true);
    let heap = Heap::with_options(Options::bump(bump::Options::new()));
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new([0x5A_u8; 384]));
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
    assert!(deallocation.freed_after_heap_release);
    #[cfg(not(miri))]
    assert!(!deallocation.call_stack.is_empty());
}

#[test]
fn aggregate_and_per_thread_histograms_record_allocated_and_live_sizes() {
    let _test = test_lock();
    rallocator::initialize();
    track_callers(false);
    track_callers(true);
    let value = Box::new([0xC3_u8; 512]);
    let address = value.as_ptr() as usize;
    drop(value);
    track_callers(false);

    let snapshot = decoded_snapshot();
    let bucket = usize::BITS as usize - 512_usize.leading_zeros() as usize;
    assert!(snapshot.histograms.allocated[bucket] >= 1);
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
fn snapshot_reports_bounded_size_class_and_region_telemetry() {
    let _test = test_lock();
    rallocator::initialize();
    track_callers(false);
    let value = Box::new([7_u8; 64]);
    let snapshot = decoded_snapshot();
    assert!(snapshot.stats.live_bytes >= 64);
    assert!(snapshot.stats.mapped_bytes >= snapshot.stats.live_bytes);

    let class = snapshot.size_classes.iter().find(|class| class.block_bytes == 64).unwrap();
    assert!(class.live_allocations.value >= 1);
    assert!(class.requested_bytes.value >= 64);
    assert!(class.usable_bytes.value >= 64);
    for estimate in [class.live_allocations, class.requested_bytes, class.usable_bytes] {
        assert!(estimate.lower_bound <= estimate.value);
        assert!(estimate.value <= estimate.upper_bound);
    }
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
        .filter(|segment| segment.class_index == class.class_index)
        .collect::<Vec<_>>();
    assert!(!segments.is_empty());
    assert!(segments.iter().all(|segment| segment.utilization_tracked));
    assert!(
        segments
            .iter()
            .any(|segment| segment.live_blocks >= 1 && segment.usable_blocks >= segment.live_blocks)
    );

    drop(value);
}

#[test]
fn snapshot_reports_explicit_domain_region_use() {
    let _test = test_lock();
    rallocator::initialize();
    let domain = Domain::new();
    let heap = Heap::with_options(Options::default().with_domain(domain));
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new([9_u8; 64]));

    let snapshot = decoded_snapshot();
    let explicit = snapshot
        .domains
        .iter()
        .find(|domain| !domain.is_default && domain.small_slices != 0)
        .expect("explicit domain telemetry");
    assert_eq!(explicit.region_count, 1);
    assert_eq!(explicit.region_indices.len(), 1);

    drop(value);
}

#[test]
fn sampler_and_session_report_interval_deltas() {
    let _test = test_lock();
    rallocator::initialize();
    let mut sampler = Sampler::new().unwrap();
    drop(Box::new(11_u64));
    let sample = sampler.sample().unwrap();
    assert!(sample.delta().allocations() >= 1);
    assert!(sample.delta().deallocations() >= 1);

    let session = Session::start().unwrap();
    drop(Box::new([3_u8; 128]));
    let report = session.finish().unwrap();
    assert!(report.delta().allocations() >= 1);
    assert!(report.delta().deallocations() >= 1);
}

#[test]
fn opaque_snapshot_suppresses_allocator_operations() {
    let _test = test_lock();
    rallocator::initialize();
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
    rallocator_telemetry::decode(snapshot.as_bytes()).unwrap()
}
