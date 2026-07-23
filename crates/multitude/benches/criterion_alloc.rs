// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock allocation benchmarks for multitude.
//!
//! Mirrors `benches/gungraun_alloc/linux.rs` 1:1. Setup creates fresh inputs,
//! preallocates outputs, and leaves allocator pages as the last state touched.

#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]
#![allow(clippy::type_complexity, reason = "benchmark state tuples")]
#![allow(unused_results, reason = "benchmark code")]

#[path = "multitude_alloc_common/mod.rs"]
mod alloc_common;

use core::mem::MaybeUninit;

use alloc_common as common;
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use multitude::{Arc, Box, Rc};

fn bench_arena_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("arena_creation");
    group.bench_function("multitude_new", |b| b.iter(common::multitude_new));
    group.bench_function("bumpalo_new", |b| b.iter(common::bumpalo_new));
    group.finish();
}

macro_rules! arena_collect_bench {
    ($group:ident, $name:literal, $ty:ty, $count:expr, $hot:path) => {
        $group.bench_function($name, |b| {
            b.iter_batched(
                || common::setup_arena_out::<$ty>($count),
                |(arena, mut out)| {
                    $hot(&arena, &mut out);
                    (arena, out)
                },
                BatchSize::SmallInput,
            );
        });
    };
}

