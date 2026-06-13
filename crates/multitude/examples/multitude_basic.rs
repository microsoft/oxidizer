// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic usage of `multitude::Arena` and the various smart pointer types.

#![allow(clippy::unwrap_used, reason = "example code")]
#![allow(clippy::missing_panics_doc, reason = "example code")]
#![allow(clippy::std_instead_of_core, reason = "example uses std::thread")]

use multitude::vec::{CollectIn, Vec};
use multitude::{Arc, Arena, Box};

fn main() {
    let arena = Arena::new();

    // -- Reference-counted local smart pointer --------------------------------
    let a: Arc<u32> = arena.alloc_arc(42);
    let b = a.clone();
    println!("a = {}, b = {}, ptr_eq = {}", *a, *b, Arc::ptr_eq(&a, &b));

    // -- Owned single smart pointer (mutable, immediate Drop) ----------------
    let mut owned: Box<std::vec::Vec<i32>> = arena.alloc_box(vec![1, 2, 3]);
    owned.push(4);
    println!("owned = {:?}", *owned);
    drop(owned);

    // -- Simple reference (cheapest; lifetime-bound to arena) --------
    let bump_ref: &mut u32 = arena.alloc(7);
    *bump_ref *= 6;
    let bump_str: &mut str = arena.alloc_str("hello, simple reference");
    println!("bump_ref = {bump_ref}, bump_str = {bump_str}");

    // -- Single-pointer immutable string smart pointer -----------------------
    let name: Arc<str> = arena.alloc_str_arc("Alice");
    println!("name = {} (len = {})", &*name, name.len());

    // -- format! macro returning ArenaString ----------------------------
    let greeting = multitude::strings::format!(in &arena, "Hello, {name}!");
    println!("greeting = {}", &*greeting);

    // -- Arena-backed vector ------------------------------------------
    let mut v: Vec<u64, _> = arena.alloc_vec();
    for i in 0..5 {
        v.push(i * 100);
    }
    println!("v = {v:?}");
    let frozen: Arc<[u64], _> = Arc::from(v);
    println!("frozen = {frozen:?}");

    // -- Collect from an iterator -------------------------------------
    let squares: Vec<i64, _> = (1..=5).map(|i| i * i).collect_in(&arena);
    println!("squares = {squares:?}");

    // -- Cross-thread sharing via ArenaArc ----------------------------
    let shared: Arc<u64> = arena.alloc_arc(99);
    let s2 = shared.clone();
    let h = std::thread::spawn(move || *s2);
    println!("from main: {}, from thread: {}", *shared, h.join().unwrap());

    #[cfg(feature = "stats")]
    println!("arena stats: {:?}", arena.stats());
}
