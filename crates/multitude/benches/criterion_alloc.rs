// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock allocation benchmarks for multitude.
//!
//! Mirrors `benches/gungraun_alloc.rs` 1:1: each `<group>/<variant>` here
//! corresponds to a gungraun function `<group>_<variant>`.
//!
//! Run with: `cargo bench --bench criterion_alloc`
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(deprecated, reason = "criterion::black_box is deprecated in favor of std::hint::black_box")]
#![allow(unused_results, reason = "benchmark code")]
#![allow(clippy::similar_names, reason = "intentional test-local names")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]

use core::mem::MaybeUninit;
use std::hint::black_box;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use multitude::{Arc, Arena, Box};

const N: usize = 1_000;
const SLICE_LEN: usize = 8;

/// Build an [`Arena`] sized for the timed region's full working set
/// **and fully primed** so the timed region exercises only the warm
/// bump cursor — no cold `refill_*` call on the first inner iteration.
///
/// 1. `with_capacity_local(64 KiB) + with_capacity_shared(64 KiB)` pin
///    the very first chunk for each flavor to the largest size class.
///    The arena's adaptive `1 KiB → 64 KiB` ramp would otherwise call
///    into the system allocator several times growing through the
///    smaller classes during the timed region.
/// 2. The arena's `current_local` / `current_shared` slots start in
///    the empty-mutator state (lazy first-chunk install — see the
///    design doc). A single throwaway allocation of each flavor pops
///    the preallocated chunk from the provider cache and installs it
///    in the `current_*` slot, so the timed region's very first
///    allocation hits the warm bump path. This matches bumpalo's
///    `warm_bump` (which similarly primes its cursor with a no-op
///    alloc) and isolates the comparison to "in-chunk bump cost
///    only" with no cold refill amortized into the per-op number.
fn warm_arena() -> Arena {
    let arena = Arena::builder()
        .with_capacity_local(64 * 1024)
        .with_capacity_shared(64 * 1024)
        .build();
    let _: &mut u64 = arena.alloc(0_u64);
    let _ = arena.alloc_arc(0_u64);
    arena
}

/// Build a [`bumpalo::Bump`] pre-sized to fit the timed region.
///
/// `Bump::with_capacity(N)` reserves a chunk of at least N bytes from
/// the system allocator up front, so `bench iter -> Bump::alloc(...)`
/// runs entirely against bumpalo's bump cursor. 64 KiB matches the
/// arena's largest size class for an apples-to-apples comparison.
fn warm_bump() -> bumpalo::Bump {
    let bump = bumpalo::Bump::with_capacity(64 * 1024);
    let _: &mut u64 = bump.alloc(0_u64);
    bump
}

fn word_inputs() -> Vec<String> {
    (0..N).map(|i| format!("word{i}")).collect()
}
fn slice_inputs() -> Vec<[u64; SLICE_LEN]> {
    (0..N)
        .map(|i| {
            let b = i as u64;
            [b, b + 1, b + 2, b + 3, b + 4, b + 5, b + 6, b + 7]
        })
        .collect()
}

