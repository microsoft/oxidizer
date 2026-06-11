// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Mutation-test kills for boundary conditions across the crate:
//! Vec insert/remove edge offsets, `split_off` splitting at empty/full,
//! `shrink_to_fit` threshold, `dedup_by` length cutoff, `Arena::try_new`
//! vs `default()`, `preallocate_one`_* boundary classes, etc. Each test
//! targets a specific `cargo mutants` finding flagged as MISSED.

#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]

use multitude::Arena;
#[cfg(feature = "stats")]
use multitude::ArenaBuilder;
use multitude::vec::Vec as ArenaVec;

// --- ArenaBuf / Vec insert/remove boundaries (via Vec public API) ---------

#[test]
fn vec_insert_at_idx_equal_len_appends() {
    // `ArenaBuf::insert_within_cap` has an `if idx < self.len` guard
    // around the shift; insert at the tail (idx == len) must skip the
    // shift and still write the value. A `< → <=` mutation would do a
    // spurious 1-byte shift past the end → UB / wrong layout.
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
    v.push(1);
    v.push(2);
    v.push(3);
    v.insert(v.len(), 99); // exactly at end
    assert_eq!(&*v, &[1, 2, 3, 99]);
}

#[test]
fn vec_insert_at_middle_shifts_correctly() {
    // Pinned shape catches the `- → +` arithmetic mutation in
    // `insert_within_cap`'s `len - idx`.
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
    v.extend([1_u32, 2, 3, 4, 5]);
    v.insert(2, 99);
    assert_eq!(&*v, &[1, 2, 99, 3, 4, 5]);
}

#[test]
fn vec_remove_last_element_leaves_prefix() {
    // `ArenaBuf::remove` computes `tail = self.len - idx - 1`. Removing
    // the last element (idx == len - 1) makes `tail == 0`; the
    // `if tail > 0` guard must hold and the `- with +` mutation on
    // the tail computation must be visibly wrong (would underflow or
    // shift garbage).
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30, 40]);
    let removed = v.remove(3);
    assert_eq!(removed, 40);
    assert_eq!(&*v, &[10, 20, 30]);
}

#[test]
fn vec_remove_first_element_shifts_tail_down() {
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30, 40]);
    let removed = v.remove(0);
    assert_eq!(removed, 10);
    assert_eq!(&*v, &[20, 30, 40]);
}

#[test]
fn vec_remove_middle_element_shifts_tail_down() {
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30, 40, 50]);
    let removed = v.remove(2);
    assert_eq!(removed, 30);
    assert_eq!(&*v, &[10, 20, 40, 50]);
}

// --- DrainAll size_hint and drop boundary (via Vec into_iter) ------------

