// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

fn exercise<A: GlobalAlloc>(allocator: &A, old_size: usize, new_size: usize, align: usize, retained: bool) {
    let old = Layout::from_size_align(old_size, align).unwrap();
    let new = Layout::from_size_align(new_size, align).unwrap();
    // SAFETY: each allocation is initialized within its layout and freed once,
    // using the new layout after successful reallocation.
    unsafe {
        let address = allocator.alloc(old);
        assert!(!address.is_null());
        address.write_bytes(0x5a, old_size);
        let resized = allocator.realloc(address, old, new_size);
        assert!(!resized.is_null());
        assert_eq!(resized == address, retained, "resize {old_size} -> {new_size}, alignment {align}");
        assert_eq!(resized.addr() % align, 0);
        assert!(
            std::slice::from_raw_parts(resized, old_size.min(new_size))
                .iter()
                .all(|byte| *byte == 0x5a)
        );
        resized.write_bytes(0xa5, new_size);
        allocator.dealloc(resized, new);
    }
}

#[test]
fn small_medium_growth_shrink_alignment_and_fallback() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: the standard personality is used throughout this test binary.
    let allocator = unsafe { Rallocator::<Standard>::new() };
    for (old, new, align, retained) in [
        (49, 64, 16, true),
        (64, 49, 16, true),
        (33, 63, 64, true),
        (4000, 4096, 4096, true),
        (32, 33, 16, false),
        (64, 32, 16, false),
        (32768, 65536, 16, true),
        (65536, 32768, 16, true),
        (32768, 64, 16, true),
        (32768, 65537, 16, false),
        (65537, 131_072, 65536, true),
        (131_072, 65536, 16, false),
        (32, 48, 131_072, false),
        (64, 64, 16, true),
    ] {
        exercise(&allocator, old, new, align, retained);
    }
}

#[test]
fn wrapper_uses_same_resize_logic_and_preserves_snapshot_fallback() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: the wrapper has the same standard personality as other fixtures.
    let allocator = unsafe { GlobalRallocator::<Standard>::new() };
    exercise(&allocator, 49, 64, 16, true);
    exercise(&allocator, 32768, 65536, 16, true);
    tracking::with_snapshot_arena(|| {
        exercise(&allocator, 49, 64, 16, false);
        // Snapshot storage is individually owned, including large allocations.
        exercise(&allocator, 2 * 1024 * 1024, 3 * 1024 * 1024, 16, false);
    });
}

#[test]
fn invalid_backing_preserves_original_data_and_layout() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: the standard fixture owns the old block throughout rejected calls.
    unsafe {
        let allocator = Rallocator::<Standard>::new();
        let layout = Layout::from_size_align(64, 16).unwrap();
        let address = allocator.alloc(layout);
        assert!(!address.is_null());
        address.write_bytes(0x7b, layout.size());
        for size in [0, usize::MAX] {
            // The private boundary accepts arbitrary new sizes. Passing these
            // to GlobalAlloc::realloc would violate that trait's caller contract.
            assert!(reallocate::<_, Standard>(&allocator, address, layout, size).is_null());
            assert!(std::slice::from_raw_parts(address, layout.size()).iter().all(|byte| *byte == 0x7b));
        }
        let valid_but_unbackable = Layout::from_size_align(isize::MAX as usize - 15, layout.align()).unwrap();
        assert!(allocator.realloc(address, layout, valid_but_unbackable.size()).is_null());
        assert!(std::slice::from_raw_parts(address, layout.size()).iter().all(|byte| *byte == 0x7b));
        allocator.dealloc(address, layout);
    }
}

