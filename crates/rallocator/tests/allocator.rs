// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for allocator routes and custom configurations.
#![expect(
    clippy::cast_ptr_alignment,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "Tests construct deliberately aligned raw allocator storage and keep operations together for readability"
)]

use std::alloc::{GlobalAlloc, Layout};
use std::sync::Mutex;

use allocation_hints::heap::bump::Options as BumpOptions;
use allocation_hints::heap::{Heap, Options as HeapOptions, thread_heap};
use allocation_hints::{Hint, with_hint};
use rallocator::Rallocator;
use rallocator::config::Config;
use rallocator::telemetry::{stats, track_callers};
use rallocator::tunables::{SizeClassLayout, Tunables};

enum TestSizeClasses {}

impl SizeClassLayout for TestSizeClasses {
    const SIZES: &'static [usize] = &[
        16, 32, 48, 64, 96, 112, 128, 160, 192, 224, 256, 320, 384, 448, 512, 640, 768, 896, 1024, 1280, 1536, 1792, 2048, 2560, 3072,
        3584, 4096, 5120, 6144, 7168, 8192, 10240, 12288, 14336, 16384,
    ];
}

rallocator::tunable!(TestTunables {
    size_classes: TestSizeClasses,
    partial_slab_scan_limit: 4,
    recycled_bitmap_batch_max_block_size: 256,
    medium_purge_delay_ms: 1_000,
});

rallocator::config!(TestConfig {
    track_aggregates: true,
    track_callers: true,
    tunables: TestTunables,
});

rallocator::tunable!(DefaultTunables {});
rallocator::tunable!(PartialTunables {
    medium_purge_delay_ms: 250,
});
rallocator::config!(DefaultConfig {});
rallocator::config!(TunablesOnlyConfig { tunables: PartialTunables });

const _: () = {
    assert!(!DefaultConfig::TRACK_AGGREGATES);
    assert!(!DefaultConfig::TRACK_CALLERS);
    assert!(DefaultTunables::PARTIAL_SLAB_SCAN_LIMIT == 4);
    assert!(DefaultTunables::RECYCLED_BITMAP_BATCH_MAX_BLOCK_SIZE == 256);
    assert!(DefaultTunables::MEDIUM_PURGE_DELAY_MS == 1_000);
    assert!(PartialTunables::PARTIAL_SLAB_SCAN_LIMIT == 4);
    assert!(PartialTunables::MEDIUM_PURGE_DELAY_MS == 250);
    assert!(!TunablesOnlyConfig::TRACK_AGGREGATES);
    assert!(!TunablesOnlyConfig::TRACK_CALLERS);
};

static ALLOCATOR: Rallocator<TestConfig> = unsafe { Rallocator::new() };
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct SendAddress(*mut u8);

unsafe impl Send for SendAddress {}

impl SendAddress {
    unsafe fn deallocate(self, layout: Layout) {
        unsafe { ALLOCATOR.dealloc(self.0, layout) };
    }
}

#[test]
fn allocates_aligned_memory_and_tracks_statistics() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    let warmup = Layout::from_size_align(16, 16).unwrap();
    let warmup_address = unsafe { allocator.alloc(warmup) };
    assert!(!warmup_address.is_null());
    unsafe { allocator.dealloc(warmup_address, warmup) };
    let before = stats().unwrap();
    let layout = Layout::from_size_align(257, 4096).unwrap();

    let address = unsafe { allocator.alloc(layout) };
    assert!(!address.is_null());
    assert_eq!(address as usize % layout.align(), 0);

    unsafe {
        address.write_bytes(0xA5, layout.size());
        allocator.dealloc(address, layout);
    }

    let after = stats().unwrap();
    assert_eq!(after.allocated_bytes - before.allocated_bytes, layout.size());
    assert_eq!(after.deallocated_bytes - before.deallocated_bytes, layout.size());
    assert_eq!(after.live_bytes, before.live_bytes);
    assert_eq!(after.allocations - before.allocations, 1);
    assert_eq!(after.deallocations - before.deallocations, 1);
}

#[test]
fn small_allocations_support_every_power_of_two_alignment() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    for alignment in [32, 64, 128, 256, 512, 1024, 2048, 4096] {
        for size in [1, 17, 257, 1003, 4096, 8193] {
            let layout = Layout::from_size_align(size, alignment).unwrap();
            let address = unsafe { allocator.alloc(layout) };
            assert!(!address.is_null());
            assert_eq!(address.addr() % alignment, 0);
            unsafe { allocator.dealloc(address, layout) };
        }
    }
}

