// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Basic usage of `multitude::Arena` and the various smart pointer types.

#![allow(clippy::unwrap_used, reason = "example code")]
#![allow(clippy::missing_panics_doc, reason = "example code")]
#![allow(clippy::std_instead_of_core, reason = "example uses std::thread")]

use std::thread;
use std::vec::Vec as StdVec;

use multitude::vec::{CollectIn, Vec};
use multitude::{Arc, Arena, Box};

fn main() {
    let arena = Arena::new();

    let a: Arc<u32> = arena.alloc_arc(42);
    let b = a.clone();
    println!("a = {}, b = {}, ptr_eq = {}", *a, *b, Arc::ptr_eq(&a, &b));

    let mut owned: Box<StdVec<i32>> = arena.alloc_box(vec![1, 2, 3]);
    owned.push(4);
    println!("owned = {:?}", *owned);
    drop(owned);

    let mut bump_ref = arena.alloc(7_u32);
    *bump_ref *= 6;
    let bump_str = arena.alloc_str("hello, arena allocation");
    println!("bump_ref = {bump_ref}, bump_str = {bump_str}");

    let name: Arc<str> = arena.alloc_str_arc("Alice");
    println!("name = {} (len = {})", &*name, name.len());

    let greeting = multitude::strings::format!(in &arena, "Hello, {name}!");
    println!("greeting = {}", &*greeting);

    let mut v: Vec<u64, _> = arena.alloc_vec();
    for i in 0..5 {
        v.push(i * 100);
    }
    println!("v = {v:?}");
    let frozen: Arc<[u64], _> = Arc::from(v);
    println!("frozen = {frozen:?}");

    let squares: Vec<i64, _> = (1..=5).map(|i| i * i).collect_in(&arena);
    println!("squares = {squares:?}");

    let shared: Arc<u64> = arena.alloc_arc(99);
    let s2 = shared.clone();
    let h = thread::spawn(move || *s2);
    println!("from main: {}, from thread: {}", *shared, h.join().unwrap());

    #[cfg(feature = "stats")]
    println!("arena stats: {:?}", arena.stats());
}
