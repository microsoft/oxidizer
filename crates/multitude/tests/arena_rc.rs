// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for [`Rc`]: single-threaded reference-counted smart pointer.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::missing_asserts_for_indexing, reason = "test code is direct")]
#![allow(clippy::cast_possible_truncation, reason = "test code uses small integers")]

mod common;

use core::cmp::Ordering;

use multitude::{Arena, Rc};
#[test]
fn alloc_and_clone_basic() {
    let arena = Arena::new();
    let a = arena.alloc_rc(42_u32);
    let b = a.clone();
    assert_eq!(*a, 42);
    assert_eq!(*b, 42);
    assert!(Rc::ptr_eq(&a, &b));
}

#[test]
fn handles_outlive_arena() {
    let s = {
        let arena = Arena::new();
        arena.alloc_rc(std::string::String::from("survives"))
    };
    assert_eq!(*s, "survives");
}

#[test]
fn try_alloc_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_rc(100_u32).unwrap();
    assert_eq!(*r, 100);
}

#[test]
fn alloc_with_constructs_in_place() {
    let arena = Arena::new();
    let r = arena.alloc_rc_with(|| std::string::String::from("placed"));
    assert_eq!(&*r, "placed");
}

#[test]
fn try_alloc_with_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_rc_with(|| 200_u32).unwrap();
    assert_eq!(*r, 200);
}

#[test]
fn as_ptr_returns_value_ptr() {
    let arena = Arena::new();
    let r = arena.alloc_rc(42_u32);
    let p = Rc::as_ptr(&r);
    // SAFETY: ptr returned by as_ptr is valid for the lifetime of the smart pointer.
    assert_eq!(unsafe { *p }, 42);
}

#[test]
fn ptr_eq_distinguishes_handles() {
    let arena = Arena::new();
    let a = arena.alloc_rc(1_u32);
    let b = a.clone();
    let c = arena.alloc_rc(1_u32);
    assert!(Rc::ptr_eq(&a, &b));
    assert!(!Rc::ptr_eq(&a, &c));
}

#[test]
fn debug_and_display() {
    let arena = Arena::new();
    let r = arena.alloc_rc(42_u32);
    assert_eq!(format!("{r:?}"), "42");
    assert_eq!(format!("{r}"), "42");
}

#[test]
fn compare_and_hash() {
    let arena = Arena::new();
    let a = arena.alloc_rc(1_u32);
    let b = arena.alloc_rc(2_u32);
    let a2 = arena.alloc_rc(1_u32);
    assert!(a != b);
    assert!(a == a2);
    assert_eq!(a.cmp(&b), Ordering::Less);
    assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));
    assert_eq!(common::hash_of(&a), common::hash_of(&a2));
}

#[test]
fn slice_constructors() {
    let arena = Arena::new();
    let from_copy = arena.alloc_slice_copy_rc([1u8, 2, 3, 4, 5]);
    assert_eq!(&*from_copy, &[1, 2, 3, 4, 5]);

    let from_clone = arena.alloc_slice_clone_rc(&[std::string::String::from("a"), std::string::String::from("b")]);
    assert_eq!(from_clone.len(), 2);
    assert_eq!(&*from_clone[0], "a");

    let filled = arena.alloc_slice_fill_with_rc(5, |i| i * 10);
    assert_eq!(&*filled, &[0, 10, 20, 30, 40]);
}

#[test]
fn try_alloc_slice_copy_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_slice_copy_rc([1_u8, 2, 3]).unwrap();
    assert_eq!(&*r, &[1, 2, 3]);
}

#[test]
fn try_alloc_slice_clone_succeeds() {
    let arena = Arena::new();
    let r = arena
        .try_alloc_slice_clone_rc(&[std::string::String::from("x"), std::string::String::from("y")])
        .unwrap();
    assert_eq!(&*r[0], "x");
    assert_eq!(&*r[1], "y");
}

#[test]
fn try_alloc_slice_fill_with_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_slice_fill_with_rc(4, |i| i as u32 + 100).unwrap();
    assert_eq!(&*r, &[100, 101, 102, 103]);
}

#[test]
fn alloc_slice_fill_iter_succeeds() {
    let arena = Arena::new();
    let r = arena.alloc_slice_fill_iter_rc(0_u32..5);
    assert_eq!(&*r, &[0, 1, 2, 3, 4]);
}

#[test]
fn try_alloc_slice_fill_iter_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_slice_fill_iter_rc(10_u32..13).unwrap();
    assert_eq!(&*r, &[10, 11, 12]);
}

#[test]
fn alloc_slice_fill_with_drop_type_registers_drop() {
    let arena = Arena::new();
    let r = arena.alloc_slice_fill_with_rc(3, |i| std::string::String::from(["a", "b", "c"][i]));
    assert_eq!(&*r[0], "a");
    assert_eq!(&*r[2], "c");
}

#[test]
fn alloc_slice_copy_empty() {
    let arena = Arena::new();
    let r = arena.alloc_slice_copy_rc::<u32>(&[]);
    assert_eq!(r.len(), 0);
}

#[test]
fn alloc_slice_clone_empty() {
    let arena = Arena::new();
    let r = arena.alloc_slice_clone_rc::<String>(&[]);
    assert_eq!(r.len(), 0);
}

#[test]
fn alloc_slice_fill_with_zero_len() {
    let arena = Arena::new();
    let r = arena.alloc_slice_fill_with_rc::<u32, _>(0, |_| panic!("never called"));
    assert_eq!(r.len(), 0);
}

