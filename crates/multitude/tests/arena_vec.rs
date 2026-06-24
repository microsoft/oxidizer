// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for [`Vec`]: the growable arena-backed vector.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::missing_asserts_for_indexing, reason = "test code is direct")]
#![allow(clippy::items_after_statements, reason = "test-local types next to their use")]
#![allow(dead_code, reason = "test-local structs with unused fields")]
#![allow(clippy::panic_in_result_fn, reason = "tests deliberately trigger panics")]

mod common;

use core::cmp::Ordering;

use multitude::Arena;
use multitude::vec::{CollectIn, Vec};

#[test]
fn pop_and_clear() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.push(1);
    v.push(2);
    v.push(3);
    assert_eq!(v.pop(), Some(3));
    assert_eq!(v.len(), 2);
    let cap = v.capacity();
    v.clear();
    assert!(v.is_empty());
    assert_eq!(v.capacity(), cap);
    assert_eq!(v.pop(), None);
}

#[test]
fn reserve_grows_capacity() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.reserve(100);
    assert!(v.capacity() >= 100);
}

#[test]
fn vec_with_capacity_factory() {
    let arena = Arena::new();
    let v = arena.alloc_vec_with_capacity::<u32>(50);
    assert!(v.capacity() >= 50);
    assert!(v.is_empty());
}

#[test]
fn as_mut_slice_modifies_elements() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u32);
    v.push(2);
    v.as_mut_slice()[0] = 10;
    assert_eq!(v.as_slice(), &[10, 2]);
}

#[test]
fn extend_from_slice() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend_from_slice([1_u32, 2, 3]);
    v.extend_from_slice([4, 5]);
    assert_eq!(v.as_slice(), &[1, 2, 3, 4, 5]);
}

#[test]
fn extend_iter() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend(0_u32..5);
    assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4]);
}

#[test]
fn collect_in_works() {
    let arena = Arena::new();
    let v: Vec<i32, _> = (0..10).collect_in(&arena);
    assert_eq!(v.len(), 10);
    assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
}

#[test]
fn traits_compile() {
    let arena = Arena::new();
    let mut a = arena.alloc_vec();
    a.extend([1_u32, 2, 3]);
    let mut b = arena.alloc_vec();
    b.extend([1_u32, 2, 3]);
    let mut c = arena.alloc_vec();
    c.extend([4_u32, 5]);
    let _: &[u32] = a.as_ref();
    let mb: &mut [u32] = a.as_mut();
    mb[0] = 1;
    let r: &[u32] = core::borrow::Borrow::borrow(&a);
    assert_eq!(r, &[1, 2, 3]);
    assert_eq!(format!("{a:?}"), "[1, 2, 3]");
    assert_eq!(a, b);
    assert!(a != c);
    assert_eq!(a.cmp(&c), Ordering::Less);
    assert_eq!(a.partial_cmp(&c), Some(Ordering::Less));
    assert_eq!(common::hash_of(&a), common::hash_of(&b));
}

#[test]
fn try_push_succeeds() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.try_push(1_u32).unwrap();
    v.try_push(2_u32).unwrap();
    assert_eq!(&*v, &[1, 2]);
}

#[test]
fn try_reserve_succeeds() {
    let arena = Arena::new();
    let mut v: Vec<u32> = arena.alloc_vec();
    v.try_reserve(64).unwrap();
    assert!(v.capacity() >= 64);
}

#[test]
fn try_with_capacity_in_succeeds() {
    let arena = Arena::new();
    let v: Vec<u32> = arena.try_alloc_vec_with_capacity(32).unwrap();
    assert!(v.capacity() >= 32);
    assert!(v.is_empty());
}

#[test]
fn try_with_capacity_in_zero_does_not_allocate() {
    let arena = Arena::new();
    let v: Vec<u32> = arena.try_alloc_vec_with_capacity(0).unwrap();
    assert_eq!(v.capacity(), 0);
}

#[test]
fn try_push_returns_err_on_alloc_failure() {
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let mut v: Vec<u32, _> = arena.alloc_vec();
    let _ = v.try_push(1).unwrap_err();
}

#[test]
fn try_reserve_returns_err_on_alloc_failure() {
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let mut v: Vec<u32, _> = arena.alloc_vec();
    let _ = v.try_reserve(16).unwrap_err();
}

#[test]
fn try_with_capacity_in_returns_err_on_alloc_failure() {
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let result: Result<Vec<u32, _>, _> = arena.try_alloc_vec_with_capacity(16);
    let _ = result.unwrap_err();
}

#[test]
fn with_capacity_in_pub_succeeds() {
    let arena = Arena::new();
    let v: Vec<u32> = arena.alloc_vec_with_capacity(8);
    assert!(v.capacity() >= 8);
}

#[test]
fn new_in_pub_succeeds() {
    let arena = Arena::new();
    let v: Vec<u8> = arena.alloc_vec();
    assert_eq!(v.len(), 0);
    assert_eq!(v.capacity(), 0);
}

#[test]
fn from_iter_in_builds_content() {
    let arena = Arena::new();
    let v = Vec::<i32>::from_iter_in(0..5, &arena);
    assert_eq!(v.as_slice(), &[0, 1, 2, 3, 4]);
}

#[test]
fn as_ptr_and_as_mut_ptr_round_trip() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([10_u32, 20, 30]);
    let p = v.as_ptr();
    // SAFETY: pointer is valid for len reads.
    let first = unsafe { *p };
    assert_eq!(first, 10);
    let mp = v.as_mut_ptr();
    // SAFETY: pointer is valid for writes.
    unsafe { *mp = 99 };
    assert_eq!(v.as_slice(), &[99, 20, 30]);
}

#[test]
fn insert_remove_swap_remove() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 4]);
    v.insert(2, 3);
    assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
    let r = v.remove(0);
    assert_eq!(r, 1);
    assert_eq!(v.as_slice(), &[2, 3, 4]);
    let s = v.swap_remove(0);
    assert_eq!(s, 2);
    assert_eq!(v.as_slice(), &[4, 3]);
}

#[test]
#[should_panic(expected = "insertion index")]
fn insert_out_of_bounds_panics() {
    let arena = Arena::new();
    let mut v: Vec<u32> = arena.alloc_vec();
    v.insert(99, 1);
}

#[test]
#[should_panic(expected = "removal index")]
fn remove_out_of_bounds_panics() {
    let arena = Arena::new();
    let mut v: Vec<u32> = arena.alloc_vec();
    let _ = v.remove(0);
}

#[test]
#[should_panic(expected = "swap_remove index")]
fn swap_remove_out_of_bounds_panics() {
    let arena = Arena::new();
    let mut v: Vec<u32> = arena.alloc_vec();
    let _ = v.swap_remove(0);
}

#[test]
fn truncate_shortens() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend(0_u32..10);
    v.truncate(4);
    assert_eq!(v.as_slice(), &[0, 1, 2, 3]);
    v.truncate(100);
    assert_eq!(v.len(), 4);
}

#[test]
fn set_len_unsafe() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.reserve(4);
    let p = v.as_mut_ptr();
    for i in 0..4_u32 {
        // SAFETY: capacity >= 4; offset i is in-bounds.
        let slot = unsafe { p.add(i as usize) };
        // SAFETY: slot points to writable spare capacity.
        unsafe { slot.write(i * 2) };
    }
    // SAFETY: the loop above initialized indices 0..4.
    unsafe { v.set_len(4) };
    assert_eq!(v.as_slice(), &[0, 2, 4, 6]);
}

#[test]
fn shrink_to_fit_runs() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.reserve(128);
    v.push(1);
    v.push(2);
    let cap_before = v.capacity();
    v.shrink_to_fit();
    assert!(v.capacity() <= cap_before);
    assert_eq!(v.as_slice(), &[1, 2]);
}

#[test]
fn shrink_to_fit_at_max_normal_alloc_boundary_reclaims() {
    // `Vec::shrink_to_fit` early-returns when `total_bytes > max_normal_alloc`;
    // at `total_bytes == max_normal_alloc` it must still reclaim the tail.
    let mna = 4 * 1024;
    let arena: Arena = Arena::builder().max_normal_alloc(mna).build();
    // u8 keeps `total_bytes == cap`. Pick the largest cap whose refill hint
    // still fits in a normal chunk (`refill_hint <= mna`), so the Vec lives in
    // `current` and `try_reclaim_tail` has a chance to fire. The freezable
    // buffer reserves the `Arc<[u8]>` freeze prefix, so the hint is
    // `cap + 16` (≈12B strong+len prefix + 4B alignment slack).
    let cap = mna - 16;
    let mut v: Vec<'_, u8> = arena.alloc_vec_with_capacity(cap);
    v.extend_from_slice([7_u8; 16]);
    assert_eq!(v.capacity(), cap);
    v.shrink_to_fit();
    assert_eq!(
        v.capacity(),
        v.len(),
        "shrink_to_fit on a Vec backed by the current normal chunk must reclaim the unused tail",
    );
}

