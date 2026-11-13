// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::*;

#[cfg(feature = "stats")]
#[test]
fn gamma_round2_shared_only_current_chunk_contributes_exact_wasted_tail() {
    let arena = Arena::new();
    let value = arena.alloc_box(37_u32);
    assert!(!arena.current_has_reference.get());
    assert_ne!(arena.local_shared_count.get(), 0);
    let expected = u64::from(arena.current.borrow().wasted_tail_for_stats());
    assert_ne!(expected, 0);
    assert_eq!(arena.stats().wasted_tail_bytes, expected);
    assert_eq!(*value, 37);
}

#[test]
fn gamma_survivor_arena_initial_state_preallocation_ratchet_and_refill_marker() {
    let arena = Arena::new();
    assert_eq!(arena.next_class.get(), SizeClass::ZERO);
    assert_eq!(arena.local_shared_count.get(), 0);
    assert!(!arena.current_has_reference.get());
    assert!(arena.retired_local.is_empty());
    assert_eq!(arena.provider.gamma_survivor_byte_budget(), usize::MAX);
    arena.preallocate_one(SizeClass::min_for_bytes(2048)).unwrap();
    assert_eq!(arena.next_class.get(), SizeClass::min_for_bytes(2048));
    arena.preallocate_one(SizeClass::min_for_bytes(1024)).unwrap();
    assert_eq!(arena.next_class.get(), SizeClass::min_for_bytes(2048));
    arena.preallocate_one(SizeClass::min_for_bytes(2048)).unwrap();
    assert_eq!(arena.next_class.get(), SizeClass::min_for_bytes(2048));
    let value = arena.alloc(7_u8);
    assert_eq!(*value, 7);
    assert!(arena.current_has_reference.get());
    arena.refill(1).unwrap();
    assert!(!arena.current_has_reference.get());
    assert!(!arena.retired_local.is_empty());
    assert_eq!(arena.next_class.get(), SizeClass::min_for_bytes(8192));
    assert_eq!(arena.local_shared_count.get(), 0);
}

#[cfg(feature = "stats")]
#[test]
fn gamma_survivor_arena_retired_local_reset_rewinds_without_detaching_current() {
    let mut arena = Arena::new();
    drop(arena.alloc(7_u8));
    arena.refill(1).unwrap();
    drop(arena.alloc(9_u8));
    assert!(!arena.retired_local.is_empty());
    assert_eq!(arena.local_shared_count.get(), 0);
    let class = arena.next_class.get();
    let pointer = arena.current().try_alloc_bytes(1).unwrap().init_copy_from_slice(&[1]).as_ptr();
    arena.reset();
    assert!(arena.retired_local.is_empty());
    assert!(!arena.current_has_reference.get());
    assert_eq!(arena.next_class.get(), class);
    assert_eq!(arena.stats().cached_chunks, 0);
    assert_eq!(arena.stats().wasted_tail_bytes, 0);
    let before = arena.stats().normal_chunks_allocated;
    let next = arena.alloc(3_u8);
    assert_eq!(arena.stats().normal_chunks_allocated, before);
    assert!((&raw const *next).addr() < pointer.addr());
}
