// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for [`Arc`]: cross-thread shareable refcounted smart pointer.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std for thread/sync primitives")]
#![allow(clippy::unwrap_used, reason = "test code")]

mod common;

use core::cmp::Ordering;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::thread;

use multitude::{Arc, Arena};

#[test]
fn cross_thread_arena_arc() {
    let arena = Arena::new();
    let shared: Arc<u64> = arena.alloc_arc(99);
    let s2 = shared.clone();
    let h = thread::spawn(move || *s2);
    assert_eq!(*shared, 99);
    assert_eq!(99, h.join().unwrap());

    let h: Arc<u32> = arena.alloc_arc(7);
    assert_eq!(*h, 7);
}

#[test]
fn shared_alloc_with_drop_type() {
    let arena = Arena::new();
    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    let value = std::sync::Arc::clone(&counter);
    let shared = arena.alloc_arc(value);
    let s2 = shared.clone();
    drop(shared);
    drop(s2);
    drop(arena);
    assert_eq!(std::sync::Arc::strong_count(&counter), 1);
}

#[test]
fn try_alloc_arc_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_arc(99_u32).unwrap();
    assert_eq!(*r, 99);
}

#[test]
fn try_alloc_arc_with_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_arc_with(|| 314_u32).unwrap();
    assert_eq!(*r, 314);
}

#[test]
fn alloc_arc_with_constructs_in_place() {
    let arena = Arena::new();
    let r = arena.alloc_arc_with(|| 271_u32);
    let r2 = r.clone();
    let h = thread::spawn(move || *r2);
    assert_eq!(*r, 271);
    assert_eq!(h.join().unwrap(), 271);

    let h: Arc<u64> = arena.alloc_arc_with(|| 42_u64);
    assert_eq!(*h, 42);
}

#[test]
fn get_mut_requires_unique_ownership() {
    let arena = Arena::new();
    let mut value = arena.alloc_arc(10_u32);
    *Arc::get_mut(&mut value).unwrap() = 11;

    let alias = value.clone();
    assert!(Arc::get_mut(&mut value).is_none());
    drop(alias);
    assert_eq!(*Arc::get_mut(&mut value).unwrap(), 11);

    let mut slice = arena.alloc_slice_copy_arc([1_u8, 2, 3]);
    Arc::get_mut(&mut slice).unwrap()[1] = 9;
    assert_eq!(&*slice, &[1, 9, 3]);
}

#[test]
fn alloc_slice_copy_arc_works() {
    let arena = Arena::new();
    let r = arena.alloc_slice_copy_arc([7_u32, 8, 9]);
    let r2 = r.clone();
    let h = thread::spawn(move || r2[1]);
    assert_eq!(&*r, &[7, 8, 9]);
    assert_eq!(h.join().unwrap(), 8);

    let h: Arc<[u32]> = arena.alloc_slice_copy_arc([1_u32, 2, 3, 4]);
    assert_eq!(&*h, &[1, 2, 3, 4]);
}

#[test]
fn try_alloc_slice_copy_arc_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_slice_copy_arc([7_u32, 8, 9]).unwrap();
    assert_eq!(&*r, &[7, 8, 9]);
}

#[test]
fn as_ptr_returns_value_ptr() {
    let arena = Arena::new();
    let r = arena.alloc_arc(42_u32);
    let p = Arc::as_ptr(&r);
    // SAFETY: ptr is valid while the smart pointer lives.
    assert_eq!(unsafe { *p }, 42);
}

#[test]
fn ptr_eq_distinguishes_handles() {
    let arena = Arena::new();
    let a = arena.alloc_arc(1_u32);
    let b = a.clone();
    let c = arena.alloc_arc(1_u32);
    assert!(Arc::ptr_eq(&a, &b));
    assert!(!Arc::ptr_eq(&a, &c));
}