#[test]
fn application_defined_layout_builds_derived_lookup_tables() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    let layout = Layout::from_size_align(65, 16).unwrap();
    let address = unsafe { allocator.alloc(layout) };
    assert!(!address.is_null());
    unsafe { allocator.dealloc(address, layout) };
}

#[test]
#[cfg_attr(miri, ignore = "thread/TLS lifecycle coverage is exercised by native tests")]
fn application_defined_layout_supports_context_and_remote_slab_lifecycles() {
    rallocator::initialize();
    let _test = test_lock();
    track_callers(true);
    let layout = Layout::from_size_align(64, 16).unwrap();
    let heap = Heap::new();
    let addresses = with_hint(Hint::new().with_heap(&heap), || {
        std::array::from_fn::<_, 3, _>(|_| unsafe { ALLOCATOR.alloc(layout) })
    });
    assert!(addresses.iter().all(|address| !address.is_null()));
    for address in addresses {
        unsafe { ALLOCATOR.dealloc(address, layout) };
    }

    let owner = thread_heap().unwrap();
    let owner_hint = Hint::new().with_heap(&owner);
    let local_free = std::thread::spawn(move || SendAddress(with_hint(owner_hint, || unsafe { ALLOCATOR.alloc(layout) })))
        .join()
        .unwrap();
    let local_free = local_free.0;
    assert!(!local_free.is_null());
    unsafe { ALLOCATOR.dealloc(local_free, layout) };

    let owner_hint = Hint::new().with_heap(&owner);
    let remote_free = std::thread::spawn(move || SendAddress(with_hint(owner_hint, || unsafe { ALLOCATOR.alloc(layout) })))
        .join()
        .unwrap();
    assert!(!remote_free.0.is_null());
    std::thread::spawn(move || unsafe { remote_free.deallocate(layout) })
        .join()
        .unwrap();
    track_callers(false);
}

#[test]
fn caller_capable_bump_heap_falls_back_when_tracking_header_does_not_fit() {
    rallocator::initialize();
    let _test = test_lock();
    track_callers(false);
    let layout = Layout::from_size_align(32 * 1024 - 32, 16).unwrap();
    let heap = Heap::with_options(HeapOptions::bump(BumpOptions::new().with_max_allocation_bytes(layout.size())));

    let address = with_hint(Hint::new().with_heap(&heap), || unsafe { ALLOCATOR.alloc(layout) });
    assert!(!address.is_null());
    assert_eq!(heap.usage().unwrap().bump().unwrap().allocation_count(), 0);
    unsafe {
        address.write_bytes(0xA5, layout.size());
        ALLOCATOR.dealloc(address, layout);
    }
}

#[test]
fn small_allocations_reuse_size_class_slabs() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    let layout = Layout::from_size_align(64, 8).unwrap();
    let before = stats().unwrap();

    let first = unsafe { allocator.alloc(layout) };
    assert!(!first.is_null());
    unsafe { allocator.dealloc(first, layout) };

    let mappings_after_first = stats().unwrap().os_mappings;
    let second = unsafe { allocator.alloc(layout) };
    assert!(!second.is_null());
    unsafe { allocator.dealloc(second, layout) };

    let after = stats().unwrap();
    assert_eq!(after.os_mappings, mappings_after_first);
    assert_eq!(after.allocations - before.allocations, 2);
    assert_eq!(after.deallocations - before.deallocations, 2);
}

#[test]
fn local_small_reuse_does_not_overwrite_the_block() {
    rallocator::initialize();
    let _test = test_lock();
    std::thread::spawn(|| {
        let allocator = &ALLOCATOR;
        let layout = Layout::from_size_align(64, 16).unwrap();
        let first = unsafe { allocator.alloc(layout) };
        assert!(!first.is_null());
        unsafe {
            first.cast::<u64>().write(0xA5A5_A5A5_A5A5_A5A5);
            allocator.dealloc(first, layout);
        }

        let reused = unsafe { allocator.alloc(layout) };
        assert_eq!(reused, first);
        assert_eq!(unsafe { reused.cast::<u64>().read() }, 0xA5A5_A5A5_A5A5_A5A5);
        unsafe { allocator.dealloc(reused, layout) };
    })
    .join()
    .unwrap();
}

