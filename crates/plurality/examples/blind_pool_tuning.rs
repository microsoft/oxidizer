// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sizing a blind pool's growth and bounding its memory.
//!
//! A blind pool creates an internal pool the first time it sees a layout, so
//! its growth policy has to work for layouts it has not met yet. The builder
//! expresses that policy in bytes or in slots, and caps how far growth may go.
//!
//! Run with `cargo run --example blind_pool_tuning`.

#![allow(clippy::std_instead_of_core, reason = "example prints to stdout")]

use plurality::{AllocError, BlindPool};

struct Small(#[expect(dead_code, reason = "the value's layout is what matters here")] u8);
struct Large(#[expect(dead_code, reason = "the value's layout is what matters here")] [u64; 32]);

fn main() {
    // Sizing by bytes gives every layout a comparable amount of memory per
    // growth step, so a large value does not commit far more than a small one.
    let by_bytes = BlindPool::builder().chunk_bytes(4096).build();
    by_bytes.alloc_box(Small(1));
    by_bytes.alloc_box(Large([0; 32]));
    println!("chunk_bytes(4096):");
    println!(
        "  Small -> {} slots per chunk, Large -> {} slots per chunk",
        by_bytes.chunk_size_of::<Small>(),
        by_bytes.chunk_size_of::<Large>()
    );

    // Sizing by slots gives every layout the same increment of capacity
    // instead, at the cost of very different memory per step.
    let by_slots = BlindPool::builder().chunk_size(64).build();
    by_slots.alloc_box(Small(1));
    by_slots.alloc_box(Large([0; 32]));
    println!("chunk_size(64):");
    println!(
        "  Small -> {} slots per chunk, Large -> {} slots per chunk",
        by_slots.chunk_size_of::<Small>(),
        by_slots.chunk_size_of::<Large>()
    );

    // Growth is capped from both directions: `max_chunks` limits how far any
    // one layout may grow, and `max_layouts` limits how many distinct layouts
    // will ever be served. Together they bound the memory the pool can reach,
    // though the byte figure depends on which layouts are admitted, since each
    // layout sizes its own chunks.
    //
    // The settings below are the smallest that still reach both caps: a single
    // chunk of two slots per layout, and room for one layout beyond the first.
    println!();
    let bounded = BlindPool::builder().chunk_size(2).max_chunks(1).max_layouts(2).build();

    // Fill the only chunk this layout will ever get.
    let held: Vec<_> = (0..2).map(|i| bounded.alloc_box(Small(i))).collect();
    println!("bounded pool: {} live, capacity {}", bounded.len(), bounded.capacity());

    // The fallible entry points report exhaustion instead of panicking, so
    // reaching a cap is an ordinary condition to handle rather than a crash.
    match bounded.try_alloc_box(Small(9)) {
        Ok(_) => println!("unexpected room for another Small"),
        Err(error) => println!("third Small rejected: {}", describe(error)),
    }

    // The layout cap still has room, so an unrelated type is admitted and gets
    // an internal pool of its own.
    bounded.alloc_box(1_u64);
    println!("layouts now = {}", bounded.layouts());

    // The next layout is refused. The cap applies to admitting a layout at
    // all, so a type the pool has never seen is rejected before any memory is
    // requested for it.
    match bounded.try_alloc_box([0_u64; 32]) {
        Ok(_) => println!("unexpected room for a third layout"),
        Err(error) => println!("third layout rejected: {}", describe(error)),
    }

    drop(held);
}

fn describe(error: AllocError) -> &'static str {
    if error.is_capacity_exhausted() {
        "the pool is at its configured capacity"
    } else {
        "the allocator could not provide memory"
    }
}
