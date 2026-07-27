// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for [`Rc`]: the non-atomic, single-thread refcounted smart pointer.

#![allow(clippy::clone_on_ref_ptr, reason = "tests prefer concise method-call form")]
#![allow(clippy::std_instead_of_core, reason = "tests use std")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::redundant_clone, reason = "tests exercise clone/drop refcounting explicitly")]
#![allow(clippy::cast_possible_truncation, reason = "test data is small")]
#![allow(clippy::multiple_unsafe_ops_per_block, reason = "test code groups related unsafe ops")]

mod common;

use core::cell::Cell;
use core::pin::Pin;
use std::rc::Rc as StdRc;

use multitude::{Arena, Rc};

#[test]
fn alloc_rc_value_and_clone() {
    let arena = Arena::new();
    let a: Rc<u64> = arena.alloc_rc(99);
    let b = a.clone();
    let c: Rc<u64> = arena.alloc_rc(99);
    assert_eq!(*a, 99);
    assert_eq!(*b, 99);
    assert!(Rc::ptr_eq(&a, &b), "clones share the same allocation");
    assert!(!Rc::ptr_eq(&a, &c), "distinct allocations are not ptr-equal");
}

#[test]
fn alloc_rc_with_constructs_in_place() {
    let arena = Arena::new();
    let v = arena.alloc_rc_with(|| std::vec![1, 2, 3]);
    assert_eq!(&**v, &[1, 2, 3]);
}

#[test]
fn get_mut_requires_unique_ownership() {
    let arena = Arena::new();
    let mut value = arena.alloc_rc(10_u32);
    *Rc::get_mut(&mut value).unwrap() = 11;

    let alias = value.clone();
    assert!(Rc::get_mut(&mut value).is_none());
    drop(alias);
    assert_eq!(*Rc::get_mut(&mut value).unwrap(), 11);

    let mut slice = arena.alloc_slice_copy_rc([1_u8, 2, 3]);
    Rc::get_mut(&mut slice).unwrap()[1] = 9;
    assert_eq!(&*slice, &[1, 9, 3]);
}

#[test]
fn rc_runs_drop_on_last_clone_only() {
    struct DropCounter(StdRc<Cell<usize>>);
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let arena = Arena::new();
    let counter = StdRc::new(Cell::new(0_usize));

    let a = arena.alloc_rc(DropCounter(StdRc::clone(&counter)));
    let b = a.clone();
    let c = a.clone();
    assert_eq!(counter.get(), 0);
    drop(a);
    assert_eq!(counter.get(), 0, "value lives while clones remain");
    drop(b);
    assert_eq!(counter.get(), 0);
    drop(c);
    assert_eq!(counter.get(), 1, "value drops exactly once on the last clone");
}

#[test]
fn rc_holds_non_send_value() {
    // `Rc` imposes no `Send`/`Sync` bound on `T`: an `std::rc::Rc` (which is
    // `!Send`/`!Sync`) is allocatable behind a multitude `Rc`. `Arc` could not
    // do this. This is purely a compile-time capability check.
    let arena = Arena::new();
    let inner = StdRc::new(41_u32);
    let r = arena.alloc_rc(StdRc::clone(&inner));
    assert_eq!(**r, 41);
    let r2 = r.clone();
    assert_eq!(**r2, 41);
}

#[test]
fn rc_outlives_arena() {
    let r;
    {
        let arena = Arena::new();
        r = arena.alloc_rc(0xDEAD_BEEF_u32);
        // arena dropped here; `r` keeps its chunk alive via the chunk +1.
    }
    assert_eq!(*r, 0xDEAD_BEEF);
}

#[test]
fn rc_str_and_unaligned_count() {
    let arena = Arena::new();
    // `str` is align-1: the strong count is stored unaligned. Exercise
    // clone/drop across many small strings to stress the unaligned path.
    for i in 0..64_usize {
        let s = arena.alloc_str_rc(std::format!("string number {i}"));
        let s2 = s.clone();
        assert_eq!(s.as_str(), s2.as_str());
        assert_eq!(&*s, &*std::format!("string number {i}"));
    }
}