#[cfg(not(miri))]
#[test]
fn failed_replacement_mapping_preserves_original() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: the oversized alignment forces the existing direct-mapping route;
    // the thread-local HAL failure is consumed by that replacement.
    unsafe {
        let allocator = Rallocator::<Standard>::new();
        let layout = Layout::from_size_align(32, 131_072).unwrap();
        let address = allocator.alloc(layout);
        assert!(!address.is_null());
        address.write_bytes(0x7b, layout.size());
        hal::fail_next_map();
        assert!(allocator.realloc(address, layout, 48).is_null());
        assert_eq!(*address, 0x7b);
        allocator.dealloc(address, layout);
    }
}

#[cfg(not(miri))]
#[test]
fn medium_growth_beyond_a_region_preserves_original_on_mapping_failure() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: the live medium allocation keeps its original layout on failure.
    // The injected mapping failure avoids reserving the oversized replacement.
    unsafe {
        let allocator = Rallocator::<Standard>::new();
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let address = allocator.alloc(layout);
        assert!(!address.is_null());
        address.write(0x7b);
        hal::fail_next_map();
        assert!(allocator.realloc(address, layout, MEDIUM_REGION_SIZE + 1).is_null());
        assert_eq!(address.read(), 0x7b);
        allocator.dealloc(address, layout);
    }
}

#[test]
fn context_origin_survives_without_active_recording() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: explicit context allocation uses the normal thread's heap and is
    // freed through origin-aware GlobalAlloc dispatch, not a test-only release.
    unsafe {
        let allocator = Rallocator::<Standard>::new();
        let layout = Layout::from_size_align(49, 16).unwrap();
        let address = allocator.allocate_with_context(layout, None, thread_state());
        assert!(!address.is_null());
        address.write_bytes(0x7b, layout.size());
        let resized = allocator.realloc(address, layout, 64);
        assert!(!resized.is_null());
        assert_ne!(address, resized);
        assert_eq!(*resized, 0x7b);
        allocator.dealloc(resized, Layout::from_size_align(64, 16).unwrap());
    }
}

struct Transfer(*mut u8);
// SAFETY: ownership of the live raw allocation is transferred to exactly one
// receiving thread; no source-thread access follows the transfer.
unsafe impl Send for Transfer {}

impl Transfer {
    fn resize_and_free(self, size: usize, retained: bool) {
        // SAFETY: this transfer owns a standard allocation with the supplied size.
        unsafe {
            let allocator = Rallocator::<Standard>::new();
            let layout = Layout::from_size_align(size, 16).unwrap();
            let resized = allocator.realloc(self.0, layout, size + 1);
            assert!(!resized.is_null());
            assert_eq!(self.0 == resized, retained);
            assert_eq!(*resized, 0x7b);
            allocator.dealloc(resized, Layout::from_size_align(size + 1, 16).unwrap());
        }
    }
}

#[test]
fn foreign_and_exited_owners_keep_origin_and_new_layout_valid() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    for size in [49, 32768] {
        // SAFETY: standard blocks are initialized before transferring ownership.
        let allocation = unsafe {
            let allocator = Rallocator::<Standard>::new();
            let address = allocator.alloc(Layout::from_size_align(size, 16).unwrap());
            assert!(!address.is_null());
            address.write(0x7b);
            Transfer(address)
        };
        std::thread::spawn(move || allocation.resize_and_free(size, size == 32768))
            .join()
            .unwrap();
        let allocation = std::thread::spawn(move || {
            // SAFETY: the returned block outlives its allocating thread through
            // the allocator's existing outstanding-allocation lifetime protocol.
            unsafe {
                let allocator = Rallocator::<Standard>::new();
                let address = allocator.alloc(Layout::from_size_align(size, 16).unwrap());
                assert!(!address.is_null());
                address.write(0x7b);
                Transfer(address)
            }
        })
        .join()
        .unwrap();
        allocation.resize_and_free(size, size == 32768);
    }
}

