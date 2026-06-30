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
//! `as_ptr`/`as_mut_ptr`, `ptr_eq`, `get_mut`, `into_pin` / `From` for `Pin`,
//! `Unpin`, `assume_init_pin`, the auto-trait (`Send`/`Sync`) bounds, and the
//! forwarding trait impls (`AsRef`, `AsMut`, `Borrow`, `BorrowMut`,
//! `PartialEq`/`Eq`, `PartialOrd`/`Ord`, `Hash`, `Debug`/`Display`/`Pointer`).

use core::borrow::{Borrow, BorrowMut};
use core::cmp::Ordering;
use core::pin::Pin;
use std::collections::{BTreeSet, HashSet};

use plurality::Pool;

// ── AsRef / Borrow (all handles) and AsMut / BorrowMut (unique handles) ──

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
    // AsRef<String> -> &String -> &str
    assert_eq!(AsRef::<String>::as_ref(&b).as_str(), "box");
    assert_eq!(AsRef::<String>::as_ref(&a).as_str(), "arc");
    assert_eq!(AsRef::<String>::as_ref(&r).as_str(), "rc");
    assert_eq!(AsRef::<String>::as_ref(&l).as_str(), "alloc");
    assert_eq!(takes_str(&b), 3);

    // Borrow<String>
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

// ── as_ptr / as_mut_ptr ──

#[test]
fn as_ptr_points_at_the_value_and_is_stable() {
    let pool = Pool::<u32>::builder().chunk_size(4).build();

    let b = pool.alloc_box(7);
    let p = plurality::Box::as_ptr(&b);
    // SAFETY: the box keeps the slot alive, so the pointer is valid.
    assert_eq!(unsafe { *p }, 7);
    assert_eq!(p, core::ptr::from_ref::<u32>(&*b));

    let a = pool.alloc_arc(8);
    // SAFETY: live arc.
    assert_eq!(unsafe { *plurality::Arc::as_ptr(&a) }, 8);

    let r = pool.alloc_rc(9);
    // SAFETY: live rc.
    assert_eq!(unsafe { *plurality::Rc::as_ptr(&r) }, 9);

    let l = pool.alloc(10);
    // SAFETY: live alloc.
    assert_eq!(unsafe { *plurality::Alloc::as_ptr(&l) }, 10);
}

#[test]
fn as_mut_ptr_allows_in_place_mutation() {
    let pool = Pool::<u32>::builder().chunk_size(2).build();

    let mut b = pool.alloc_box(1);
    let p = plurality::Box::as_mut_ptr(&mut b);
    // SAFETY: unique owner, slot kept alive by `b`.
    unsafe { *p = 42 };
    assert_eq!(*b, 42);

    let mut l = pool.alloc(2);
    let q = plurality::Alloc::as_mut_ptr(&mut l);
    // SAFETY: unique owner.
    unsafe { *q = 99 };
    assert_eq!(*l, 99);
}

// ── ptr_eq (shared handles) ──

#[test]
fn ptr_eq_detects_same_and_distinct_slots() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();

    let a = pool.alloc_arc(1);
    let a2 = a.clone();
    let b = pool.alloc_arc(1); // equal value, different slot
    assert!(plurality::Arc::ptr_eq(&a, &a2));
    assert!(!plurality::Arc::ptr_eq(&a, &b));

    let r = pool.alloc_rc(2);
    let r2 = r.clone();
    let s = pool.alloc_rc(2);
    assert!(plurality::Rc::ptr_eq(&r, &r2));
    assert!(!plurality::Rc::ptr_eq(&r, &s));
}

// ── get_mut (shared handles) ──

#[test]
fn get_mut_branches() {
    let pool = Pool::<u32>::builder().chunk_size(4).build();

    let mut a = pool.alloc_arc(1);
    assert!(plurality::Arc::get_mut(&mut a).is_some());
    let a2 = a.clone();
    assert!(plurality::Arc::get_mut(&mut a).is_none());
    drop(a2);

    let mut r = pool.alloc_rc(1);
    assert!(plurality::Rc::get_mut(&mut r).is_some());
    let r2 = r.clone();
    assert!(plurality::Rc::get_mut(&mut r).is_none());
    drop(r2);
}