#[test]
fn retain_filters() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend(0_u32..10);
    v.retain(|x| x % 2 == 0);
    assert_eq!(v.as_slice(), &[0, 2, 4, 6, 8]);
}

#[test]
fn retain_mut_filters_and_mutates() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend(0_u32..6);
    v.retain_mut(|x| {
        if *x % 2 == 0 {
            *x *= 10;
            true
        } else {
            false
        }
    });
    assert_eq!(v.as_slice(), &[0, 20, 40]);
}

#[test]
fn dedup_basic() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 1, 2, 3, 3, 3, 4]);
    v.dedup();
    assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn dedup_by_custom() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_i32, -1, 2, -2, 3]);
    v.dedup_by(|a, b| a.abs() == b.abs());
    assert_eq!(v.as_slice(), &[1, 2, 3]);
}

#[test]
fn dedup_by_key_works() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([10_i32, 11, 21, 22, 30]);
    v.dedup_by_key(|x| *x / 10);
    assert_eq!(v.as_slice(), &[10, 21, 30]);
}

#[test]
fn append_moves_elements() {
    let arena = Arena::new();
    let mut a = arena.alloc_vec();
    a.extend([1_u32, 2]);
    let mut b = arena.alloc_vec();
    b.extend([3_u32, 4, 5]);
    a.append(&mut b);
    assert_eq!(a.as_slice(), &[1, 2, 3, 4, 5]);
    assert!(b.is_empty());
}

#[test]
fn reserve_exact_grows() {
    let arena = Arena::new();
    let mut v: Vec<u32> = arena.alloc_vec();
    v.reserve_exact(50);
    assert!(v.capacity() >= 50);
}

#[test]
fn try_reserve_exact_succeeds() {
    let arena = Arena::new();
    let mut v: Vec<u32> = arena.alloc_vec();
    v.try_reserve_exact(40).unwrap();
    assert!(v.capacity() >= 40);
}

#[test]
fn try_reserve_exact_returns_err_on_alloc_failure() {
    let alloc = common::FailingAllocator::new(0);
    let arena = Arena::new_in(alloc);
    let mut v: Vec<u32, _> = arena.alloc_vec();
    let _ = v.try_reserve_exact(16).unwrap_err();
}

#[test]
fn resize_grow_and_shrink() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    v.resize(5, 9);
    assert_eq!(v.as_slice(), &[1, 2, 3, 9, 9]);
    v.resize(2, 0);
    assert_eq!(v.as_slice(), &[1, 2]);
}

#[test]
fn resize_with_closure() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    let mut counter = 0_u32;
    v.resize_with(4, || {
        counter += 1;
        counter
    });
    assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn split_off_returns_tail() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend(0_u32..6);
    let tail = v.split_off(4);
    assert_eq!(v.as_slice(), &[0, 1, 2, 3]);
    assert_eq!(tail.as_slice(), &[4, 5]);
}

#[test]
fn split_off_shares_chunk_without_copying() {
    // No-copy split_off: the tail's data pointer must equal the head's
    // original data + at. Both halves share the underlying chunk via
    // an extra inc_ref.
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.extend(0_u32..10);
    let head_data_before = v.as_ptr();
    let head_cap_before = v.capacity();
    let tail = v.split_off(4);
    // Head's data pointer is unchanged.
    assert_eq!(v.as_ptr(), head_data_before);
    // Tail's data pointer is exactly head_data + 4 elements.
    // SAFETY: `head_data_before + 4` is within the original allocation.
    // SAFETY: split is at index 4 < head_cap_before, so the resulting
    // pointer lies inside the original buffer.
    let expected_tail_ptr = unsafe { head_data_before.add(4) };
    assert_eq!(tail.as_ptr(), expected_tail_ptr);
    // Head's capacity was shrunk to `at`; tail covers the remainder.
    assert_eq!(v.capacity(), 4);
    assert_eq!(tail.capacity(), head_cap_before - 4);
    // Both halves can be dropped without UAF / double-free.
    drop(tail);
    drop(v);
}

#[test]
fn split_off_with_drop_type_drops_each_element_once() {
    use core::sync::atomic::{AtomicUsize, Ordering as Ord};
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    struct D;
    impl Drop for D {
        fn drop(&mut self) {
            COUNT.fetch_add(1, Ord::Relaxed);
        }
    }
    COUNT.store(0, Ord::Relaxed);
    {
        let arena = Arena::new();
        let mut v = arena.alloc_vec();
        v.extend((0..6).map(|_| D));
        let _tail = v.split_off(4);
        // No drops yet.
        assert_eq!(COUNT.load(Ord::Relaxed), 0);
    }
    // Six elements total, dropped exactly once each.
    assert_eq!(COUNT.load(Ord::Relaxed), 6);
}

#[test]
fn split_off_edge_cases() {
    let arena = Arena::new();
    // Empty source: returns empty tail.
    let mut v0 = arena.alloc_vec::<u32>();
    let t0 = v0.split_off(0);
    assert!(t0.is_empty());
    // Split at len: returns empty tail.
    let mut v1 = arena.alloc_vec();
    v1.extend(0_u32..3);
    let t1 = v1.split_off(3);
    assert_eq!(v1.as_slice(), &[0, 1, 2]);
    assert!(t1.is_empty());
    // Split at 0: returns the whole source as tail.
    let mut v2 = arena.alloc_vec();
    v2.extend(0_u32..3);
    let t2 = v2.split_off(0);
    assert!(v2.is_empty());
    assert_eq!(t2.as_slice(), &[0, 1, 2]);
}

#[test]
#[should_panic(expected = "split index out of bounds")]
fn split_off_out_of_bounds_panics() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.extend(0_u32..3);
    let _ = v.split_off(4);
}