#[test]
fn debug_display_compare_hash() {
    use std::collections::HashMap;
    use std::hash::{BuildHasher, BuildHasherDefault, Hasher};

    let arena = Arena::new();
    let a = arena.alloc_arc(1_u32);
    let b = arena.alloc_arc(2_u32);
    let a2 = arena.alloc_arc(1_u32);
    assert_eq!(format!("{a:?}"), "1");
    assert_eq!(format!("{a}"), "1");
    assert_eq!(a, a2);
    assert_ne!(a, b);
    assert!(a < b);
    assert_eq!(a.cmp(&b), Ordering::Less);
    assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));

    let bh = BuildHasherDefault::<std::collections::hash_map::DefaultHasher>::default();
    let mut h_arc = bh.build_hasher();
    std::hash::Hash::hash(&a, &mut h_arc);
    let arc_hash = h_arc.finish();
    let mut h_inner = bh.build_hasher();
    std::hash::Hash::hash(&1_u32, &mut h_inner);
    let inner_hash = h_inner.finish();
    let h_empty = bh.build_hasher();
    let empty_hash = h_empty.finish();
    assert_eq!(arc_hash, inner_hash, "Arc::hash must forward to inner hash");
    assert_ne!(arc_hash, empty_hash, "Arc::hash must feed bytes (not be a no-op)");
    let mut map: HashMap<Arc<u32>, &'static str> = HashMap::new();
    map.insert(a.clone(), "v");
    assert_eq!(map.get(&a).copied(), Some("v"));
    let s = format!("{a:p}");
    assert!(s.starts_with("0x"), "expected `0x` prefix, got `{s}`");
    assert!(s.len() > 2, "expected non-empty pointer hex digits, got `{s}`");
}

#[test]
fn cross_thread_drop_no_use_after_free() {
    // Stress-test: many short-lived shared smart pointers handed across threads.
    // Validates that the dec_ref-on-non-owner-thread path is sound.
    let arena = Arena::new();
    let handles: std::vec::Vec<_> = (0..100_u32)
        .map(|i| {
            let h = arena.alloc_arc(i);
            thread::spawn(move || *h)
        })
        .collect();
    let mut sum = 0_u64;
    for h in handles {
        sum += u64::from(h.join().unwrap());
    }
    assert_eq!(sum, (0..100_u64).sum::<u64>());

    let _ = AtomicOrdering::SeqCst; // suppress unused-import warning
}

#[test]
fn arena_drop_races_last_shared_handle_drop() {
    // Arena drop and last-handle drop race; exactly one atomic decrementer
    // reclaims the allocation.
    use std::sync::Barrier;

    for _ in 0..16 {
        let arena = Arena::new();
        let handle: Arc<u64> = arena.alloc_arc(0xDEAD_BEEF);
        let barrier = std::sync::Arc::new(Barrier::new(2));
        let b2 = std::sync::Arc::clone(&barrier);
        let other = thread::spawn(move || {
            let _ = b2.wait();
            drop(handle);
        });
        let _ = barrier.wait();
        drop(arena);
        other.join().unwrap();
    }
}

// Slice / iterator constructors that produce ArenaArc smart pointers.
// (alloc_slice_copy_arc / try_alloc_slice_copy_arc are tested above.)

#[test]
fn alloc_slice_clone_arc_works() {
    let arena = Arena::new();
    let originals = [std::string::String::from("a"), std::string::String::from("b")];
    let r: Arc<[String]> = arena.alloc_slice_clone_arc(&originals);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0], "a");
    assert_eq!(r[1], "b");
}

#[test]
fn try_alloc_slice_clone_arc_works() {
    let arena = Arena::new();
    let r: Arc<[u32]> = arena.try_alloc_slice_clone_arc([10_u32, 20, 30]).unwrap();
    assert_eq!(&*r, &[10, 20, 30]);
}