#[test]
fn small_padding_tracks_resizes_without_changing_object_counts() {
    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: this test owns the slab on the current thread and never accesses it
    // after freeing the live block.
    unsafe {
        let allocator = Rallocator::<Standard>::new();
        let layout = Layout::from_size_align(49, 16).unwrap();
        let address = allocator.alloc(layout);
        assert!(!address.is_null());
        let slab = allocation_segment(address).cast::<SlabHeader>();
        let padding = (*slab).requested_bytes;
        let state = thread_state();
        flush_aggregate_batch(state);
        assert_eq!(allocator.realloc(address, layout, 64), address);
        assert_eq!((*slab).requested_bytes, padding - 15);
        assert_eq!((*state).aggregate_counts, 0);
        assert_eq!(allocator.realloc(address, Layout::from_size_align(64, 16).unwrap(), 49), address);
        assert_eq!((*slab).requested_bytes, padding);
        assert_eq!((*state).aggregate_counts, 0);
        allocator.dealloc(address, layout);
    }
}

#[test]
fn hints_bump_and_reentrant_routes_preserve_origin() {
    use allocation_hints::heaps::{Heap, bump, general};
    use allocation_hints::with_hint;

    let _test = tracking::TEST_LOCK.lock().unwrap();
    // SAFETY: all fixtures use the standard personality and explicit live layouts.
    unsafe {
        let allocator = Rallocator::<Standard>::new();
        let general = Heap::general(general::Options::new());
        let other = Heap::general(general::Options::new());
        let bump = Heap::bump(bump::Options::new());
        for size in [49, 32768] {
            let layout = Layout::from_size_align(size, 16).unwrap();
            let address = with_hint(&general, || allocator.alloc(layout));
            assert!(!address.is_null());
            address.write(0x7b);
            let resized = with_hint(&other, || allocator.realloc(address, layout, size + 1));
            assert!(!resized.is_null());
            assert_eq!(resized == address, size == 32768);
            assert_eq!(*resized, 0x7b);
            allocator.dealloc(resized, Layout::from_size_align(size + 1, 16).unwrap());
        }
        with_hint(&bump, || exercise(&allocator, 49, 64, 16, false));
        let state = thread_state();
        let previous = (*state).in_tracking;
        (*state).in_tracking = true;
        exercise(&allocator, 49, 64, 16, false);
        (*state).in_tracking = previous;
    }
}

// Exact global observations and recorder transitions require a separate process.
// Miri instead executes the real storage, ownership and padding fixtures above.
#[cfg(not(miri))]
#[test]
fn isolated_accounting_tracking_and_concurrent_snapshots() {
    const CHILD: &str = "RALLOCATOR_TEST_REALLOC_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "allocator::realloc::tests::isolated_accounting_tracking_and_concurrent_snapshots",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        return;
    }
    recording(false);
    // SAFETY: this isolated process uses only the standard personality.
    let allocator = unsafe { Rallocator::<Standard>::new() };
    exercise(&allocator, 49, 64, 16, true);
    for (old, grown, shrunk) in [(49, 64, 50), (32768, 65536, 64)] {
        let before = tracking::stats().unwrap();
        // SAFETY: layouts follow each successful resize; the final free uses
        // the current requested size, not the allocation's initial size.
        unsafe {
            let layout = Layout::from_size_align(old, 16).unwrap();
            let address = allocator.alloc(layout);
            assert!(!address.is_null());
            assert_eq!(allocator.realloc(address, layout, grown), address);
            assert_eq!(
                allocator.realloc(address, Layout::from_size_align(grown, 16).unwrap(), shrunk),
                address
            );
            let live = tracking::stats().unwrap();
            assert_eq!(live.live_bytes - before.live_bytes, shrunk);
            assert_eq!(live.allocations - before.allocations, 1);
            assert_eq!(live.deallocations, before.deallocations);
            allocator.dealloc(address, Layout::from_size_align(shrunk, 16).unwrap());
        }
        let after = tracking::stats().unwrap();
        assert_eq!(after.allocated_bytes - before.allocated_bytes, grown);
        assert_eq!(after.deallocated_bytes - before.deallocated_bytes, grown);
        assert_eq!(after.live_bytes, before.live_bytes);
        assert_eq!(after.allocations - before.allocations, 1);
        assert_eq!(after.deallocations - before.deallocations, 1);
    }
    exercise_recording_transitions(&allocator);
    exercise_recorded_resize_events(&allocator);
    exercise_concurrent_resize_accounting(&allocator);
}

