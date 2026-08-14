// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Criterion wall-clock allocation benchmarks for multitude.
//!
//! Paired with `criterion_alloc_cg.rs`. Setup creates fresh inputs,
//! preallocates outputs, and leaves allocator pages as the last state touched.

#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(clippy::too_many_lines, reason = "benchmark file")]
#![allow(clippy::type_complexity, reason = "benchmark state tuples")]
#![allow(unused_results, reason = "benchmark code")]

#[path = "multitude_alloc_common/mod.rs"]
mod alloc_common;

use core::mem::MaybeUninit;
use std::alloc::System;
use std::hint::black_box;
use std::time::{Duration, Instant};

use alloc_common as common;
use alloc_tracker::{Allocator as TrackingAllocator, Session};
use criterion::{Bencher, Criterion, criterion_group, criterion_main};
use multitude::{Arc, Arena, Box, Rc};

#[global_allocator]
static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();

fn iter_with_setup<T>(bencher: &mut Bencher<'_>, mut setup: impl FnMut() -> T, mut routine: impl FnMut(&mut T)) {
    bencher.iter_custom(|iters| {
        let mut elapsed = Duration::ZERO;

        for _ in 0..iters {
            let mut input = setup();
            let start = Instant::now();
            routine(&mut input);
            elapsed += start.elapsed();
            drop(input);
        }

        elapsed
    });
}

fn assert_allocation_free<T>(session: &Session, name: &str, mut input: T, routine: impl FnOnce(&mut T)) {
    let operation = session.operation(name);
    {
        let _measurement = operation.measure_thread().iterations(1);
        routine(&mut input);
    }
    drop(input);

    let report = session.to_report();
    let (_, metrics) = report
        .operations()
        .find(|(operation_name, _)| *operation_name == name)
        .expect("allocation operation was registered immediately above");
    assert_eq!(
        metrics.total_allocations_count(),
        0,
        "{name} unexpectedly called the backing allocator"
    );
    assert_eq!(metrics.total_bytes_allocated(), 0, "{name} unexpectedly allocated backing bytes");
}

fn validate_allocation_contract(_: &mut Criterion) {
    let session = Session::new().no_stdout().no_file();

    assert_allocation_free(&session, "alloc/multitude", common::warm_arena_local(), |arena| {
        common::alloc(arena);
    });
    assert_allocation_free(&session, "alloc/bumpalo", common::warm_bump(), |bump| {
        common::bumpalo_alloc(bump);
    });

    assert_allocation_free(&session, "alloc_str/multitude", common::setup_arena_words(), |state| {
        let (arena, words) = state;
        common::alloc_str(arena, words);
    });
    assert_allocation_free(&session, "alloc_str/bumpalo", common::setup_bump_words(), |state| {
        let (bump, words) = state;
        common::bumpalo_alloc_str(bump, words);
    });

    assert_allocation_free(
        &session,
        "alloc_slice_copy/multitude",
        common::setup_arena_slices(common::N),
        |state| {
            let (arena, slices) = state;
            common::alloc_slice_copy(arena, slices);
        },
    );
    assert_allocation_free(&session, "alloc_slice_copy/bumpalo", common::setup_bump_slices(), |state| {
        let (bump, slices) = state;
        common::bumpalo_alloc_slice_copy(bump, slices);
    });
    assert_allocation_free(
        &session,
        "alloc_slice_clone/multitude",
        common::setup_arena_slices(common::N),
        |state| {
            let (arena, slices) = state;
            common::alloc_slice_clone(arena, slices);
        },
    );
    assert_allocation_free(&session, "alloc_slice_clone/bumpalo", common::setup_bump_slices(), |state| {
        let (bump, slices) = state;
        common::bumpalo_alloc_slice_clone(bump, slices);
    });
    assert_allocation_free(&session, "alloc_slice_fill_with/multitude", common::warm_arena_local(), |arena| {
        common::alloc_slice_fill_with(arena);
    });
    assert_allocation_free(&session, "alloc_slice_fill_with/bumpalo", common::warm_bump(), |bump| {
        common::bumpalo_alloc_slice_fill_with(bump);
    });
    assert_allocation_free(&session, "alloc_slice_fill_iter/multitude", common::warm_arena_local(), |arena| {
        common::alloc_slice_fill_iter(arena);
    });
    assert_allocation_free(&session, "alloc_slice_fill_iter/bumpalo", common::warm_bump(), |bump| {
        common::bumpalo_alloc_slice_fill_iter(bump);
    });

    assert_allocation_free(&session, "string_new/multitude", common::setup_arena_words(), |state| {
        let (arena, words) = state;
        _ = black_box(common::alloc_string(arena, words));
    });
    assert_allocation_free(&session, "string_new/bumpalo", common::setup_bump_words(), |state| {
        let (bump, words) = state;
        _ = black_box(common::bumpalo_string_new_in(bump, words));
    });
    assert_allocation_free(
        &session,
        "string_capacity/multitude",
        common::setup_arena_words_with_len(),
        |state| {
            let (arena, words, len) = state;
            _ = black_box(common::alloc_string_with_capacity(arena, words, *len));
        },
    );
    assert_allocation_free(&session, "string_capacity/bumpalo", common::setup_bump_words_with_len(), |state| {
        let (bump, words, len) = state;
        _ = black_box(common::bumpalo_string_with_capacity_in(bump, words, *len));
    });

    assert_allocation_free(&session, "vec_new/multitude", common::setup_arena_ints(), |state| {
        let (arena, ints) = state;
        _ = black_box(common::alloc_vec(arena, ints));
    });
    assert_allocation_free(&session, "vec_new/bumpalo", common::setup_bump_ints(), |state| {
        let (bump, ints) = state;
        _ = black_box(common::bumpalo_vec_new_in(bump, ints));
    });
    assert_allocation_free(&session, "vec_capacity/multitude", common::setup_arena_ints(), |state| {
        let (arena, ints) = state;
        _ = black_box(common::alloc_vec_with_capacity(arena, ints));
    });
    assert_allocation_free(&session, "vec_capacity/bumpalo", common::setup_bump_ints(), |state| {
        let (bump, ints) = state;
        _ = black_box(common::bumpalo_vec_with_capacity_in(bump, ints));
    });
}