#[test]
#[should_panic(expected = "caller violated ExactSizeIterator contract")]
fn alloc_slice_fill_iter_panics_on_short_iter() {
    struct Liar(u32);
    impl Iterator for Liar {
        type Item = u32;
        fn next(&mut self) -> Option<u32> {
            if self.0 == 0 {
                None
            } else {
                self.0 -= 1;
                Some(0)
            }
        }
        fn size_hint(&self) -> (usize, Option<usize>) {
            (10, Some(10))
        }
    }
    impl ExactSizeIterator for Liar {
        fn len(&self) -> usize {
            10
        }
    }
    let arena = Arena::new();
    let _ = arena.alloc_slice_fill_iter_rc(Liar(2));
}

#[test]
fn slice_clone_and_compare() {
    let arena = Arena::new();
    let a = arena.alloc_slice_copy_rc([1_u32, 2, 3]);
    let b = a.clone();
    assert_eq!(&*a, &*b);
    assert!(Rc::ptr_eq(&a, &b));
    let c = arena.alloc_slice_copy_rc([1_u32, 2, 3]);
    assert!(!Rc::ptr_eq(&a, &c));
}

#[test]
fn slice_debug() {
    let arena = Arena::new();
    let r = arena.alloc_slice_copy_rc([1_u32, 2]);
    assert_eq!(format!("{r:?}"), "[1, 2]");
}

#[test]
fn arena_rc_as_ref_borrow_pointer() {
    use core::borrow::Borrow;
    let arena = Arena::new();
    let r: Rc<i32> = arena.alloc_rc(7);
    let v: &i32 = r.as_ref();
    assert_eq!(*v, 7);
    let v: &i32 = Borrow::borrow(&r);
    assert_eq!(*v, 7);
    let s = format!("{r:p}");
    assert!(s.starts_with("0x"), "Pointer format: {s}");
}

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

struct DropCount<'a>(&'a AtomicUsize);
impl Drop for DropCount<'_> {
    fn drop(&mut self) {
        let _ = self.0.fetch_add(1, AtomicOrdering::SeqCst);
    }
}

#[test]
fn uninit_rc_dropped_without_init_does_not_run_drop() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        {
            let _r: Rc<MaybeUninit<DropCount<'_>>> = arena.alloc_uninit_rc::<DropCount<'_>>();
        }
        // chunk torn down with no real init.
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn uninit_rc_assume_init_runs_drop_at_chunk_teardown() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let mut b = arena.alloc_uninit_box::<DropCount<'_>>();
        let _ = b.write(DropCount(&counter));
        // SAFETY: just initialized.
        let b = unsafe { b.assume_init() };
        let r: Rc<DropCount<'_>> = b.into_rc();
        let r2 = r.clone();
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
        drop(r);
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
        drop(r2);
        // Last refcount gone — chunk torn down inside arena.
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn uninit_rc_via_raw_assume_init_runs_drop_once() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let u = arena.alloc_uninit_rc::<DropCount<'_>>();
        // SAFETY: u is the unique handle so we have exclusive write access
        // to its target.
        unsafe {
            let p = Rc::as_ptr(&u).cast::<MaybeUninit<DropCount<'_>>>().cast_mut();
            let _ = (*p).write(DropCount(&counter));
        }
        // SAFETY: just initialized.
        let r = unsafe { u.assume_init() };
        let _ = r;
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);
}

#[test]
fn zeroed_rc_produces_zero_bytes() {
    let arena = Arena::new();
    let r = arena.alloc_zeroed_rc::<u32>();
    // SAFETY: zero is a valid bit pattern for u32.
    let r = unsafe { r.assume_init() };
    assert_eq!(*r, 0);
}

#[test]
fn uninit_slice_rc_dropped_without_init_does_not_run_drop() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let _s = arena.alloc_uninit_slice_rc::<DropCount<'_>>(4);
        // chunk torn down at end of arena scope.
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 0);
}

#[test]
fn uninit_slice_rc_assume_init_runs_drop_per_element() {
    let counter = AtomicUsize::new(0);
    {
        let arena = Arena::new();
        let s = arena.alloc_uninit_slice_rc::<DropCount<'_>>(3);
        // SAFETY: s is the unique handle so we have exclusive write access.
        #[expect(clippy::multiple_unsafe_ops_per_block, reason = "single tightly-coupled write loop")]
        unsafe {
            let base = Rc::as_ptr(&s).cast::<MaybeUninit<DropCount<'_>>>().cast_mut();
            for i in 0..3 {
                let _ = (*base.add(i)).write(DropCount(&counter));
            }
        }
        // SAFETY: all elements just initialized.
        let _typed = unsafe { s.assume_init() };
    }
    assert_eq!(counter.load(AtomicOrdering::SeqCst), 3);
}

#[test]
fn zeroed_slice_rc_produces_zero_bytes() {
    let arena = Arena::new();
    let s = arena.alloc_zeroed_slice_rc::<u16>(7);
    // SAFETY: zeros are a valid bit pattern for u16.
    let s = unsafe { s.assume_init() };
    assert_eq!(&*s, &[0_u16; 7]);
}

#[test]
fn try_alloc_uninit_rc_succeeds() {
    let arena = Arena::new();
    let r = arena.try_alloc_uninit_rc::<u32>().unwrap();
    drop(r);
}

#[test]
fn unpin_impl() {
    fn assert_unpin<T: Unpin>() {}
    assert_unpin::<Rc<i32>>();
    #[cfg(feature = "dst")]
    assert_unpin::<Rc<[u8]>>();
}

#[test]
fn from_arena_vec_freezes_to_rc() {
    let arena = Arena::new();
    let mut v = arena.alloc_vec();
    v.push(1_u32);
    v.push(2);
    v.push(3);
    let r: Rc<[u32]> = v.into();
    assert_eq!(&*r, &[1, 2, 3]);
}
