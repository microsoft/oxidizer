// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Coverage-gap tests: each test targets specific lines reported as
//! uncovered by `cargo llvm-cov`, exercising the public API path that
//! reaches them.

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

mod common;

/// Sync version of `common::FailingAllocator`, needed for tests that
/// allocate `Arc`/`Arc<str>`/`ArcUtf16Str` (whose builders require
/// `A: Send + Sync`).
mod sync_failing {
    use core::alloc::Layout;
    use core::ptr::NonNull;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use allocator_api2::alloc::{AllocError, Allocator, Global};

    #[derive(Clone)]
    pub(crate) struct SyncFailingAllocator {
        remaining: Arc<AtomicUsize>,
    }

    impl SyncFailingAllocator {
        pub(crate) fn new(allow_n_allocs: usize) -> Self {
            Self {
                remaining: Arc::new(AtomicUsize::new(allow_n_allocs)),
            }
        }
    }

    // SAFETY: forwards to Global on success; fails atomically on exhaustion.
    unsafe impl Allocator for SyncFailingAllocator {
        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            let mut cur = self.remaining.load(Ordering::Relaxed);
            loop {
                if cur == 0 {
                    return Err(AllocError);
                }
                match self
                    .remaining
                    .compare_exchange_weak(cur, cur - 1, Ordering::Relaxed, Ordering::Relaxed)
                {
                    Ok(_) => break,
                    Err(actual) => cur = actual,
                }
            }
            Global.allocate(layout)
        }

        unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
            // SAFETY: forwarded per Allocator contract.
            unsafe { Global.deallocate(ptr, layout) };
        }
    }
}

// ============================================================================
// strings/box_utf16_str.rs — trait impls (Deref/AsRef/Borrow/Display/Debug/
// PartialEq/Ord/Hash/Pointer/Unpin); lines 36–137.
// ============================================================================
#[cfg(feature = "utf16")]
mod box_utf16_str_traits {
    use core::borrow::{Borrow, BorrowMut};
    use core::cmp::Ordering;
    use core::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    use multitude::Arena;
    use widestring::{Utf16Str, utf16str};

    #[test]
    fn methods_len_is_empty_and_as_mut_utf16_str() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("hello"));
        assert_eq!(b.len(), 5);
        assert!(!b.is_empty());
        let _: &mut Utf16Str = b.as_mut_utf16_str();
        let empty = arena.alloc_utf16_str_box(utf16str!(""));
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn deref_and_deref_mut() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("hi"));
        let _: &Utf16Str = &b;
        let _: &mut Utf16Str = &mut b;
    }

    #[test]
    fn as_ref_and_as_mut_via_trait() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("abc"));
        let _: &Utf16Str = AsRef::as_ref(&b);
        let _: &mut Utf16Str = AsMut::as_mut(&mut b);
    }

    #[test]
    fn borrow_and_borrow_mut_via_trait() {
        let arena = Arena::new();
        let mut b = arena.alloc_utf16_str_box(utf16str!("xyz"));
        let _: &Utf16Str = Borrow::borrow(&b);
        let _: &mut Utf16Str = BorrowMut::borrow_mut(&mut b);
    }

    #[test]
    fn debug_and_display_format() {
        let arena = Arena::new();
        let b = arena.alloc_utf16_str_box(utf16str!("dbg"));
        let _ = format!("{b:?}");
        let _ = format!("{b}");
    }

    #[test]
    fn eq_ord_partialord() {
        let arena = Arena::new();
        let a = arena.alloc_utf16_str_box(utf16str!("alpha"));
        let b = arena.alloc_utf16_str_box(utf16str!("alpha"));
        let c = arena.alloc_utf16_str_box(utf16str!("beta"));
        assert!(a == b);
        assert!(a != c);
        assert_eq!(a.cmp(&b), Ordering::Equal);
        assert_eq!(a.cmp(&c), Ordering::Less);
        assert_eq!(a.partial_cmp(&c), Some(Ordering::Less));
    }

    #[test]
    fn hash_and_pointer_format() {
        let arena = Arena::new();
        let a = arena.alloc_utf16_str_box(utf16str!("hh"));
        let mut h = DefaultHasher::new();
        a.hash(&mut h);
        let _ = h.finish();
        let _ = format!("{a:p}");
    }

    #[test]
    fn unpin_impl_compiles() {
        fn assert_unpin<T: Unpin>() {}
        assert_unpin::<multitude::strings::BoxUtf16Str>();
    }
}

// ============================================================================
// `alloc_*` panic paths: each non-`try_*` entry point should panic on
// allocator failure. We use `FailingAllocator::new(0)` so the very first
// chunk acquire fails.
// ============================================================================
mod alloc_panics_on_failing_allocator {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use multitude::Arena;
    #[cfg(feature = "utf16")]
    use widestring::utf16str;

    use crate::common::FailingAllocator;
    use crate::sync_failing::SyncFailingAllocator;

    fn fa() -> Arena<FailingAllocator> {
        Arena::new_in(FailingAllocator::new(0))
    }
    fn sfa() -> Arena<SyncFailingAllocator> {
        Arena::new_in(SyncFailingAllocator::new(0))
    }

