// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for [`Box`]: owned, mutable single smart pointer whose `Drop`
//! runs `T::drop` immediately when the smart pointer is dropped.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::collection_is_never_read, reason = "tests retain smart pointers to keep chunks alive")]

mod common;

use core::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use multitude::{Arena, Box, Rc};
#[test]
fn alloc_box_runs_drop_immediately() {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    struct Counter;
    impl Drop for Counter {
        fn drop(&mut self) {
            let _ = COUNT.fetch_add(1, AtomicOrdering::SeqCst);
        }
    }

    COUNT.store(0, AtomicOrdering::SeqCst);
    let arena = Arena::new();
    let b = arena.alloc_box(Counter);
    assert_eq!(COUNT.load(AtomicOrdering::SeqCst), 0);
    drop(b);
    assert_eq!(COUNT.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn alloc_box_mutable_access() {
    let arena = Arena::new();
    let mut b = arena.alloc_box(vec![1, 2, 3]);
    b.push(4);
    assert_eq!(*b, vec![1, 2, 3, 4]);
}

#[test]
fn alloc_box_with_copy_type_no_panic() {
    // Regression: ArenaBox<T: Copy> originally tried to unlink a non-existent
    // DropEntry. The bug would panic on the *first* alloc; any modest N
    // proves the fix.
    let arena = Arena::new();
    let mut handles = std::vec::Vec::new();
    let n: u64 = 256;
    for i in 0..n {
        handles.push(arena.alloc_box(i));
    }
    let sum: u64 = handles.iter().map(|h| **h).sum();
    drop(handles);
    drop(arena);
    assert_eq!(sum, (0..n).sum());
}

#[test]
fn try_alloc_box_succeeds() {
    let arena = Arena::new();
    let mut b = arena.try_alloc_box(vec![1_u32, 2, 3]).unwrap();
    b.push(4);
    assert_eq!(&*b, &[1, 2, 3, 4]);
}

#[test]
fn alloc_box_with_constructs_in_place() {
    let arena = Arena::new();
    let b = arena.alloc_box_with(|| std::string::String::from("placed-box"));
    assert_eq!(&**b, "placed-box");
}

#[test]
fn try_alloc_box_with_succeeds() {
    let arena = Arena::new();
    let b = arena.try_alloc_box_with(|| 42_u64).unwrap();
    assert_eq!(*b, 42);
}

// Regression test for the entry-alignment bug discovered during the
// safety audit (2026-04-26): when `align_of::<T>() > align_of::<DropEntry>() = 8`,
// the alloc path placed the `DropEntry` at an 8-aligned position that was
// NOT `align_of::<T>()`-aligned. The reverse formula in `ArenaBox::Drop`
// (which only knows the value's alignment) then computed a wrong entry
// address, causing `unlink_drop_entry` to corrupt the chunk's drop list.
//
// Fix: alloc now over-aligns the entry slot to
// `max(align_of::<DropEntry>(), align_of::<T>())`, so the reverse formula
// matches the layout regardless of `T`'s alignment.
#[test]
fn alloc_box_high_alignment_drop_locates_entry_correctly() {
    #[repr(align(16))]
    struct Aligned16 {
        _s: String,
    }
    #[repr(align(32))]
    struct Aligned32 {
        _s: String,
    }
    #[repr(align(64))]
    struct Aligned64 {
        _s: String,
    }

    // Force the bump cursor to a non-`align_of::<T>()`-aligned position
    // by allocating a `u8` first, then verify multiple high-alignment
    // ArenaBox allocations (and their drops) work.
    let arena = Arena::new();
    let _decoy = arena.alloc(0_u8);
    let b16_1 = arena.alloc_box(Aligned16 { _s: "a".to_string() });
    let b16_2 = arena.alloc_box(Aligned16 { _s: "b".to_string() });
    let b32 = arena.alloc_box(Aligned32 { _s: "c".to_string() });
    let b64 = arena.alloc_box(Aligned64 { _s: "d".to_string() });

    // Each value must actually be at its required alignment.
    assert_eq!(core::ptr::from_ref::<Aligned16>(&*b16_1) as usize % 16, 0);
    assert_eq!(core::ptr::from_ref::<Aligned16>(&*b16_2) as usize % 16, 0);
    assert_eq!(core::ptr::from_ref::<Aligned32>(&*b32) as usize % 32, 0);
    assert_eq!(core::ptr::from_ref::<Aligned64>(&*b64) as usize % 64, 0);

    // Drops must locate the right DropEntry slots — without the fix,
    // this corrupts the chunk's drop list and produces a heap fault on
    // chunk teardown (or earlier under heavy heap-checking).
    drop(b16_1);
    drop(b16_2);
    drop(b32);
    drop(b64);
}

// Trait impls (Debug, Display, PartialEq, Eq, PartialOrd, Ord, Hash)
// and as_ptr / as_mut_ptr

#[test]
fn arena_box_debug_display() {
    let arena = Arena::new();
    let b = arena.alloc_box(42_u32);
    assert_eq!(format!("{b:?}"), "42");
    assert_eq!(format!("{b}"), "42");
}

#[test]
fn arena_box_eq_and_ord() {
    use core::cmp::Ordering;
    let arena = Arena::new();
    let a = arena.alloc_box(1_u32);
    let b = arena.alloc_box(1_u32);
    let c = arena.alloc_box(2_u32);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert!(a < c);
    assert_eq!(a.cmp(&c), Ordering::Less);
    assert_eq!(a.partial_cmp(&c), Some(Ordering::Less));
}

#[test]
fn arena_box_hash_via_hashmap() {
    use std::collections::HashMap;
    use std::hash::{BuildHasher, BuildHasherDefault, Hasher};

    let arena = Arena::new();
    let k = arena.alloc_box(7_u32);

    // Folded mutants_extras::box_hash_forwards_to_inner.
    let bh = BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default();
    let mut h_box = bh.build_hasher();
    std::hash::Hash::hash(&k, &mut h_box);
    let box_hash = h_box.finish();
    let mut h_inner = bh.build_hasher();
    std::hash::Hash::hash(&7_u32, &mut h_inner);
    let inner_hash = h_inner.finish();
    let h_empty = bh.build_hasher();
    let empty_hash = h_empty.finish();
    assert_eq!(box_hash, inner_hash);
    assert_ne!(box_hash, empty_hash);

    let mut m: HashMap<Box<u32>, &'static str> = HashMap::new();
    let _ = m.insert(k, "seven");
    let probe = arena.alloc_box(7_u32);
    assert_eq!(m.get(&probe), Some(&"seven"));
}

#[test]
fn arena_box_as_ptr_and_as_mut_ptr() {
    let arena = Arena::new();
    let mut b = arena.alloc_box(99_u64);
    let p = Box::as_ptr(&b);
    // SAFETY: ptr valid for the lifetime of `b`.
    assert_eq!(unsafe { *p }, 99);
    let mp = Box::as_mut_ptr(&mut b);
    // SAFETY: exclusive access via &mut self.
    unsafe { *mp = 100 };
    assert_eq!(*b, 100);
}

#[test]
fn arena_box_as_ref_borrow_pointer() {
    use core::borrow::{Borrow, BorrowMut};
    let arena = Arena::new();
    let mut b: Box<i32> = arena.alloc_box(7);
    let r: &i32 = b.as_ref();
    assert_eq!(*r, 7);
    let r: &i32 = Borrow::borrow(&b);
    assert_eq!(*r, 7);
    let m: &mut i32 = b.as_mut();
    *m = 8;
    let m: &mut i32 = BorrowMut::borrow_mut(&mut b);
    *m = 9;
    assert_eq!(*b, 9);
    let s = format!("{b:p}");
    assert!(s.starts_with("0x"), "Pointer format: {s}");
}

#[test]
fn arena_box_into_rc_via_from() {
    let arena = Arena::new();
    let b: Box<u32> = arena.alloc_box(42);
    let r: Rc<u32> = b.into();
    assert_eq!(*r, 42);
    let r2: Rc<u32> = r.clone();
    assert_eq!(*r2, 42);
    drop(r);
    assert_eq!(*r2, 42);
}

use core::mem::MaybeUninit;

struct DropCount<'a>(&'a AtomicUsize);
impl Drop for DropCount<'_> {
    fn drop(&mut self) {
        let _ = self.0.fetch_add(1, AtomicOrdering::SeqCst);
    }
}