#[test]
fn alloc_slice_fill_with_arc_works() {
    let arena = Arena::new();
    let r: Arc<[u64]> = arena.alloc_slice_fill_with_arc(5, |i| (i as u64) * 2);
    assert_eq!(&*r, &[0, 2, 4, 6, 8]);

    let h: Arc<[u32]> = arena.alloc_slice_fill_with_arc(5, |i| u32::try_from(i).unwrap() * 10);
    assert_eq!(&*h, &[0, 10, 20, 30, 40]);
}

#[test]
fn try_alloc_slice_fill_with_arc_works() {
    let arena = Arena::new();
    let r: Arc<[u32]> = arena.try_alloc_slice_fill_with_arc(3, |i| u32::try_from(100 + i).unwrap()).unwrap();
    assert_eq!(&*r, &[100, 101, 102]);
}

#[test]
fn alloc_slice_fill_iter_arc_works() {
    let arena = Arena::new();
    let r: Arc<[i32]> = arena.alloc_slice_fill_iter_arc([0_i32, 1, 2, 3]);
    assert_eq!(&*r, &[0, 1, 2, 3]);

    let h: Arc<[u32]> = arena.alloc_slice_fill_iter_arc(0_u32..3);
    assert_eq!(&*h, &[0, 1, 2]);
}

#[test]
fn try_alloc_slice_fill_iter_arc_works() {
    let arena = Arena::new();
    let r: Arc<[u32]> = arena.try_alloc_slice_fill_iter_arc([42_u32, 43, 44]).unwrap();
    assert_eq!(&*r, &[42, 43, 44]);
}

#[test]
fn arena_arc_slice_clones_share_chunk() {
    let arena = Arena::new();
    let r: Arc<[u8]> = arena.alloc_slice_copy_arc([1_u8, 2, 3]);
    let r2 = r.clone();
    assert_eq!(&*r, &*r2);
}

#[test]
fn arena_arc_slice_send_to_thread() {
    let arena = Arena::new();
    let r: Arc<[u32]> = arena.alloc_slice_copy_arc([5_u32, 6, 7]);
    let r2 = r.clone();
    let h = thread::spawn(move || r2.iter().sum::<u32>());
    assert_eq!(h.join().unwrap(), 18);
    assert_eq!(&*r, &[5, 6, 7]);
}

#[test]
fn arena_arc_outlives_arena_single_thread() {
    // The last handle keeps its chunk alive after the arena is dropped.
    let h: Arc<u64> = {
        let arena = Arena::new();
        arena.alloc_arc(0xDEAD_BEEF)
    };
    // arena is gone; `h` still holds the chunk via virtual refcount.
    assert_eq!(*h, 0xDEAD_BEEF);
    drop(h); // dec_ref → 0 → teardown_chunk(chunk, false).
}

use core::mem::MaybeUninit;

struct DropCount<'a>(&'a AtomicUsize);
impl Drop for DropCount<'_> {
    fn drop(&mut self) {
        let _ = self.0.fetch_add(1, AtomicOrdering::SeqCst);
    }
}

#[test]
fn uninit_arc_dropped_without_init_does_not_run_drop() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        {
            let _a: Arc<MaybeUninit<DropCount<'_>>> = arena.alloc_uninit_arc::<DropCount<'_>>();
        }
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn uninit_arc_assume_init_runs_drop_at_chunk_teardown() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let a = arena.alloc_uninit_arc::<DropCount<'_>>();
        // SAFETY: a is the unique handle so we have exclusive write access.
        unsafe {
            let p = Arc::as_ptr(&a).cast::<MaybeUninit<DropCount<'_>>>().cast_mut();
            let _ = (*p).write(DropCount(&counter));
        }
        // SAFETY: just initialized.
        let typed = unsafe { a.assume_init() };
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
        drop(typed);
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn zeroed_arc_produces_zero_bytes() {
    let arena = Arena::new();
    let a = arena.alloc_zeroed_arc::<u64>();
    // SAFETY: zero is a valid bit pattern for u64.
    let a = unsafe { a.assume_init() };
    assert_eq!(*a, 0);
}

