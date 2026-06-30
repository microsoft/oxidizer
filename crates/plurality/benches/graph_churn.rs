// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![allow(
    clippy::allow_attributes,
    clippy::clone_on_ref_ptr,
    clippy::unwrap_used,
    clippy::assertions_on_result_states,
    clippy::cast_possible_truncation,
    clippy::collection_is_never_read,
    clippy::items_after_statements,
    clippy::many_single_char_names,
    clippy::borrow_as_ptr,
    clippy::doc_markdown,
    clippy::cast_precision_loss,
    reason = "test and benchmark code"
)]

//! Graph-churn benchmark: build and continuously mutate a large node graph,
//! allocating ~1,000,000 nodes with a realistic random add/remove pattern.
//!
//! The exact same precomputed operation stream is replayed against two
//! allocation backends:
//!
//!   1. `plurality::Pool` + `plurality::Box` (this crate).
//!   2. straight `std::boxed::Box` backed by the **mimalloc** global allocator.
//!
//! Both runs build identical node contents, in identical slab positions, in the
//! identical order — verified by an end-to-end checksum that must match. The
//! workload also chases graph edges on every insert, so it exercises memory
//! locality (where the pool's contiguous chunks tend to win) as well as raw
//! allocation throughput.
//!
//! Run with: `cargo bench --bench graph_churn`.

use core::ops::Deref;
use std::time::{Duration, Instant};

// Use mimalloc for the whole process. The "mimalloc" backend allocates every
// node through it; the pool backend only allocates its (few, large) chunks
// through it, serving individual nodes from its own slots.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Max out-degree stored inline in each node (no secondary heap allocation, so
/// the comparison stays focused on node allocation, not inner `Vec`s).
const DEG: usize = 8;
/// Maximum number of simultaneously-live nodes (the slab capacity).
const CAP: usize = 250_000;
/// Total number of node allocations to perform.
const TARGET_INSERTS: usize = 1_000_000;
/// Timed repetitions; the best (min) wall time is reported.
const ITERS: usize = 5;

/// A graph node. Fixed size (~96 bytes), no inner heap allocation.
#[derive(Clone)]
struct Node {
    id: u64,
    payload: [u64; 6],
    degree: u32,
    neighbors: [u32; DEG],
}

/// A single replayable operation. Generated once, replayed by every backend.
enum Op {
    Insert {
        slot: u32,
        id: u64,
        degree: u8,
        neighbors: [u32; DEG],
    },
    Remove {
        slot: u32,
    },
}

/// Tiny deterministic PRNG (SplitMix64) so the workload needs no extra deps and
/// is byte-for-byte reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Generates the shared operation stream: a churny add/remove sequence that
/// performs exactly `TARGET_INSERTS` allocations while keeping the live set
/// bounded by `CAP`.
fn generate_ops() -> Vec<Op> {
    let mut rng = Rng(0x0C0F_FEE1_2345_6789);
    let mut free: Vec<u32> = (0..CAP as u32).rev().collect();
    let mut live: Vec<u32> = Vec::with_capacity(CAP);
    let mut ops: Vec<Op> = Vec::with_capacity(TARGET_INSERTS * 2);
    let mut inserts = 0usize;
    let mut next_id = 0u64;

    while inserts < TARGET_INSERTS {
        let do_insert = if live.is_empty() {
            true
        } else if free.is_empty() {
            false
        } else {
            // 60% inserts / 40% removes -> grows to CAP then churns steadily.
            rng.below(100) < 60
        };

        if do_insert {
            let slot = free.pop().expect("free slot available");
            let mut neighbors = [0u32; DEG];
            let mut degree = 0u8;
            if !live.is_empty() {
                let k = rng.below(DEG as u64 + 1) as usize;
                for n in neighbors.iter_mut().take(k) {
                    *n = live[rng.below(live.len() as u64) as usize];
                    degree += 1;
                }
            }
            ops.push(Op::Insert {
                slot,
                id: next_id,
                degree,
                neighbors,
            });
            next_id += 1;
            live.push(slot);
            inserts += 1;
        } else {
            let i = rng.below(live.len() as u64) as usize;
            let slot = live.swap_remove(i);
            free.push(slot);
            ops.push(Op::Remove { slot });
        }
    }
    ops
}