#[test]
fn uninit_box_dropped_without_init_does_not_run_drop() {
    let counter = AtomicUsize::new(0);
    let arena = Arena::new();
    {
        let _b: Box<MaybeUninit<DropCount<'_>>> = arena.alloc_uninit_box::<DropCount<'_>>();
        // never written, dropped here
    }
    drop(arena);
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn uninit_box_assume_init_runs_drop_once() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let mut b: Box<MaybeUninit<DropCount<'_>>> = arena.alloc_uninit_box::<DropCount<'_>>();
        let _ = b.write(DropCount(&counter));
        // SAFETY: just initialized.
        let b: Box<DropCount<'_>> = unsafe { b.assume_init() };
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
        drop(b);
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn zeroed_box_produces_zero_bytes() {
    let arena = Arena::new();
    let b = arena.alloc_zeroed_box::<u32>();
    // SAFETY: zero is a valid bit pattern for u32.
    let v = unsafe { b.assume_init() };
    assert_eq!(*v, 0);

    let b = arena.alloc_zeroed_box::<[u8; 16]>();
    // SAFETY: all-zeros is a valid bit pattern for [u8; 16].
    let v = unsafe { b.assume_init() };
    assert_eq!(*v, [0_u8; 16]);
}

#[test]
fn uninit_box_with_no_drop_type_works() {
    // Type doesn't need drop — no DropEntry is reserved.
    let arena = Arena::new();
    let mut b = arena.alloc_uninit_box::<u64>();
    let _ = b.write(0xDEAD_BEEF_CAFE_BABE);
    // SAFETY: just initialized.
    let v = unsafe { b.assume_init() };
    assert_eq!(*v, 0xDEAD_BEEF_CAFE_BABE);
}

#[test]
fn try_alloc_uninit_box_succeeds() {
    let arena = Arena::new();
    let mut b = arena.try_alloc_uninit_box::<u32>().unwrap();
    let _ = b.write(7);
    // SAFETY: just initialized.
    let v = unsafe { b.assume_init() };
    assert_eq!(*v, 7);
}

#[cfg(feature = "dst")]
#[test]
fn uninit_slice_box_dropped_without_init_does_not_run_drop() {
    let counter = AtomicUsize::new(0);
    let arena = Arena::new();
    {
        let _s = arena.alloc_uninit_slice_box::<DropCount<'_>>(8);
    }
    drop(arena);
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[cfg(feature = "dst")]
#[test]
fn uninit_slice_box_assume_init_runs_drop_per_element() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let mut s = arena.alloc_uninit_slice_box::<DropCount<'_>>(4);
        for slot in s.iter_mut() {
            let _ = slot.write(DropCount(&counter));
        }
        // SAFETY: all 4 elements just initialized.
        let s: Box<[DropCount<'_>]> = unsafe { s.assume_init() };
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
        drop(s);
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 4);
    }
}

