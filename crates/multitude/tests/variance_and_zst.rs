// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time variance assertions and ZST regression tests.

#![allow(clippy::needless_lifetimes, reason = "explicit lifetimes clarify variance tests")]
#![allow(clippy::missing_const_for_fn, reason = "variance assertions are never called; const is irrelevant")]
#![allow(
    single_use_lifetimes,
    reason = "variance assertions require two lifetimes even though 'b appears only once"
)]
#![allow(dead_code, reason = "variance assertion functions are never called at runtime")]

use std::thread;

use multitude::{Arc, Arena, Box};

// Covariance assertions (compile-only)
//
// If these functions compile, the types are covariant in T (and
// transitively in the lifetime inside T). If a type were invariant,
// the compiler would reject the implicit lifetime shortening.

fn _assert_box_covariant<'a, 'b: 'a>(x: Box<&'b str>) -> Box<&'a str> {
    x
}

fn _assert_arc_covariant<'a, 'b: 'a>(x: Arc<&'b str>) -> Arc<&'a str> {
    x
}

fn _assert_box_slice_covariant<'a, 'b: 'a>(x: Box<[&'b str]>) -> Box<[&'a str]> {
    x
}

fn _assert_arc_slice_covariant<'a, 'b: 'a>(x: Arc<[&'b str]>) -> Arc<[&'a str]> {
    x
}

// ZST allocation regression tests (bumpalo #289)
//
// Multitude's sentinel has bump=1, total_size=0. Any allocation (even
// ZST) computes `end >= 1 > 0 = total_size`, failing the fit-check
// and routing through the slow path to a real chunk. This means the
// shared static sentinel is NEVER written to — no data race is possible.

#[test]
fn zst_alloc_arc_succeeds() {
    let arena = Arena::new();
    let a = arena.alloc_arc(());
    let b = a.clone();
    let h = thread::spawn(move || *b);
    assert_eq!(*a, ());
    assert_eq!((), h.join().unwrap());
}

#[test]
fn zst_alloc_box_succeeds() {
    let arena = Arena::new();
    let a = arena.alloc_box(());
    let b = arena.alloc_box(());
    assert_eq!(*a, ());
    assert_eq!(*b, ());
}
