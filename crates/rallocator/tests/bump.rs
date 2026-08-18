// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for bump heaps and their lifecycle.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "Bounded fixtures and grouped unsafe operations make bump allocator scenarios concise"
)]

use std::alloc::{Layout, alloc, dealloc};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Barrier};

use allocation_hints::domain::Domain;
use allocation_hints::heap::bump::Options as BumpOptions;
use allocation_hints::heap::{Heap, InfoKind, Options};
use allocation_hints::{Hint, with_hint};

rallocator::rallocator!();

fn bump(options: BumpOptions) -> Heap {
    Heap::with_options(Options::bump(options))
}

#[test]
fn bump_heaps_and_their_pools_preserve_domain_identity() {
    rallocator::initialize();
    let domain = Domain::new();
    let first_address = {
        let heap = Heap::from_thread_pool_in(domain, BumpOptions::new());
        assert_eq!(heap.info().domain(), domain);
        let value = with_hint(Hint::new().with_heap(&heap), || Box::new(1_u64));
        let address = (&raw const *value).addr();
        drop(value);
        address
    };

    let default_heap = Heap::from_thread_pool(BumpOptions::new());
    let default_value = with_hint(Hint::new().with_heap(&default_heap), || Box::new(2_u64));
    assert_ne!((&raw const *default_value).addr(), first_address);
    drop(default_value);
    drop(default_heap);

    let heap = Heap::from_thread_pool_in(domain, BumpOptions::new());
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new(3_u64));
    assert_eq!((&raw const *value).addr(), first_address);
}

#[test]
fn bump_info_and_usage_report_configuration_and_live_bytes() {
    rallocator::initialize();
    let options = BumpOptions::new().with_max_allocation_bytes(1024).with_retained_chunks(2);
    let heap = bump(options);
    let info = heap.info();
    let InfoKind::Bump(bump_info) = info.kind() else {
        panic!("expected bump heap info");
    };
    assert_eq!(bump_info.options(), options);
    assert!(!info.is_active());

    let value = with_hint(Hint::new().with_heap(&heap), || {
        assert!(heap.info().is_active());
        let value = Box::new([1_u8; 48]);
        let usage = heap.usage().unwrap();
        assert_eq!(usage.live_allocations(), 1);
        assert_eq!(usage.live_requested_bytes(), 48);
        assert_eq!(usage.live_usable_bytes(), 48);
        value
    });
    drop(value);
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn ordinary_global_allocations_use_the_active_bump_heap() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new(42_u64));

    assert_eq!(*value, 42);
    assert_eq!(heap.usage().unwrap().bump().unwrap().allocation_count(), 1);
    assert_eq!(heap.usage().unwrap().live_allocations(), 1);

    drop(value);
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn reverse_order_frees_reuse_the_active_bump_tail() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let layout = Layout::new::<u64>();

    with_hint(Hint::new().with_heap(&heap), || unsafe {
        let first = alloc(layout);
        let second = alloc(layout);
        let third = alloc(layout);
        assert!(!first.is_null());
        assert!(!second.is_null());
        assert!(!third.is_null());

        dealloc(third, layout);
        dealloc(second, layout);

        assert_eq!(alloc(layout), second);
        assert_eq!(alloc(layout), third);

        dealloc(third, layout);
        dealloc(second, layout);
        dealloc(first, layout);
    });
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn allocations_cross_the_second_chunk_segment() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let values = with_hint(Hint::new().with_heap(&heap), || {
        (0..3).map(|value| Box::new([value as u8; 16 * 1024])).collect::<Vec<_>>()
    });

    assert!(heap.usage().unwrap().bump().unwrap().cursor_used_bytes() > 32 * 1024);
    assert_eq!(values[2][0], 2);
    drop(values);
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn nested_bump_heaps_restore_the_previous_heap() {
    rallocator::initialize();
    let outer = bump(BumpOptions::new());
    let inner = bump(BumpOptions::new());

    let (outer_first, inner_value, outer_second) = with_hint(Hint::new().with_heap(&outer), || {
        let outer_first = Box::new(1_u64);
        let inner_value = with_hint(Hint::new().with_heap(&inner), || Box::new(2_u64));
        let outer_second = Box::new(3_u64);
        (outer_first, inner_value, outer_second)
    });

    assert_eq!(outer.usage().unwrap().bump().unwrap().allocation_count(), 2);
    assert_eq!(inner.usage().unwrap().bump().unwrap().allocation_count(), 1);
    drop((outer_first, inner_value, outer_second));
    assert!(outer.usage().unwrap().is_empty());
    assert!(inner.usage().unwrap().is_empty());
}