    #[test]
    fn alloc_value_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc(0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_with(|| 0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_arc(0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_arc_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_arc_with(|| 0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_box(0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_box_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_box_with(|| 0_u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_copy_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_copy(&[1_u8, 2, 3][..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_clone_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
            let _ = a.alloc_slice_clone(&v[..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_with::<u32, _>(3, |i| i as u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_iter_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_iter::<u32, _>([1_u32, 2, 3]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_clone_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
            let _ = a.alloc_slice_clone_arc(&v[..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_copy_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_copy_arc(&[1_u8, 2, 3][..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_with_arc::<u32, _>(3, |i| i as u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_iter_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_iter_arc::<u32, _>([1_u32, 2, 3]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_clone_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
            let _ = a.alloc_slice_clone_box(&v[..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_copy_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_copy_box(&[1_u8, 2, 3][..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_with_box::<u32, _>(3, |i| i as u32);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_iter_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_slice_fill_iter_box::<u32, _>([1_u32, 2, 3]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_str_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_str("abc");
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_str_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_str_arc("abc");
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_str_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_str_box("abc");
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_arc(utf16str!("abc"));
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_box(utf16str!("abc"));
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_from_str_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_arc_from_str("abc");
        }));
        assert!(r.is_err());
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_box_from_str_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_utf16_str_box_from_str("abc");
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_arc::<D>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_arc::<D>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_slice_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_slice_arc::<D>(2);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_slice_arc_drop_type_panics() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_slice_arc::<D>(2);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_box::<u32>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_box::<u32>();
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_uninit_slice_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_uninit_slice_box::<u32>(2);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_zeroed_slice_box_panics() {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let a = sfa();
            let _ = a.alloc_zeroed_slice_box::<u32>(2);
        }));
        assert!(r.is_err());
    }
}

// ============================================================================
// `try_alloc_*` Err paths: each fallible entry point returns Err on
// allocator failure. Covers the `Err(e) => ... return Err(e)` arms in
// each impl_alloc_* refill loop.
// ============================================================================
mod try_alloc_returns_err_on_failing_allocator {
    use multitude::Arena;
    #[cfg(feature = "utf16")]
    use widestring::utf16str;

    use crate::sync_failing::SyncFailingAllocator;

    fn fa() -> Arena<SyncFailingAllocator> {
        Arena::new_in(SyncFailingAllocator::new(0))
    }

    #[test]
    fn try_alloc_value_err() {
        let a = fa();
        assert!(a.try_alloc(0_u32).is_err());
    }
    #[test]
    fn try_alloc_with_err() {
        let a = fa();
        assert!(a.try_alloc_with(|| 0_u32).is_err());
    }
    #[test]
    fn try_alloc_arc_err() {
        let a = fa();
        assert!(a.try_alloc_arc(0_u32).is_err());
    }
    #[test]
    fn try_alloc_arc_with_err() {
        let a = fa();
        assert!(a.try_alloc_arc_with(|| 0_u32).is_err());
    }
    #[test]
    fn try_alloc_box_err() {
        let a = fa();
        assert!(a.try_alloc_box(0_u32).is_err());
    }
    #[test]
    fn try_alloc_box_with_err() {
        let a = fa();
        assert!(a.try_alloc_box_with(|| 0_u32).is_err());
    }
    #[test]
    fn try_alloc_slice_copy_err() {
        let a = fa();
        assert!(a.try_alloc_slice_copy(&[1_u8, 2, 3][..]).is_err());
    }
    #[test]
    fn try_alloc_slice_clone_err() {
        let a = fa();
        let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
        assert!(a.try_alloc_slice_clone(&v[..]).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_with_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_with::<u32, _>(3, |i| i as u32).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_iter_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_iter::<u32, _>([1_u32, 2, 3]).is_err());
    }
    #[test]
    fn try_alloc_slice_clone_arc_err() {
        let a = fa();
        let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
        assert!(a.try_alloc_slice_clone_arc(&v[..]).is_err());
    }
    #[test]
    fn try_alloc_slice_copy_arc_err() {
        let a = fa();
        assert!(a.try_alloc_slice_copy_arc(&[1_u8, 2, 3][..]).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_with_arc_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_with_arc::<u32, _>(3, |i| i as u32).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_iter_arc_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_iter_arc::<u32, _>([1_u32, 2, 3]).is_err());
    }
    #[test]
    fn try_alloc_slice_clone_box_err() {
        let a = fa();
        let v: std::vec::Vec<u32> = std::vec![1, 2, 3];
        assert!(a.try_alloc_slice_clone_box(&v[..]).is_err());
    }
    #[test]
    fn try_alloc_slice_copy_box_err() {
        let a = fa();
        assert!(a.try_alloc_slice_copy_box(&[1_u8, 2, 3][..]).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_with_box_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_with_box::<u32, _>(3, |i| i as u32).is_err());
    }
    #[test]
    fn try_alloc_slice_fill_iter_box_err() {
        let a = fa();
        assert!(a.try_alloc_slice_fill_iter_box::<u32, _>([1_u32, 2, 3]).is_err());
    }
    #[test]
    fn try_alloc_str_err() {
        let a = fa();
        assert!(a.try_alloc_str("abc").is_err());
    }
    #[test]
    fn try_alloc_str_arc_err() {
        let a = fa();
        assert!(a.try_alloc_str_arc("abc").is_err());
    }
    #[test]
    fn try_alloc_str_box_err() {
        let a = fa();
        assert!(a.try_alloc_str_box("abc").is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_arc_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_arc(utf16str!("abc")).is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_box_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_box(utf16str!("abc")).is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_arc_from_str_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_arc_from_str("abc").is_err());
    }
    #[cfg(feature = "utf16")]
    #[test]
    fn try_alloc_utf16_str_box_from_str_err() {
        let a = fa();
        assert!(a.try_alloc_utf16_str_box_from_str("abc").is_err());
    }
    #[test]
    fn try_alloc_uninit_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_uninit_arc::<D>().is_err());
    }
    #[test]
    fn try_alloc_zeroed_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_zeroed_arc::<D>().is_err());
    }
    #[test]
    fn try_alloc_uninit_slice_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_uninit_slice_arc::<D>(2).is_err());
    }
    #[test]
    fn try_alloc_zeroed_slice_arc_drop_err() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = fa();
        assert!(a.try_alloc_zeroed_slice_arc::<D>(2).is_err());
    }
    #[test]
    fn try_alloc_uninit_box_err() {
        let a = fa();
        assert!(a.try_alloc_uninit_box::<u32>().is_err());
    }
    #[test]
    fn try_alloc_zeroed_box_err() {
        let a = fa();
        assert!(a.try_alloc_zeroed_box::<u32>().is_err());
    }
    #[test]
    fn try_alloc_uninit_slice_box_err() {
        let a = fa();
        assert!(a.try_alloc_uninit_slice_box::<u32>(2).is_err());
    }
    #[test]
    fn try_alloc_zeroed_slice_box_err() {
        let a = fa();
        assert!(a.try_alloc_zeroed_slice_box::<u32>(2).is_err());
    }
}

// ============================================================================
// Drop-typed slice with len > u16::MAX: must return `Err` from try_*
// helpers (covers the `reject_drop_slice_too_long` PANIC=false arms).
// ============================================================================
mod drop_slice_over_u16_max_returns_err {
    use multitude::Arena;

    #[derive(Clone)]
    struct D(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);
    impl Drop for D {
        fn drop(&mut self) {}
    }

    const TOO_LONG: usize = (u16::MAX as usize) + 1;

    #[test]
    fn try_alloc_slice_clone_drop_over_u16_err() {
        let a = Arena::new();
        let v: std::vec::Vec<D> = (0..TOO_LONG).map(|i| D(i as u8)).collect();
        assert!(a.try_alloc_slice_clone(&v[..]).is_err());
    }

    #[test]
    fn try_alloc_slice_fill_with_drop_over_u16_err() {
        let a = Arena::new();
        assert!(a.try_alloc_slice_fill_with::<D, _>(TOO_LONG, |i| D(i as u8)).is_err());
    }

    #[test]
    fn try_alloc_slice_fill_iter_drop_over_u16_err() {
        let a = Arena::new();
        assert!(a.try_alloc_slice_fill_iter::<D, _>((0..TOO_LONG).map(|i| D(i as u8))).is_err());
    }

    // `Arc<[T]>` uninit/zeroed slices have no `u16` element-count cap
    // under per-`Arc` reference counting (they drop via
    // `drop_in_place::<[T]>`, not a `u16`-counted chunk entry), so a
    // Drop-typed slice longer than `u16::MAX` now allocates successfully.
    #[cfg(not(miri))]
    #[test]
    fn uninit_slice_arc_over_u16_succeeds() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = Arena::new();
        let arc = a.try_alloc_uninit_slice_arc::<D>(TOO_LONG).expect("Arc slices have no u16 cap");
        assert_eq!(arc.len(), TOO_LONG);
    }

    #[cfg(not(miri))]
    #[test]
    fn zeroed_slice_arc_over_u16_succeeds() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = Arena::new();
        let arc = a.try_alloc_zeroed_slice_arc::<D>(TOO_LONG).expect("Arc slices have no u16 cap");
        assert_eq!(arc.len(), TOO_LONG);
    }
}

// ============================================================================
// vec/freeze.rs — `try_into_arc` (lines 46–60). The infallible
// `into_arc` is exercised elsewhere; this targets the fallible
// variant and its allocator-error path via `FailingAllocator(1)` (the
// initial vec buffer consumes the one allowed chunk, the freeze refill
// then fails).
// ============================================================================
mod vec_freeze_try_into_arc {
    use multitude::Arena;

    use crate::sync_failing::SyncFailingAllocator;

    #[test]
    fn try_into_arc_ok() {
        let a = Arena::new();
        let mut v = a.alloc_vec::<u32>();
        v.push(1);
        v.push(2);
        v.push(3);
        let arc = v.try_into_arc().unwrap();
        assert_eq!(&*arc, &[1, 2, 3][..]);
    }

    #[test]
    fn try_into_arc_err_on_failing_allocator() {
        let a = Arena::new_in(SyncFailingAllocator::new(1));
        let mut v = a.alloc_vec::<u32>();
        v.push(1);
        let r = v.try_into_arc();
        assert!(r.is_err());
    }
}

// ============================================================================
// vec/into_iter.rs — `DoubleEndedIterator::next_back` (55–57) and
// `Debug` (64–66).
// ============================================================================
mod vec_into_iter_traits {
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

// ============================================================================
// vec/mod.rs:116 — `try_grow_to` early return when `new_cap <= buf.cap()`.
// ============================================================================
mod vec_try_grow_to_noop {
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

// ============================================================================
// vec/mutate.rs — `insert` allocator-failure panic path (line 38) and
// `dedup_by` early return for `len < 2` (line 170).
// ============================================================================
mod vec_mutate_extras {
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

// ============================================================================
// internal/uninit.rs:462–470 — `UninitDrop::init_from_iter` for drop
// types. Exercised by `alloc_slice_fill_iter` with `T: Drop`.
// ============================================================================
mod uninit_drop_init_from_iter {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[derive(Clone)]
    struct Counted(StdArc<AtomicUsize>);
    impl Drop for Counted {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn alloc_slice_fill_iter_drop_type_runs_drop_at_arena_teardown() {
        let counter = StdArc::new(AtomicUsize::new(0));
        {
            let a = Arena::new();
            let _: &mut [Counted] = a.alloc_slice_fill_iter((0..4).map(|_| Counted(counter.clone())));
            assert_eq!(counter.load(Ordering::Relaxed), 0);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 4);
    }
}

// ============================================================================
// `alloc_zeroed_slice_arc` for a drop type zero-fills the payload (the
// `MaybeUninit::zeroed` fill path).
// ============================================================================
mod zeroed_slice_arc_zeroes_payload {
    use core::mem::MaybeUninit;

    use multitude::Arena;

    #[test]
    fn zeroed_slice_arc_drop_type_zeroes_payload() {
        struct D(u32);
        impl Drop for D {
            fn drop(&mut self) {}
        }
        let a = Arena::new();
        let slice = a.alloc_zeroed_slice_arc::<D>(3);
        // SAFETY: the slice is freshly zeroed; reading the raw bytes is
        // well defined for any zero pattern.
        unsafe {
            let bytes = core::slice::from_raw_parts(slice.as_ptr() as *const MaybeUninit<u8>, core::mem::size_of::<D>() * 3);
            for b in bytes {
                assert_eq!(b.assume_init(), 0);
            }
        }
    }
}

// ============================================================================
// vec_into_iter::IntoIter created via `IntoIterator::into_iter` on Vec.
// Already covers `IntoIter::new` indirectly.
// ============================================================================
mod vec_intoiterator_impl {
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

// ============================================================================
// strings/arc_str.rs — PartialEq<str>, PartialEq<&str>, Pointer fmt;
// strings/arc_utf16_str.rs — is_empty, PartialEq, Display, etc.
// ============================================================================
mod arc_str_traits {
    use multitude::Arena;
    #[cfg(feature = "utf16")]
    use widestring::utf16str;

    #[test]
    fn arc_str_partial_eq_str_and_ref_str() {
        let a = Arena::new();
        let s = a.alloc_str_arc("hello");
        // PartialEq<str>
        assert!(s == *"hello");
        // PartialEq<&str>
        assert!(s == "hello");
        let bad = "world";
        assert!(s != *bad);
    }

    #[test]
    fn arc_str_pointer_fmt() {
        let a = Arena::new();
        let s = a.alloc_str_arc("p");
        let _ = format!("{s:p}");
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn arc_utf16_str_is_empty_and_eq_and_display() {
        let a = Arena::new();
        let s = a.alloc_utf16_str_arc(utf16str!(""));
        assert!(s.is_empty());
        let one = a.alloc_utf16_str_arc(utf16str!("x"));
        let two = a.alloc_utf16_str_arc(utf16str!("x"));
        assert!(one == two);
        let _ = format!("{one}");
    }
}

// ============================================================================
// strings/box_str.rs — From<Box<str, A>> for Box<[u8], A> (lines 265–278).
// ============================================================================
mod box_str_into_box_u8_slice {
    use multitude::{Arena, Box as ArenaBox};

    #[test]
    fn from_box_str_to_box_u8_slice() {
        let a = Arena::new();
        let s = a.alloc_str_box("hello");
        let bytes: ArenaBox<[u8]> = ArenaBox::from(s);
        assert_eq!(&*bytes, b"hello");
    }
}

// ============================================================================
// allocator_impl.rs — ZST allocate/dealloc (49–50, 68) and grow with
// non-zero copy_bytes (95).
// ============================================================================
mod allocator_impl_paths {
    use core::alloc::Layout;

    use allocator_api2::alloc::{Allocator, Global};
    use multitude::Arena;

    #[test]
    fn arena_as_allocator_zst_allocate_returns_dangling() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let layout = Layout::from_size_align(0, 8).unwrap();
        let nn = alloc.allocate(layout).unwrap();
        assert_eq!(nn.len(), 0);
    }

    #[test]
    fn arena_as_allocator_zst_dealloc_is_noop() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let layout = Layout::from_size_align(0, 8).unwrap();
        let nn = alloc.allocate(layout).unwrap();
        // SAFETY: pair the dealloc with the allocation above; layout matches.
        unsafe { alloc.deallocate(nn.cast::<u8>(), layout) };
    }

    #[test]
    fn arena_as_allocator_grow_copies_overlap() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let old = Layout::from_size_align(8, 1).unwrap();
        let nn = alloc.allocate(old).unwrap();
        // Write into the old allocation.
        // SAFETY: nn points to 8 writable bytes inside the arena.
        unsafe {
            for i in 0..8 {
                nn.cast::<u8>().as_ptr().add(i).write(i as u8);
            }
        }
        let new = Layout::from_size_align(32, 1).unwrap();
        // SAFETY: nn was returned by Self::allocate with `old` layout.
        let grown = unsafe { alloc.grow(nn.cast::<u8>(), old, new).unwrap() };
        // Verify the prefix was copied.
        // SAFETY: grown is a fresh allocation of at least 32 bytes.
        unsafe {
            for i in 0..8_u8 {
                assert_eq!(*grown.cast::<u8>().as_ptr().add(usize::from(i)), i);
            }
            // Allocator API requires the caller to release the +1 chunk
            // refcount the grow path took; otherwise the chunk leaks.
            alloc.deallocate(grown.cast::<u8>(), new);
        }
    }
}

// ============================================================================
// arc.rs — Borrow impl (413–415).
// ============================================================================
mod arc_borrow {
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

// ============================================================================
// arc.rs — slice assume_init is a pure reinterpret under per-`Arc`
// reference counting; element destructors run eagerly in `Arc::drop`.
// ============================================================================
mod arc_assume_init_slice_drops_each_element {
    use core::mem::MaybeUninit;
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use multitude::Arena;

    #[test]
    fn slice_assume_init_for_drop_type_drops_each_element() {
        // `alloc_slice_fill_with_arc::<MaybeUninit<D>>` + `assume_init`
        // used to be rejected (no placeholder drop entry). Now
        // `assume_init` is a pure reinterpret and `Arc::drop` runs each
        // element's destructor via `drop_in_place::<[D]>`.
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

// ============================================================================
// internal/in_chunk.rs — Clone impl (37–40).
// ============================================================================
mod in_chunk_clone_is_copy_proxy {
    // InChunk is a pub(crate) type; we exercise its Clone via the public
    // Arc-clone path (each clone of an arena Arc internally derives a
    // chunk reference whose pointer is wrapped through InChunk machinery).
    use multitude::Arena;

    #[test]
    fn arc_clone_exercises_inchunk_clone() {
        let arena = Arena::new();
        let a = arena.alloc_arc(7_u32);
        let b = a.clone();
        assert_eq!(*a, 7);
        assert_eq!(*b, 7);
    }
}

// ============================================================================
// vec/mod.rs:116 — try_grow_to early return (already-sufficient cap).
// Also covers ZST early-return at 113 implicitly.
// ============================================================================
mod vec_try_grow_to_within_cap {
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

// ============================================================================
// internal/chunk_ops.rs — `SharedChunk::teardown_and_release` destroy
// branch when the provider Weak no longer upgrades. (The corresponding
// LocalChunk branch was removed entirely once LocalChunk's back-pointer
// stopped being a Weak: a LocalChunk can never outlive its provider, so
// the orphan path was dead in practice.)
// ============================================================================
mod chunk_ops_destroy_branch {
    use multitude::Arena;

    #[test]
    fn box_outlives_arena_takes_destroy_branch_on_release() {
        // Create an arena, allocate a Box, drop the arena. The Box keeps
        // its (shared) chunk alive via +1; when the Box drops, the chunk's
        // release path runs after the arena (and its ChunkProvider) is
        // already gone, exercising the `SharedChunk::destroy` arm.
        let arena = Arena::new();
        let b = arena.alloc_box(42_u32);
        drop(arena);
        assert_eq!(*b, 42);
        drop(b);
    }
}

// ============================================================================
// arena/mod.rs — try_new / try_new_in / Default for Arena<Global>.
// ============================================================================
mod arena_constructors {
    use allocator_api2::alloc::Global;
    use multitude::Arena;

    use crate::sync_failing::SyncFailingAllocator;

    #[test]
    fn arena_try_new_ok() {
        let a: Arena<Global> = Arena::try_new().unwrap();
        let _ = a.alloc(0_u32);
    }

    #[test]
    fn arena_default_constructs_global() {
        let a: Arena<Global> = Arena::default();
        let _ = a.alloc(0_u32);
    }

    #[test]
    fn arena_try_new_in_ok() {
        let a = Arena::try_new_in(SyncFailingAllocator::new(usize::MAX)).unwrap();
        let _ = a.alloc(0_u32);
    }
}

// ============================================================================
// alloc_slice_*: overflow paths in `worst_case_slice_payload` (None branch)
// and `reject_drop_slice_too_long` for the panicking variants.
// ============================================================================
mod alloc_slice_overflow_paths {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use multitude::Arena;

    const HUGE: usize = usize::MAX / 2;

    // -- try_alloc_slice_* fallible overflow paths --

    #[test]
    fn try_alloc_slice_fill_with_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_with::<u32, _>(HUGE, |_| 0);
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_iter_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_iter::<u32, _>((0..HUGE).map(|_| 0_u32));
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_with_box_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_with_box::<u32, _>(HUGE, |_| 0);
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_iter_box_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_iter_box::<u32, _>((0..HUGE).map(|_| 0_u32));
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_with_arc_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_with_arc::<u32, _>(HUGE, |_| 0);
        assert!(r.is_err());
    }

    #[test]
    fn try_alloc_slice_fill_iter_arc_u32_huge_len_returns_err() {
        let a = Arena::new();
        let r = a.try_alloc_slice_fill_iter_arc::<u32, _>((0..HUGE).map(|_| 0_u32));
        assert!(r.is_err());
    }

    // -- panicking variants overflow paths --

    fn p<F: FnOnce()>(f: F) -> bool {
        catch_unwind(AssertUnwindSafe(f)).is_err()
    }

    #[test]
    fn alloc_slice_fill_with_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_with::<u32, _>(HUGE, |_| 0);
        }));
    }
    #[test]
    fn alloc_slice_fill_iter_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_iter::<u32, _>((0..HUGE).map(|_| 0_u32));
        }));
    }
    #[test]
    fn alloc_slice_fill_with_box_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_with_box::<u32, _>(HUGE, |_| 0);
        }));
    }
    #[test]
    fn alloc_slice_fill_iter_box_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_iter_box::<u32, _>((0..HUGE).map(|_| 0_u32));
        }));
    }
    #[test]
    fn alloc_slice_fill_with_arc_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_with_arc::<u32, _>(HUGE, |_| 0);
        }));
    }
    #[test]
    fn alloc_slice_fill_iter_arc_u32_huge_len_panics() {
        assert!(p(|| {
            let a = Arena::new();
            let _ = a.alloc_slice_fill_iter_arc::<u32, _>((0..HUGE).map(|_| 0_u32));
        }));
    }

    // -- reject_drop_slice_too_long panic path for &mut [T] (PANIC=true)
    // Use a heap Vec to avoid stack overflow; `alloc_slice_clone(&v[..])`
    // takes the panicking path and `reject_drop_slice_too_long` rejects
    // up front for T:Drop with len > u16::MAX.

    #[derive(Clone)]
    struct D(#[expect(dead_code, reason = "field gives the type a non-zero size")] u8);
    impl Drop for D {
        fn drop(&mut self) {}
    }

    // The rejection happens up front (`reject_drop_slice_too_long`)
    // before any element is read, but a real allocation is required
    // to satisfy Miri's reference-validity check at slice
    // construction time. `vec![value; n]` for a `Clone`-derivable
    // type lowers to a single capacity allocation plus a bulk
    // initializing loop — much cheaper than `(0..N).map(...).collect()`
    // which runs the closure N times.
    #[test]
    fn alloc_slice_clone_drop_over_u16_panics() {
        let v: std::vec::Vec<D> = std::vec![D(0); u16::MAX as usize + 1];
        let arena = Arena::new();
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _ = arena.alloc_slice_clone(&v[..]);
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_with_drop_over_u16_panics() {
        let arena = Arena::new();
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _ = arena.alloc_slice_fill_with::<D, _>(u16::MAX as usize + 1, |i| D(i as u8));
        }));
        assert!(r.is_err());
    }

    #[test]
    fn alloc_slice_fill_iter_drop_over_u16_panics() {
        let arena = Arena::new();
        let r = catch_unwind(AssertUnwindSafe(|| {
            let _ = arena.alloc_slice_fill_iter::<D, _>((0..(u16::MAX as usize + 1)).map(|i| D(i as u8)));
        }));
        assert!(r.is_err());
    }
}