// ── equality / ordering / hashing forward to the value ──

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

    // Across Arc/Rc/Alloc too.
    let x = pool.alloc_arc(5);
    let y = pool.alloc_arc(5);
    assert_eq!(x, y);
    let r1 = pool.alloc_rc(3);
    let r2 = pool.alloc_rc(4);
    assert!(r1 < r2);
    let l1 = pool.alloc(7);
    let l2 = pool.alloc(7);
    assert_eq!(l1, l2);
}

#[test]
fn handles_work_as_set_keys() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();

    // Hash via HashSet.
    let mut hset = HashSet::new();
    hset.insert(pool.alloc_box(1));
    hset.insert(pool.alloc_box(1)); // equal value -> deduplicated
    hset.insert(pool.alloc_box(2));
    assert_eq!(hset.len(), 2);

    // Ord via BTreeSet.
    let mut bset = BTreeSet::new();
    bset.insert(pool.alloc_rc(3));
    bset.insert(pool.alloc_rc(1));
    bset.insert(pool.alloc_rc(2));
    let collected: Vec<u32> = bset.iter().map(|h| **h).collect();
    assert_eq!(collected, vec![1, 2, 3]);
}

// ── Debug / Display / Pointer formatting ──

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
    // `Pointer` formats the slot address.
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

// ── Pin support ──

fn assert_unpin<T: Unpin>(_: &T) {}

#[test]
fn into_pin_and_from_and_unpin() {
    let pool = Pool::<u32>::builder().chunk_size(8).build();

    let b = pool.alloc_box(1);
    assert_unpin(&b);
    let pb: Pin<plurality::Box<u32>> = plurality::Box::into_pin(b);
    assert_eq!(*pb, 1);

    // `From<Self> for Pin<Self>` for each handle type.
    let pa: Pin<plurality::Arc<u32>> = pool.alloc_arc(2).into();
    assert_eq!(*pa, 2);
    let pr: Pin<plurality::Rc<u32>> = pool.alloc_rc(3).into();
    assert_eq!(*pr, 3);
    let pl: Pin<plurality::Alloc<'_, u32>> = pool.alloc(4).into();
    assert_eq!(*pl, 4);
}

#[test]
fn assume_init_pin_round_trips() {
    let pool = Pool::<u32>::builder().chunk_size(4).build();

    // Box
    let mut ub = pool.alloc_uninit_box();
    ub.write(11);
    // SAFETY: just initialized.
    let b = unsafe { plurality::Box::assume_init_pin(plurality::Box::into_pin(ub)) };
    assert_eq!(*b, 11);

    // Arc
    let mut ua = pool.alloc_uninit_arc();
    plurality::Arc::get_mut(&mut ua).unwrap().write(12);
    let pa = plurality::Arc::into_pin(ua);
    // SAFETY: initialized above.
    let a = unsafe { plurality::Arc::assume_init_pin(pa) };
    assert_eq!(*a, 12);

    // Rc
    let mut ur = pool.alloc_uninit_rc();
    plurality::Rc::get_mut(&mut ur).unwrap().write(13);
    // SAFETY: initialized above.
    let r = unsafe { plurality::Rc::assume_init_pin(plurality::Rc::into_pin(ur)) };
    assert_eq!(*r, 13);

    // Alloc
    let mut ul = pool.alloc_uninit();
    ul.write(14);
    // SAFETY: just initialized.
    let l = unsafe { plurality::Alloc::assume_init_pin(plurality::Alloc::into_pin(ul)) };
    assert_eq!(*l, 14);
}

// ── auto-trait (Send / Sync) bounds ──

fn assert_send<T: Send>() {}
fn assert_send_sync<T: Send + Sync>() {}

// `Rc` and `Alloc` must stay `!Send + !Sync`: their refcount / value access is
// non-atomic and single-threaded, so crossing a thread boundary would be
// unsound. The property holds today via their `NonNull` field (raw pointers
// carry neither auto trait), but that is an implementation detail — these
// compile-time assertions lock it in so a future field change or a stray
// `unsafe impl Send` can never silently make them thread-crossable.
static_assertions::assert_not_impl_any!(plurality::Rc<u32>: Send, Sync);
static_assertions::assert_not_impl_any!(plurality::Alloc<'static, u32>: Send, Sync);

#[test]
fn auto_trait_bounds() {
    assert_send::<Pool<u32>>();
    assert_send::<plurality::Box<u32>>();
    assert_send_sync::<plurality::Arc<u32>>();
}