#[test]
fn a_bump_heap_can_be_reentered_around_another_heap() {
    rallocator::initialize();
    let outer = bump(BumpOptions::new());
    let inner = bump(BumpOptions::new());

    with_hint(Hint::new().with_heap(&outer), || {
        with_hint(Hint::new().with_heap(&inner), || {
            with_hint(Hint::new().with_heap(&outer), || {
                let value = Box::new(1_u64);
                assert_eq!(*value, 1);
            });
        });
    });
}

#[test]
fn global_hint_temporarily_bypasses_the_bump_heap() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let (bump_first, global, bump_second) = with_hint(Hint::new().with_heap(&heap), || {
        let bump_first = Box::new(1_u64);
        let global = with_hint(Hint::global(), || Box::new(2_u64));
        let bump_second = Box::new(3_u64);
        (bump_first, global, bump_second)
    });

    assert_eq!(heap.usage().unwrap().bump().unwrap().allocation_count(), 2);
    drop(global);
    assert_eq!(heap.usage().unwrap().live_allocations(), 2);
    drop((bump_first, bump_second));
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn hint_is_restored_during_unwinding() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        with_hint(Hint::new().with_heap(&heap), || {
            let _value = Box::new(7_u64);
            panic!("expected test panic");
        });
    }));

    assert!(result.is_err());
    let allocations_after_unwind = heap.usage().unwrap().bump().unwrap().allocation_count();
    let outside = Box::new(9_u64);
    assert_eq!(heap.usage().unwrap().bump().unwrap().allocation_count(), allocations_after_unwind);
    drop(outside);
}

#[test]
fn allocation_can_be_dropped_on_another_thread() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new(123_u64));

    std::thread::spawn(move || drop(value)).join().unwrap();
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn usage_remains_consistent_during_cross_thread_frees() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let allocation_count = if cfg!(miri) { 16 } else { 256 };
    let values = with_hint(&heap, || (0..allocation_count).map(Box::new).collect::<Vec<_>>());
    let worker = std::thread::spawn(move || {
        for value in values {
            drop(value);
            std::thread::yield_now();
        }
    });

    let mut previous_live = usize::MAX;
    while !worker.is_finished() {
        let usage = heap.usage().unwrap();
        assert!(usage.live_allocations() <= previous_live);
        assert!(usage.live_requested_bytes() <= usage.live_usable_bytes());
        previous_live = usage.live_allocations();
        std::thread::yield_now();
    }
    worker.join().unwrap();
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn cross_thread_free_does_not_rewind_an_active_bump_tail() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let slot = Arc::new(std::sync::Mutex::new(None));
    let ready = Arc::new(Barrier::new(2));
    let freed = Arc::new(Barrier::new(2));
    let worker_slot = Arc::clone(&slot);
    let worker_ready = Arc::clone(&ready);
    let worker_freed = Arc::clone(&freed);
    let worker = std::thread::spawn(move || {
        worker_ready.wait();
        drop(worker_slot.lock().unwrap().take());
        worker_freed.wait();
    });

    with_hint(Hint::new().with_heap(&heap), || {
        let value = Box::new(123_u64);
        let address = (&raw const *value).addr();
        *slot.lock().unwrap() = Some(value);
        ready.wait();
        freed.wait();

        let next = Box::new(456_u64);
        assert_ne!((&raw const *next).addr(), address);
    });
    worker.join().unwrap();
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn allocations_keep_the_state_alive_after_handle_drop() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new(123_u64));
    drop(heap);

    std::thread::spawn(move || drop(value)).join().unwrap();

    let recycled = Heap::from_thread_pool(BumpOptions::new());
    let next = with_hint(Hint::new().with_heap(&recycled), || Box::new(456_u64));
    assert_eq!(*next, 456);
}

#[test]
fn hint_keeps_bump_state_alive_after_handle_drop() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let hint = Hint::new().with_heap(&heap);
    drop(heap);

    let value = with_hint(hint, || Box::new(321_u64));
    assert_eq!(*value, 321);
    std::thread::spawn(move || drop(value)).join().unwrap();
}

#[test]
fn empty_bump_heaps_are_reused_from_the_thread_pool() {
    rallocator::initialize();
    let first_address = {
        let heap = Heap::from_thread_pool(BumpOptions::new());
        let value = with_hint(Hint::new().with_heap(&heap), || Box::new(1_u64));
        let address = (&raw const *value).addr();
        drop(value);
        address
    };

    let heap = Heap::from_thread_pool(BumpOptions::new());
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new(2_u64));
    assert_eq!((&raw const *value).addr(), first_address);
}