#[test]
fn rc_slice_variants() {
    let arena = Arena::new();
    let copied = arena.alloc_slice_copy_rc([1_u32, 2, 3, 4]);
    assert_eq!(&*copied, &[1, 2, 3, 4]);

    let cloned = arena.alloc_slice_clone_rc(std::vec![10_u64, 20, 30]);
    assert_eq!(&*cloned, &[10, 20, 30]);

    let filled = arena.alloc_slice_fill_with_rc::<u32, _>(5, |i| u32::try_from(i).unwrap() * 2);
    assert_eq!(&*filled, &[0, 2, 4, 6, 8]);

    let from_iter = arena.alloc_slice_fill_iter_rc((0..4_u8).map(|x| x + 100));
    assert_eq!(&*from_iter, &[100, 101, 102, 103]);

    // clone keeps elements alive
    let c2 = cloned.clone();
    assert_eq!(&*c2, &[10, 20, 30]);
}

#[test]
fn rc_slice_with_drop_elements() {
    struct D(StdRc<Cell<usize>>);
    impl Drop for D {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    let arena = Arena::new();
    let counter = StdRc::new(Cell::new(0_usize));

    let s = arena.alloc_slice_fill_with_rc::<D, _>(4, |_| D(StdRc::clone(&counter)));
    let s2 = s.clone();
    assert_eq!(counter.get(), 0);
    drop(s);
    assert_eq!(counter.get(), 0);
    drop(s2);
    assert_eq!(counter.get(), 4, "every element drops exactly once on the last clone");
}

#[test]
fn rc_uninit_assume_init() {
    let arena = Arena::new();
    let u = arena.alloc_zeroed_rc::<u32>();
    // SAFETY: zeroed bytes are a valid `u32` (0).
    let r: Rc<u32> = unsafe { u.assume_init() };
    assert_eq!(*r, 0);

    // Slice zeroed -> assume_init.
    let s = arena.alloc_zeroed_slice_rc::<u16>(4);
    // SAFETY: zeroed bytes are valid `u16`s.
    let s: Rc<[u16]> = unsafe { s.assume_init() };
    assert_eq!(&*s, &[0, 0, 0, 0]);
}

#[test]
fn rc_from_vec_and_string() {
    let arena = Arena::new();
    let v = arena.alloc_vec_with_capacity::<u32>(3);
    let mut v = v;
    v.push(1);
    v.push(2);
    v.push(3);
    let rc: Rc<[u32]> = Rc::from(v);
    assert_eq!(&*rc, &[1, 2, 3]);

    let mut s = arena.alloc_string();
    s.push_str("hello rc");
    let rs: Rc<str> = Rc::from(s);
    assert_eq!(&*rs, "hello rc");

    // Rc<str> -> Rc<[u8]> retag
    let bytes: Rc<[u8]> = Rc::from(rs);
    assert_eq!(&*bytes, b"hello rc");
}

#[test]
fn rc_pin() {
    struct NotUnpin {
        value: u32,
        _pin: core::marker::PhantomPinned,
    }

    let arena = Arena::new();
    let pinned = arena.alloc_rc_pin_with(|| NotUnpin {
        value: 123,
        _pin: core::marker::PhantomPinned,
    });
    let address = (&raw const *pinned) as usize;
    let clone = pinned.clone();
    drop(pinned);
    assert_eq!((&raw const *clone) as usize, address);
    assert_eq!(clone.value, 123);
}

#[test]
fn rc_from_vec_then_clone_and_drop() {
    // The freeze prefix's strong count was written as an `AtomicU32`; cloning
    // the frozen `Rc` exercises the non-atomic `write_unaligned` increment over
    // that location, and the staged drops exercise the decrement-to-teardown
    // path. (Validated under Miri.)
    let arena = Arena::new();
    let mut v = arena.alloc_vec_with_capacity::<u64>(4);
    for i in 0..4_u64 {
        v.push(i * 11);
    }
    let rc: Rc<[u64]> = Rc::from(v);
    let c1 = rc.clone();
    let c2 = c1.clone();
    assert_eq!(&*rc, &[0, 11, 22, 33]);
    assert!(Rc::ptr_eq(&rc, &c2));
    drop(rc);
    drop(c1);
    assert_eq!(&*c2, &[0, 11, 22, 33]);
    drop(c2);
}

// Exercises `Rc` allocation variants, conversions, and trait implementations.
#[test]
fn rc_remaining_surface_coverage() {
    // value: try_alloc_rc, pin family
    let arena = Arena::new();
    assert_eq!(*arena.try_alloc_rc(1_u32).unwrap(), 1);
    assert_eq!(*arena.alloc_rc_pin(2_u32), 2);
    assert_eq!(*arena.try_alloc_rc_pin(3_u32).unwrap(), 3);
    assert_eq!(*arena.alloc_rc_pin_with(|| 4_u32), 4);
    assert_eq!(*arena.try_alloc_rc_pin_with(|| 5_u32).unwrap(), 5);

    // slice: try_ + fill_with_rc_pin family
    assert_eq!(&*arena.try_alloc_slice_copy_rc([1_u8, 2]).unwrap(), &[1, 2]);
    assert_eq!(&*arena.try_alloc_slice_clone_rc(std::vec![3_u16, 4]).unwrap(), &[3, 4]);
    assert_eq!(&*arena.try_alloc_slice_fill_with_rc::<u8, _>(2, |i| i as u8).unwrap(), &[0, 1]);
    assert_eq!(&*arena.try_alloc_slice_fill_iter_rc(0..2_u8).unwrap(), &[0, 1]);
    let _: Pin<Rc<[u32]>> = arena.alloc_slice_fill_with_rc_pin::<u32, _>(2, |_| 9);
    let _: Pin<Rc<[u32]>> = arena.try_alloc_slice_fill_with_rc_pin::<u32, _>(2, |_| 9).unwrap();

    // str: try_, PartialEq, From<Rc<str>> for Rc<[u8]>
    let s = arena.try_alloc_str_rc("hi").unwrap();
    assert_eq!(s, *"hi");
    assert_eq!(s, "hi");
    assert_ne!(s, *"bye");
    assert_ne!(s, "bye");
    let _: Rc<[u8]> = Rc::from(s);

    // uninit/zeroed value + slice, try_
    let _ = arena.alloc_uninit_rc::<u32>();
    let _ = arena.try_alloc_uninit_rc::<u32>().unwrap();
    let _ = arena.alloc_zeroed_rc::<u32>();
    let _ = arena.try_alloc_zeroed_rc::<u32>().unwrap();
    let _ = arena.try_alloc_uninit_slice_rc::<u32>(2).unwrap();
    let _ = arena.alloc_uninit_slice_rc::<u32>(2);
    let _ = arena.try_alloc_zeroed_slice_rc::<u32>(2).unwrap();

    // assume_init slice
    let zs = arena.alloc_zeroed_slice_rc::<u8>(3);
    // SAFETY: zeroed bytes are valid u8s.
    let zs: Rc<[u8]> = unsafe { zs.assume_init() };
    assert_eq!(&*zs, &[0, 0, 0]);
    // String -> Rc<str>
    let mut st = arena.alloc_string();
    st.push_str("abc");
    assert_eq!(&*st.into_rc_str(), "abc");
    let mut st = arena.alloc_string();
    st.push_str("def");
    assert_eq!(&*st.try_into_rc_str().unwrap(), "def");

    // Vec -> Rc (try_into_rc_slice)
    let mut v = arena.alloc_vec_with_capacity::<u32>(2);
    v.push(7);
    v.push(8);
    assert_eq!(&*v.try_into_rc_slice().unwrap(), &[7, 8]);

    // Vec -> Rc copy/drain fallback: a `split_off` tail has no freeze prefix of
    // its own, so freezing it copies into a fresh allocation instead of
    // freezing in place. Exercises both the fallible and infallible paths.
    let mut tv = arena.alloc_vec::<u32>();
    tv.extend(0..6);
    let tail = tv.split_off(2);
    assert_eq!(&*tail.try_into_rc_slice().unwrap(), &[2, 3, 4, 5]);

    let mut tv2 = arena.alloc_vec::<u32>();
    tv2.extend(0..6);
    let tail2 = tv2.split_off(2);
    assert_eq!(&*tail2.into_rc_slice(), &[2, 3, 4, 5]);
}

#[cfg(feature = "serde")]
#[test]
fn rc_str_serialize() {
    let arena = Arena::new();
    let s = arena.alloc_str_rc("json");
    assert_eq!(serde_json::to_string(&s).unwrap(), "\"json\"");
}

#[cfg(feature = "utf16")]
#[test]
fn rc_utf16_coverage() {
    use widestring::utf16str;

    let arena = Arena::new();
    let w = utf16str!("hello");
    let r = arena.alloc_utf16_str_rc(w);
    let _ = arena.try_alloc_utf16_str_rc(w).unwrap();
    let _ = arena.alloc_utf16_str_rc_from_str("hello");
    let _ = arena.try_alloc_utf16_str_rc_from_str("hello").unwrap();
    assert_eq!(r.as_widestring_utf16_str(), w);
    // From<Rc<Utf16Str>> for Rc<[u16]>
    let units: Rc<[u16]> = Rc::from(r);
    assert_eq!(units.len(), w.len());

    // Utf16String -> Rc<Utf16Str>
    let mut us = arena.alloc_utf16_string();
    us.push_from_str("xy");
    let _ = us.into_rc_utf16_str();
    let mut us = arena.alloc_utf16_string();
    us.push_from_str("zw");
    let _ = us.try_into_rc_utf16_str().unwrap();
    // `From<Utf16String> for Rc<Utf16Str>`.
    let mut us = arena.alloc_utf16_string();
    us.push_from_str("qr");
    let r: Rc<multitude::strings::Utf16Str> = us.into();
    assert_eq!(r.as_widestring_utf16_str(), utf16str!("qr"));
}

#[cfg(feature = "dst")]
#[test]
fn rc_dst_coverage() {
    use core::alloc::Layout;

    let arena = Arena::new();
    // slice DST via alloc_dst_rc
    let layout = Layout::array::<u32>(3).unwrap();
    // SAFETY: layout/metadata/init describe a valid [u32; 3].
    let r: Rc<[u32]> = unsafe {
        arena.alloc_dst_rc::<[u32]>(layout, 3, |p: *mut [u32]| {
            let base = p.cast::<u32>();
            for i in 0..3 {
                base.add(i).write(i as u32 + 1);
            }
        })
    };
    assert_eq!(&*r, &[1, 2, 3]);
    // SAFETY: as above.
    let _ = unsafe { arena.try_alloc_dst_rc::<[u32]>(layout, 3, |p| p.cast::<u32>().write_bytes(0, 3)) }.unwrap();
    // SAFETY: as above.
    let _ = unsafe { arena.alloc_dst_rc_pin::<[u32]>(layout, 3, |p| p.cast::<u32>().write_bytes(0, 3)) };
    // SAFETY: as above.
    let _ = unsafe { arena.try_alloc_dst_rc_pin::<[u32]>(layout, 3, |p| p.cast::<u32>().write_bytes(0, 3)) }.unwrap();
}

#[cfg(feature = "bytemuck")]
#[test]
fn rc_bytemuck_coverage() {
    let arena = Arena::new();
    let v = arena.bytemuck();
    assert_eq!(*v.alloc_rc::<u32>(), 0);
    assert_eq!(*v.try_alloc_rc::<u32>().unwrap(), 0);
    assert_eq!(&*v.alloc_slice_rc::<u32>(2), &[0, 0]);
    assert_eq!(&*v.try_alloc_slice_rc::<u32>(2).unwrap(), &[0, 0]);
}

#[cfg(feature = "zerocopy")]
#[test]
fn rc_zerocopy_coverage() {
    let arena = Arena::new();
    let v = arena.zerocopy();
    assert_eq!(*v.alloc_rc::<u32>(), 0);
    assert_eq!(*v.try_alloc_rc::<u32>().unwrap(), 0);
    assert_eq!(&*v.alloc_slice_rc::<u32>(2), &[0, 0]);
    assert_eq!(&*v.try_alloc_slice_rc::<u32>(2).unwrap(), &[0, 0]);
}

#[test]
fn rc_value_is_aligned() {
    // `LocalStrong::block_align` must align the reservation to the value's
    // alignment. Misalign the bump cursor with a 1-byte allocation first, then
    // allocate an 8-aligned value: with the correct `block_align` the value
    // pointer is 8-aligned; replacing it with `1` leaves it misaligned.
    let arena = Arena::new();
    let _odd = arena.alloc(1_u8); // advance the cursor to an odd offset
    let r = arena.alloc_rc(0x0102_0304_0506_0708_u64);
    let addr = r.as_ptr() as usize;
    assert_eq!(
        addr % core::mem::align_of::<u64>(),
        0,
        "Rc value must be aligned to align_of::<T>()",
    );
    // SAFETY: `read_unaligned` is sound regardless of alignment.
    assert_eq!(unsafe { core::ptr::read_unaligned(r.as_ptr()) }, 0x0102_0304_0506_0708);
}
