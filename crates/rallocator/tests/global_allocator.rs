// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for the installed global allocator.
#![expect(
    clippy::cast_ptr_alignment,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    clippy::unwrap_used,
    reason = "Tests validate deliberately aligned raw allocations and fail immediately on invalid layouts"
)]

use std::alloc::{Layout, alloc, alloc_zeroed, dealloc, realloc};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use allocation_hints::heaps::{Heap, general};
use allocation_hints::with_hint;
use support::stats;

mod support;

rallocator::rallocator!();

static TEST_LOCK: Mutex<()> = Mutex::new(());

const OLD_BUMP_MARKER: usize = 0x5241_4C4C_4152_454E;

fn general_heap() -> Heap {
    Heap::general(
        general::Options::new()
            .with_locality_segment_bytes(64 * 1024)
            .with_medium_cache_max_bytes(0),
    )
}

#[test]
fn global_allocator_and_passive_general_heap_lifecycle_work() {
    let _test = TEST_LOCK.lock().unwrap();
    let mut values = Vec::with_capacity(128);
    values.extend(0..128_u64);
    assert_eq!(values.iter().sum::<u64>(), 8_128);
    let boxed = Box::new(String::from("allocated by rallocator"));
    assert_eq!(boxed.as_str(), "allocated by rallocator");

    let original = Layout::from_size_align(32, 16).unwrap();
    let zeroed = unsafe { alloc_zeroed(original) };
    assert!(!zeroed.is_null());
    assert!(
        unsafe { std::slice::from_raw_parts(zeroed, original.size()) }
            .iter()
            .all(|byte| *byte == 0)
    );
    let grown = unsafe { realloc(zeroed, original, 96) };
    assert!(!grown.is_null());
    unsafe { dealloc(grown, Layout::from_size_align(96, 16).unwrap()) };
    let current_stats = stats().unwrap();
    assert!(current_stats.allocations > 0);
    assert!(current_stats.live_bytes > 0);

    let before = stats().unwrap();
    for _ in 0..10 {
        let heap = general_heap();
        with_hint(&heap, || drop(Box::new([0_u8; 64])));
        drop(heap);
    }
    assert!(stats().unwrap().os_unmappings > before.os_unmappings);

    let heap = general_heap();
    let (first, second) = with_hint(&heap, || (Box::new([1_u8; 64]), Box::new([2_u8; 128])));
    drop(heap);
    drop(first);
    assert_eq!(*second, [2_u8; 128]);
    drop(second);

    let value = Arc::new(AtomicPtr::<[u8; 64]>::new(ptr::null_mut()));
    let completed = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let remote_value = Arc::clone(&value);
    let remote_completed = Arc::clone(&completed);
    let remote_stop = Arc::clone(&stop);
    let remote = std::thread::spawn(move || {
        let mut sequence = 1;
        remote_completed.store(sequence, Ordering::Release);
        while !remote_stop.load(Ordering::Acquire) {
            let value = remote_value.swap(ptr::null_mut(), Ordering::AcqRel);
            if value.is_null() {
                std::hint::spin_loop();
                continue;
            }
            unsafe { drop(Box::from_raw(value)) };
            sequence += 1;
            remote_completed.store(sequence, Ordering::Release);
        }
    });
    wait_for_sequence(&completed, 1);
    value.store(Box::into_raw(Box::new([0_u8; 64])), Ordering::Release);
    wait_for_sequence(&completed, 2);
    let iterations = if cfg!(miri) { 4 } else { 100 };
    for iteration in 0..iterations {
        let heap = general_heap();
        let allocation = with_hint(&heap, || Box::new([3_u8; 64]));
        value.store(Box::into_raw(allocation), Ordering::Release);
        drop(heap);
        wait_for_sequence(&completed, iteration + 3);
    }
    stop.store(true, Ordering::Release);
    remote.join().unwrap();
}