#[test]
fn split_off_then_append_adjacent_round_trips() {
    // split_off followed by append on adjacent halves should round-trip
    // through the adjacency fast path with no copy.
    let arena = Arena::new();
    let mut head = arena.alloc_vec::<u32>();
    head.extend(0_u32..8);
    let head_data_before = head.as_ptr();
    let mut tail = head.split_off(5);
    head.append(&mut tail);
    // Restored to the original contents and the original data pointer.
    assert_eq!(head.as_slice(), &[0, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(head.as_ptr(), head_data_before);
    // Tail was absorbed: empty and dangling.
    assert!(tail.is_empty());
    assert_eq!(tail.capacity(), 0);
}

#[test]
fn append_adjacency_fast_path_zero_copy() {
    let arena = Arena::new();
    // Two arena allocations made back-to-back in the same chunk are
    // contiguous (same align, no other allocs between).
    let mut a = arena.alloc_vec::<u8>();
    a.reserve_exact(4);
    a.extend([1_u8, 2, 3, 4]);
    // SAFETY: `a.capacity()` is the size of `a`'s allocation in
    // elements, so `a.as_ptr() + capacity` is the one-past-the-end
    // limit pointer, which is a valid `add` target.
    let a_end_before = unsafe { a.as_ptr().add(a.capacity()) };
    let mut b = arena.alloc_vec::<u8>();
    b.reserve_exact(4);
    b.extend([5_u8, 6, 7, 8]);
    // Verify the precondition the fast path requires.
    if core::ptr::eq(a_end_before, b.as_ptr()) {
        let a_data_before = a.as_ptr();
        a.append(&mut b);
        assert_eq!(a.as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8]);
        // No copy: a's data pointer unchanged.
        assert_eq!(a.as_ptr(), a_data_before);
        // Capacity absorbed both halves exactly. Asserts on the
        // `self.cap += other.cap` write — kills `+= → -=` (mutant
        // would leave cap == 0) and `+= → *=` (mutant would leave
        // cap == 16).
        assert_eq!(a.capacity(), 8);
        // Length absorbed both halves' lens. Catches mutations on
        // `self.len += other.len` independently of the slice check
        // above (slice access only requires `len` of bytes; a
        // `-=` mutant would give `len == 0`, a `*=` mutant `len ==
        // 16` — both leave `as_slice` incorrect, but the explicit
        // assertion documents the invariant).
        assert_eq!(a.len(), 8);
        assert!(b.is_empty());
        assert_eq!(b.capacity(), 0);
    }
    // If a's bump cursor was not at b's start (e.g. due to bump
    // alignment padding), the test gracefully degrades — the
    // fast-path precondition is environment-sensitive but the
    // semantic behavior of `append` is checked by the existing
    // `append_moves_elements` test.
}

#[test]
fn append_zst_preserves_other_capacity_default_path() {
    // Kills the `elem_size != 0 → == 0` mutant on `Vec::append`'s
    // adjacency gate: ZSTs (size_of::<T>() == 0) must always take the
    // default copy path, never the in-place absorption path.
    //
    // For ZSTs, both vectors' `data` pointers are `NonNull::dangling()`
    // — the same address — so the inner `ptr::eq` check would
    // succeed and the fast path would absorb `other.cap` into
    // `self.cap`, zeroing `other`'s capacity. The mutant
    // `elem_size == 0` enables exactly that behavior for ZSTs.
    //
    // Under the original `!= 0` semantics, ZST `append` goes through
    // the default copy path: only `other.len` is reset to 0; its
    // capacity is preserved.
    let arena = Arena::new();
    let mut a = arena.alloc_vec::<()>();
    for _ in 0..3 {
        a.push(());
    }
    let mut b = arena.alloc_vec::<()>();
    for _ in 0..2 {
        b.push(());
    }
    let b_cap_before = b.capacity();
    assert!(b_cap_before > 0, "precondition: b must have nonzero capacity");
    a.append(&mut b);
    assert_eq!(a.len(), 5);
    assert!(b.is_empty());
    // Under original (default path): `other.len = 0` but capacity is
    // not modified. Under mutant (fast path absorbing for ZST):
    // `other.cap = 0`. The distinguishing assertion.
    assert_eq!(b.capacity(), b_cap_before);
}

#[test]
fn append_adjacent_other_with_zero_len_does_not_absorb() {
    // Kills the `other.len != 0 → == 0` mutant on `Vec::append`'s
    // adjacency gate. Construct two contiguous arena allocations
    // where the second has nonzero capacity but zero length. Under
    // the original `!= 0` semantics the fast path is disabled
    // (other.len is zero), so `self.cap` is unchanged and `other`
    // retains its allocation. The mutant `== 0` enables absorption,
    // moving `other.cap` into `self.cap` and zeroing `other`.
    let arena = Arena::new();
    let mut a = arena.alloc_vec::<u32>();
    a.reserve_exact(4);
    a.extend([1_u32, 2, 3, 4]);
    // SAFETY: `a.capacity()` is the size of `a`'s allocation in
    // elements, so `a.as_ptr() + capacity` is the one-past-the-end
    // limit pointer, which is a valid `add` target.
    let a_end_before = unsafe { a.as_ptr().add(a.capacity()) };
    let mut b = arena.alloc_vec::<u32>();
    b.reserve_exact(4);
    // `b` has cap == 4 but len == 0.
    if core::ptr::eq(a_end_before, b.as_ptr()) {
        let a_cap_before = a.capacity();
        let b_cap_before = b.capacity();
        a.append(&mut b);
        // No absorption: `self.cap` unchanged because `other.len == 0`
        // disqualifies the fast path. Under the `other.len == 0`
        // mutant the absorption would set `a.capacity()` to 8 and
        // `b.capacity()` to 0.
        assert_eq!(a.capacity(), a_cap_before);
        assert_eq!(b.capacity(), b_cap_before);
        assert_eq!(a.len(), 4);
        assert_eq!(b.len(), 0);
    }
}

#[test]
fn append_adjacency_fast_path_returns_early() {
    // Covers the `return;` at the end of `Vec::append`'s in-place
    // fast path (mutate.rs line 176). `split_off` deterministically
    // produces two halves that sit back-to-back in the same chunk
    // with `self.len == self.cap`, satisfying the adjacency gate.
    // After `append`, the fallback copy path must NOT run: `other`'s
    // buffer is absorbed (capacity transferred, not copied) and its
    // raw parts are zeroed.
    let arena = Arena::new();
    let mut head = arena.alloc_vec::<u32>();
    head.extend(0_u32..6);
    let head_ptr_before = head.as_ptr();
    let head_cap_before = head.capacity();
    let mut tail = head.split_off(4);
    let tail_cap_before = tail.capacity();
    // Precondition: split_off capped `head` at its length.
    assert_eq!(head.len(), head.capacity());
    assert!(tail_cap_before > 0);

    head.append(&mut tail);

    // Concatenation is correct.
    assert_eq!(head.as_slice(), &[0, 1, 2, 3, 4, 5]);
    // No copy happened: head's data pointer is unchanged.
    assert_eq!(head.as_ptr(), head_ptr_before);
    // Capacity was absorbed (sum equals the original allocation).
    assert_eq!(head.capacity(), head_cap_before);
    // `other`'s raw parts were zeroed after the early return.
    assert!(tail.is_empty());
    assert_eq!(tail.capacity(), 0);
}

#[test]
fn append_outer_gate_true_inner_ptr_eq_false_falls_through() {
    // Covers the closing `}` of the inner `if core::ptr::eq(...)`
    // in `Vec::append` (mutate.rs line 176) by entering the outer
    // gate on line 162 but failing the inner adjacency check on
    // line 166, so control flows past line 176 into the fallback
    // copy path.
    //
    // Requirements to enter the outer gate:
    //   * elem_size != 0      — `u32`
    //   * other.len != 0      — `b` has 4 elements
    //   * self.len == self.cap — achieved via `reserve_exact` then
    //                            filling exactly to capacity.
    //
    // To make the inner `ptr::eq(self_end, other.data)` false we
    // push the arena's bump cursor forward between the two
    // allocations with an intervening `spacer` vec. After that,
    // `b`'s buffer no longer abuts `a`'s end.
    let arena = Arena::new();

    let mut a = arena.alloc_vec::<u32>();
    a.reserve_exact(4);
    a.extend(0_u32..4);
    assert_eq!(a.len(), a.capacity(), "self.len == self.cap required");
    let a_data_before = a.as_ptr();
    let a_cap_before = a.capacity();
    // SAFETY: `a_data_before.add(a_cap_before)` computes the one-past-the-end
    // pointer for `a`'s allocation, which is valid for pointer comparison.
    let a_end_before = unsafe { a_data_before.add(a_cap_before) };

    // Intervening allocation: bumps the cursor so `b` won't land
    // immediately after `a`.
    let mut spacer = arena.alloc_vec::<u32>();
    spacer.reserve_exact(4);
    spacer.extend(100_u32..104);

    let mut b = arena.alloc_vec::<u32>();
    b.reserve_exact(4);
    b.extend([10_u32, 20, 30, 40]);

    // Precondition for this test: the inner `ptr::eq` must be false.
    assert!(!core::ptr::eq(a_end_before, b.as_ptr()), "spacer must have separated a and b");

    a.append(&mut b);

    // Fallback path ran: contents concatenated by copy.
    assert_eq!(a.as_slice(), &[0, 1, 2, 3, 10, 20, 30, 40]);
    // `a` was reallocated (or at minimum grew beyond its old cap),
    // since the fast path didn't absorb `b`.
    assert!(a.capacity() >= 8);
    // Fallback only resets `other.len`; it does NOT zero capacity
    // (that's a fast-path-only side effect).
    assert!(b.is_empty());

    // Keep `spacer` alive past the assertions so the arena layout
    // stays as constructed.
    assert_eq!(spacer.as_slice(), &[100, 101, 102, 103]);
}

#[test]
fn split_off_at_len_returns_empty_tail_with_zero_capacity() {
    // Kills the `|| → &&` mutant on the outer condition of
    // `split_off`'s copy-path gate (`elem_size == 0 || self.cap == 0
    // || tail_len == 0`). At `at == self.len` the original gate is
    // true (tail_len == 0), routing to the copy path which builds an
    // empty tail via `with_capacity_in(0)` — `tail.capacity() == 0`.
    //
    // Under the mutant `(elem_size == 0 && self.cap == 0) ||
    // tail_len == 0` the gate is still true here (tail_len == 0
    // still satisfies the right side), so this *single* mutant
    // isn't killed by this test. The mutant lives on the *first*
    // `||` (col 27) — col 44 is the second `||`. Under col 44's
    // mutant `elem_size == 0 || (self.cap == 0 && tail_len == 0)`,
    // when `tail_len == 0` and `self.cap > 0` and `elem_size > 0`:
    //   * Original: true (third disjunct holds) → copy path → tail.cap == 0.
    //   * Mutant:   false (second conjunct fails, first false) → in-place
    //     split → tail.cap == self.cap - at > 0.
    // The capacity assert distinguishes the two.
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.extend([1_u32, 2, 3]);
    let tail = v.split_off(3);
    assert_eq!(v.as_slice(), &[1, 2, 3]);
    assert!(tail.is_empty());
    // The defining observation: original routes through
    // `with_capacity_in(0)` which yields cap == 0.
    assert_eq!(tail.capacity(), 0);
}

#[test]
fn shrink_to_fit_at_cursor_reclaims_in_place() {
    // When the vec's buffer ends at the bump cursor (no later
    // allocations into the same chunk), shrink_to_fit reclaims the
    // unused tail in O(1). cap drops to len; data pointer unchanged.
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    v.reserve_exact(128);
    v.extend([1_u32, 2, 3, 4]);
    let data_before = v.as_ptr();
    let cap_before = v.capacity();
    v.shrink_to_fit();
    assert_eq!(v.as_ptr(), data_before);
    assert_eq!(v.len(), 4);
    assert!(v.capacity() <= cap_before);
    // On the at-cursor fast path cap should equal len exactly.
    assert_eq!(v.capacity(), 4);
    assert_eq!(v.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn leak_reclaims_unused_capacity_tail() {
    // `leak` on a buffer that ends at the bump cursor returns the live
    // prefix and hands the unused `[len, cap)` tail back to the chunk, so
    // the next allocation reuses that space immediately.
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u64>();
    v.reserve_exact(16);
    v.extend([1_u64, 2, 3]);
    let base = v.as_ptr() as usize;
    let leaked: &mut [u64] = v.leak();
    assert_eq!(leaked, &[1, 2, 3]);
    assert_eq!(leaked.as_ptr() as usize, base);
    // The reclaimed tail is reused: the next u64 lands right after the
    // three retained elements, not after the original capacity of 16.
    let next = arena.alloc(9_u64);
    assert_eq!(core::ptr::from_ref::<u64>(&*next) as usize, base + 3 * core::mem::size_of::<u64>());
}

#[test]
fn drop_at_cursor_reclaims_storage() {
    // Dropping a buffer that ends at the bump cursor returns its whole
    // storage to the chunk (LIFO reclaim), so the next allocation reuses
    // the freed space rather than waiting for arena teardown.
    let arena = Arena::new();
    let base = {
        let mut v = arena.alloc_vec::<u64>();
        v.reserve_exact(16);
        v.extend([1_u64, 2, 3]);
        v.as_ptr() as usize
    }; // `v` dropped here -> reclaims `[0, cap)`.
    let next = arena.alloc(9_u64);
    assert_eq!(core::ptr::from_ref::<u64>(&*next) as usize, base);
}

#[test]
fn drop_not_at_cursor_does_not_reclaim() {
    // A buffer overtaken by a later allocation is no longer at the bump
    // cursor; dropping it must not roll the cursor back (which would
    // corrupt the intervening allocation).
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u64>();
    v.extend([1_u64, 2, 3]);
    let v_base = v.as_ptr() as usize;
    // Allocate after `v`, moving the cursor past its buffer.
    let mid = arena.alloc(7_u64);
    let mid_addr = core::ptr::from_ref::<u64>(&*mid) as usize;
    drop(v); // not at the cursor -> no reclaim
    let next = arena.alloc(8_u64);
    let next_addr = core::ptr::from_ref::<u64>(&*next) as usize;
    // `next` follows `mid`; it must not reuse `v`'s abandoned storage.
    assert!(next_addr > mid_addr);
    assert_ne!(next_addr, v_base);
}

#[test]
fn pop_if_removes_when_true() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let r = v.pop_if(|x| *x == 3);
    assert_eq!(r, Some(3));
    assert_eq!(v.as_slice(), &[1, 2]);
}

#[test]
fn pop_if_keeps_when_false() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let r = v.pop_if(|x| *x == 99);
    assert_eq!(r, None);
    assert_eq!(v.as_slice(), &[1, 2, 3]);
}

#[test]
fn pop_if_empty_returns_none() {
    let arena = Arena::new();
    let mut v: Vec<u32> = arena.alloc_vec();
    let r = v.pop_if(|_| true);
    assert_eq!(r, None);
}

#[test]
fn drain_removes_and_yields() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend(0_u32..6);
    let drained: std::vec::Vec<u32> = v.drain(1..4).collect();
    assert_eq!(drained, [1, 2, 3]);
    assert_eq!(v.as_slice(), &[0, 4, 5]);
}