// ============================================================================
// alloc_utf16.rs — try_alloc_utf16_str_arc / box Ok closure body (50, 51, 77, 78).
// ============================================================================
#[cfg(feature = "utf16")]
mod alloc_utf16_try_variants_ok {
    use multitude::Arena;
    use widestring::utf16str;

    #[test]
    fn try_alloc_utf16_str_arc_ok() {
        let a = Arena::new();
        let r = a.try_alloc_utf16_str_arc(utf16str!("ok")).unwrap();
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn try_alloc_utf16_str_box_ok() {
        let a = Arena::new();
        let r = a.try_alloc_utf16_str_box(utf16str!("ok")).unwrap();
        assert_eq!(r.len(), 2);
    }
}

// ============================================================================
// internal/arena_buf.rs:300–306 — split_off_buf ZST tail.
// Exercised by Vec::drain on a ZST element type (drain uses ArenaBuf::drain
// which internally calls split_off_buf).
// ============================================================================
mod arena_buf_zst_split {
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

// ============================================================================
// allocator_impl.rs:95 — grow with copy_bytes == 0 (new_layout smaller
// than old, edge case where min is 0).
// ============================================================================
mod allocator_impl_grow_to_zero_overlap {
    use core::alloc::Layout;

    use allocator_api2::alloc::{Allocator, Global};
    use multitude::Arena;