// =========================================================================
// alloc_u64
// =========================================================================
fn bench_alloc_u64(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc_u64");

    g.bench_function("alloc", |b| {
        b.iter_batched(
            warm_arena,
            |arena| {
                for i in 0..N {
                    let _: &mut u64 = black_box(arena.alloc(black_box(i as u64)));
                }
                arena
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_with", |b| {
        b.iter_batched(
            warm_arena,
            |arena| {
                for i in 0..N {
                    let _: &mut u64 = black_box(arena.alloc_with(|| black_box(i as u64)));
                }
                arena
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("alloc_box", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Box<u64>>::with_capacity(N)),
            |(arena, mut h)| {
                for i in 0..N {
                    h.push(arena.alloc_box(black_box(i as u64)));
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_box_with", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Box<u64>>::with_capacity(N)),
            |(arena, mut h)| {
                for i in 0..N {
                    h.push(arena.alloc_box_with(|| black_box(i as u64)));
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_uninit_box", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Box<MaybeUninit<u64>>>::with_capacity(N)),
            |(arena, mut h)| {
                for _ in 0..N {
                    h.push(arena.alloc_uninit_box::<u64>());
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_zeroed_box", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Box<MaybeUninit<u64>>>::with_capacity(N)),
            |(arena, mut h)| {
                for _ in 0..N {
                    h.push(arena.alloc_zeroed_box::<u64>());
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("alloc_arc", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Arc<u64>>::with_capacity(N)),
            |(arena, mut h)| {
                for i in 0..N {
                    h.push(arena.alloc_arc(black_box(i as u64)));
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_arc_with", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Arc<u64>>::with_capacity(N)),
            |(arena, mut h)| {
                for i in 0..N {
                    h.push(arena.alloc_arc_with(|| black_box(i as u64)));
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_uninit_arc", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Arc<MaybeUninit<u64>>>::with_capacity(N)),
            |(arena, mut h)| {
                for _ in 0..N {
                    h.push(arena.alloc_uninit_arc::<u64>());
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_zeroed_arc", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Arc<MaybeUninit<u64>>>::with_capacity(N)),
            |(arena, mut h)| {
                for _ in 0..N {
                    h.push(arena.alloc_zeroed_arc::<u64>());
                }
                (h, arena)
            },
            BatchSize::SmallInput,
        );
    });

    g.bench_function("bumpalo_alloc", |b| {
        b.iter_batched(
            warm_bump,
            |bump| {
                for i in 0..N {
                    let _: &mut u64 = black_box(bump.alloc(black_box(i as u64)));
                }
                bump
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("bumpalo_alloc_with", |b| {
        b.iter_batched(
            warm_bump,
            |bump| {
                for i in 0..N {
                    let _: &mut u64 = black_box(bump.alloc_with(|| black_box(i as u64)));
                }
                bump
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// =========================================================================
// alloc_str
// =========================================================================
fn bench_alloc_str(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc_str");
    let words = word_inputs();

    g.bench_function("alloc_str", |b| {
        b.iter_batched(
            warm_arena,
            |arena| {
                for w in &words {
                    let _: &mut str = black_box(arena.alloc_str(black_box(w)));
                }
                arena
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_str_box", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Box<str>>::with_capacity(N)),
            |(arena, mut o)| {
                for w in &words {
                    o.push(arena.alloc_str_box(black_box(w)));
                }
                (o, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_str_arc", |b| {
        b.iter_batched(
            || (warm_arena(), Vec::<Arc<str>>::with_capacity(N)),
            |(arena, mut o)| {
                for w in &words {
                    o.push(arena.alloc_str_arc(black_box(w)));
                }
                (o, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("bumpalo_alloc_str", |b| {
        b.iter_batched(
            warm_bump,
            |bump| {
                for w in &words {
                    let _: &mut str = black_box(bump.alloc_str(black_box(w)));
                }
                bump
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// =========================================================================
// alloc_slice
// =========================================================================
fn bench_alloc_slice(c: &mut Criterion) {
    let mut g = c.benchmark_group("alloc_slice");
    let slices = slice_inputs();

    macro_rules! bench_arena {
        ($name:literal, $body:expr) => {
            g.bench_function($name, |b| {
                b.iter_batched(
                    warm_arena,
                    |arena| {
                        let r = $body(&arena);
                        // Return owned values so their `Drop` runs outside the timed region.
                        (r, arena)
                    },
                    BatchSize::SmallInput,
                )
            });
        };
    }
    macro_rules! bench_arena_collect {
        ($name:literal, $T:ty, $body:expr) => {
            g.bench_function($name, |b| {
                b.iter_batched(
                    || (warm_arena(), Vec::<$T>::with_capacity(N)),
                    |(arena, mut o)| {
                        $body(&arena, &mut o);
                        (o, arena)
                    },
                    BatchSize::SmallInput,
                )
            });
        };
    }
    macro_rules! bench_bumpalo {
        ($name:literal, $body:expr) => {
            g.bench_function($name, |b| {
                b.iter_batched(
                    warm_bump,
                    |bump| {
                        $body(&bump);
                        // Return so `Bump::drop` runs outside the timed region.
                        bump
                    },
                    BatchSize::SmallInput,
                )
            });
        };
    }

    // ref
    bench_arena!("alloc_slice_copy", |arena: &Arena| {
        for s in &slices {
            let _: &mut [u64] = black_box(arena.alloc_slice_copy(black_box(s)));
        }
    });
    bench_arena!("alloc_slice_clone", |arena: &Arena| {
        for s in &slices {
            let _: &mut [u64] = black_box(arena.alloc_slice_clone(black_box(s.as_slice())));
        }
    });
    bench_arena!("alloc_slice_fill_with", |arena: &Arena| {
        for _ in 0..N {
            let _: &mut [u64] = black_box(arena.alloc_slice_fill_with::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
        }
    });
    bench_arena!("alloc_slice_fill_iter", |arena: &Arena| {
        for _ in 0..N {
            let _: &mut [u64] = black_box(arena.alloc_slice_fill_iter((0..SLICE_LEN).map(|j| black_box(j as u64))));
        }
    });

    // box
    bench_arena_collect!("alloc_slice_copy_box", Box<[u64]>, |arena: &Arena, o: &mut Vec<Box<[u64]>>| {
        for s in &slices {
            o.push(arena.alloc_slice_copy_box(black_box(s)));
        }
    });
    bench_arena_collect!("alloc_slice_clone_box", Box<[u64]>, |arena: &Arena, o: &mut Vec<Box<[u64]>>| {
        for s in &slices {
            o.push(arena.alloc_slice_clone_box(black_box(s.as_slice())));
        }
    });
    bench_arena_collect!("alloc_slice_fill_with_box", Box<[u64]>, |arena: &Arena, o: &mut Vec<Box<[u64]>>| {
        for _ in 0..N {
            o.push(arena.alloc_slice_fill_with_box::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
        }
    });
    bench_arena_collect!("alloc_slice_fill_iter_box", Box<[u64]>, |arena: &Arena, o: &mut Vec<Box<[u64]>>| {
        for _ in 0..N {
            o.push(arena.alloc_slice_fill_iter_box((0..SLICE_LEN).map(|j| black_box(j as u64))));
        }
    });
    bench_arena_collect!("alloc_uninit_slice_box", Box<[MaybeUninit<u64>]>, |arena: &Arena,
                                                                             o: &mut Vec<
        Box<[MaybeUninit<u64>]>,
    >| {
        for _ in 0..N {
            o.push(arena.alloc_uninit_slice_box::<u64>(SLICE_LEN));
        }
    });
    bench_arena_collect!("alloc_zeroed_slice_box", Box<[MaybeUninit<u64>]>, |arena: &Arena,
                                                                             o: &mut Vec<
        Box<[MaybeUninit<u64>]>,
    >| {
        for _ in 0..N {
            o.push(arena.alloc_zeroed_slice_box::<u64>(SLICE_LEN));
        }
    });

    // arc
    bench_arena_collect!("alloc_slice_copy_arc", Arc<[u64]>, |arena: &Arena, o: &mut Vec<Arc<[u64]>>| {
        for s in &slices {
            o.push(arena.alloc_slice_copy_arc(black_box(s)));
        }
    });
    bench_arena_collect!("alloc_slice_clone_arc", Arc<[u64]>, |arena: &Arena, o: &mut Vec<Arc<[u64]>>| {
        for s in &slices {
            o.push(arena.alloc_slice_clone_arc(black_box(s.as_slice())));
        }
    });
    bench_arena_collect!("alloc_slice_fill_with_arc", Arc<[u64]>, |arena: &Arena, o: &mut Vec<Arc<[u64]>>| {
        for _ in 0..N {
            o.push(arena.alloc_slice_fill_with_arc::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
        }
    });
    bench_arena_collect!("alloc_slice_fill_iter_arc", Arc<[u64]>, |arena: &Arena, o: &mut Vec<Arc<[u64]>>| {
        for _ in 0..N {
            o.push(arena.alloc_slice_fill_iter_arc((0..SLICE_LEN).map(|j| black_box(j as u64))));
        }
    });
    bench_arena_collect!("alloc_uninit_slice_arc", Arc<[MaybeUninit<u64>]>, |arena: &Arena,
                                                                             o: &mut Vec<
        Arc<[MaybeUninit<u64>]>,
    >| {
        for _ in 0..N {
            o.push(arena.alloc_uninit_slice_arc::<u64>(SLICE_LEN));
        }
    });
    bench_arena_collect!("alloc_zeroed_slice_arc", Arc<[MaybeUninit<u64>]>, |arena: &Arena,
                                                                             o: &mut Vec<
        Arc<[MaybeUninit<u64>]>,
    >| {
        for _ in 0..N {
            o.push(arena.alloc_zeroed_slice_arc::<u64>(SLICE_LEN));
        }
    });

    // bumpalo
    bench_bumpalo!("bumpalo_alloc_slice_copy", |bump: &bumpalo::Bump| {
        for s in &slices {
            let _: &mut [u64] = black_box(bump.alloc_slice_copy(black_box(s.as_slice())));
        }
    });
    bench_bumpalo!("bumpalo_alloc_slice_clone", |bump: &bumpalo::Bump| {
        for s in &slices {
            let _: &mut [u64] = black_box(bump.alloc_slice_clone(black_box(s.as_slice())));
        }
    });
    bench_bumpalo!("bumpalo_alloc_slice_fill_with", |bump: &bumpalo::Bump| {
        for _ in 0..N {
            let _: &mut [u64] = black_box(bump.alloc_slice_fill_with::<u64, _>(SLICE_LEN, |j| black_box(j as u64)));
        }
    });
    bench_bumpalo!("bumpalo_alloc_slice_fill_iter", |bump: &bumpalo::Bump| {
        for _ in 0..N {
            let _: &mut [u64] = black_box(bump.alloc_slice_fill_iter((0..SLICE_LEN).map(|j| black_box(j as u64))));
        }
    });

    g.finish();
}

// =========================================================================
// string_builder
// =========================================================================
fn bench_string_builder(c: &mut Criterion) {
    let mut g = c.benchmark_group("string_builder");
    let words = word_inputs();

    g.bench_function("alloc_string", |b| {
        b.iter_batched(
            warm_arena,
            |arena| {
                let mut s = arena.alloc_string();
                for w in &words {
                    s.push_str(black_box(w.as_str()));
                }
                let frozen = s.into_arena_box_str();
                (frozen, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_string_with_capacity", |b| {
        b.iter_batched(
            warm_arena,
            |arena| {
                let mut s = arena.alloc_string_with_capacity(N * 6);
                for w in &words {
                    s.push_str(black_box(w.as_str()));
                }
                let frozen = s.into_arena_box_str();
                (frozen, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("bumpalo_string_new_in", |b| {
        b.iter_batched(
            warm_bump,
            |bump| {
                let mut s = bumpalo::collections::String::new_in(&bump);
                for w in &words {
                    s.push_str(black_box(w.as_str()));
                }
                let frozen: &str = s.into_bump_str();
                black_box(frozen);
                bump
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("bumpalo_string_with_capacity_in", |b| {
        b.iter_batched(
            warm_bump,
            |bump| {
                let mut s = bumpalo::collections::String::with_capacity_in(N * 6, &bump);
                for w in &words {
                    s.push_str(black_box(w.as_str()));
                }
                let frozen: &str = s.into_bump_str();
                black_box(frozen);
                bump
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// =========================================================================
// vec_builder
// =========================================================================
fn bench_vec_builder(c: &mut Criterion) {
    let mut g = c.benchmark_group("vec_builder");
    let ints: Vec<i32> = (0..N).map(|i| i32::try_from(i).unwrap_or(0)).collect();

    g.bench_function("alloc_vec", |b| {
        b.iter_batched(
            warm_arena,
            |arena| {
                let mut v = arena.alloc_vec::<i32>();
                for &i in &ints {
                    v.push(black_box(i));
                }
                let frozen = v.into_arena_arc();
                (frozen, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("alloc_vec_with_capacity", |b| {
        b.iter_batched(
            warm_arena,
            |arena| {
                let mut v = arena.alloc_vec_with_capacity::<i32>(N);
                for &i in &ints {
                    v.push(black_box(i));
                }
                let frozen = v.into_arena_arc();
                (frozen, arena)
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("bumpalo_vec_new_in", |b| {
        b.iter_batched(
            warm_bump,
            |bump| {
                let mut v: bumpalo::collections::Vec<'_, i32> = bumpalo::collections::Vec::new_in(&bump);
                for &i in &ints {
                    v.push(black_box(i));
                }
                let frozen: &[i32] = v.into_bump_slice();
                black_box(frozen);
                bump
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("bumpalo_vec_with_capacity_in", |b| {
        b.iter_batched(
            warm_bump,
            |bump| {
                let mut v: bumpalo::collections::Vec<'_, i32> = bumpalo::collections::Vec::with_capacity_in(N, &bump);
                for &i in &ints {
                    v.push(black_box(i));
                }
                let frozen: &[i32] = v.into_bump_slice();
                black_box(frozen);
                bump
            },
            BatchSize::SmallInput,
        );
    });

    g.finish();
}

// =========================================================================
// arena_creation
// =========================================================================
fn bench_arena_creation(c: &mut Criterion) {
    let mut g = c.benchmark_group("arena_creation");

    g.bench_function("multitude", |b| {
        b.iter(|| {
            let arena = Arena::new();
            black_box(&arena);
            // Drop is part of the lifecycle — included in the timed region.
            drop(arena);
        });
    });

    g.bench_function("bumpalo_new", |b| {
        b.iter(|| {
            let bump = bumpalo::Bump::new();
            black_box(&bump);
            drop(bump);
        });
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_arena_creation,
    bench_alloc_u64,
    bench_alloc_str,
    bench_alloc_slice,
    bench_string_builder,
    bench_vec_builder,
);
criterion_main!(benches);
