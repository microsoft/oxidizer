// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::clone_on_ref_ptr,
    clippy::unwrap_used,
    clippy::assertions_on_result_states,
    clippy::cast_possible_truncation,
    clippy::collection_is_never_read,
    clippy::items_after_statements,
    clippy::many_single_char_names,
    clippy::borrow_as_ptr,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    reason = "test code"
)]

//! Tests for the smart-pointer surface shared by all four handle types:
//! `as_ptr`/`as_mut_ptr`, `ptr_eq`, `get_mut`, construction-time pinning,
//! `Unpin`, the auto-trait (`Send`/`Sync`) bounds, and the forwarding trait
//! impls (`AsRef`, `AsMut`, `Borrow`, `BorrowMut`,
//! `PartialEq`/`Eq`, `PartialOrd`/`Ord`, `Hash`, `Debug`/`Display`/`Pointer`).

use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::from_ref;
use std::collections::{BTreeSet, HashSet};

use plurality::{Alloc, Arc, Box, Coercion, Pool, Rc};

#[test]
fn as_ref_and_borrow_forward_to_value() {
    let pool = Pool::<String>::builder().chunk_size(8).build();

    let b = pool.alloc_box(String::from("box"));
    let a = pool.alloc_arc(String::from("arc"));
    let r = pool.alloc_rc(String::from("rc"));
    let l = pool.alloc(String::from("alloc"));

    fn takes_str(s: &str) -> usize {
        s.len()
    }
    assert_eq!(AsRef::<String>::as_ref(&b).as_str(), "box");
    assert_eq!(AsRef::<String>::as_ref(&a).as_str(), "arc");
    assert_eq!(AsRef::<String>::as_ref(&r).as_str(), "rc");
    assert_eq!(AsRef::<String>::as_ref(&l).as_str(), "alloc");
    assert_eq!(takes_str(&b), 3);

    assert_eq!(Borrow::<String>::borrow(&b), "box");
    assert_eq!(Borrow::<String>::borrow(&a), "arc");
    assert_eq!(Borrow::<String>::borrow(&r), "rc");
    assert_eq!(Borrow::<String>::borrow(&l), "alloc");
}

#[test]
fn as_mut_and_borrow_mut_on_unique_handles() {
    let pool = Pool::<String>::builder().chunk_size(4).build();

    let mut b = pool.alloc_box(String::from("a"));
    AsMut::<String>::as_mut(&mut b).push_str("bc");
    assert_eq!(&*b, "abc");
    BorrowMut::<String>::borrow_mut(&mut b).push('d');
    assert_eq!(&*b, "abcd");

    let mut l = pool.alloc(String::from("x"));
    AsMut::<String>::as_mut(&mut l).push('y');
    BorrowMut::<String>::borrow_mut(&mut l).push('z');
    assert_eq!(&*l, "xyz");
}

#[test]
fn as_ptr_points_at_the_value_and_is_stable() {
    let pool = Pool::<u32>::builder().chunk_size(4).build();

    let b = pool.alloc_box(7);
    let p = Box::as_ptr(&b);
    // SAFETY: the box keeps the slot alive, so the pointer is valid.
    assert_eq!(unsafe { *p }, 7);
    assert_eq!(p, from_ref::<u32>(&*b));

    let a = pool.alloc_arc(8);
    // SAFETY: live arc.
    assert_eq!(unsafe { *Arc::as_ptr(&a) }, 8);

    let r = pool.alloc_rc(9);
    // SAFETY: live rc.
    assert_eq!(unsafe { *Rc::as_ptr(&r) }, 9);

    let l = pool.alloc(10);
    // SAFETY: live alloc.
    assert_eq!(unsafe { *Alloc::as_ptr(&l) }, 10);
}

#[test]
fn as_mut_ptr_allows_in_place_mutation() {
    let pool = Pool::<u32>::builder().chunk_size(2).build();

    let mut b = pool.alloc_box(1);
    let p = Box::as_mut_ptr(&mut b);
    // SAFETY: unique owner, slot kept alive by `b`.
    unsafe { *p = 42 };
    assert_eq!(*b, 42);

    let mut l = pool.alloc(2);
    let q = Alloc::as_mut_ptr(&mut l);
    // SAFETY: unique owner.
    unsafe { *q = 99 };
    assert_eq!(*l, 99);
}

