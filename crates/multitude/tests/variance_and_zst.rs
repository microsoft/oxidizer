// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time variance assertions and ZST regression tests.
//!
//! - **Covariance** (bumpalo #170): verifies that `Box`, `Rc`, and `Arc`
//!   are covariant in `T`, so `Container<&'long str>` can be used where
//!   `Container<&'short str>` is expected.
//!
//! - **ZST sentinel safety** (bumpalo #289): verifies that zero-sized
//!   allocations work correctly (the shared sentinel is never mutated
//!   because the fit-check always fails for it).

#![allow(clippy::needless_lifetimes, reason = "explicit lifetimes clarify variance tests")]
#![allow(clippy::missing_const_for_fn, reason = "variance assertions are never called; const is irrelevant")]
#![allow(
    single_use_lifetimes,
    reason = "variance assertions require two lifetimes even though 'b appears only once"
)]
#![allow(dead_code, reason = "variance assertion functions are never called at runtime")]

use multitude::{Arc, Arena, Box, Rc};

// Covariance assertions (compile-only)
//
// If these functions compile, the types are covariant in T (and
// transitively in the lifetime inside T). If a type were invariant,
// the compiler would reject the implicit lifetime shortening.

fn _assert_box_covariant<'a, 'b: 'a>(x: Box<&'b str>) -> Box<&'a str> {
    x
}

fn _assert_rc_covariant<'a, 'b: 'a>(x: Rc<&'b str>) -> Rc<&'a str> {
    x
}

fn _assert_arc_covariant<'a, 'b: 'a>(x: Arc<&'b str>) -> Arc<&'a str> {
    x
}

fn _assert_box_slice_covariant<'a, 'b: 'a>(x: Box<[&'b str]>) -> Box<[&'a str]> {
    x
}

fn _assert_rc_slice_covariant<'a, 'b: 'a>(x: Rc<[&'b str]>) -> Rc<[&'a str]> {
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
fn zst_alloc_rc_succeeds() {
    let arena = Arena::new();
    let a = arena.alloc_rc(());
    let b = arena.alloc_rc(());
    // Both allocations succeed; values are accessible.
    assert_eq!(*a, ());
    assert_eq!(*b, ());
}

#[test]
fn zst_alloc_arc_succeeds() {
    let arena = Arena::new();
    let a = arena.alloc_arc(());
    let b = a.clone();
    let h = std::thread::spawn(move || *b);
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

#[test]
fn zst_many_allocations_do_not_panic() {
    let arena = Arena::new();
    // Allocate many ZSTs to stress the path. The sentinel must never
    // be mutated; all allocs go through the slow path on the first
    // call, then fit in the real chunk thereafter.
    let handles: Vec<Rc<()>> = (0..1000).map(|_| arena.alloc_rc(())).collect();
    assert_eq!(handles.len(), 1000);
}