fn bench_arena_lifecycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("criterion_alloc/arena_lifecycle");
    group.bench_function("multitude_new", |b| b.iter(common::multitude_new));
    group.bench_function("bumpalo_new", |b| b.iter(common::bumpalo_new));
    group.finish();
}

macro_rules! arena_collect_bench {
    ($group:ident, $name:literal, $ty:ty, $count:expr, $hot:path) => {
        $group.bench_function($name, |b| {
            iter_with_setup(
                b,
                || common::setup_arena_out::<$ty>($count),
                |state: &mut (Arena, Vec<$ty>)| {
                    let (arena, out) = state;
                    $hot(arena, out);
                },
            );
        });
    };
}

fn bench_alloc_u64(c: &mut Criterion) {
    let mut group = c.benchmark_group("criterion_alloc/alloc_u64");

    group.bench_function("alloc", |b| {
        iter_with_setup(b, common::warm_arena_local, |arena| common::alloc(arena));
    });
    group.bench_function("bumpalo_alloc", |b| {
        iter_with_setup(b, common::warm_bump, |bump| common::bumpalo_alloc(bump));
    });
    group.bench_function("alloc_with", |b| {
        iter_with_setup(b, common::warm_arena_local, |arena| common::alloc_with(arena));
    });
    group.bench_function("bumpalo_alloc_with", |b| {
        iter_with_setup(b, common::warm_bump, |bump| common::bumpalo_alloc_with(bump));
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
            iter_with_setup(
                b,
                common::setup_arena_words_out::<$ty>,
                |state: &mut (Arena, Vec<String>, Vec<$ty>)| {
                    let (arena, words, out) = state;
                    $hot(arena, words, out);
                },
            );
        });
    };
}

fn bench_alloc_str(c: &mut Criterion) {
    let mut group = c.benchmark_group("criterion_alloc/alloc_str");

    group.bench_function("alloc_str", |b| {
        iter_with_setup(b, common::setup_arena_words, |state| {
            let (arena, words) = state;
            common::alloc_str(arena, words);
        });
    });
    group.bench_function("bumpalo_alloc_str", |b| {
        iter_with_setup(b, common::setup_bump_words, |state| {
            let (bump, words) = state;
            common::bumpalo_alloc_str(bump, words);
        });
    });
    arena_words_collect_bench!(group, "alloc_str_box", Box<str>, common::alloc_str_box);
    arena_words_collect_bench!(group, "alloc_str_arc", Arc<str>, common::alloc_str_arc);
    arena_words_collect_bench!(group, "alloc_str_rc", Rc<str>, common::alloc_str_rc);
    group.finish();
}

macro_rules! arena_slice_input_bench {
    ($group:ident, $name:literal, $count:expr, $hot:path) => {
        $group.bench_function($name, |b| {
            iter_with_setup(
                b,
                || common::setup_arena_slices($count),
                |state| {
                    let (arena, slices) = state;
                    $hot(arena, slices);
                },
            );
        });
    };
}

