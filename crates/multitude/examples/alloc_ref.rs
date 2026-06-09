// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Simple-reference allocations: `&'arena mut T` with no per-pointer
//! refcount, lifetime-coupled to the arena.

#![allow(clippy::unwrap_used, reason = "example code")]
#![allow(clippy::missing_panics_doc, reason = "example code")]
#![allow(clippy::std_instead_of_core, reason = "example uses std")]
#![allow(dead_code, reason = "Debug printout consumes the fields")]
#![allow(clippy::cast_possible_truncation, reason = "example data is small")]
#![allow(clippy::cast_sign_loss, reason = "example data is small")]

use multitude::Arena;

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

    // -- Single value --------------------------------------------------
    // `alloc` returns `&'a mut T` borrowed from the arena reference.
    let counter: &mut u32 = arena.alloc(0);
    *counter += 1;
    *counter += 10;
    println!("counter = {counter}");

    // -- String --------------------------------------------------------
    // `alloc_str` copies `&str` into the arena and returns `&'a mut str`.
    let greeting: &mut str = arena.alloc_str("hello, world");
    println!("greeting = {greeting}");

    // -- Slice (Copy) --------------------------------------------------
    let primes: &mut [u32] = arena.alloc_slice_copy([2, 3, 5, 7, 11, 13]);
    primes[0] = 1; // we own the slice, can mutate
    println!("primes = {primes:?}");

    // -- Slice (filled by closure, supports T: Drop) -------------------
    let tokens: &mut [Token] = arena.alloc_slice_fill_with(3, |i| Token {
        kind: "ident",
        text: format!("name_{i}"),
    });
    println!("tokens = {tokens:#?}");

    // -- Many disjoint &mut references coexist -------------------------
    // Simple references: each `alloc` reborrows `&arena` for the
    // returned reference's lifetime; the &mut values are at disjoint
    // memory regions so they're all simultaneously valid.
    let xs: Vec<&mut u64> = (0..5).map(|i| arena.alloc(i as u64)).collect();
    for r in &xs {
        // Read each one (they're &mut so read via *_)
        println!("x = {}", **r);
    }

    // -- Coexists with refcount smart pointers --------------------------------
    // simple references and `ArenaRc` smart pointers share the same chunk
    // pool. Use the cheap one when you don't need the smart pointer to outlive
    // the arena; use ArenaRc when you do.
    let rc = arena.alloc_arc(Point { x: 3.0, y: 4.0 });
    let bump_ref: &mut Vec<i32> = arena.alloc(vec![1, 2, 3]);
    bump_ref.push(4);
    println!("rc = {:?}, bump_ref = {bump_ref:?}", &*rc);

    // The bump_ref's lifetime expires when `arena` is dropped (compile
    // error if we tried to use it after). The rc can live on:
    drop(arena);
    println!("rc still valid after arena drop: {:?}", &*rc);
}