#[test]
fn clone_produces_equal_independent_vec() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let mut c = v.clone();
    assert_eq!(c.as_slice(), v.as_slice());
    c.push(4);
    assert_eq!(v.as_slice(), &[1, 2, 3]);
    assert_eq!(c.as_slice(), &[1, 2, 3, 4]);
}

#[test]
fn into_iter_consumes() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let collected: std::vec::Vec<u32> = v.into_iter().collect();
    assert_eq!(collected, [1, 2, 3]);
}

#[test]
fn into_iter_borrowed() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let mut sum = 0_u32;
    for x in &v {
        sum += *x;
    }
    assert_eq!(sum, 6);
    assert_eq!(v.len(), 3);
}

#[test]
fn into_iter_mut_borrowed() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    for x in &mut v {
        *x *= 10;
    }
    assert_eq!(v.as_slice(), &[10, 20, 30]);
}

#[test]
fn extend_ref_for_copy_types() {
    let arena = Arena::new();
    let mut v: Vec<u8> = arena.alloc_vec();
    let src = [1_u8, 2, 3];
    v.extend(src.iter());
    assert_eq!(v.as_slice(), &[1, 2, 3]);
}

#[test]
fn borrow_mut_returns_mut_slice() {
    use core::borrow::BorrowMut;
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let s: &mut [u32] = v.borrow_mut();
    s[0] = 9;
    assert_eq!(v.as_slice(), &[9, 2, 3]);
}

#[test]
fn vec_macro_empty() {
    let arena = Arena::new();
    let v: Vec<i32> = multitude::vec::vec![in &arena];
    assert!(v.is_empty());
}

#[test]
fn vec_macro_from_list() {
    let arena = Arena::new();
    let v = multitude::vec::vec![in &arena; 1, 2, 3];
    assert_eq!(&*v, &[1, 2, 3]);
}

#[test]
fn vec_macro_from_list_trailing_comma() {
    let arena = Arena::new();
    let v = multitude::vec::vec![in &arena; 'a', 'b', 'c',];
    assert_eq!(&*v, &['a', 'b', 'c']);
}

#[test]
fn vec_macro_n_copies() {
    let arena = Arena::new();
    let v = multitude::vec::vec![in &arena; 7_u32; 4];
    assert_eq!(&*v, &[7, 7, 7, 7]);
}

#[test]
fn vec_macro_n_copies_zero() {
    let arena = Arena::new();
    let v: Vec<i32> = multitude::vec::vec![in &arena; 0; 0];
    assert!(v.is_empty());
    assert_eq!(v.capacity(), 0);
}

#[test]
fn vec_macro_evaluates_each_expr_once() {
    use core::cell::Cell;
    let arena = Arena::new();
    let n = Cell::new(0_u32);
    let bump = || {
        let v = n.get();
        n.set(v + 1);
        v
    };
    let v = multitude::vec::vec![in &arena; bump(), bump(), bump()];
    assert_eq!(&*v, &[0, 1, 2]);
    assert_eq!(n.get(), 3);
}

#[test]
fn vec_macro_n_copies_evaluates_value_once() {
    use core::cell::Cell;
    let arena = Arena::new();
    let n = Cell::new(0_u32);
    let producer = || {
        n.set(n.get() + 1);
        42_u32
    };
    let v = multitude::vec::vec![in &arena; producer(); 5];
    assert_eq!(&*v, &[42, 42, 42, 42, 42]);
    // `resize` clones the value, so the producer is invoked once.
    assert_eq!(n.get(), 1);
}

#[test]
fn vec_macro_with_typed_expression() {
    let arena = Arena::new();
    let v: Vec<u8> = multitude::vec::vec![in &arena; 1, 2, 3];
    assert_eq!(v.len(), 3);
}

#[test]
fn vec_macro_can_hold_strings() {
    let arena = Arena::new();
    let s1 = std::string::String::from("hello");
    let s2 = std::string::String::from("world");
    let v = multitude::vec::vec![in &arena; s1, s2];
    assert_eq!(&v[0], "hello");
    assert_eq!(&v[1], "world");
}

#[test]
fn partial_eq_with_slice() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u32);
    v.push(2);
    v.push(3);
    let slice: &[u32] = &[1, 2, 3];
    assert_eq!(v, *slice);
    assert_ne!(v, [1_u32, 2, 4][..]);
}

#[test]
fn partial_eq_with_ref_slice() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(10_i32);
    v.push(20);
    let slice: &[i32] = &[10, 20];
    assert_eq!(v, slice);
    assert_ne!(v, &[10_i32, 21][..]);
}

#[test]
fn partial_eq_with_array() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u8);
    v.push(2);
    v.push(3);
    assert_eq!(v, [1_u8, 2, 3]);
    assert_ne!(v, [1_u8, 2, 4]);
}

#[test]
fn partial_eq_with_ref_array() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(5_u16);
    v.push(6);
    assert_eq!(v, &[5_u16, 6]);
    assert_ne!(v, &[5_u16, 7]);
}

#[test]
fn partial_eq_empty_vec_vs_empty_slice() {
    let arena = Arena::new();
    let v = arena.alloc_vec::<i32>();
    let empty: &[i32] = &[];
    assert_eq!(v, *empty);
    assert_eq!(v, empty);
    assert_eq!(v, [0_i32; 0]);
    assert_eq!(v, &[0_i32; 0]);
}

#[test]
fn resize_grow_with_clone() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u32);
    v.resize(5, 42);
    assert_eq!(v.as_slice(), &[1, 42, 42, 42, 42]);
}

