// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A heterogeneous collection of trait objects backed by one pool.
//!
//! This is the case a typed pool cannot serve. The implementations of a trait
//! have different sizes, so a `Pool<T>` would need one pool per implementation
//! and a way to keep them all alive. A [`MultiPool`] holds them together, and
//! the handles coerce to `dyn Trait` so callers see a uniform collection.
//!
//! Run with `cargo run --example multi_pool_dyn_dispatch`.

use plurality::{Box, MultiPool, coerce};

/// A step in a rendering pipeline.
trait Stage {
    fn name(&self) -> &str;
    fn apply(&self, pixel: u32) -> u32;
}

struct Tint {
    name: String,
    mask: u32,
}

/// Deliberately much larger than `Tint`, so the two land in different layouts.
struct Convolve {
    name: String,
    kernel: [i32; 9],
    divisor: u32,
}

struct Clamp {
    name: &'static str,
    ceiling: u32,
}

impl Stage for Tint {
    fn name(&self) -> &str {
        &self.name
    }

    fn apply(&self, pixel: u32) -> u32 {
        pixel & self.mask
    }
}

impl Stage for Convolve {
    fn name(&self) -> &str {
        &self.name
    }

    fn apply(&self, pixel: u32) -> u32 {
        let weight: u32 = self.kernel.iter().map(|k| k.unsigned_abs()).sum();
        pixel.wrapping_mul(weight).wrapping_div(self.divisor)
    }
}

impl Stage for Clamp {
    fn name(&self) -> &str {
        self.name
    }

    fn apply(&self, pixel: u32) -> u32 {
        pixel.min(self.ceiling)
    }
}

fn main() {
    let pool = MultiPool::new();

    // Each stage is a different concrete type of a different size. They are
    // allocated from one pool, then unsized to a common handle type.
    let stages: Vec<Box<dyn Stage>> = vec![
        Box::unsize(
            pool.alloc_box(Tint {
                name: String::from("tint"),
                mask: 0x0000_FFFF,
            }),
            coerce!(dyn Stage),
        ),
        Box::unsize(
            pool.alloc_box(Convolve {
                name: String::from("sharpen"),
                kernel: [0, -1, 0, -1, 5, -1, 0, -1, 0],
                divisor: 3,
            }),
            coerce!(dyn Stage),
        ),
        Box::unsize(
            pool.alloc_box(Clamp {
                name: "clamp",
                ceiling: 4096,
            }),
            coerce!(dyn Stage),
        ),
    ];

    println!("pipeline of {} stages across {} layouts", stages.len(), pool.layouts());

    let mut pixel = 123_456_u32;
    for stage in &stages {
        pixel = stage.apply(pixel);
        println!("  after {:<8} pixel = {pixel}", stage.name());
    }

    // An unsized handle still knows how to return its own slot: it recovers
    // the pool from the value's address, so no stage needs to remember which
    // internal pool served it.
    let clamp_slot = core::ptr::from_ref(&*stages[2]).cast::<u8>();
    println!();
    println!("live before teardown = {}", pool.len());
    drop(stages);
    println!("live after teardown  = {}", pool.len());

    // The slots stay warm, so rebuilding a stage lands in the memory its
    // predecessor released instead of asking the allocator for more.
    let capacity_before = pool.capacity();
    let rebuilt: Box<dyn Stage> = Box::unsize(
        pool.alloc_box(Clamp {
            name: "clamp",
            ceiling: 255,
        }),
        coerce!(dyn Stage),
    );
    println!(
        "rebuilt {} in the slot it released: {}, without growing: {}",
        rebuilt.name(),
        core::ptr::from_ref(&*rebuilt).cast::<u8>() == clamp_slot,
        pool.capacity() == capacity_before
    );
}
