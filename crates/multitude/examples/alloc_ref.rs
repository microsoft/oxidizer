// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-lifetime allocations: owning [`multitude::Alloc`] handles with no
//! per-pointer refcount, lifetime-coupled to the arena.

#![allow(clippy::unwrap_used, reason = "example code")]
#![allow(clippy::missing_panics_doc, reason = "example code")]
#![allow(clippy::std_instead_of_core, reason = "example uses std")]
#![allow(dead_code, reason = "Debug printout consumes the fields")]
#![allow(clippy::cast_possible_truncation, reason = "example data is small")]
#![allow(clippy::cast_sign_loss, reason = "example data is small")]

use multitude::{Alloc, Arena};

#[derive(Debug)]
struct Token {
    kind: &'static str,
    text: String,
}

#[derive(Debug)]
struct Point {
    x: f64,
    y: f64,
}

fn main() {
    let arena = Arena::new();

    // Single value
    // `alloc` returns an owning `Alloc<u32>` borrowed from the arena.
    let mut counter = arena.alloc(0_u32);
    *counter += 1;
    *counter += 10;
    println!("counter = {counter}");

    // String
    // `alloc_str` copies `&str` into the arena and returns `Alloc<str>`.
    let greeting = arena.alloc_str("hello, world");
    println!("greeting = {greeting}");

    // Copy slice
    let mut primes = arena.alloc_slice_copy([2, 3, 5, 7, 11, 13]);
    primes[0] = 1; // we own the slice, can mutate
    println!("primes = {:?}", &*primes);

    // Slice filled by a closure, including values with destructors
    let tokens = arena.alloc_slice_fill_with(3, |i| Token {
        kind: "ident",
        text: format!("name_{i}"),
    });
    println!("tokens = {:#?}", &*tokens);

    // Many disjoint handles coexist
    // Each `alloc` reborrows `&arena` for the returned handle's lifetime;
    // the values live at disjoint memory regions so the handles are all
    // simultaneously valid.
    let xs: Vec<Alloc<u64>> = (0..5).map(|i| arena.alloc(i as u64)).collect();
    for r in &xs {
        println!("x = {}", **r);
    }

    // Coexists with refcounted smart pointers
    // `Alloc` handles and `Arc` smart pointers share the same chunk pool. Use
    // the cheap one when you don't need the value to outlive the arena; use
    // `Arc` when you do.
    let rc = arena.alloc_arc(Point { x: 3.0, y: 4.0 });
    let mut bump_ref = arena.alloc(vec![1, 2, 3]);
    bump_ref.push(4);
    println!("rc = {:?}, bump_ref = {:?}", *rc, *bump_ref);

    // The handles' lifetimes expire when `arena` is dropped. Because each
    // `Alloc` runs its value's destructor on drop, drop the outstanding
    // handles before the arena (the `Arc` may live on):
    drop((counter, greeting, primes, tokens, xs, bump_ref));
    drop(arena);
    println!("rc still valid after arena drop: {:?}", *rc);
}