    #[test]
    fn grow_from_zero_old_does_not_copy() {
        let arena: Arena<Global> = Arena::new();
        let alloc = &arena;
        let zero = Layout::from_size_align(0, 1).unwrap();
        let nn = alloc.allocate(zero).unwrap();
        let one = Layout::from_size_align(8, 1).unwrap();
        // SAFETY: nn was returned by Self::allocate with `zero` layout.
        let grown = unsafe { alloc.grow(nn.cast::<u8>(), zero, one).unwrap() };
        assert_eq!(grown.len(), 8);
        // SAFETY: pair with the grow above; the resulting +1 chunk
        // refcount must be released or the chunk leaks.
        unsafe { alloc.deallocate(grown.cast::<u8>(), one) };
    }
}

// ============================================================================
// alloc_unsized.rs — metadata-too-large handling for `[D]` slice DSTs
// and refill-failure path for `try_alloc_dst_box` (lines 229–230, 261).
// The `Box` path rejects `T: Drop` DSTs whose metadata does not pack
// into the chunk drop-list's `u16` slot; the `Arc` path stores metadata
// verbatim and runs `drop_in_place` eagerly, so it has no such limit.
// Lives here rather than as a `src/` unit test so the empty `Drop`
// impl on the probe type doesn't bloat src-coverage counts.
// ============================================================================
#[cfg(feature = "dst")]
mod alloc_unsized_extras {
    use core::alloc::Layout;