macro_rules! arena_slice_collect_bench {
    ($group:ident, $name:literal, $ty:ty, $count:expr, $hot:path) => {
        $group.bench_function($name, |b| {
            iter_with_setup(
                b,
                || common::setup_arena_slices_out::<$ty>($count),
                |state: &mut (Arena, Vec<[u64; common::SLICE_LEN]>, Vec<$ty>)| {
                    let (arena, slices, out) = state;
                    $hot(arena, slices, out);
                },
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
    let mut group = c.benchmark_group("criterion_alloc/alloc_slice");

    arena_slice_input_bench!(group, "alloc_slice_copy", common::N, common::alloc_slice_copy);
    group.bench_function("bumpalo_alloc_slice_copy", |b| {
        iter_with_setup(b, common::setup_bump_slices, |state| {
            let (bump, slices) = state;
            common::bumpalo_alloc_slice_copy(bump, slices);
        });
    });
    arena_slice_input_bench!(group, "alloc_slice_clone", common::N, common::alloc_slice_clone);
    group.bench_function("bumpalo_alloc_slice_clone", |b| {
        iter_with_setup(b, common::setup_bump_slices, |state| {
            let (bump, slices) = state;
            common::bumpalo_alloc_slice_clone(bump, slices);
        });
    });
    group.bench_function("alloc_slice_fill_with", |b| {
        iter_with_setup(b, common::warm_arena_local, |arena| common::alloc_slice_fill_with(arena));
    });
    group.bench_function("bumpalo_alloc_slice_fill_with", |b| {
        iter_with_setup(b, common::warm_bump, |bump| common::bumpalo_alloc_slice_fill_with(bump));
    });
    group.bench_function("alloc_slice_fill_iter", |b| {
        iter_with_setup(b, common::warm_arena_local, |arena| common::alloc_slice_fill_iter(arena));
    });
    group.bench_function("bumpalo_alloc_slice_fill_iter", |b| {
        iter_with_setup(b, common::warm_bump, |bump| common::bumpalo_alloc_slice_fill_iter(bump));
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
    let mut group = c.benchmark_group("criterion_alloc/string_builder");

    group.bench_function("alloc_string", |b| {
        iter_with_setup(b, common::setup_arena_words, |state| {
            let (arena, words) = state;
            _ = black_box(common::alloc_string(arena, words));
        });
    });
    group.bench_function("bumpalo_string_new_in", |b| {
        iter_with_setup(b, common::setup_bump_words, |state| {
            let (bump, words) = state;
            _ = black_box(common::bumpalo_string_new_in(bump, words));
        });
    });
    group.bench_function("alloc_string_with_capacity", |b| {
        iter_with_setup(b, common::setup_arena_words_with_len, |state| {
            let (arena, words, len) = state;
            _ = black_box(common::alloc_string_with_capacity(arena, words, *len));
        });
    });
    group.bench_function("bumpalo_string_with_capacity_in", |b| {
        iter_with_setup(b, common::setup_bump_words_with_len, |state| {
            let (bump, words, len) = state;
            _ = black_box(common::bumpalo_string_with_capacity_in(bump, words, *len));
        });
    });
    group.finish();
}

fn bench_vec_builder(c: &mut Criterion) {
    let mut group = c.benchmark_group("criterion_alloc/vec_builder");

    group.bench_function("alloc_vec", |b| {
        iter_with_setup(b, common::setup_arena_ints, |state| {
            let (arena, ints) = state;
            _ = black_box(common::alloc_vec(arena, ints));
        });
    });
    group.bench_function("bumpalo_vec_new_in", |b| {
        iter_with_setup(b, common::setup_bump_ints, |state| {
            let (bump, ints) = state;
            _ = black_box(common::bumpalo_vec_new_in(bump, ints));
        });
    });
    group.bench_function("alloc_vec_with_capacity", |b| {
        iter_with_setup(b, common::setup_arena_ints, |state| {
            let (arena, ints) = state;
            _ = black_box(common::alloc_vec_with_capacity(arena, ints));
        });
    });
    group.bench_function("bumpalo_vec_with_capacity_in", |b| {
        iter_with_setup(b, common::setup_bump_ints, |state| {
            let (bump, ints) = state;
            _ = black_box(common::bumpalo_vec_with_capacity_in(bump, ints));
        });
    });
    group.finish();
}

fn bench_allocator_grow(c: &mut Criterion) {
    let mut group = c.benchmark_group("criterion_alloc/allocator_grow");

    group.bench_function("in_place", |b| {
        iter_with_setup(b, common::warm_arena_local, |arena| common::allocator_grow_in_place(arena));
    });
    group.bench_function("zeroed_in_place", |b| {
        iter_with_setup(b, common::warm_arena_local, |arena| {
            common::allocator_grow_zeroed_in_place(arena);
        });
    });
    group.bench_function("shrink_in_place", |b| {
        iter_with_setup(b, common::warm_arena_local, |arena| common::allocator_shrink_in_place(arena));
    });
    group.finish();
}

criterion_group!(
    benches,
    validate_allocation_contract,
    bench_arena_lifecycle,
    bench_alloc_u64,
    bench_alloc_str,
    bench_alloc_slice,
    bench_string_builder,
    bench_vec_builder,
    bench_allocator_grow,
);
criterion_main!(benches);