/// Replays the op stream using `make` to allocate each node. Generic over the
/// handle type so both backends run byte-identical control flow — only the
/// allocation call differs.
///
/// Returns the elapsed time (including freeing every remaining node) and a
/// checksum that encodes the full visited sequence.
fn replay<H, F>(ops: &[Op], mut make: F) -> (Duration, u64)
where
    H: Deref<Target = Node>,
    F: FnMut(Node) -> H,
{
    let mut slab: Vec<Option<H>> = Vec::with_capacity(CAP);
    slab.resize_with(CAP, || None);
    let mut checksum = 0u64;

    let start = Instant::now();
    for op in ops {
        match *op {
            Op::Insert {
                slot,
                id,
                degree,
                neighbors,
            } => {
                // Chase existing edges (pointer-following reads through the
                // graph) to fold neighbor state into the new node.
                let mut acc = id;
                for &nb in &neighbors[..degree as usize] {
                    if let Some(node) = &slab[nb as usize] {
                        acc ^= node.id.wrapping_mul(0x0000_0100_0000_01B3);
                    }
                }
                let node = Node {
                    id,
                    payload: [acc, id, !id, acc ^ id, 0, 0],
                    degree: u32::from(degree),
                    neighbors,
                };
                checksum = checksum.wrapping_add(acc);
                slab[slot as usize] = Some(make(node));
            }
            Op::Remove { slot } => {
                if let Some(node) = slab[slot as usize].take() {
                    checksum ^= node.id.rotate_left(node.degree & 63) ^ node.payload[0] ^ u64::from(node.neighbors[0]);
                }
            }
        }
    }
    // Free everything still live (timed).
    for entry in &mut slab {
        if let Some(node) = entry.take() {
            checksum ^= node.id ^ node.payload[1];
        }
    }
    (start.elapsed(), checksum)
}

fn bench_pool(ops: &[Op]) -> (Duration, u64) {
    let mut best = Duration::MAX;
    let mut checksum = 0;
    for _ in 0..ITERS {
        let pool = plurality::Pool::<Node>::builder().chunk_size(8192).build();
        let (elapsed, sum) = replay(ops, |node| pool.alloc_box(node));
        best = best.min(elapsed);
        checksum = sum;
    }
    (best, checksum)
}

fn bench_mimalloc(ops: &[Op]) -> (Duration, u64) {
    let mut best = Duration::MAX;
    let mut checksum = 0;
    for _ in 0..ITERS {
        let (elapsed, sum) = replay(ops, Box::new);
        best = best.min(elapsed);
        checksum = sum;
    }
    (best, checksum)
}

fn main() {
    println!("plurality graph-churn benchmark");
    println!(
        "  op stream: {TARGET_INSERTS} inserts, live cap {CAP}, node size {} bytes, best of {ITERS}",
        size_of::<Node>()
    );

    print!("  generating shared op stream... ");
    let ops = generate_ops();
    let frees = ops.len() - TARGET_INSERTS;
    println!("{} ops ({TARGET_INSERTS} inserts + {frees} removes)", ops.len());

    // Warm both backends once (page in, prime mimalloc) before timing.
    let _ = bench_mimalloc(&ops);
    let _ = bench_pool(&ops);

    let (mi_time, mi_sum) = bench_mimalloc(&ops);
    let (pool_time, pool_sum) = bench_pool(&ops);

    assert_eq!(mi_sum, pool_sum, "checksums differ: the two runs did NOT replay the same pattern");
    println!("  checksum (both backends): {pool_sum:#018x}  identical pattern verified");

    let total_allocs = TARGET_INSERTS as f64;
    let report = |name: &str, t: Duration| {
        let secs = t.as_secs_f64();
        let ns_per = t.as_nanos() as f64 / total_allocs;
        let mops = total_allocs / secs / 1e6;
        println!("  {name:<22} {secs:>8.4} s   {ns_per:>7.2} ns/alloc   {mops:>7.2} Malloc/s");
    };

    println!();
    report("std::Box + mimalloc", mi_time);
    report("plurality::Pool", pool_time);

    let ratio = mi_time.as_secs_f64() / pool_time.as_secs_f64();
    println!();
    if ratio >= 1.0 {
        println!("  => plurality::Pool is {ratio:.2}x faster than std::Box + mimalloc");
    } else {
        println!("  => plurality::Pool is {:.2}x slower than std::Box + mimalloc", 1.0 / ratio);
    }
}