    use multitude::Arena;

    use crate::sync_failing::SyncFailingAllocator;

    #[derive(Default)]
    struct D(#[expect(dead_code, reason = "field gives the type a non-zero size")] u32);
    impl Drop for D {
        fn drop(&mut self) {}
    }

    #[test]
    // Skipped under Miri: writing + dropping `u16::MAX + 1` elements
    // (~65K) to exercise the slice-length boundary exceeds Miri's test
    // budget; the lifted restriction is a runtime property, not a
    // memory-safety one, so native + cargo-careful runs cover it.
    #[cfg_attr(miri, ignore)]
    fn try_alloc_dst_box_slice_drop_metadata_too_large_succeeds() {
        // Like the `Arc` path, the `Box` path stores slice metadata
        // full-width in the chunk prefix and drops via
        // `drop_in_place::<[D]>` (no `u16` drop-list slot), so a `T: Drop`
        // slice longer than `u16::MAX` is accepted.
        let arena = Arena::new();
        let len = (u16::MAX as usize) + 1;
        let layout = Layout::array::<D>(len).unwrap();
        // SAFETY: `layout` describes `[D; len]`; `init` writes a valid
        // `D` into every element before the `Box` is observed, so the
        // eager `drop_in_place::<[D]>` in `Box::drop` runs on live values.
        let r = unsafe {
            arena.try_alloc_dst_box::<[D]>(layout, len, |p: *mut [D]| {
                let base = p.cast::<D>();
                for i in 0..len {
                    // SAFETY: `base..base + len` is the freshly reserved
                    // `[D]` buffer described by `layout`.
                    base.add(i).write(D::default());
                }
            })
        };
        assert!(r.is_ok());
    }

    #[test]
    // Skipped under Miri: writing + dropping `u16::MAX + 1` elements
    // (~65K) to exercise the slice-length boundary exceeds Miri's test
    // budget (~8 min observed). The lifted-restriction behavior is a
    // runtime property, not a memory-safety one; native + cargo-careful
    // runs verify it on every CI execution.
    #[cfg_attr(miri, ignore)]
    fn try_alloc_dst_arc_slice_drop_metadata_too_large_succeeds() {
        // Unlike the `Box` path, the `Arc` path stores slice metadata
        // verbatim (not in the `u16` drop-list slot), so a `T: Drop`
        // slice longer than `u16::MAX` is accepted.
        let arena = Arena::new();
        let len = (u16::MAX as usize) + 1;
        let layout = Layout::array::<D>(len).unwrap();
        // SAFETY: `layout` describes `[D; len]`; `init` writes a valid
        // `D` into every element before the `Arc` is observed, so the
        // eager `drop_in_place::<[D]>` on teardown runs on live values.
        let r = unsafe {
            arena.try_alloc_dst_arc::<[D]>(layout, len, |p: *mut [D]| {
                let base = p.cast::<D>();
                for i in 0..len {
                    // SAFETY: `base..base + len` is the freshly reserved
                    // `[D]` buffer described by `layout`.
                    base.add(i).write(D::default());
                }
            })
        };
        assert!(r.is_ok());
    }

    #[test]
    fn try_alloc_dst_box_refill_failure_returns_err() {
        let arena = Arena::new_in(SyncFailingAllocator::new(0));
        let layout = Layout::new::<u32>();
        // SAFETY: refill failure fires before init.
        let r = unsafe {
            arena.try_alloc_dst_box::<u32>(layout, (), |p: *mut u32| {
                p.write(0);
            })
        };
        assert!(r.is_err());
    }
}

// Oversized-allocation branches: each public API below, when handed a
// request larger than the default `max_normal_alloc` (16 KiB) on a
// fresh arena (whose current chunk is the 512-byte minimum class),
// fails the normal in-chunk reservation and routes through the
// one-shot oversized-chunk path. These paths were previously
// uncovered (alloc_slice_ref.rs, alloc_value.rs, alloc_utf16.rs).
mod oversized_paths {
    use multitude::Arena;

