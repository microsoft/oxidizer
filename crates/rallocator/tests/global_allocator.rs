use std::alloc::{Layout, alloc, dealloc};
use std::mem::size_of;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use allocation_hints::heap::general::Options as GeneralOptions;
use allocation_hints::heap::{Heap, Options};
use allocation_hints::{Hint, with_hint};
use rallocator::telemetry::stats;

rallocator::config!(TrackingConfig { track_aggregates: true });

rallocator::rallocator!(TrackingConfig);

static TEST_LOCK: Mutex<()> = Mutex::new(());

const OLD_BUMP_MARKER: usize = 0x5241_4C4C_4152_454E;

fn general_heap() -> Heap {
    Heap::with_options(Options::general(
        GeneralOptions::new()
            .with_locality_segment_bytes(64 * 1024)
            .with_medium_cache_max_bytes(0),
    ))
}

#[test]
fn global_allocator_and_general_heap_retirement_work() {
    let _test = TEST_LOCK.lock().unwrap();
    let mut values = with_hint(Hint::new(), || Vec::with_capacity(128));
    values.extend(0..128_u64);
    assert_eq!(values.iter().sum::<u64>(), 8_128);
    let boxed = Box::new(String::from("allocated by rallocator"));
    assert_eq!(boxed.as_str(), "allocated by rallocator");
    let current_stats = stats().unwrap();
    assert!(current_stats.allocations > 0);
    assert!(current_stats.live_bytes > 0);

    drop(Heap::new());
    let before = stats().unwrap();
    let heap = general_heap();
    with_hint(Hint::new().with_heap(&heap), || drop(Box::new([0_u8; 64])));
    let allocated = stats().unwrap();
    assert!(allocated.os_mappings > before.os_mappings);
    drop(heap);
    assert!(stats().unwrap().os_unmappings > allocated.os_unmappings);

    let before = stats().unwrap();
    let heap = general_heap();
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new([1_u8; 64]));
    drop(heap);
    assert_eq!(*value, [1_u8; 64]);
    drop(value);
    assert!(stats().unwrap().os_unmappings > before.os_unmappings);

    let before = stats().unwrap();
    let heap = general_heap();
    let (first, second) = with_hint(Hint::new().with_heap(&heap), || (Box::new([1_u8; 64]), Box::new([2_u8; 128])));
    drop(heap);
    drop(first);
    assert_eq!(*second, [2_u8; 128]);
    drop(second);
    assert!(stats().unwrap().os_unmappings > before.os_unmappings);

    let before = stats().unwrap();
    let heap = Heap::with_options(Options::general(
        GeneralOptions::new()
            .with_locality_segment_bytes(64 * 1024)
            .with_medium_cache_max_bytes(64 * 1024),
    ));
    with_hint(Hint::new().with_heap(&heap), || drop(Vec::<u8>::with_capacity(64 * 1024)));
    drop(heap);
    assert!(stats().unwrap().os_unmappings > before.os_unmappings);

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
        let before = stats().unwrap();
        let heap = general_heap();
        let allocation = with_hint(Hint::new().with_heap(&heap), || Box::new([3_u8; 64]));
        value.store(Box::into_raw(allocation), Ordering::Release);
        drop(heap);
        wait_for_sequence(&completed, iteration + 3);
        assert!(stats().unwrap().os_unmappings > before.os_unmappings);
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
    let (medium, direct) = with_hint(Hint::new().with_heap(&heap), || unsafe {
        (alloc(medium_layout), alloc(direct_layout))
    });
    assert!(!medium.is_null());
    assert!(!direct.is_null());
    drop(heap);

    unsafe {
        dealloc(medium, medium_layout);
        dealloc(direct, direct_layout);
    }
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
    let address = unsafe { alloc(layout) };
    assert!(!address.is_null());
    let words = address.cast::<usize>();
    unsafe {
        words.write(OLD_BUMP_MARKER);
        words.add(1).write(!OLD_BUMP_MARKER);
        words.add(2).write(1);
        dealloc(address, layout);
    }
}