#[cfg(feature = "dst")]
#[test]
fn zeroed_slice_box_produces_zero_bytes() {
    let arena = Arena::new();
    let s = arena.alloc_zeroed_slice_box::<u32>(5);
    // SAFETY: zeros are a valid bit pattern for u32.
    let s = unsafe { s.assume_init() };
    assert_eq!(&*s, &[0_u32; 5]);
}

#[test]
fn iterator_forwarding_concrete() {
    let arena = Arena::new();
    let it = arena.alloc_box(0_u32..5);
    // `ArenaBox<Range<u32>, _>` implements Iterator via the forwarding impl.
    let collected: std::vec::Vec<u32> = it.collect();
    assert_eq!(collected, std::vec![0, 1, 2, 3, 4]);
}

#[test]
fn iterator_size_hint_and_nth() {
    let arena = Arena::new();
    let mut it = arena.alloc_box(0_u32..10);
    assert_eq!(it.size_hint(), (10, Some(10)));
    assert_eq!(it.nth(3), Some(3));
    assert_eq!(it.next(), Some(4));
}

#[test]
fn double_ended_iterator_forwarding() {
    let arena = Arena::new();
    let mut it = arena.alloc_box(0_u32..5);
    assert_eq!(it.next_back(), Some(4));
    assert_eq!(it.next(), Some(0));
    assert_eq!(it.nth_back(0), Some(3));
}

#[test]
fn exact_size_iterator_forwarding() {
    let arena = Arena::new();
    let it = arena.alloc_box(0_u32..7);
    assert_eq!(ExactSizeIterator::len(&it), 7);
}

#[test]
fn fused_iterator_forwarding() {
    fn assert_fused<I: core::iter::FusedIterator>(_: &I) {}
    let arena = Arena::new();
    let it = arena.alloc_box(0_u32..2);
    assert_fused(&it);
}

#[test]
fn unpin_impl() {
    fn assert_unpin<T: Unpin>() {}
    assert_unpin::<Box<i32>>();
    // `ArenaBox<[u8]>` is gated on `dst`; under that feature the impl
    // applies via the `T: ?Sized` bound.
    #[cfg(feature = "dst")]
    assert_unpin::<Box<[u8]>>();
}

// If `T::drop` panics, the chunk's +1 hold must still be released
// (otherwise the chunk leaks until arena drop).
#[test]
fn box_drop_panic_releases_refcount() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DROPS: AtomicUsize = AtomicUsize::new(0);

    struct PanickyDrop {
        _payload: u64,
    }
    impl Drop for PanickyDrop {
        fn drop(&mut self) {
            let n = DROPS.fetch_add(1, Ordering::SeqCst);
            assert!(n != 0, "intentional panic from PanickyDrop");
        }
    }

    DROPS.store(0, Ordering::SeqCst);
    let arena = Arena::new();

    let result = catch_unwind(AssertUnwindSafe(|| {
        let b = arena.alloc_box(PanickyDrop { _payload: 1 });
        drop(b);
    }));
    assert!(result.is_err(), "expected panic from PanickyDrop");

    // Subsequent alloc must succeed — the arena state is consistent.
    let _b2 = arena.alloc_box(PanickyDrop { _payload: 2 });
    drop(arena);

    assert!(DROPS.load(Ordering::SeqCst) >= 1, "at least the panicking drop should have run");
}