    // Drop wrapper so the slice/value allocators take their
    // `needs_drop::<T>()` oversized arm (registers a drop entry).
    #[derive(Clone)]
    struct DropU64(u64);
    impl Drop for DropU64 {
        fn drop(&mut self) {}
    }

    // 24 KiB single value with Drop ⇒ oversized-local value arm
    // (`alloc_value.rs` 433-436).
    #[derive(Clone)]
    struct BigDrop([u64; 3000]);
    impl Drop for BigDrop {
        fn drop(&mut self) {}
    }

    // Counter-backed Drop element to assert that oversized-chunk drop
    // entries are actually replayed (not just that the branch is taken).
    struct CountedDrop<'a>(&'a core::sync::atomic::AtomicUsize);
    impl Drop for CountedDrop<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        }
    }

    // Verifies the oversized-local slice drop path *replays* destructors
    // at arena teardown (correctness, not just coverage of the branch).
    #[test]
    fn alloc_slice_fill_with_oversized_drop_replays_destructors() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        let counter = AtomicUsize::new(0);
        {
            let arena = Arena::new();
            let out = arena.alloc_slice_fill_with(3000, |_| CountedDrop(&counter));
            assert_eq!(out.len(), 3000);
            assert_eq!(counter.load(Ordering::SeqCst), 0, "no drops before teardown");
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3000, "every element dropped at arena teardown");
    }

    #[test]
    fn alloc_slice_clone_oversized_drop() {
        let arena = Arena::new();
        let src: Vec<DropU64> = (0..3000).map(DropU64).collect();
        let out = arena.alloc_slice_clone(&src);
        assert_eq!(out.len(), 3000);
        assert_eq!(out[2999].0, 2999);
    }

    #[test]
    fn alloc_slice_fill_with_oversized_drop() {
        let arena = Arena::new();
        let out = arena.alloc_slice_fill_with(3000, |i| DropU64(i as u64));
        assert_eq!(out.len(), 3000);
        assert_eq!(out[2999].0, 2999);
    }

    #[test]
    fn alloc_slice_fill_iter_oversized_drop() {
        let arena = Arena::new();
        let out = arena.alloc_slice_fill_iter((0_u32..3000).map(|i| DropU64(u64::from(i))));
        assert_eq!(out.len(), 3000);
        assert_eq!(out[0].0, 0);
    }

    #[test]
    fn alloc_with_oversized_drop_value() {
        let arena = Arena::new();
        let v = arena.alloc_with(|| BigDrop([7_u64; 3000]));
        assert_eq!(v.0[0], 7);
        assert_eq!(v.0[2999], 7);
    }

    #[test]
    fn alloc_uninit_arc_oversized() {
        use core::mem::MaybeUninit;

        use multitude::Arc;
        let arena = Arena::new();
        // A 24 KiB *Drop* value: `alloc_uninit_arc` only routes through
        // `impl_alloc_uninit_arc` (the placeholder-drop-entry path) for
        // `T: Drop`; a fresh arena's small current chunk can't hold it,
        // so the request goes to a one-shot oversized chunk.
        let a = arena.alloc_uninit_arc::<BigDrop>();
        // SAFETY: `a` is the unique handle, so we have exclusive write access.
        unsafe {
            let p = Arc::as_ptr(&a).cast::<MaybeUninit<BigDrop>>().cast_mut();
            (*p).write(BigDrop([9_u64; 3000]));
        }
        // SAFETY: just initialized above.
        let typed = unsafe { a.assume_init() };
        assert_eq!(typed.0[0], 9);
        assert_eq!(typed.0[2999], 9);
    }

    #[test]
    fn alloc_uninit_slice_arc_oversized() {
        use core::mem::MaybeUninit;

        use multitude::Arc;
        let arena = Arena::new();
        // A Drop element type routes through `impl_alloc_uninit_slice_arc`;
        // 3000 × 8 B = 24 KiB exceeds the fresh arena's current chunk, so
        // the slice lands in a one-shot oversized chunk.
        let len = 3000_usize;
        let s = arena.alloc_uninit_slice_arc::<DropU64>(len);
        // SAFETY: `s` is the unique handle, so we have exclusive write access.
        unsafe {
            let base = Arc::as_ptr(&s).cast::<MaybeUninit<DropU64>>().cast_mut();
            for i in 0..len {
                (*base.add(i)).write(DropU64(i as u64));
            }
        }
        // SAFETY: every element initialized above.
        let typed = unsafe { s.assume_init() };
        assert_eq!(typed.len(), len);
        assert_eq!(typed[123].0, 123);
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_arc_oversized() {
        let arena = Arena::new();
        // ~10 000 ASCII chars ⇒ 20 KiB of UTF-16 payload ⇒ oversized.
        let s = "a".repeat(10_000);
        let u = arena.alloc_utf16_str_arc_from_str(&s);
        assert_eq!(u.len(), 10_000);
    }

    #[cfg(feature = "utf16")]
    #[test]
    fn alloc_utf16_str_box_oversized() {
        let arena = Arena::new();
        let s = "b".repeat(10_000);
        let u = arena.alloc_utf16_str_box_from_str(&s);
        assert_eq!(u.len(), 10_000);
    }

    #[test]
    fn alloc_zeroed_arc_oversized() {
        use core::mem::MaybeUninit;

        use multitude::Arc;
        let arena = Arena::new();
        // Zeroed variant of the oversized uninit-arc path (Drop type ⇒
        // `impl_alloc_uninit_arc(zeroed = true)`); never `assume_init`,
        // so the placeholder drop shim tears down without touching the
        // zeroed bytes.
        let _a: Arc<MaybeUninit<BigDrop>> = arena.alloc_zeroed_arc::<BigDrop>();
    }

    #[test]
    fn box_str_as_mut_str() {
        let arena = Arena::new();
        let mut b = arena.alloc_str_box("hello");
        // Exercises `Box<str>::as_mut_str` (str_impls.rs 93-95).
        let m: &mut str = b.as_mut_str();
        m.make_ascii_uppercase();
        assert_eq!(b.as_str(), "HELLO");
    }
}