fn bench_alloc_u64(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_u64");

    group.bench_function("alloc", |b| {
        b.iter_batched(
            common::warm_arena_local,
            |arena| {
                common::alloc(&arena);
                arena
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_alloc", |b| {
        b.iter_batched(
            common::warm_bump,
            |bump| {
                common::bumpalo_alloc(&bump);
                bump
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("alloc_with", |b| {
        b.iter_batched(
            common::warm_arena_local,
            |arena| {
                common::alloc_with(&arena);
                arena
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_alloc_with", |b| {
        b.iter_batched(
            common::warm_bump,
            |bump| {
                common::bumpalo_alloc_with(&bump);
                bump
            },
            BatchSize::SmallInput,
        );
    });

    arena_collect_bench!(group, "alloc_box", Box<u64>, common::N, common::alloc_box);
    arena_collect_bench!(group, "alloc_box_with", Box<u64>, common::N, common::alloc_box_with);
    arena_collect_bench!(
        group,
        "alloc_uninit_box",
        Box<MaybeUninit<u64>>,
        common::N,
        common::alloc_uninit_box
    );
    arena_collect_bench!(
        group,
        "alloc_zeroed_box",
        Box<MaybeUninit<u64>>,
        common::N,
        common::alloc_zeroed_box
    );
    arena_collect_bench!(group, "alloc_arc", Arc<u64>, common::N, common::alloc_arc);
    arena_collect_bench!(group, "alloc_arc_with", Arc<u64>, common::N, common::alloc_arc_with);
    arena_collect_bench!(
        group,
        "alloc_uninit_arc",
        Arc<MaybeUninit<u64>>,
        common::N,
        common::alloc_uninit_arc
    );
    arena_collect_bench!(
        group,
        "alloc_zeroed_arc",
        Arc<MaybeUninit<u64>>,
        common::N,
        common::alloc_zeroed_arc
    );
    arena_collect_bench!(group, "alloc_rc", Rc<u64>, common::N, common::alloc_rc);
    arena_collect_bench!(group, "alloc_rc_with", Rc<u64>, common::N, common::alloc_rc_with);
    arena_collect_bench!(group, "alloc_uninit_rc", Rc<MaybeUninit<u64>>, common::N, common::alloc_uninit_rc);
    arena_collect_bench!(group, "alloc_zeroed_rc", Rc<MaybeUninit<u64>>, common::N, common::alloc_zeroed_rc);

    group.finish();
}

macro_rules! arena_words_collect_bench {
    ($group:ident, $name:literal, $ty:ty, $hot:path) => {
        $group.bench_function($name, |b| {
            b.iter_batched(
                common::setup_arena_words_out::<$ty>,
                |(arena, words, mut out)| {
                    $hot(&arena, &words, &mut out);
                    (arena, words, out)
                },
                BatchSize::SmallInput,
            );
        });
    };
}

fn bench_alloc_str(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_str");

    group.bench_function("alloc_str", |b| {
        b.iter_batched(
            common::setup_arena_words,
            |(arena, words)| {
                common::alloc_str(&arena, &words);
                (arena, words)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_alloc_str", |b| {
        b.iter_batched(
            common::setup_bump_words,
            |(bump, words)| {
                common::bumpalo_alloc_str(&bump, &words);
                (bump, words)
            },
            BatchSize::SmallInput,
        );
    });
    arena_words_collect_bench!(group, "alloc_str_box", Box<str>, common::alloc_str_box);
    arena_words_collect_bench!(group, "alloc_str_arc", Arc<str>, common::alloc_str_arc);
    arena_words_collect_bench!(group, "alloc_str_rc", Rc<str>, common::alloc_str_rc);
    group.finish();
}

macro_rules! arena_slice_input_bench {
    ($group:ident, $name:literal, $count:expr, $hot:path) => {
        $group.bench_function($name, |b| {
            b.iter_batched(
                || common::setup_arena_slices($count),
                |(arena, slices)| {
                    $hot(&arena, &slices);
                    (arena, slices)
                },
                BatchSize::SmallInput,
            );
        });
    };
}

macro_rules! arena_slice_collect_bench {
    ($group:ident, $name:literal, $ty:ty, $count:expr, $hot:path) => {
        $group.bench_function($name, |b| {
            b.iter_batched(
                || common::setup_arena_slices_out::<$ty>($count),
                |(arena, slices, mut out)| {
                    $hot(&arena, &slices, &mut out);
                    (arena, slices, out)
                },
                BatchSize::SmallInput,
            );
        });
    };
}

macro_rules! arena_generated_slice_bench {
    ($group:ident, $name:literal, $ty:ty, $count:expr, $hot:path) => {
        arena_collect_bench!($group, $name, $ty, $count, $hot);
    };
}

fn bench_alloc_slice(c: &mut Criterion) {
    let mut group = c.benchmark_group("alloc_slice");

    arena_slice_input_bench!(group, "alloc_slice_copy", common::N, common::alloc_slice_copy);
    group.bench_function("bumpalo_alloc_slice_copy", |b| {
        b.iter_batched(
            common::setup_bump_slices,
            |(bump, slices)| {
                common::bumpalo_alloc_slice_copy(&bump, &slices);
                (bump, slices)
            },
            BatchSize::SmallInput,
        );
    });
    arena_slice_input_bench!(group, "alloc_slice_clone", common::N, common::alloc_slice_clone);
    group.bench_function("bumpalo_alloc_slice_clone", |b| {
        b.iter_batched(
            common::setup_bump_slices,
            |(bump, slices)| {
                common::bumpalo_alloc_slice_clone(&bump, &slices);
                (bump, slices)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("alloc_slice_fill_with", |b| {
        b.iter_batched(
            common::warm_arena_local,
            |arena| {
                common::alloc_slice_fill_with(&arena);
                arena
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_alloc_slice_fill_with", |b| {
        b.iter_batched(
            common::warm_bump,
            |bump| {
                common::bumpalo_alloc_slice_fill_with(&bump);
                bump
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("alloc_slice_fill_iter", |b| {
        b.iter_batched(
            common::warm_arena_local,
            |arena| {
                common::alloc_slice_fill_iter(&arena);
                arena
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_alloc_slice_fill_iter", |b| {
        b.iter_batched(
            common::warm_bump,
            |bump| {
                common::bumpalo_alloc_slice_fill_iter(&bump);
                bump
            },
            BatchSize::SmallInput,
        );
    });

    arena_slice_collect_bench!(
        group,
        "alloc_slice_copy_box",
        Box<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_copy_box
    );
    arena_slice_collect_bench!(
        group,
        "alloc_slice_clone_box",
        Box<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_clone_box
    );
    arena_generated_slice_bench!(
        group,
        "alloc_slice_fill_with_box",
        Box<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_fill_with_box
    );
    arena_generated_slice_bench!(
        group,
        "alloc_slice_fill_iter_box",
        Box<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_fill_iter_box
    );
    arena_generated_slice_bench!(
        group,
        "alloc_uninit_slice_box",
        Box<[MaybeUninit<u64>]>,
        common::OWNED_SLICE_N,
        common::alloc_uninit_slice_box
    );
    arena_generated_slice_bench!(
        group,
        "alloc_zeroed_slice_box",
        Box<[MaybeUninit<u64>]>,
        common::OWNED_SLICE_N,
        common::alloc_zeroed_slice_box
    );

    arena_slice_collect_bench!(
        group,
        "alloc_slice_copy_arc",
        Arc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_copy_arc
    );
    arena_slice_collect_bench!(
        group,
        "alloc_slice_clone_arc",
        Arc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_clone_arc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_slice_fill_with_arc",
        Arc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_fill_with_arc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_slice_fill_iter_arc",
        Arc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_fill_iter_arc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_uninit_slice_arc",
        Arc<[MaybeUninit<u64>]>,
        common::OWNED_SLICE_N,
        common::alloc_uninit_slice_arc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_zeroed_slice_arc",
        Arc<[MaybeUninit<u64>]>,
        common::OWNED_SLICE_N,
        common::alloc_zeroed_slice_arc
    );

    arena_slice_collect_bench!(
        group,
        "alloc_slice_copy_rc",
        Rc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_copy_rc
    );
    arena_slice_collect_bench!(
        group,
        "alloc_slice_clone_rc",
        Rc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_clone_rc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_slice_fill_with_rc",
        Rc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_fill_with_rc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_slice_fill_iter_rc",
        Rc<[u64]>,
        common::OWNED_SLICE_N,
        common::alloc_slice_fill_iter_rc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_uninit_slice_rc",
        Rc<[MaybeUninit<u64>]>,
        common::OWNED_SLICE_N,
        common::alloc_uninit_slice_rc
    );
    arena_generated_slice_bench!(
        group,
        "alloc_zeroed_slice_rc",
        Rc<[MaybeUninit<u64>]>,
        common::OWNED_SLICE_N,
        common::alloc_zeroed_slice_rc
    );

    group.finish();
}

fn bench_string_builder(c: &mut Criterion) {
    let mut group = c.benchmark_group("string_builder");

    group.bench_function("alloc_string", |b| {
        b.iter_batched(
            common::setup_arena_words,
            |(arena, words)| {
                let pointer = common::alloc_string(&arena, &words);
                (pointer, arena, words)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_string_new_in", |b| {
        b.iter_batched(
            common::setup_bump_words,
            |(bump, words)| {
                let pointer = common::bumpalo_string_new_in(&bump, &words);
                (pointer, bump, words)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("alloc_string_with_capacity", |b| {
        b.iter_batched(
            common::setup_arena_words_with_len,
            |(arena, words, len)| {
                let pointer = common::alloc_string_with_capacity(&arena, &words, len);
                (pointer, arena, words)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_string_with_capacity_in", |b| {
        b.iter_batched(
            common::setup_bump_words_with_len,
            |(bump, words, len)| {
                let pointer = common::bumpalo_string_with_capacity_in(&bump, &words, len);
                (pointer, bump, words)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_vec_builder(c: &mut Criterion) {
    let mut group = c.benchmark_group("vec_builder");

    group.bench_function("alloc_vec", |b| {
        b.iter_batched(
            common::setup_arena_ints,
            |(arena, ints)| {
                let pointer = common::alloc_vec(&arena, &ints);
                (pointer, arena, ints)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_vec_new_in", |b| {
        b.iter_batched(
            common::setup_bump_ints,
            |(bump, ints)| {
                let pointer = common::bumpalo_vec_new_in(&bump, &ints);
                (pointer, bump, ints)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("alloc_vec_with_capacity", |b| {
        b.iter_batched(
            common::setup_arena_ints,
            |(arena, ints)| {
                let pointer = common::alloc_vec_with_capacity(&arena, &ints);
                (pointer, arena, ints)
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("bumpalo_vec_with_capacity_in", |b| {
        b.iter_batched(
            common::setup_bump_ints,
            |(bump, ints)| {
                let pointer = common::bumpalo_vec_with_capacity_in(&bump, &ints);
                (pointer, bump, ints)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn bench_allocator_grow(c: &mut Criterion) {
    let mut group = c.benchmark_group("allocator_grow");

    group.bench_function("in_place", |b| {
        b.iter_batched(
            common::warm_arena_local,
            |arena| {
                common::allocator_grow_in_place(&arena);
                arena
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("zeroed_in_place", |b| {
        b.iter_batched(
            common::warm_arena_local,
            |arena| {
                common::allocator_grow_zeroed_in_place(&arena);
                arena
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("shrink_in_place", |b| {
        b.iter_batched(
            common::warm_arena_local,
            |arena| {
                common::allocator_shrink_in_place(&arena);
                arena
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_arena_creation,
    bench_alloc_u64,
    bench_alloc_str,
    bench_alloc_slice,
    bench_string_builder,
    bench_vec_builder,
    bench_allocator_grow,
);
criterion_main!(benches);