#[test]
fn fresh_bump_heap_does_not_alias_the_thread_pool() {
    rallocator::initialize();
    let pooled_address = {
        let heap = Heap::from_thread_pool(BumpOptions::new());
        let value = with_hint(Hint::new().with_heap(&heap), || Box::new(1_u64));
        let address = (&raw const *value).addr();
        drop(value);
        address
    };

    let heap = bump(BumpOptions::new());
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new(2_u64));
    assert_ne!((&raw const *value).addr(), pooled_address);
}

#[test]
fn configured_size_and_alignment_thresholds_fall_back_to_general() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new().with_max_allocation_bytes(256).with_max_alignment(256));
    let bump_layout = Layout::from_size_align(256, 16).unwrap();
    let large_layout = Layout::from_size_align(257, 16).unwrap();
    let overaligned_layout = Layout::from_size_align(64, 512).unwrap();

    let (bump_address, large_address, overaligned_address) = with_hint(Hint::new().with_heap(&heap), || unsafe {
        (alloc(bump_layout), alloc(large_layout), alloc(overaligned_layout))
    });

    assert!(!bump_address.is_null());
    assert!(!large_address.is_null());
    assert!(!overaligned_address.is_null());
    assert_eq!(overaligned_address.addr() % overaligned_layout.align(), 0);
    assert_eq!(heap.usage().unwrap().bump().unwrap().allocation_count(), 1);

    unsafe {
        dealloc(bump_address, bump_layout);
        dealloc(large_address, large_layout);
        dealloc(overaligned_address, overaligned_layout);
    }
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn escaped_vec_reallocates_into_the_general_allocator() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let mut values = with_hint(Hint::new().with_heap(&heap), || vec![1_u64]);
    assert_eq!(heap.usage().unwrap().live_allocations(), 1);

    values.reserve(1_024);
    assert!(heap.usage().unwrap().is_empty());
    assert_eq!(values[0], 1);
}

#[test]
fn bump_heap_handle_can_move_between_threads() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let (heap, value) = std::thread::spawn(move || {
        let value = with_hint(Hint::new().with_heap(&heap), || Box::new(88_u64));
        (heap, value)
    })
    .join()
    .unwrap();

    assert_eq!(*value, 88);
    drop(value);
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn lowering_retained_chunks_trims_reused_backing_state() {
    rallocator::initialize();
    let heap = Heap::from_thread_pool(BumpOptions::new().with_retained_chunks(8));
    let values = with_hint(Hint::new().with_heap(&heap), || {
        (0..40).map(|index| Box::new([index as u8; 16 * 1024])).collect::<Vec<_>>()
    });
    assert!(heap.usage().unwrap().bump().unwrap().chunk_count() > 8);

    drop(values);
    drop(heap);

    let recycled = Heap::from_thread_pool(BumpOptions::new().with_retained_chunks(1));
    assert_eq!(recycled.usage().unwrap().bump().unwrap().chunk_count(), 1);
}

#[test]
fn pooled_bump_retention_grows_with_demand_and_decays_after_underuse() {
    rallocator::initialize();
    let options = BumpOptions::new().with_retained_chunks(2).with_max_retained_chunks(8);
    let heap = Heap::from_thread_pool(options);
    let values = with_hint(Hint::new().with_heap(&heap), || {
        (0..40).map(|index| Box::new([index as u8; 16 * 1024])).collect::<Vec<_>>()
    });
    assert!(heap.usage().unwrap().bump().unwrap().chunk_count() > 8);

    drop(values);
    drop(heap);

    let recycled = Heap::from_thread_pool(options);
    assert_eq!(recycled.usage().unwrap().bump().unwrap().chunk_count(), 8);
    drop(recycled);

    for _ in 0..48 {
        let heap = Heap::from_thread_pool(options);
        let value = with_hint(Hint::new().with_heap(&heap), || Box::new(1_u64));
        drop(value);
        drop(heap);
    }

    let recycled = Heap::from_thread_pool(options);
    assert_eq!(recycled.usage().unwrap().bump().unwrap().chunk_count(), 2);
}

#[test]
fn bump_heap_rejects_simultaneous_activation() {
    rallocator::initialize();
    let heap = bump(BumpOptions::new());
    let entered = Arc::new(Barrier::new(2));
    let leave = Arc::new(Barrier::new(2));
    let worker_hint = Hint::new().with_heap(&heap);
    let worker_entered = Arc::clone(&entered);
    let worker_leave = Arc::clone(&leave);

    let worker = std::thread::spawn(move || {
        with_hint(worker_hint, || {
            worker_entered.wait();
            worker_leave.wait();
        });
    });

    entered.wait();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        with_hint(Hint::new().with_heap(&heap), || {});
    }));
    leave.wait();
    worker.join().unwrap();

    assert!(result.is_err());
}