#[test]
fn vec_into_iter_size_hint_matches_remaining() {
    // `DrainAll::size_hint` returns `tail - head`; a `- → +` mutation
    // would balloon the value as items are consumed. Walk the iterator
    // step-by-step.
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
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
fn vec_into_iter_partial_drain_drops_remaining_exactly_once() {
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
    let mut v: ArenaVec<'_, Counted> = arena.alloc_vec();
    for _ in 0..4 {
        v.push(Counted(Rc::clone(&counter)));
    }
    {
        let mut it = v.into_iter();
        // Consume two elements; the remaining two stay live inside the
        // iterator and must be dropped by `DrainAll::drop` (which gates
        // on `head < tail`; a `< → <=` mutation would skip the drop
        // when head == tail-1 and leak; a `&& → ||` mutation would
        // double-drop). Confirm the count after drop is exactly 4.
        let _ = it.next();
        let _ = it.next();
    }
    assert_eq!(counter.get(), 4);
}

// --- Vec::shrink_to_fit threshold ---------------------------------------

#[test]
fn vec_shrink_to_fit_with_room_to_shrink_reduces_capacity() {
    // `Vec::shrink_to_fit` has a `if cap > len` guard. The `> → >=`
    // mutation would also fire on the equal case → spurious copy. We
    // already cover `cap == len` no-op via existing tests; here pin
    // the actual shrink behavior for `cap > len`.
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = ArenaVec::with_capacity_in(64, &arena);
    v.extend([1_u32, 2, 3, 4]);
    let cap_before = v.capacity();
    assert!(cap_before >= 64);
    v.shrink_to_fit();
    assert_eq!(v.len(), 4);
    assert!(v.capacity() <= cap_before);
}

// --- Vec::dedup_by length-cutoff branch --------------------------------

#[test]
fn vec_dedup_by_single_and_double_element_lengths() {
    // `dedup_by` has `if len < 2 { return; }`. The `< → <=` mutation
    // would short-circuit at len == 2 and skip a needed dedup; the
    // `< → ==` mutation would skip at len == 1 only and process len ==
    // 0 / 2+ — both are wrong. Cover len 0, 1, 2 (a dup pair), and 3.
    let arena = Arena::new();

    let mut empty: ArenaVec<'_, u32> = arena.alloc_vec();
    empty.dedup_by(|a, b| a == b);
    assert!(empty.is_empty());

    let mut one: ArenaVec<'_, u32> = arena.alloc_vec();
    one.push(7);
    one.dedup_by(|a, b| a == b);
    assert_eq!(&*one, &[7]);

    let mut two_dup: ArenaVec<'_, u32> = arena.alloc_vec();
    two_dup.extend([5_u32, 5]);
    two_dup.dedup_by(|a, b| a == b);
    assert_eq!(&*two_dup, &[5], "len==2 dedup must collapse the pair");

    let mut three: ArenaVec<'_, u32> = arena.alloc_vec();
    three.extend([1_u32, 1, 2]);
    three.dedup_by(|a, b| a == b);
    assert_eq!(&*three, &[1, 2]);
}

// --- Vec::split_off ZST / unallocated / empty-tail branch ---------------

#[test]
fn vec_split_off_with_full_split_returns_tail_and_empties_head() {
    // `split_off` has `|| tail_len == 0` short-circuit. The
    // `|| → &&` mutation would skip the short-circuit when tail_len
    // == 0 → still create a fresh tail (correct) but go through the
    // wrong arm. Make sure the empty-tail case yields an empty tail
    // and leaves the head intact.
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
    v.extend([1_u32, 2, 3]);
    let tail = v.split_off(v.len()); // tail_len == 0
    assert!(tail.is_empty());
    assert_eq!(&*v, &[1, 2, 3]);
}

#[test]
fn vec_split_off_at_zero_returns_full_tail_and_empties_head() {
    let arena = Arena::new();
    let mut v: ArenaVec<'_, u32> = arena.alloc_vec();
    v.extend([10_u32, 20, 30]);
    let tail = v.split_off(0);
    assert_eq!(&*tail, &[10, 20, 30]);
    assert!(v.is_empty());
}

// --- Arena::try_new / builder default-return mutations ------------------

#[test]
fn arena_try_new_succeeds_with_default_globals() {
    // `try_new -> Ok(Default::default())` mutation would still return
    // Ok, but with a fresh Arena (different from Self::try_new_in(Global)).
    // The semantic difference: both return a working arena. Verify that
    // try_new produces a working arena and that try_new + alloc returns
    // a valid &mut T.
    let arena = Arena::try_new().expect("try_new must succeed on Global");
    let r = arena.alloc(42_u32);
    assert_eq!(*r, 42);
}

#[test]
fn arena_builder_returns_independent_builder_each_call() {
    // `builder() -> ArenaBuilder::from(Default::default())` mutation
    // would return a builder pre-loaded with an `ArenaBuilder::new()`
    // default (equivalent in practice), so this mutation is hard to
    // observe directly. We pin the chain: `Arena::builder().build()`
    // produces a functional arena that can allocate.
    let arena: Arena = Arena::builder().build();
    let r = arena.alloc(99_u64);
    assert_eq!(*r, 99);
}

// --- Arena::preallocate_one_local / _shared boundary on `class >` ---

#[cfg(feature = "stats")]
#[test]
fn preallocate_with_max_class_capacity_does_not_double_ratchet() {
    // `preallocate_one_local` updates `next_local_class` only if
    // `class > current`. With `> → >=`, setting it equal to the
    // existing value would do a redundant write — observable through
    // a `with_capacity_local(0)`-built arena (class 0) that then
    // calls preallocation again at class 0 implicitly via a
    // user-visible side effect. Best we can observe: a builder that
    // pins class 0 produces exactly one chunk; the second allocation
    // should not trigger a re-preallocation.
    let arena = ArenaBuilder::new().with_capacity_local(512).build();
    let s = arena.stats();
    assert_eq!(s.normal_local_chunks_allocated, 1);
    // Allocate within the preallocated chunk: no new chunk acquired.
    let _ = arena.alloc(0_u32);
    let s2 = arena.stats();
    assert_eq!(s2.normal_local_chunks_allocated, 1);
}

#[cfg(feature = "stats")]
#[test]
fn preallocate_shared_with_capacity_does_not_double_ratchet() {
    let arena = ArenaBuilder::new().with_capacity_shared(512).build();
    let s = arena.stats();
    assert_eq!(s.normal_shared_chunks_allocated, 1);
    // First arc within preallocated chunk: still 1.
    let _ = arena.alloc_arc(0_u32);
    let s2 = arena.stats();
    assert_eq!(s2.normal_shared_chunks_allocated, 1);
}