#[test]
fn mixed_small_classes_share_a_locality_segment() {
    rallocator::initialize();
    let _test = test_lock();
    std::thread::spawn(move || {
        let allocating = &ALLOCATOR;
        let layouts = [Layout::from_size_align(96, 16).unwrap(), Layout::from_size_align(224, 16).unwrap()];
        let allocation_count = if cfg!(miri) { 64 } else { 1_600 };
        let mut allocations = Vec::with_capacity(allocation_count);
        let mut slab_bases = Vec::new();
        for index in 0..allocation_count {
            let layout = layouts[index % layouts.len()];
            let address = unsafe { allocating.alloc(layout) };
            assert!(!address.is_null());
            let slab = address.addr() & !((32 * 1024) - 1);
            if !slab_bases.contains(&slab) {
                slab_bases.push(slab);
            }
            allocations.push((address, layout));
        }

        let first = *slab_bases.iter().min().unwrap();
        let last = *slab_bases.iter().max().unwrap();
        assert!(last - first < 4 * 1024 * 1024);

        for (address, layout) in allocations {
            unsafe { allocating.dealloc(address, layout) };
        }
    })
    .join()
    .unwrap();
}

#[test]
fn large_allocations_release_direct_mappings() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    let layout = if cfg!(miri) {
        Layout::from_size_align(16, 128 * 1024).unwrap()
    } else {
        Layout::from_size_align(64 * 1024 * 1024, 128 * 1024).unwrap()
    };
    let before = stats().unwrap();

    let address = unsafe { allocator.alloc(layout) };
    assert!(!address.is_null());
    assert_eq!(stats().unwrap().os_mappings - before.os_mappings, 1);

    unsafe { allocator.dealloc(address, layout) };

    let after = stats().unwrap();
    assert_eq!(after.os_mappings - before.os_mappings, 1);
    assert!(after.mapped_bytes <= before.mapped_bytes);
}

#[test]
fn medium_spans_are_aligned_and_reused() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    let layout = Layout::from_size_align(64 * 1024, 64 * 1024).unwrap();

    let first = unsafe { allocator.alloc(layout) };
    assert!(!first.is_null());
    assert_eq!(first as usize % layout.align(), 0);
    unsafe { allocator.dealloc(first, layout) };

    let second = unsafe { allocator.alloc(layout) };
    assert_eq!(second, first);
    unsafe { allocator.dealloc(second, layout) };
}

#[test]
fn medium_region_supports_cached_and_variable_length_spans() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    let before = stats().unwrap();
    let mut allocations = Vec::new();

    let slice_counts: &[usize] = if cfg!(miri) {
        &[1, 2, 4, 8, 9, 12, 16]
    } else {
        &[1, 2, 8, 16, 64, 256, 512, 1024, 2048]
    };
    for &slices in slice_counts {
        let size = slices * 64 * 1024;
        let layout = Layout::from_size_align(size, 64 * 1024).unwrap();
        let address = unsafe { allocator.alloc(layout) };
        assert!(!address.is_null());
        assert_eq!(address as usize % layout.align(), 0);
        allocations.push((address, layout));
    }

    for (address, layout) in allocations {
        unsafe { allocator.dealloc(address, layout) };
    }

    assert_eq!(stats().unwrap().live_bytes, before.live_bytes);
}

#[test]
fn adjacent_large_extents_coalesce_and_split() {
    rallocator::initialize();
    let _test = test_lock();
    let allocator = &ALLOCATOR;
    let (first_size, combined_size) = if cfg!(miri) {
        (12 * 64 * 1024, 24 * 64 * 1024)
    } else {
        (64 * 1024 * 1024, 128 * 1024 * 1024)
    };
    let first_layout = Layout::from_size_align(first_size, 64 * 1024).unwrap();
    let combined_layout = Layout::from_size_align(combined_size, 64 * 1024).unwrap();

    let first = unsafe { allocator.alloc(first_layout) };
    let second = unsafe { allocator.alloc(first_layout) };
    assert!(!first.is_null());
    assert!(!second.is_null());
    assert_eq!(second.addr() - first.addr(), first_layout.size());

    unsafe {
        allocator.dealloc(second, first_layout);
        allocator.dealloc(first, first_layout);
    }

    let mappings_before_reuse = stats().unwrap().os_mappings;
    let combined = unsafe { allocator.alloc(combined_layout) };
    assert_eq!(combined, first);
    assert_eq!(stats().unwrap().os_mappings, mappings_before_reuse);
    unsafe { allocator.dealloc(combined, combined_layout) };
}

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let layout = Layout::new::<u8>();
    let address = unsafe { ALLOCATOR.alloc(layout) };
    assert!(!address.is_null());
    unsafe { ALLOCATOR.dealloc(address, layout) };
    guard
}