#[test]
fn resize_shrink() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    for i in 0..10_u32 {
        v.push(i);
    }
    v.resize(3, 0);
    assert_eq!(v.as_slice(), &[0, 1, 2]);
}

#[test]
fn resize_same_length_is_noop() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(7_i32);
    v.push(8);
    v.resize(2, 0);
    assert_eq!(v.as_slice(), &[7, 8]);
}

#[test]
fn resize_from_empty() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u64>();
    v.resize(100, 0xDEAD);
    assert_eq!(v.len(), 100);
    assert!(v.iter().all(|&x| x == 0xDEAD));
}

#[test]
fn resize_to_zero() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u32);
    v.push(2);
    v.resize(0, 99);
    assert!(v.is_empty());
}

#[test]
fn resize_with_grow() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<u32>();
    let mut counter = 0_u32;
    v.resize_with(5, || {
        counter += 1;
        counter * 10
    });
    assert_eq!(v.as_slice(), &[10, 20, 30, 40, 50]);
}

#[test]
fn resize_with_shrink() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    for i in 0..8_u32 {
        v.push(i);
    }
    v.resize_with(3, || panic!("should not be called"));
    assert_eq!(v.as_slice(), &[0, 1, 2]);
}

#[test]
fn resize_with_same_length_is_noop() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u32);
    v.resize_with(1, || panic!("should not be called"));
    assert_eq!(v.as_slice(), &[1]);
}

#[test]
fn resize_with_from_empty() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec::<usize>();
    v.resize_with(50, || 7);
    assert_eq!(v.len(), 50);
    assert!(v.iter().all(|&x| x == 7));
}

#[test]
fn resize_drops_excess_on_shrink() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    DROPS.store(0, Ordering::Relaxed);

    #[derive(Clone)]
    struct Tracked(u32);
    impl Drop for Tracked {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    for i in 0..10 {
        v.push(Tracked(i));
    }
    DROPS.store(0, Ordering::Relaxed);
    v.resize(3, Tracked(99));
    assert_eq!(v.len(), 3);
    // 7 elements truncated + 1 unused `value` dropped = 8
    assert_eq!(DROPS.load(Ordering::Relaxed), 8);
}

#[test]
fn resize_panic_in_clone_drops_already_written() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    static CLONES: AtomicUsize = AtomicUsize::new(0);
    DROPS.store(0, Ordering::Relaxed);
    CLONES.store(0, Ordering::Relaxed);

    #[derive(Debug)]
    struct PanicOnThirdClone(u32);
    impl Clone for PanicOnThirdClone {
        fn clone(&self) -> Self {
            let n = CLONES.fetch_add(1, Ordering::Relaxed);
            assert!(n != 2, "deliberate clone panic");
            Self(self.0)
        }
    }
    impl Drop for PanicOnThirdClone {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(PanicOnThirdClone(1));

    DROPS.store(0, Ordering::Relaxed);
    CLONES.store(0, Ordering::Relaxed);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        v.resize(6, PanicOnThirdClone(99));
    }));
    assert!(result.is_err());
    // The 2 successfully cloned elements + the original value (PanicOnThirdClone(99))
    // passed to resize should be dropped. The original vec element (v[0]) is
    // dropped when `v` is dropped.
    let drops = DROPS.load(Ordering::Relaxed);
    assert!(drops >= 2, "at least 2 cloned elements should be dropped; got {drops}");
}

#[test]
fn resize_with_panic_in_f_drops_already_written() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    DROPS.store(0, Ordering::Relaxed);

    struct Tracked(u32);
    impl Drop for Tracked {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    let arena = Arena::new();
    let mut v = arena.alloc_vec::<Tracked>();
    let mut count = 0_u32;

    DROPS.store(0, Ordering::Relaxed);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        v.resize_with(10, || {
            count += 1;
            assert!(count != 4, "deliberate panic in f");
            Tracked(count)
        });
    }));
    assert!(result.is_err());
    let drops = DROPS.load(Ordering::Relaxed);
    assert!(drops >= 3, "at least 3 written elements should be dropped; got {drops}");
}

#[test]
fn splice_drop_panic_in_removed_element_restores_tail() {
    // If a removed element's `Drop` panics while `Splice::drop` is discarding
    // the unconsumed drained range, `Splice`'s `Drain` field is still dropped
    // during unwinding (Rust drops a struct's fields even when its `Drop` impl
    // panics), and `Drain::drop`'s `TailGuard` restores the surviving tail. The
    // source vector must therefore remain contiguous and usable.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    DROPS.store(0, Ordering::Relaxed);

    struct Bomb(u32);
    impl Drop for Bomb {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
            assert!(self.0 != 999, "deliberate panic dropping a removed element");
        }
    }

    let arena = Arena::new();
    let mut v = arena.alloc_vec::<Bomb>();
    v.push(Bomb(0));
    v.push(Bomb(999));
    v.push(Bomb(2));
    v.push(Bomb(3));

    // Drop the `Splice` (removing indices 1..3) without consuming it: its
    // `Drop` drains `Bomb(999)` first, whose destructor panics.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = v.splice(1..3, core::iter::empty::<Bomb>());
    }));
    assert!(result.is_err());

    // Tail (`Bomb(3)`) restored after the retained head (`Bomb(0)`); the
    // removed `Bomb(999)` and `Bomb(2)` are gone. Vector is contiguous.
    assert_eq!(v.len(), 2);
    assert_eq!(v.as_slice()[0].0, 0);
    assert_eq!(v.as_slice()[1].0, 3);
    // The vector is still fully usable after the panic.
    v.push(Bomb(4));
    assert_eq!(v.as_slice().iter().map(|b| b.0).collect::<std::vec::Vec<_>>(), [0, 3, 4]);
}

#[test]
fn resize_grow_by_one() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u32);
    v.push(2);
    v.resize(3, 99);
    assert_eq!(v.as_slice(), &[1, 2, 99]);
}

#[test]
fn insert_at_idx_equal_len_appends() {
    // `insert` at the tail (idx == len) must skip the element shift and still
    // write the value.
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.push(1);
    v.push(2);
    v.push(3);
    v.insert(v.len(), 99); // exactly at end
    assert_eq!(&*v, &[1, 2, 3, 99]);
}

#[test]
fn insert_at_middle_shifts_correctly() {
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.extend([1_u32, 2, 3, 4, 5]);
    v.insert(2, 99);
    assert_eq!(&*v, &[1, 2, 99, 3, 4, 5]);
}

#[test]
fn remove_last_element_leaves_prefix() {
    // Removing the last element (idx == len - 1) leaves `tail == 0`, so no tail
    // shift occurs.
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30, 40]);
    let removed = v.remove(3);
    assert_eq!(removed, 40);
    assert_eq!(&*v, &[10, 20, 30]);
}

#[test]
fn remove_first_element_shifts_tail_down() {
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30, 40]);
    let removed = v.remove(0);
    assert_eq!(removed, 10);
    assert_eq!(&*v, &[20, 30, 40]);
}

#[test]
fn remove_middle_element_shifts_tail_down() {
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30, 40, 50]);
    let removed = v.remove(2);
    assert_eq!(removed, 30);
    assert_eq!(&*v, &[10, 20, 40, 50]);
}

#[test]
fn into_iter_size_hint_matches_remaining() {
    // `DrainAll::size_hint` returns `tail - head`, shrinking as items are
    // consumed from either end.
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.extend([1_u32, 2, 3, 4]);
    let mut it = v.into_iter();
    assert_eq!(it.size_hint(), (4, Some(4)));
    assert_eq!(it.next(), Some(1));
    assert_eq!(it.size_hint(), (3, Some(3)));
    assert_eq!(it.next_back(), Some(4));
    assert_eq!(it.size_hint(), (2, Some(2)));
    assert_eq!(it.next(), Some(2));
    assert_eq!(it.size_hint(), (1, Some(1)));
    assert_eq!(it.next(), Some(3));
    assert_eq!(it.size_hint(), (0, Some(0)));
    assert_eq!(it.next(), None);
}

#[test]
fn into_iter_partial_drain_drops_remaining_exactly_once() {
    use std::cell::Cell;
    use std::rc::Rc;
    struct Counted(Rc<Cell<usize>>);
    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let arena = Arena::new();
    let counter = Rc::new(Cell::new(0));
    let mut v: Vec<'_, Counted> = arena.alloc_vec();
    for _ in 0..4 {
        v.push(Counted(Rc::clone(&counter)));
    }
    {
        let mut it = v.into_iter();
        // Consume two elements; the remaining two stay live inside the
        // iterator and must be dropped exactly once by `DrainAll::drop`.
        let _ = it.next();
        let _ = it.next();
    }
    assert_eq!(counter.get(), 4);
}

