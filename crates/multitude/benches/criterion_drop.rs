// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock drop benchmarks for multitude.
//!
//! Mirrors `benches/gungraun_drop.rs` 1:1: each `drop/<variant>` here
//! corresponds to a gungraun function `drop_<variant>`.
//!
//! Run with: `cargo bench --bench criterion_drop`

#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(deprecated, reason = "criterion::black_box is deprecated in favor of std::hint::black_box")]
#![allow(unused_results, reason = "benchmark code")]
#![allow(clippy::similar_names, reason = "intentional test-local names")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]

use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use multitude::{Arc, Arena, Box, Rc};

const N: usize = 1_000;
const SLICE_LEN: usize = 8;

type DroppyT = std::boxed::Box<u64>;
#[expect(clippy::unnecessary_box_returns, reason = "Box<u64> is the T: Drop probe")]
fn make_droppy(i: usize) -> DroppyT {
    std::boxed::Box::new(i as u64)
}

// =========================================================================
// drop
// =========================================================================
fn bench_drop(c: &mut Criterion) {
    let mut g = c.benchmark_group("drop");

    macro_rules! drop_bench {
        ($name:literal, $setup:expr) => {
            g.bench_function($name, |b| {
                b.iter_batched(
                    || $setup,
                    |state| {
                        // drop happens here — keep black_box to prevent the
                        // optimizer from sinking the drop outside the closure.
                        drop(black_box(state));
                    },
                    BatchSize::SmallInput,
                );
            });
        };
    }

    drop_bench!("box_u64", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_box(i as u64));
        }
        (h, arena)
    });
    drop_bench!("rc_u64", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_rc(i as u64));
        }
        (h, arena)
    });
    drop_bench!("arc_u64", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_arc(i as u64));
        }
        (h, arena)
    });

    drop_bench!("box_droppy", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_box(make_droppy(i)));
        }
        (h, arena)
    });
    drop_bench!("rc_droppy", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_rc(make_droppy(i)));
        }
        (h, arena)
    });
    drop_bench!("arc_droppy", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_arc(make_droppy(i)));
        }
        (h, arena)
    });

    drop_bench!("str_box", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Box<str>> = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_str_box(format!("word{i}")));
        }
        (h, arena)
    });
    drop_bench!("str_rc", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Rc<str>> = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_str_rc(format!("word{i}")));
        }
        (h, arena)
    });
    drop_bench!("str_arc", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Arc<str>> = Vec::with_capacity(N);
        for i in 0..N {
            h.push(arena.alloc_str_arc(format!("word{i}")));
        }
        (h, arena)
    });

    drop_bench!("slice_box_u64", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Box<[u64]>> = Vec::with_capacity(N);
        for _ in 0..N {
            h.push(arena.alloc_slice_fill_with_box::<u64, _>(SLICE_LEN, |j| j as u64));
        }
        (h, arena)
    });
    drop_bench!("slice_rc_u64", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Rc<[u64]>> = Vec::with_capacity(N);
        for _ in 0..N {
            h.push(arena.alloc_slice_fill_with_rc::<u64, _>(SLICE_LEN, |j| j as u64));
        }
        (h, arena)
    });
    drop_bench!("slice_arc_u64", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Arc<[u64]>> = Vec::with_capacity(N);
        for _ in 0..N {
            h.push(arena.alloc_slice_fill_with_arc::<u64, _>(SLICE_LEN, |j| j as u64));
        }
        (h, arena)
    });

    drop_bench!("slice_box_droppy", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Box<[DroppyT]>> = Vec::with_capacity(N);
        for _ in 0..N {
            h.push(arena.alloc_slice_fill_with_box::<DroppyT, _>(SLICE_LEN, make_droppy));
        }
        (h, arena)
    });
    drop_bench!("slice_rc_droppy", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Rc<[DroppyT]>> = Vec::with_capacity(N);
        for _ in 0..N {
            h.push(arena.alloc_slice_fill_with_rc::<DroppyT, _>(SLICE_LEN, make_droppy));
        }
        (h, arena)
    });
    drop_bench!("slice_arc_droppy", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        let mut h: Vec<Arc<[DroppyT]>> = Vec::with_capacity(N);
        for _ in 0..N {
            h.push(arena.alloc_slice_fill_with_arc::<DroppyT, _>(SLICE_LEN, make_droppy));
        }
        (h, arena)
    });

    drop_bench!("alloc", {
        let arena = Arena::builder().with_capacity(64 * 1024).build();
        for i in 0..N {
            let _ = arena.alloc(i as u64);
        }
        arena
    });

    g.finish();
}

criterion_group!(benches, bench_drop);
criterion_main!(benches);