#[cfg(not(miri))]
fn recording(enabled: bool) {
    seismograph::recorder(seismograph::recorder::Configuration {
        allocations: seismograph::recorder::RecordingPolicy {
            enabled,
            capture_backtraces: false,
            ..Default::default()
        },
        ..Default::default()
    });
}

#[cfg(not(miri))]
fn exercise_recording_transitions(allocator: &Rallocator<Standard>) {
    for _ in 0..4 {
        for size in [49, 32768] {
            recording(true);
            // SAFETY: tracked context/medium origins are inspected while live.
            // Disabling recording must not turn those origins into ordinary blocks.
            unsafe {
                let layout = Layout::from_size_align(size, 16).unwrap();
                let address = allocator.alloc(layout);
                assert!(!address.is_null());
                if size == 32768 {
                    let region = region_containing(address).unwrap();
                    let slice = (address.addr() - (*region).base.addr()) / MEDIUM_SLICE_SIZE;
                    assert_ne!((*region).allocations[slice].tracking_allocation_id.load(Ordering::Relaxed), 0);
                } else {
                    let marker = (*allocation_segment(address).cast::<SlabHeader>()).marker.load(Ordering::Acquire);
                    assert!(is_context_marker::<Standard>(marker));
                }
                assert_eq!(allocator.realloc(address, layout, size), address);
                recording(false);
                address.write(0x7b);
                let resized = allocator.realloc(address, layout, size + 1);
                assert!(!resized.is_null());
                assert_ne!(address, resized);
                assert_eq!(*resized, 0x7b);
                // Existing untracked objects do not acquire fabricated tracking
                // identities merely because recording becomes enabled during resize.
                recording(true);
                assert_eq!(
                    allocator.realloc(resized, Layout::from_size_align(size + 1, 16).unwrap(), size + 2),
                    resized
                );
                allocator.dealloc(resized, Layout::from_size_align(size + 2, 16).unwrap());
            }
            recording(false);
        }
    }
}

#[cfg(not(miri))]
fn exercise_concurrent_resize_accounting(allocator: &Rallocator<Standard>) {
    let before = tracking::stats().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel::<Transfer>();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let sender = sender.clone();
            scope.spawn(move || {
                for _ in 0..32 {
                    // SAFETY: ownership is transferred after the local resize;
                    // the existing medium lifetime protocol covers owner exit.
                    unsafe {
                        let layout = Layout::from_size_align(32768, 16).unwrap();
                        let address = allocator.alloc(layout);
                        assert!(!address.is_null());
                        address.write(0x7b);
                        assert_eq!(allocator.realloc(address, layout, 40000), address);
                        sender.send(Transfer(address)).unwrap();
                    }
                }
            });
        }
        drop(sender);
        scope.spawn(move || {
            for allocation in receiver {
                allocation.resize_and_free(40000, true);
            }
        });
        for _ in 0..64 {
            let _ = tracking::stats().unwrap();
            #[cfg(feature = "tuning-telemetry")]
            {
                crate::tuning_telemetry::TuningTelemetry::enable();
                crate::tuning_telemetry::TuningTelemetry::enable();
                crate::tuning_telemetry::TuningTelemetry::disable();
            }
            std::thread::yield_now();
        }
    });
    let after = tracking::stats().unwrap();
    assert_eq!(after.allocations - before.allocations, 128);
    assert_eq!(after.deallocations - before.deallocations, 128);
    assert_eq!(after.allocated_bytes - before.allocated_bytes, 128 * 40001);
    assert_eq!(after.deallocated_bytes - before.deallocated_bytes, 128 * 40001);
    assert_eq!(after.live_bytes, before.live_bytes);
    assert_eq!(after.pending_remote_blocks, before.pending_remote_blocks);
}