#[test]
fn try_uninit_arc_with_drop_type_uses_slice_path() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let a = arena.try_alloc_uninit_arc::<DropCount<'_>>().expect("must allocate");
        // SAFETY: `a` is the unique handle, so we have exclusive write access.
        unsafe {
            let p = Arc::as_ptr(&a).cast::<MaybeUninit<DropCount<'_>>>().cast_mut();
            (*p).write(DropCount(&counter));
        }
        // SAFETY: just initialized.
        let typed = unsafe { a.assume_init() };
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
        drop(typed);
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn zeroed_arc_with_drop_type_uses_slice_path() {
    // T: Drop. We never `assume_init`, so the noop drop shim runs at
    // chunk teardown (it doesn't dereference the uninitialized bytes),
    // and the real `DropCount::drop` never fires — so the counter
    // stays at 0.
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let _a: Arc<MaybeUninit<DropCount<'_>>> = arena.alloc_zeroed_arc::<DropCount<'_>>();
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn try_zeroed_arc_with_drop_type_uses_slice_path() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let _a: Arc<MaybeUninit<DropCount<'_>>> = arena.try_alloc_zeroed_arc::<DropCount<'_>>().expect("must allocate");
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn uninit_slice_arc_dropped_without_init_does_not_run_drop() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let _s = arena.alloc_uninit_slice_arc::<DropCount<'_>>(4);
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn uninit_slice_arc_assume_init_runs_drop_per_element() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let s = arena.alloc_uninit_slice_arc::<DropCount<'_>>(5);
        // SAFETY: s is the unique handle so we have exclusive write access.
        #[expect(clippy::multiple_unsafe_ops_per_block, reason = "single tightly-coupled write loop")]
        unsafe {
            let base = Arc::as_ptr(&s).cast::<MaybeUninit<DropCount<'_>>>().cast_mut();
            for i in 0..5 {
                let _ = (*base.add(i)).write(DropCount(&counter));
            }
        }
        // SAFETY: all elements just initialized.
        let _typed = unsafe { s.assume_init() };
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 5);
}

#[test]
fn zeroed_slice_arc_produces_zero_bytes() {
    let arena = Arena::new();
    let s = arena.alloc_zeroed_slice_arc::<u32>(6);
    // SAFETY: zeros are a valid bit pattern for u32.
    let s = unsafe { s.assume_init() };
    assert_eq!(&*s, &[0_u32; 6]);
}

#[test]
fn unpin_impl() {
    fn assert_unpin<T: Unpin>() {}
    assert_unpin::<Arc<i32>>();
    #[cfg(feature = "dst")]
    assert_unpin::<Arc<[u8]>>();
}

mod arc_borrow_coverage {
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
    use core::borrow::Borrow;

    use multitude::Arena;

    #[test]
    fn arc_borrow_returns_inner() {
        let a = Arena::new();
        let arc = a.alloc_arc(42_u32);
        let b: &u32 = Borrow::borrow(&arc);
        assert_eq!(*b, 42);
    }
}

mod arc_assume_init_slice_drops_each_element {
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
    use core::mem::MaybeUninit;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[test]
    fn slice_assume_init_for_drop_type_drops_each_element() {
        // `assume_init` reinterprets the initialized slice; `Arc::drop`
        // runs each element's destructor.
        struct D(StdArc<AtomicUsize>);
        impl Drop for D {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let arena = Arena::new();
            let arc: multitude::Arc<[MaybeUninit<D>]> =
                arena.alloc_slice_fill_with_arc(2, |_| MaybeUninit::new(D(StdArc::clone(&counter))));
            // SAFETY: both elements were initialized above.
            let init: multitude::Arc<[D]> = unsafe { arc.assume_init() };
            assert_eq!(init.len(), 2);
            drop(init);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }
}
