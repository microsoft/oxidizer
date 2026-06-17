// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Materializes an **object tree** from a mocked, statically-allocated data
//! backend into an arena-backed, typed object model ([`multitude::Arc`]).
//!
//! Object models often use a fixed-size `Value` (a tagged union) plus a few
//! reference-counted leaf types, each independently heap-allocated. This builds
//! the same model in an arena instead, demonstrating that it:
//!
//! - **stays small** — arena `Arc`s are *thin* (8 bytes) even for DSTs, so
//!   [`object::Value`] is **16 bytes**;
//! - **outlives the arena** — the `Arc` handles keep their chunks alive after
//!   the arena is dropped;
//! - **allocates better** — one tree takes a few large chunk allocations rather
//!   than one per node, measured below with [`alloc_tracker`].
//!
//! Layers: [`mod@backend`] (the data source), [`mod@object`] (the [`Value`]
//! model), and [`mod@loader`] (materializes the tree from a [`backend::DataAccess`]).
//!
//! Run with: `cargo run --release --example object_tree --features utf16`
#![allow(clippy::unwrap_used, reason = "example code")]
#![allow(clippy::missing_panics_doc, reason = "example code")]
#![allow(clippy::std_instead_of_core, reason = "example uses std::time/std::sync")]
#![allow(dead_code, reason = "the Value model defines variants this example does not read back")]

mod backend;
mod loader;
mod object;
mod rc;

use std::time::Instant;

use alloc_tracker::{Allocator, Session};
use multitude::Arena;

use crate::backend::DataAccess;
use crate::object::Value;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

const ITERATIONS: u32 = 10;

/// Creates an arena and warms it with a throwaway load, then resets it for
/// reuse so its chunks are already allocated.
fn create_warmed_up_arena(dataset: DataAccess<'_>) -> Arena {
    let mut arena = Arena::new();
    let _ = loader::load(&arena, dataset);
    arena.reset();
    arena
}

fn main() {
    let dataset = backend::make_dataset();

    // 1. Fixed, small per-instance `Value` size.
    println!("== value size ==");
    println!("Value (multitude::Arc, thin) = {} bytes", size_of::<Value>());
    println!();

    // 2. Object count and memory footprint of one materialized tree.
    let probe_arena = Arena::new();
    let tree = loader::load(&probe_arena, dataset);
    let stats = object::measure(&tree);
    println!("== tree shape ==");
    println!("objects (Value nodes) : {}", stats.objects);
    println!("memory used by tree   : {} bytes", stats.bytes);
    println!();
    drop(tree);
    drop(probe_arena);

    // 3. Allocation profile + timing: warm the arena once, then reset and reuse
    //    it each iteration.
    let session = Session::new();
    let mut arena = create_warmed_up_arena(dataset);

    let arena_op = session.operation("load-tree");
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        let _span = arena_op.measure_thread();
        let _ = loader::load(&arena, dataset);
        arena.reset();
    }
    let elapsed = start.elapsed();

    println!("== timing ({ITERATIONS} iterations) ==");
    println!("arena : {}ms ({}ms/tree)", elapsed.as_millis(), (elapsed / ITERATIONS).as_millis());
    println!();

    println!("== allocation profile (per tree) ==");
    session.print_to_stdout();
}