#[cfg(not(miri))]
fn exercise_recorded_resize_events(allocator: &Rallocator<Standard>) {
    use seismograph_rallocator::callers::EventKind;

    for size in [49, 32768] {
        recording(true);
        // SAFETY: each tracked allocation is live until its replacement succeeds;
        // the replacement is freed with its own layout.
        let addresses = unsafe {
            let layout = Layout::from_size_align(size, 16).unwrap();
            let old = allocator.alloc(layout);
            assert!(!old.is_null());
            old.write(0x7b);
            assert_eq!(allocator.realloc(old, layout, size), old);
            let new = allocator.realloc(old, layout, size + 1);
            assert!(!new.is_null());
            assert_ne!(new, old);
            assert_eq!(*new, 0x7b);
            allocator.dealloc(new, Layout::from_size_align(size + 1, 16).unwrap());
            [old.addr() as u64, new.addr() as u64]
        };
        recording(false);
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let source = decoded
            .sources
            .iter()
            .find(|source| source.id == seismograph_rallocator::source::ID)
            .unwrap();
        let decoded = seismograph_rallocator::decode(&source.data).unwrap();
        let events = decoded.callers.unwrap().events;
        let mut ids = [0; 2];
        for (index, address) in addresses.into_iter().enumerate() {
            let pair: Vec<_> = events.iter().filter(|event| event.address == address).collect();
            assert_eq!(pair.len(), 2);
            assert_eq!(pair[0].kind, EventKind::Allocated);
            assert_eq!(pair[1].kind, EventKind::Deallocated);
            assert_eq!(pair[0].allocation_id, pair[1].allocation_id);
            assert_eq!(pair[0].size, (size + index) as u64);
            assert_eq!(pair[1].size, pair[0].size);
            ids[index] = pair[0].allocation_id;
        }
        assert_ne!(ids[0], ids[1]);
    }
}

#[test]
fn remote_heap_small_origin_uses_fallback() {
    use allocation_hints::heaps::thread_heap;
    use allocation_hints::with_hint;

    // SAFETY: standard personality; warming also registers the exporting owner.
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let owner = thread_heap();
    exercise(&allocator, 49, 64, 16, true);
    let allocation = std::thread::spawn(move || {
        // SAFETY: the hinted allocation is initialized and transferred once.
        unsafe {
            let allocator = Rallocator::<Standard>::new();
            with_hint(&owner, || {
                let address = allocator.alloc(Layout::from_size_align(49, 16).unwrap());
                assert!(!address.is_null());
                let slab = allocation_segment(address).cast::<SlabHeader>();
                assert!(is_remote_slab(slab));
                address.write(0x7b);
                Transfer(address)
            })
        }
    })
    .join()
    .unwrap();
    allocation.resize_and_free(49, false);
}

struct LateReallocator;

impl Drop for LateReallocator {
    fn drop(&mut self) {
        // SAFETY: the allocator's TLS storage remains usable after its guard
        // retires the heap. This fixture registers its destructor first.
        unsafe {
            assert!((*thread_state()).tearing_down);
            let allocator = Rallocator::<Standard>::new();
            exercise(&allocator, 49, 64, 16, false);
        }
    }
}

thread_local! {
    static LATE_REALLOCATOR: LateReallocator = const { LateReallocator };
}

#[test]
fn realloc_after_allocator_tls_teardown_uses_direct_fallback() {
    std::thread::spawn(|| {
        LATE_REALLOCATOR.with(|_| {});
        // SAFETY: ordinary standard allocations establish the later TLS guard.
        let allocator = unsafe { Rallocator::<Standard>::new() };
        exercise(&allocator, 49, 64, 16, true);
    })
    .join()
    .unwrap();
}