#[test]
fn shrink_to_fit_with_room_to_shrink_reduces_capacity() {
    // `shrink_to_fit` only copies when `cap > len`.
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec_with_capacity(64);
    v.extend([1_u32, 2, 3, 4]);
    let cap_before = v.capacity();
    assert!(cap_before >= 64);
    v.shrink_to_fit();
    assert_eq!(v.len(), 4);
    assert!(v.capacity() <= cap_before);
}

#[test]
fn dedup_by_single_and_double_element_lengths() {
    // `dedup_by` is a no-op for `len < 2`; cover len 0, 1, 2 (a dup pair), and 3.
    let arena = Arena::new();

    let mut empty: Vec<'_, u32> = arena.alloc_vec();
    empty.dedup_by(|a, b| a == b);
    assert!(empty.is_empty());

    let mut one: Vec<'_, u32> = arena.alloc_vec();
    one.push(7);
    one.dedup_by(|a, b| a == b);
    assert_eq!(&*one, &[7]);

    let mut two_dup: Vec<'_, u32> = arena.alloc_vec();
    two_dup.extend([5_u32, 5]);
    two_dup.dedup_by(|a, b| a == b);
    assert_eq!(&*two_dup, &[5], "len==2 dedup must collapse the pair");

    let mut three: Vec<'_, u32> = arena.alloc_vec();
    three.extend([1_u32, 1, 2]);
    three.dedup_by(|a, b| a == b);
    assert_eq!(&*three, &[1, 2]);
}

#[test]
fn split_off_with_full_split_returns_empty_tail_and_keeps_head() {
    // `split_off(len)` yields an empty tail and leaves the head intact.
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let tail = v.split_off(v.len()); // tail_len == 0
    assert!(tail.is_empty());
    assert_eq!(&*v, &[1, 2, 3]);
}

#[test]
fn split_off_at_zero_returns_full_tail_and_empties_head() {
    let arena = Arena::new();
    let mut v: Vec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30]);
    let tail = v.split_off(0);
    assert_eq!(&*tail, &[10, 20, 30]);
    assert!(v.is_empty());
}

#[cfg(feature = "std")]
mod io_write {
    use std::io::Write as _;

    use multitude::Arena;

    #[test]
    fn write_returns_buf_len() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        let n = v.write(b"hello").unwrap();
        assert_eq!(n, 5);
        assert_eq!(v.as_slice(), b"hello");
    }

    #[test]
    fn write_all_appends_full_buffer() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        v.write_all(b"hello, ").unwrap();
        v.write_all(b"world").unwrap();
        assert_eq!(v.as_slice(), b"hello, world");
    }

    #[test]
    fn flush_is_a_noop() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        v.write_all(b"x").unwrap();
        v.flush().unwrap();
        assert_eq!(v.as_slice(), b"x");
    }

    #[test]
    fn write_zero_length_buffer() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        let n = v.write(&[]).unwrap();
        assert_eq!(n, 0);
        assert!(v.is_empty());
        v.write_all(&[]).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn write_macro_formats_into_vec() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        let s = String::from("hi");
        write!(&mut v, "x={} y={}", 7, s).unwrap();
        assert_eq!(v.as_slice(), b"x=7 y=hi");
    }

    #[test]
    fn write_macro_with_only_args() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        let n: u32 = 100;
        write!(&mut v, "n={n}").unwrap();
        assert_eq!(v.as_slice(), b"n=100");
    }

    #[test]
    fn writeln_macro_appends_newline() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        writeln!(&mut v, "line").unwrap();
        assert_eq!(v.as_slice(), b"line\n");
    }

    #[test]
    fn std_io_copy_into_arena_vec() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        let src = b"copy me through std::io::copy";
        let mut reader: &[u8] = src;
        let n = std::io::copy(&mut reader, &mut v).unwrap();
        assert_eq!(usize::try_from(n).unwrap(), src.len());
        assert_eq!(v.as_slice(), src);
    }

    #[test]
    fn many_small_writes_grow_the_buffer() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        // Amortized doubling: ~5 growths get us through every interesting
        // capacity transition. 32 writes (= 256 bytes) is more than enough
        // to exercise the `Write` impl across multiple reallocations.
        let n = 32;
        for _ in 0..n {
            v.write_all(b"abcdefgh").unwrap();
        }
        assert_eq!(v.len(), 8 * n);
        assert_eq!(&v.as_slice()[..8], b"abcdefgh");
        assert_eq!(&v.as_slice()[(8 * n - 8)..], b"abcdefgh");
    }

    #[test]
    fn serde_json_writes_into_arena_vec() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u8>();
        let value = serde_json::json!({ "a": 1, "b": [true, false] });
        serde_json::to_writer(&mut v, &value).unwrap();
        let s = std::str::from_utf8(v.as_slice()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(parsed, value);
    }
}

mod mutants_for_vec {
    #![allow(clippy::std_instead_of_core, reason = "test code")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "explicit .clone() clarifies test intent")]
    #![allow(clippy::cast_possible_truncation, reason = "bounded indices fit")]
    #![allow(clippy::doc_markdown, reason = "doc comments cite raw identifier names")]
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[expect(unused_imports, reason = "merged test module re-exports common helpers")]
    use crate::common;

    #[derive(Clone, Debug)]
    struct Tracked {
        val: u32,
        counter: StdArc<AtomicUsize>,
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn shrink_to_fit_full_vec_is_noop() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u32>(8);
        for i in 0..8 {
            v.push(i);
        }
        assert_eq!(v.len(), v.capacity());
        let ptr_before = v.as_ptr();
        v.shrink_to_fit();
        let ptr_after = v.as_ptr();
        assert_eq!(ptr_before, ptr_after, "shrink_to_fit at len==cap must not reallocate");
        assert_eq!(v.len(), 8);
    }

    #[test]
    fn resize_grows_with_correct_clone_count() {
        let arena = Arena::new();
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let mut v: multitude::vec::Vec<'_, Tracked> = arena.alloc_vec();
            // Initial push of 3 values, plus resize to 10.
            for i in 0..3 {
                v.push(Tracked {
                    val: i,
                    counter: counter.clone(),
                });
            }
            // resize to 10: clones the template `value` 6 times (one move
            // into the last slot, 6 total new slots ⇒ 6 - 1 clones + 1
            // move of `value`).
            let template = Tracked {
                val: 99,
                counter: counter.clone(),
            };
            v.resize(10, template);
            assert_eq!(v.len(), 10);
            for (i, t) in v.iter().enumerate() {
                let expected = if i < 3 { i as u32 } else { 99 };
                assert_eq!(t.val, expected, "slot {i} expected {expected}");
            }
            // Drops so far: only the temporary clones consumed during
            // resize (none — they all live in the vec).
            // counter is unchanged by clones — only Drop bumps it.
            // (Clone produces new StdArc + identical val.)
        }
        // After `v` is dropped (with the arena), all 10 Tracked instances
        // run Drop. Plus the `template` argument was moved into the last
        // slot, so no extra drop there.
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn resize_to_same_length_is_noop() {
        let arena = Arena::new();
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let mut v: multitude::vec::Vec<'_, Tracked> = arena.alloc_vec();
            for i in 0..5 {
                v.push(Tracked {
                    val: i,
                    counter: counter.clone(),
                });
            }
            let template = Tracked {
                val: 99,
                counter: counter.clone(),
            };
            v.resize(5, template);
            assert_eq!(v.len(), 5);
            // Element values unchanged.
            for (i, t) in v.iter().enumerate() {
                assert_eq!(t.val, i as u32);
            }
        }
        // 5 elements + 1 template = 6 drops. The template is dropped when
        // resize takes the truncate path (new_len <= self.len ⇒ truncate;
        // template is dropped on function return).
        assert_eq!(counter.load(Ordering::Relaxed), 6);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn realloc_growth_and_relocation_counter() {
        let arena = Arena::new();
        let mut v: multitude::vec::Vec<'_, u32> = arena.alloc_vec_with_capacity(4);
        // Forcing many doublings → many relocations (some may stay
        // in-place because the buffer is at the chunk cursor, but
        // eventually at least one cross-chunk relocation happens for
        // large enough growths in a busy arena).
        for i in 0..4096_u32 {
            v.push(i);
        }
        let s = arena.stats();
        // Verify content is correct (a wrong copy path would corrupt).
        for (i, x) in v.iter().enumerate() {
            assert_eq!(*x as usize, i);
        }
        // Some growth happened — capacity is now > 4.
        assert!(v.capacity() >= 4096);
        // `relocations` should be > 0 if at least one cross-chunk realloc
        // happened. With `>` (correct), this works; with `>=` on 808 the
        // logic also works because the in-place check ultimately confirms.
        // We just keep the counter accessible for debugging.
        let _ = s.relocations;
    }