// ============================================================================
// vec/freeze.rs — `Vec::leak` (the O(1), allocation-free freeze for
// `T: !Drop`, plus `&*v.leak()` for the shared variant), plus the empty-slice
// early returns and the oversized `!Drop` arm of the slice-ref allocators
// (alloc_slice_ref.rs).
// ============================================================================
mod freeze_and_slice_edges {
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
        let s: &mut [u32] = arena.alloc_slice_copy::<u32>(&[]);
        assert!(s.is_empty());
    }

    #[test]
    fn alloc_slice_clone_empty_returns_empty_slice() {
        let arena = Arena::new();
        let src: [String; 0] = [];
        let s: &mut [String] = arena.alloc_slice_clone(&src);
        assert!(s.is_empty());
    }

    #[test]
    fn alloc_slice_fill_iter_oversized_non_drop() {
        let arena = Arena::new();
        // 5000 × u32 = 20 KiB > MAX_NORMAL_ALLOC (16 KiB) ⇒ oversized path;
        // `u32: !Drop` ⇒ the non-drop oversized arm of
        // `impl_alloc_slice_fill_iter`.
        let s: &mut [u32] = arena.alloc_slice_fill_iter((0_u32..5000).map(|i| i.wrapping_mul(3)));
        assert_eq!(s.len(), 5000);
        assert_eq!(s[0], 0);
        assert_eq!(s[4999], 4999_u32.wrapping_mul(3));
    }
}
