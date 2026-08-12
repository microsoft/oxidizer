// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! One pool holding values of unrelated types.
//!
//! A [`BlindPool`] moves the type parameter from the pool to the allocation, so
//! a single pool object backs a mixed working set. Each type is routed to the
//! internal pool serving its layout, so a small value is never charged for the
//! largest type the pool holds.
//!
//! Run with `cargo run --example blind_pool_basic`.

#![allow(clippy::std_instead_of_core, reason = "example prints to stdout")]

use plurality::BlindPool;

#[derive(Debug)]
struct Connection {
    #[expect(dead_code, reason = "printed through the Debug impl")]
    id: u64,
    #[expect(dead_code, reason = "printed through the Debug impl")]
    peer: String,
}

fn main() {
    let pool = BlindPool::new();

    // Types need nothing in common. The pool never names them.
    let count = pool.alloc_box(42_u64);
    let label = pool.alloc_box(String::from("session"));
    let flags = pool.alloc_box([true, false, true]);
    let connection = pool.alloc_box(Connection {
        id: 7,
        peer: String::from("10.0.0.1"),
    });

    println!("count      = {}", *count);
    println!("label      = {}", *label);
    println!("flags      = {:?}", *flags);
    println!("connection = {connection:?}");

    // The same handles a typed pool produces, with the same guarantees.
    let shared = pool.alloc_arc(3.5_f64);
    let borrowed = pool.alloc(u8::MAX);
    println!("shared     = {}", *shared);
    println!("borrowed   = {}", *borrowed);

    // Distinct layouts get distinct internal pools. Types that happen to share
    // a layout share one, which is why the count below is not simply the
    // number of types used: `u64` and `f64` agree on size and alignment.
    println!();
    println!("live values  = {} (the borrowed handle is not counted)", pool.len());
    println!("layouts seen = {}", pool.layouts());

    // Capacity is a per-layout question, so the accessors take the type. The
    // `u64` figures cover the `f64` too, since they share an internal pool.
    println!(
        "u64: {} live of {} slots; String: {} live of {} slots",
        pool.len_of::<u64>(),
        pool.capacity_of::<u64>(),
        pool.len_of::<String>(),
        pool.capacity_of::<String>()
    );

    // Dropping a handle runs the value's destructor and returns its slot to
    // the pool that served it, ready for the next value of that layout.
    let freed = &raw const *label;
    drop(label);
    println!("after dropping the String: {} live", pool.len_of::<String>());

    let reused = pool.alloc_box(String::from("recycled"));
    println!("{:?} took over the slot just freed: {}", *reused, &raw const *reused == freed);
}