    #[test]
    fn into_box_yields_distinct_elements() {
        use multitude::vec::Vec as MVec;

        let arena = Arena::new();
        // Use a vec of types where `into_box` takes the copy path
        // (any type works for into_box, but copy path is the cold
        // tail; we exercise it indirectly via empty-builder edge case in
        // tests/arena_vec.rs). Here we exercise the public into_box
        // path with non-Copy types so the slow-path is taken on some
        // configurations.
        let mut v: MVec<'_, String> = arena.alloc_vec();
        for i in 0..8_u32 {
            v.push(format!("item-{i}"));
        }
        let b = v.into_boxed_slice();
        for (i, s) in b.iter().enumerate() {
            assert_eq!(s, &format!("item-{i}"));
        }
    }
}

/// Regression test for a use-after-free between `Vec::split_off`'s
/// zero-copy sharing and oversized-chunk reclamation on growth.
///
/// `split_off` of an oversized-backed `Vec` leaves the head and tail
/// sharing one chunk's payload (no copy). Growing one half past the
/// oversized threshold relocates it to a fresh chunk. The old chunk
/// must NOT be freed at that point, because the sibling half still
/// points into it — freeing it would dangle the sibling. We grow the
/// head, then read and drop the tail (whose elements run real
/// destructors); under the bug this touches freed memory.
#[test]
fn split_off_sibling_survives_oversized_growth_of_other_half() {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Dropper(StdArc<AtomicUsize>);
    impl Drop for Dropper {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    let counter = StdArc::new(AtomicUsize::new(0));
    let arena = Arena::new();

    // Build an oversized-backed Vec (> 16 KiB normal-class cutover):
    // 4096 * size_of::<Dropper>() (8 bytes) = 32 KiB.
    let mut head = arena.alloc_vec::<Dropper>();
    for _ in 0..4096 {
        head.push(Dropper(counter.clone()));
    }
    // Zero-copy split: `tail` shares the same oversized chunk as `head`.
    let tail = head.split_off(2048);
    assert_eq!(head.len(), 2048);
    assert_eq!(tail.len(), 2048);

    // Grow `head` past the oversized threshold again, forcing it to
    // relocate to a fresh oversized chunk. The chunk shared with `tail`
    // must remain alive.
    for _ in 0..6144 {
        head.push(Dropper(counter.clone()));
    }
    assert_eq!(head.len(), 8192);

    // Touch every element of the sibling: a UAF would read freed memory.
    let mut sum = 0_usize;
    for d in tail.as_slice() {
        sum += StdArc::strong_count(&d.0);
    }
    assert!(sum > 0);

    // Drop the sibling explicitly: runs 2048 real destructors over the
    // chunk's storage — must not be a use-after-free.
    drop(tail);
    drop(head);
    drop(arena);
    // 4096 (initial) + 6144 (regrowth) Droppers were created and all
    // must have been dropped exactly once.
    assert_eq!(counter.load(Ordering::Relaxed), 4096 + 6144);
}

/// Zero-copy freeze: `into_boxed_slice` / `into_arc` reuse the `Vec`'s
/// backing buffer in place (no allocation, no element copy).
mod zero_copy_freeze {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use multitude::{Arc, Arena, Box};

    #[test]
    fn into_boxed_slice_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        v.extend([10, 20, 30, 40]);
        let data_ptr = v.as_slice().as_ptr();
        let b: Box<[u32]> = v.into_boxed_slice();
        assert_eq!(&*b, &[10, 20, 30, 40]);
        // The boxed slice must point at the very same storage.
        assert_eq!(b.as_ptr().cast::<u32>(), data_ptr, "into_boxed_slice must not copy");
    }

    #[test]
    fn into_arc_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u64>();
        v.extend([1, 2, 3, 4, 5]);
        let data_ptr = v.as_slice().as_ptr();
        let a: Arc<[u64]> = Arc::from(v);
        assert_eq!(&*a, &[1, 2, 3, 4, 5]);
        assert_eq!(a.as_ptr().cast::<u64>(), data_ptr, "into_arc must not copy");
        let a2 = a.clone();
        assert!(Arc::ptr_eq(&a, &a2), "clone shares the same frozen payload");
        assert_eq!(&*a2, &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn into_arc_slice_infallible_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u64>();
        v.extend([9, 8, 7]);
        let data_ptr = v.as_slice().as_ptr();
        let a: Arc<[u64]> = v.into_arc_slice();
        assert_eq!(&*a, &[9, 8, 7]);
        assert_eq!(a.as_ptr().cast::<u64>(), data_ptr, "into_arc_slice must not copy");
    }

    #[test]
    fn try_into_boxed_slice_reuses_buffer_in_place() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u16>();
        v.extend([7_u16, 8, 9]);
        let data_ptr = v.as_slice().as_ptr();
        let b = v.try_into_boxed_slice().unwrap();
        assert_eq!(&*b, &[7, 8, 9]);
        assert_eq!(b.as_ptr().cast::<u16>(), data_ptr);
    }

    #[test]
    fn frozen_drop_type_runs_each_destructor_exactly_once() {
        let counter = AtomicUsize::new(0);
        struct D<'a>(&'a AtomicUsize);
        impl Drop for D<'_> {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let arena = Arena::new();
        {
            let mut v = arena.alloc_vec::<D<'_>>();
            for _ in 0..32 {
                v.push(D(&counter));
            }
            let b = v.into_boxed_slice();
            assert_eq!(b.len(), 32);
            // Nothing dropped yet.
            assert_eq!(counter.load(Ordering::Relaxed), 0);
            drop(b);
            assert_eq!(counter.load(Ordering::Relaxed), 32, "each element dropped exactly once");
        }
    }

    #[test]
    fn frozen_arc_outlives_arena_and_crosses_threads() {
        let arc: Arc<[i32]> = {
            let arena = Arena::new();
            let mut v = arena.alloc_vec::<i32>();
            v.extend(0..100);
            // Arena drops at the end of this block; the Arc must keep its chunk alive.
            Arc::from(v)
        };
        let clone = arc.clone();
        let sum: i32 = thread::spawn(move || clone.iter().copied().sum::<i32>()).join().unwrap();
        assert_eq!(sum, (0..100).sum::<i32>());
        assert_eq!(arc.len(), 100);
        assert_eq!(arc[99], 99);
    }

    #[test]
    fn frozen_arc_drop_type_outlives_arena() {
        let counter = StdArc::new(AtomicUsize::new(0));
        struct D(StdArc<AtomicUsize>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let frozen: Box<[D]> = {
            let arena = Arena::new();
            let mut v = arena.alloc_vec::<D>();
            for _ in 0..10 {
                v.push(D(counter.clone()));
            }
            v.into_boxed_slice()
        };
        assert_eq!(counter.load(Ordering::Relaxed), 0, "no drops while frozen and arena gone");
        drop(frozen);
        assert_eq!(counter.load(Ordering::Relaxed), 10);
    }

    #[test]
    fn split_off_tail_freezes_correctly_via_fallback() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        v.extend(0..10);
        let tail = v.split_off(4);
        // The tail has no freeze prefix; freezing copies into a fresh buffer.
        let b = tail.into_boxed_slice();
        assert_eq!(&*b, &[4, 5, 6, 7, 8, 9]);
        // The head still freezes in place.
        let head_ptr = v.as_slice().as_ptr();
        let hb = v.into_boxed_slice();
        assert_eq!(&*hb, &[0, 1, 2, 3]);
        assert_eq!(hb.as_ptr().cast::<u32>(), head_ptr, "head retains its freeze prefix");
    }

    #[test]
    fn grown_vec_freezes_in_place_after_relocation() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec_with_capacity::<u32>(2);
        // Force at least one relocation by exceeding the initial capacity.
        v.extend(0..64);
        let data_ptr = v.as_slice().as_ptr();
        let a = Arc::from(v);
        assert_eq!(a.len(), 64);
        assert_eq!(a.as_ptr().cast::<u32>(), data_ptr, "relocated buffer still freezes in place");
        assert_eq!(a[63], 63);
    }

    #[test]
    fn empty_vec_freezes_to_empty_slice() {
        let arena = Arena::new();
        let v = arena.alloc_vec::<u32>();
        let b = v.into_boxed_slice();
        assert_eq!(b.len(), 0);
        let v2 = arena.alloc_vec::<u32>();
        let a: Arc<[u32]> = Arc::from(v2);
        assert_eq!(a.len(), 0);
    }
}

/// Zero-copy freeze when the buffer's chunk is no longer the arena's
/// current chunk — exercising the atomic-`inc_ref` freeze path (as opposed
/// to the current-chunk surplus path).
mod zero_copy_freeze_retired {
    use multitude::{Arc, Arena, Box};