#[test]
fn ptr_eq_detects_same_and_distinct_slots() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();

    let a = pool.alloc_arc(1);
    let a2 = a.clone();
    let b = pool.alloc_arc(1); // equal value, different slot
    assert!(Arc::ptr_eq(&a, &a2));
    assert!(!Arc::ptr_eq(&a, &b));

    let r = pool.alloc_rc(2);
    let r2 = r.clone();
    let s = pool.alloc_rc(2);
    assert!(Rc::ptr_eq(&r, &r2));
    assert!(!Rc::ptr_eq(&r, &s));
}

#[test]
fn get_mut_branches() {
    let pool = Pool::<u32>::builder().chunk_size(4).build();

    let mut a = pool.alloc_arc(1);
    assert!(Arc::get_mut(&mut a).is_some());
    let a2 = a.clone();
    assert!(Arc::get_mut(&mut a).is_none());
    drop(a2);
    *Arc::get_mut(&mut a).unwrap() = 2;
    assert_eq!(*a, 2);

    let mut r = pool.alloc_rc(1);
    assert!(Rc::get_mut(&mut r).is_some());
    let r2 = r.clone();
    assert!(Rc::get_mut(&mut r).is_none());
    drop(r2);
    *Rc::get_mut(&mut r).unwrap() = 2;
    assert_eq!(*r, 2);
}

#[test]
fn get_mut_supports_unpinned_slices() {
    let pool = Pool::<[u32; 3]>::builder().chunk_size(4).build();

    let mut a: Arc<[u32]> = Arc::unsize(pool.alloc_arc([1, 2, 3]), Coercion::to_slice());
    let a2 = a.clone();
    assert!(Arc::get_mut(&mut a).is_none());
    drop(a2);
    Arc::get_mut(&mut a).unwrap()[1] = 20;
    assert_eq!(&*a, &[1, 20, 3]);

    let mut r: Rc<[u32]> = Rc::unsize(pool.alloc_rc([4, 5, 6]), Coercion::to_slice());
    let r2 = r.clone();
    assert!(Rc::get_mut(&mut r).is_none());
    drop(r2);
    Rc::get_mut(&mut r).unwrap()[1] = 50;
    assert_eq!(&*r, &[4, 50, 6]);
}

#[test]
fn eq_ord_forward_to_value() {
    let pool = Pool::<i32>::builder().chunk_size(8).build();

    let a = pool.alloc_box(1);
    let b = pool.alloc_box(1);
    let c = pool.alloc_box(2);
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert!(a < c);
    assert_eq!(a.cmp(&c), Ordering::Less);
    assert_eq!(a.partial_cmp(&b), Some(Ordering::Equal));

    let x = pool.alloc_arc(5);
    let y = pool.alloc_arc(5);
    assert_eq!(x, y);
    let r1 = pool.alloc_rc(3);
    let r2 = pool.alloc_rc(4);
    assert!(r1 < r2);
    let l1 = pool.alloc(7);
    let l2 = pool.alloc(7);
    assert_eq!(l1, l2);
    assert_eq!(l1.partial_cmp(&l2), Some(Ordering::Equal));
    assert_eq!(l1.cmp(&l2), Ordering::Equal);
    let mut local_set = HashSet::new();
    local_set.insert(l1);
    local_set.insert(l2);
    assert_eq!(local_set.len(), 1);
}

#[test]
fn handles_work_as_set_keys() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();

    let mut hset = HashSet::new();
    hset.insert(pool.alloc_box(1));
    hset.insert(pool.alloc_box(1)); // equal value -> deduplicated
    hset.insert(pool.alloc_box(2));
    assert_eq!(hset.len(), 2);

    let mut bset = BTreeSet::new();
    bset.insert(pool.alloc_rc(3));
    bset.insert(pool.alloc_rc(1));
    bset.insert(pool.alloc_rc(2));
    let collected: Vec<u32> = bset.iter().map(|h| **h).collect();
    assert_eq!(collected, vec![1, 2, 3]);
}

#[test]
fn debug_and_display() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();
    assert!(format!("{pool:?}").contains("Pool"));

    let b = pool.alloc_box(1);
    let l = pool.alloc(2);
    let a = pool.alloc_arc(3);
    let r = pool.alloc_rc(4);
    assert_eq!(format!("{b:?}"), "1");
    assert_eq!(format!("{b}"), "1");
    assert_eq!(format!("{l:?}"), "2");
    assert_eq!(format!("{l}"), "2");
    assert_eq!(format!("{a:?}"), "3");
    assert_eq!(format!("{a}"), "3");
    assert_eq!(format!("{r:?}"), "4");
    assert_eq!(format!("{r}"), "4");
}