fn wait_for_sequence(completed: &AtomicUsize, expected: usize) {
    while completed.load(Ordering::Acquire) < expected {
        std::hint::spin_loop();
    }
}

#[test]
fn escaped_medium_and_direct_allocations_outlive_their_heap_handle() {
    let _test = TEST_LOCK.lock().unwrap();
    let heap = general_heap();
    let medium_layout = Layout::from_size_align(128 * 1024, 16).unwrap();
    let direct_layout = Layout::from_size_align(64, 128 * 1024).unwrap();
    let (medium, direct) = with_hint(&heap, || unsafe { (alloc(medium_layout), alloc(direct_layout)) });
    assert!(!medium.is_null());
    assert!(!direct.is_null());
    drop(heap);

    unsafe {
        dealloc(medium, medium_layout);
        dealloc(direct, direct_layout);
    }
}

#[test]
fn zeroed_allocation_and_reallocation_cover_general_routes() {
    let _test = TEST_LOCK.lock().unwrap();

    for layout in [
        Layout::from_size_align(64, 16).unwrap(),
        Layout::from_size_align(128 * 1024, 64).unwrap(),
    ] {
        unsafe { assert_zeroed_allocation(layout) };
    }
    #[cfg(not(miri))]
    unsafe {
        assert_zeroed_allocation(Layout::from_size_align(64, 128 * 1024).unwrap());
    }

    unsafe {
        assert_reallocation_preserves_prefix(64, 16, 128 * 1024);
        #[cfg(not(miri))]
        assert_reallocation_preserves_prefix(128 * 1024, 128 * 1024, 20 * 1024 * 1024);
    }
}

unsafe fn assert_zeroed_allocation(layout: Layout) {
    let address = unsafe { alloc_zeroed(layout) };
    assert!(!address.is_null());
    assert_eq!(address.addr() % layout.align(), 0);
    assert!(
        unsafe { std::slice::from_raw_parts(address, layout.size()) }
            .iter()
            .all(|byte| *byte == 0)
    );
    unsafe { dealloc(address, layout) };
}

unsafe fn assert_reallocation_preserves_prefix(original_size: usize, alignment: usize, new_size: usize) {
    let original = Layout::from_size_align(original_size, alignment).unwrap();
    let address = unsafe { alloc(original) };
    assert!(!address.is_null());
    unsafe { ptr::write_bytes(address, 0xA5, original_size) };

    let grown = unsafe { realloc(address, original, new_size) };
    assert!(!grown.is_null());
    assert_eq!(grown.addr() % original.align(), 0);
    assert!(
        unsafe { std::slice::from_raw_parts(grown, original_size) }
            .iter()
            .all(|byte| *byte == 0xA5)
    );
    unsafe { dealloc(grown, Layout::from_size_align(new_size, original.align()).unwrap()) };
}

#[test]
fn medium_payload_cannot_forge_bump_ownership() {
    let _test = TEST_LOCK.lock().unwrap();
    unsafe { allocate_forged_payload(Layout::from_size_align(3 * size_of::<usize>(), 64 * 1024).unwrap()) };
}

#[test]
fn direct_payload_cannot_forge_bump_ownership() {
    let _test = TEST_LOCK.lock().unwrap();
    let before = stats().unwrap();
    unsafe { allocate_forged_payload(Layout::from_size_align(3 * size_of::<usize>(), 128 * 1024).unwrap()) };
    assert!(stats().unwrap().os_unmappings > before.os_unmappings);
}

unsafe fn allocate_forged_payload(layout: Layout) {
    assert!(layout.size() >= 3 * size_of::<usize>());
    assert!(layout.align() >= std::mem::align_of::<usize>());
    let address = unsafe { alloc(layout) };
    assert!(!address.is_null());
    let words = address.cast::<usize>();
    unsafe {
        words.write(OLD_BUMP_MARKER);
        words.add(1).write(!OLD_BUMP_MARKER);
        words.add(2).write(1);
        std::hint::black_box(address);
        dealloc(address, layout);
    }
}