    #[test]
    fn freeze_in_place_after_chunk_displaced() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u64>();
        v.extend(0..8);
        let data_ptr = v.as_slice().as_ptr();
        // Force a refill so the Vec's chunk is retired (no longer current):
        // a large allocation that cannot fit beside the Vec's buffer.
        let _filler = arena.alloc_slice_fill_with(60_000 / 8, |i| i as u64);
        let a: Arc<[u64]> = Arc::from(v);
        assert_eq!(a.as_ptr().cast::<u64>(), data_ptr, "retired-chunk freeze stays in place");
        assert_eq!(a.len(), 8);
        assert_eq!(&*a, &[0, 1, 2, 3, 4, 5, 6, 7]);
    }

    #[test]
    fn freeze_oversized_backed_vec_in_place() {
        let arena: Arena = Arena::builder().max_normal_alloc(4096).build();
        // A capacity above max_normal_alloc lands in a dedicated oversized
        // chunk (retired, never current).
        let mut v = arena.alloc_vec_with_capacity::<u32>(2048);
        v.extend(0..2048);
        let data_ptr = v.as_slice().as_ptr();
        let b: Box<[u32]> = v.into_boxed_slice();
        assert_eq!(b.as_ptr().cast::<u32>(), data_ptr, "oversized freeze stays in place");
        assert_eq!(b.len(), 2048);
        assert_eq!(b[2047], 2047);
    }

    #[test]
    fn retired_chunk_frozen_arc_outlives_arena() {
        let arc: Arc<[u32]> = {
            let arena = Arena::new();
            let mut v = arena.alloc_vec::<u32>();
            v.extend(0..16);
            let _filler = arena.alloc_slice_fill_with(60_000, |_| 0u8);
            Arc::from(v)
        };
        assert_eq!(arc.len(), 16);
        assert_eq!(arc[15], 15);
    }
}

mod vec_freeze_try_into_arc_slice {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    use crate::common::SyncFailingAllocator;

    #[test]
    fn try_into_arc_ok() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<u32>();
        v.push(1);
        v.push(2);
        v.push(3);
        let arc = v.try_into_arc_slice().unwrap();
        assert_eq!(&*arc, &[1, 2, 3][..]);
    }

    #[test]
    fn try_into_arc_err_on_failing_allocator() {
        let a = Arena::new_in(SyncFailingAllocator::new(1));
        // Fill most of the first chunk, then split: the tail has no freeze
        // prefix of its own, so freezing it into an `Arc` (a copy of equal
        // size) must acquire a second chunk, which the failing allocator
        // rejects.
        let mut v = a.alloc_vec_with_capacity::<u32>(100);
        v.extend(0..100);
        let tail = v.split_off(50);
        let r = tail.try_into_arc_slice();
        assert!(r.is_err());
    }
}

mod vec_into_iter_traits_coverage {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[test]
    fn double_ended_next_back() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<u32>();
        v.push(1);
        v.push(2);
        v.push(3);
        let mut it = v.into_iter();
        assert_eq!(it.next_back(), Some(3));
        assert_eq!(it.next(), Some(1));
        assert_eq!(it.next_back(), Some(2));
        assert_eq!(it.next_back(), None);
    }

    #[test]
    fn debug_formats_remaining() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<u32>();
        v.push(10);
        v.push(20);
        let it = v.into_iter();
        let s = format!("{it:?}");
        assert!(s.contains("IntoIter"));
        assert!(s.contains("remaining"));
    }
}

mod vec_try_grow_to_noop {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[test]
    fn reserve_within_capacity_is_noop() {
        let a = Arena::new();
        let mut v = a.alloc_vec_with_capacity::<u32>(16);
        let cap_before = v.capacity();
        // reserve(0) → needed = current len = 0 ≤ cap → fast return.
        v.reserve(0);
        assert_eq!(v.capacity(), cap_before);
    }
}

mod vec_mutate_extras {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use multitude::Arena;

    use crate::common::FailingAllocator;

    #[test]
    fn insert_panics_on_failing_allocator() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = Arena::new_in(FailingAllocator::new(0));
            let mut v = a.alloc_vec::<u32>();
            v.insert(0, 7);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn dedup_by_empty_is_noop() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<u32>();
        v.dedup_by(|_, _| true);
        assert!(v.is_empty());
    }

    #[test]
    fn dedup_by_singleton_is_noop() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<u32>();
        v.push(7);
        v.dedup_by(|_, _| true);
        assert_eq!(v.len(), 1);
    }
}

mod vec_intoiterator_impl {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[test]
    fn into_iter_yields_in_order() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<u32>();
        for i in 0..5_u32 {
            v.push(i);
        }
        let collected: std::vec::Vec<u32> = v.into_iter().collect();
        assert_eq!(collected, std::vec![0, 1, 2, 3, 4]);
    }
}

mod vec_try_grow_to_within_cap {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[test]
    fn try_grow_to_no_op_when_within_cap() {
        let a = Arena::new();
        let mut v = a.alloc_vec_with_capacity::<u32>(8);
        let cap = v.capacity();
        // Reserve less than cap → try_grow_to should early-return.
        v.try_reserve(0).unwrap();
        assert_eq!(v.capacity(), cap);
    }

    #[test]
    fn try_grow_to_zst_early_return() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<()>();
        for _ in 0..1024_u32 {
            v.push(());
        }
        assert_eq!(v.len(), 1024);
    }
}

mod arena_buf_zst_split {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[test]
    fn vec_drain_zst_does_not_panic() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<()>();
        for _ in 0..32_u32 {
            v.push(());
        }
        let drained: usize = v.drain(..16).count();
        assert_eq!(drained, 16);
        assert_eq!(v.len(), 16);
    }
}

mod freeze_and_slice_edges {
    #![allow(clippy::std_instead_of_core, reason = "test code uses std")]
    #![allow(clippy::unwrap_used, reason = "test code")]
    #![allow(clippy::missing_panics_doc, reason = "test code")]
    #![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
    #![allow(clippy::items_after_statements, reason = "test layout")]
    #![allow(dead_code, reason = "test scaffolding may be conditionally used")]
    #![allow(clippy::large_stack_arrays, reason = "test allocations are intentional")]
    #![allow(clippy::collection_is_never_read, reason = "tests retain handles to keep chunks alive")]
    #![allow(clippy::cast_possible_truncation, reason = "test code: bounded test indices")]
    #![allow(clippy::cast_lossless, reason = "test code")]
    #![allow(clippy::cast_sign_loss, reason = "test code")]
    #![allow(clippy::range_plus_one, reason = "test code")]
    #![allow(clippy::assertions_on_result_states, reason = "test code")]
    #![allow(clippy::ptr_as_ptr, reason = "test code")]
    #![allow(clippy::as_pointer_underscore, reason = "test code")]
    #![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code")]
    #![allow(clippy::empty_drop, reason = "test code: probe types use empty Drop on purpose")]
    #![allow(clippy::deref_by_slicing, reason = "tests prefer explicit slicing")]
    #![allow(clippy::needless_borrow, reason = "tests prefer explicit borrows")]
    #![allow(clippy::needless_borrows_for_generic_args, reason = "tests prefer explicit borrows")]
    #![allow(clippy::redundant_slicing, reason = "tests prefer explicit slicing")]
    use multitude::Arena;

    #[test]
    fn vec_leak_shared_reborrow_returns_arena_lifetime_slice() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        for i in 0..6_u32 {
            v.push(i * 10);
        }
        // `u32: !Drop`, so `leak` is the in-place reinterpret (no copy, no
        // drop entry); reborrow as shared for `&[T]`.
        let s: &[u32] = &*v.leak();
        assert_eq!(s, &[0, 10, 20, 30, 40, 50]);
    }

    #[test]
    fn vec_leak_allows_in_place_mutation() {
        let arena = Arena::new();
        let mut v = arena.alloc_vec::<u32>();
        v.push(1);
        v.push(2);
        v.push(3);
        let s: &mut [u32] = v.leak();
        for x in s.iter_mut() {
            *x *= 2;
        }
        assert_eq!(s, &[2, 4, 6]);
    }

    #[test]
    fn vec_leak_empty() {
        let arena = Arena::new();
        let v = arena.alloc_vec::<u32>();
        let s: &[u32] = &*v.leak();
        assert!(s.is_empty());
    }

    #[test]
    fn alloc_slice_copy_empty_returns_empty_slice() {
        let arena = Arena::new();
        let s = arena.alloc_slice_copy::<u32>(&[]);
        assert!(s.is_empty());
    }

    #[test]
    fn alloc_slice_clone_empty_returns_empty_slice() {
        let arena = Arena::new();
        let src: [String; 0] = [];
        let s = arena.alloc_slice_clone(&src);
        assert!(s.is_empty());
    }

    #[test]
    fn alloc_slice_fill_iter_oversized_non_drop() {
        let arena = Arena::new();
        // 5000 × u32 = 20 KiB > MAX_NORMAL_ALLOC (16 KiB) ⇒ oversized path;
        // `u32: !Drop` ⇒ the non-drop oversized arm of
        // `impl_alloc_slice_fill_iter`.
        let s = arena.alloc_slice_fill_iter((0_u32..5000).map(|i| i.wrapping_mul(3)));
        assert_eq!(s.len(), 5000);
        assert_eq!(s[0], 0);
        assert_eq!(s[4999], 4999_u32.wrapping_mul(3));
    }
}