#[test]
fn formatting_forwards_and_pointer_works() {
    let pool = Pool::<i32>::builder().chunk_size(4).build();

    let b = pool.alloc_box(-5);
    assert_eq!(format!("{b:?}"), "-5");
    assert_eq!(format!("{b}"), "-5");
    assert!(format!("{b:p}").starts_with("0x"));

    let a = pool.alloc_arc(1);
    assert_eq!(format!("{a:?}"), "1");
    assert!(format!("{a:p}").starts_with("0x"));

    let r = pool.alloc_rc(2);
    assert_eq!(format!("{r}"), "2");
    assert!(format!("{r:p}").starts_with("0x"));

    let l = pool.alloc(3);
    assert_eq!(format!("{l:?}"), "3");
    assert!(format!("{l:p}").starts_with("0x"));
}

fn assert_unpin<T: Unpin>(_: &T) {}

#[test]
fn box_into_pin_and_from_are_preserved() {
    let pool = Pool::<u32>::builder().chunk_size(2).build();

    let b = pool.alloc_box(1);
    assert_unpin(&b);
    let pb: Pin<Box<u32>> = Box::into_pin(b);
    assert_eq!(*pb, 1);

    let pb: Pin<Box<u32>> = pool.alloc_box(2).into();
    assert_eq!(*pb, 2);
}

#[test]
fn box_assume_init_pin_round_trips() {
    let pool = Pool::<u32>::new();
    let mut ub = pool.alloc_uninit_box();
    ub.write(11);
    // SAFETY: just initialized.
    let b = unsafe { Box::assume_init_pin(Box::into_pin(ub)) };
    assert_eq!(*b, 11);
}

struct PinnedValue {
    value: u32,
    _pin: PhantomPinned,
}

#[test]
fn box_pins_phantom_pinned_value() {
    let pool = Pool::<PinnedValue>::new();
    let pinned: Pin<Box<PinnedValue>> = pool
        .alloc_box(PinnedValue {
            value: 1,
            _pin: PhantomPinned,
        })
        .into();
    assert_eq!(pinned.value, 1);
}

#[test]
fn fresh_pinned_shared_clones_keep_the_value_address() {
    let pool = Pool::<PinnedValue>::builder().chunk_size(4).build();

    let a = pool.alloc_arc_pin(PinnedValue {
        value: 2,
        _pin: PhantomPinned,
    });
    let a_address = from_ref(a.as_ref().get_ref());
    let a2 = a.clone();
    assert_eq!(a_address, from_ref(a2.as_ref().get_ref()));
    assert_eq!(a.value, 2);
    assert_eq!(a2.value, 2);

    let r = pool.alloc_rc_pin(PinnedValue {
        value: 3,
        _pin: PhantomPinned,
    });
    let r_address = from_ref(r.as_ref().get_ref());
    let r2 = r.clone();
    assert_eq!(r_address, from_ref(r2.as_ref().get_ref()));
    assert_eq!(r.value, 3);
    assert_eq!(r2.value, 3);
}

#[test]
fn pinned_shared_constructor_variants() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();

    assert_eq!(*pool.alloc_arc_pin_with(|| 1), 1);
    assert_eq!(*pool.try_alloc_arc_pin(2).unwrap(), 2);
    assert_eq!(*pool.try_alloc_arc_pin_with(|| 3).unwrap(), 3);
    assert_eq!(*pool.alloc_rc_pin_with(|| 4), 4);
    assert_eq!(*pool.try_alloc_rc_pin(5).unwrap(), 5);
    assert_eq!(*pool.try_alloc_rc_pin_with(|| 6).unwrap(), 6);
}

fn assert_send<T: Send>() {}
fn assert_send_sync<T: Send + Sync>() {}

// Non-atomic handle access requires these negative auto-trait guarantees.
static_assertions::assert_not_impl_any!(Rc<u32>: Send, Sync);
static_assertions::assert_not_impl_any!(Alloc<'static, u32>: Send, Sync);

#[test]
fn auto_trait_bounds() {
    assert_send::<Pool<u32>>();
    assert_send::<Box<u32>>();
    assert_send_sync::<Arc<u32>>();
}
