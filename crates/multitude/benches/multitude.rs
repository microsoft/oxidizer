// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consolidated Criterion and Gungraun benchmark suite for multitude.

#![allow(missing_docs, reason = "benchmark code needs no API documentation")]
#![allow(dead_code, reason = "shared benchmark helpers support multiple groups")]
#![allow(unused_results, reason = "benchmark code intentionally black-boxes results")]
#![allow(clippy::missing_panics_doc, reason = "benchmark code")]
#![allow(
    clippy::needless_pass_by_value,
    reason = "gungraun benchmark inputs are passed and returned by value by the framework"
)]
#![allow(clippy::ref_as_ptr, reason = "benchmark plumbing uses trivial pointer casts")]
#![allow(clippy::similar_names, reason = "benchmark-local names mirror the measured operations")]
#![allow(clippy::std_instead_of_core, reason = "benchmark code")]
#![allow(clippy::too_many_lines, reason = "preserving benchmark structure in one file")]
#![allow(clippy::type_complexity, reason = "benchmark state tuples are inherently complex")]
#![allow(clippy::unwrap_used, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion (emitted for every platform; Gungraun itself only runs on Linux). Upstream tracking issues are pending."
)]

#[path = "multitude_alloc_common/mod.rs"]
mod alloc_common;
#[path = "multitude_record_batch/shared.rs"]
mod record_batch_shared;
#[path = "multitude_serde/shared.rs"]
mod serde_shared;
#[path = "multitude_teardown/shared.rs"]
mod teardown_shared;

use std::alloc::{GlobalAlloc, Layout};
use std::hint::black_box;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

use alloc_common as common;
use alloc_tracker::Session;
use allocator_api2::alloc::{AllocError, Allocator as ApiAllocator};
use allocator_api2::boxed::Box as HeapBox;
use allocator_api2::vec::Vec as HeapVec;
use benchmarking::time_sample;
use criterion::{BatchSize, Bencher, Criterion};
use gungraun::{Callgrind, LibraryBenchmarkConfig};
use multitude::{Alloc, Arc, Arena, Box, Rc};
use record_batch_shared::{
    ArenaRefreshState, RecordBatchState, ReusableVectorState, StandardRefreshState, arena_box_slice_hot_path, arena_each_refresh_iteration,
    arena_each_refresh_state, arena_raw_each_refresh_iteration, arena_raw_each_refresh_state, arena_raw_index_refresh_iteration,
    arena_raw_index_refresh_state, arena_vec_baseline_hot_path, arena_vec_refresh_iteration, arena_vec_refresh_state,
    malformed_arena_hot_path, malformed_json, malformed_standard_hot_path, repeated_no_reset_iteration, reset_recreate_hot_path,
    reset_recreate_state, resource_limited_hot_path, reusable_vector_state, sparse_arena_hot_path, sparse_lazy_standard_hot_path,
    sparse_standard_hot_path, standard_refresh_iteration, standard_refresh_state, standard_vec_hot_path,
    warm_arena as warm_record_batch_arena, workload_json,
};
use serde_shared::{
    ArenaOutput, ArenaRecord as SerdeArenaRecord, StandardRecord as SerdeStandardRecord, arena_output, batch_bumpalo_lifecycle,
    batch_multitude_lifecycle, batch_standard_lifecycle, dynamic_arena_hot_path, dynamic_standard_hot_path, typed_arena_hot_path,
    typed_bumpalo_lifecycle, typed_multitude_lifecycle, typed_standard_hot_path, typed_standard_lifecycle, warm_bump as warm_serde_bump,
    warm_reset_arena,
};
use teardown_shared::{
    LARGE, MEDIUM, SMALL, StandardState, bumpalo_state, free_standard, multitude_state, reset_allocate_bumpalo, reset_allocate_multitude,
    reset_bumpalo, reset_multitude, standard_state,
};

fn callgrind_branch_config() -> LibraryBenchmarkConfig {
    let mut config = LibraryBenchmarkConfig::default();
    config.tool(Callgrind::with_args(["--branch-sim=yes"]));
    config
}

fn is_list_mode() -> bool {
    std::env::args_os().any(|arg| arg == "--list")
}

fn iter_with_setup_alloc<T>(bencher: &mut Bencher<'_>, mut setup: impl FnMut() -> T, mut routine: impl FnMut(&mut T)) {
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

fn validate_allocation_contract() {
    let session = Session::new().no_stdout().no_file();

    assert_allocation_free(&session, "alloc/multitude", common::warm_arena_local(), |arena| {
        common::alloc(arena);
    });
    assert_allocation_free(&session, "alloc/bumpalo", common::warm_bump(), |bump| common::bumpalo_alloc(bump));

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

#[metabench::benchmark(ARENA_LIFECYCLE_MULTITUDE_NEW, "criterion_alloc/arena_lifecycle", "multitude_new")]
fn arena_lifecycle_multitude_new() {
    common::multitude_new();
}

#[metabench::benchmark(ARENA_LIFECYCLE_BUMPALO_NEW, "criterion_alloc/arena_lifecycle", "bumpalo_new")]
fn arena_lifecycle_bumpalo_new() {
    common::bumpalo_new();
}

macro_rules! arena_only_benchmark {
    ($const_name:ident, $fn_name:ident, $group:literal, $label:literal, $setup:expr, $hot:path) => {
        #[metabench::benchmark($const_name, $group, $label)]
        #[bench::run(&mut $setup)]
        fn $fn_name(arena: &mut Arena) {
            $hot(arena);
        }
    };
}

macro_rules! bump_only_benchmark {
    ($const_name:ident, $fn_name:ident, $group:literal, $label:literal, $hot:path) => {
        #[metabench::benchmark($const_name, $group, $label)]
        #[bench::run(&mut common::warm_bump())]
        fn $fn_name(bump: &mut bumpalo::Bump) {
            $hot(bump);
        }
    };
}

macro_rules! arena_collect_benchmark {
    ($const_name:ident, $fn_name:ident, $group:literal, $label:literal, $ty:ty, $count:expr, $hot:path) => {
        #[metabench::benchmark($const_name, $group, $label)]
        #[bench::run(&mut common::setup_arena_out($count))]
        fn $fn_name(state: &mut (Arena, Vec<$ty>)) {
            let (arena, out) = state;
            $hot(arena, out);
        }
    };
}

macro_rules! arena_words_collect_benchmark {
    ($const_name:ident, $fn_name:ident, $group:literal, $label:literal, $ty:ty, $hot:path) => {
        #[metabench::benchmark($const_name, $group, $label)]
        #[bench::run(&mut common::setup_arena_words_out())]
        fn $fn_name(state: &mut (Arena, Vec<String>, Vec<$ty>)) {
            let (arena, words, out) = state;
            $hot(arena, words, out);
        }
    };
}

macro_rules! arena_slice_input_benchmark {
    ($const_name:ident, $fn_name:ident, $group:literal, $label:literal, $count:expr, $hot:path) => {
        #[metabench::benchmark($const_name, $group, $label)]
        #[bench::run(&mut common::setup_arena_slices($count))]
        fn $fn_name(state: &mut (Arena, Vec<[u64; common::SLICE_LEN]>)) {
            let (arena, slices) = state;
            $hot(arena, slices);
        }
    };
}

macro_rules! arena_slice_collect_benchmark {
    ($const_name:ident, $fn_name:ident, $group:literal, $label:literal, $ty:ty, $count:expr, $hot:path) => {
        #[metabench::benchmark($const_name, $group, $label)]
        #[bench::run(&mut common::setup_arena_slices_out($count))]
        fn $fn_name(state: &mut (Arena, Vec<[u64; common::SLICE_LEN]>, Vec<$ty>)) {
            let (arena, slices, out) = state;
            $hot(arena, slices, out);
        }
    };
}

arena_only_benchmark!(
    ALLOC_U64_ALLOC,
    alloc_u64_alloc,
    "criterion_alloc/alloc_u64",
    "alloc",
    common::warm_arena_local(),
    common::alloc
);
bump_only_benchmark!(
    ALLOC_U64_BUMPALO_ALLOC,
    alloc_u64_bumpalo_alloc,
    "criterion_alloc/alloc_u64",
    "bumpalo_alloc",
    common::bumpalo_alloc
);
arena_only_benchmark!(
    ALLOC_U64_ALLOC_WITH,
    alloc_u64_alloc_with,
    "criterion_alloc/alloc_u64",
    "alloc_with",
    common::warm_arena_local(),
    common::alloc_with
);
bump_only_benchmark!(
    ALLOC_U64_BUMPALO_ALLOC_WITH,
    alloc_u64_bumpalo_alloc_with,
    "criterion_alloc/alloc_u64",
    "bumpalo_alloc_with",
    common::bumpalo_alloc_with
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_BOX,
    alloc_u64_alloc_box,
    "criterion_alloc/alloc_u64",
    "alloc_box",
    Box<u64>,
    common::N,
    common::alloc_box
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_BOX_WITH,
    alloc_u64_alloc_box_with,
    "criterion_alloc/alloc_u64",
    "alloc_box_with",
    Box<u64>,
    common::N,
    common::alloc_box_with
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_UNINIT_BOX,
    alloc_u64_alloc_uninit_box,
    "criterion_alloc/alloc_u64",
    "alloc_uninit_box",
    Box<std::mem::MaybeUninit<u64>>,
    common::N,
    common::alloc_uninit_box
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_ZEROED_BOX,
    alloc_u64_alloc_zeroed_box,
    "criterion_alloc/alloc_u64",
    "alloc_zeroed_box",
    Box<std::mem::MaybeUninit<u64>>,
    common::N,
    common::alloc_zeroed_box
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_ARC,
    alloc_u64_alloc_arc,
    "criterion_alloc/alloc_u64",
    "alloc_arc",
    Arc<u64>,
    common::N,
    common::alloc_arc
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_ARC_WITH,
    alloc_u64_alloc_arc_with,
    "criterion_alloc/alloc_u64",
    "alloc_arc_with",
    Arc<u64>,
    common::N,
    common::alloc_arc_with
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_UNINIT_ARC,
    alloc_u64_alloc_uninit_arc,
    "criterion_alloc/alloc_u64",
    "alloc_uninit_arc",
    Arc<std::mem::MaybeUninit<u64>>,
    common::N,
    common::alloc_uninit_arc
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_ZEROED_ARC,
    alloc_u64_alloc_zeroed_arc,
    "criterion_alloc/alloc_u64",
    "alloc_zeroed_arc",
    Arc<std::mem::MaybeUninit<u64>>,
    common::N,
    common::alloc_zeroed_arc
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_RC,
    alloc_u64_alloc_rc,
    "criterion_alloc/alloc_u64",
    "alloc_rc",
    Rc<u64>,
    common::N,
    common::alloc_rc
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_RC_WITH,
    alloc_u64_alloc_rc_with,
    "criterion_alloc/alloc_u64",
    "alloc_rc_with",
    Rc<u64>,
    common::N,
    common::alloc_rc_with
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_UNINIT_RC,
    alloc_u64_alloc_uninit_rc,
    "criterion_alloc/alloc_u64",
    "alloc_uninit_rc",
    Rc<std::mem::MaybeUninit<u64>>,
    common::N,
    common::alloc_uninit_rc
);
arena_collect_benchmark!(
    ALLOC_U64_ALLOC_ZEROED_RC,
    alloc_u64_alloc_zeroed_rc,
    "criterion_alloc/alloc_u64",
    "alloc_zeroed_rc",
    Rc<std::mem::MaybeUninit<u64>>,
    common::N,
    common::alloc_zeroed_rc
);

#[metabench::benchmark(ALLOC_STR_ALLOC_STR, "criterion_alloc/alloc_str", "alloc_str")]
#[bench::run(&mut common::setup_arena_words())]
fn alloc_str_alloc_str(state: &mut (Arena, Vec<String>)) {
    let (arena, words) = state;
    common::alloc_str(arena, words);
}

#[metabench::benchmark(ALLOC_STR_BUMPALO_ALLOC_STR, "criterion_alloc/alloc_str", "bumpalo_alloc_str")]
#[bench::run(&mut common::setup_bump_words())]
fn alloc_str_bumpalo_alloc_str(state: &mut (bumpalo::Bump, Vec<String>)) {
    let (bump, words) = state;
    common::bumpalo_alloc_str(bump, words);
}

arena_words_collect_benchmark!(
    ALLOC_STR_ALLOC_STR_BOX,
    alloc_str_alloc_str_box,
    "criterion_alloc/alloc_str",
    "alloc_str_box",
    Box<str>,
    common::alloc_str_box
);
arena_words_collect_benchmark!(
    ALLOC_STR_ALLOC_STR_ARC,
    alloc_str_alloc_str_arc,
    "criterion_alloc/alloc_str",
    "alloc_str_arc",
    Arc<str>,
    common::alloc_str_arc
);
arena_words_collect_benchmark!(
    ALLOC_STR_ALLOC_STR_RC,
    alloc_str_alloc_str_rc,
    "criterion_alloc/alloc_str",
    "alloc_str_rc",
    Rc<str>,
    common::alloc_str_rc
);

arena_slice_input_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_COPY,
    alloc_slice_alloc_slice_copy,
    "criterion_alloc/alloc_slice",
    "alloc_slice_copy",
    common::N,
    common::alloc_slice_copy
);
#[metabench::benchmark(ALLOC_SLICE_BUMPALO_ALLOC_SLICE_COPY, "criterion_alloc/alloc_slice", "bumpalo_alloc_slice_copy")]
#[bench::run(&mut common::setup_bump_slices())]
fn alloc_slice_bumpalo_alloc_slice_copy(state: &mut (bumpalo::Bump, Vec<[u64; common::SLICE_LEN]>)) {
    let (bump, slices) = state;
    common::bumpalo_alloc_slice_copy(bump, slices);
}

arena_slice_input_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_CLONE,
    alloc_slice_alloc_slice_clone,
    "criterion_alloc/alloc_slice",
    "alloc_slice_clone",
    common::N,
    common::alloc_slice_clone
);
#[metabench::benchmark(ALLOC_SLICE_BUMPALO_ALLOC_SLICE_CLONE, "criterion_alloc/alloc_slice", "bumpalo_alloc_slice_clone")]
#[bench::run(&mut common::setup_bump_slices())]
fn alloc_slice_bumpalo_alloc_slice_clone(state: &mut (bumpalo::Bump, Vec<[u64; common::SLICE_LEN]>)) {
    let (bump, slices) = state;
    common::bumpalo_alloc_slice_clone(bump, slices);
}

arena_only_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_WITH,
    alloc_slice_alloc_slice_fill_with,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_with",
    common::warm_arena_local(),
    common::alloc_slice_fill_with
);
bump_only_benchmark!(
    ALLOC_SLICE_BUMPALO_ALLOC_SLICE_FILL_WITH,
    alloc_slice_bumpalo_alloc_slice_fill_with,
    "criterion_alloc/alloc_slice",
    "bumpalo_alloc_slice_fill_with",
    common::bumpalo_alloc_slice_fill_with
);
arena_only_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_ITER,
    alloc_slice_alloc_slice_fill_iter,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_iter",
    common::warm_arena_local(),
    common::alloc_slice_fill_iter
);
bump_only_benchmark!(
    ALLOC_SLICE_BUMPALO_ALLOC_SLICE_FILL_ITER,
    alloc_slice_bumpalo_alloc_slice_fill_iter,
    "criterion_alloc/alloc_slice",
    "bumpalo_alloc_slice_fill_iter",
    common::bumpalo_alloc_slice_fill_iter
);
arena_slice_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_COPY_BOX,
    alloc_slice_alloc_slice_copy_box,
    "criterion_alloc/alloc_slice",
    "alloc_slice_copy_box",
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_copy_box
);
arena_slice_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_CLONE_BOX,
    alloc_slice_alloc_slice_clone_box,
    "criterion_alloc/alloc_slice",
    "alloc_slice_clone_box",
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_clone_box
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_BOX,
    alloc_slice_alloc_slice_fill_with_box,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_with_box",
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_with_box
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_BOX,
    alloc_slice_alloc_slice_fill_iter_box,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_iter_box",
    Box<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_iter_box
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_UNINIT_SLICE_BOX,
    alloc_slice_alloc_uninit_slice_box,
    "criterion_alloc/alloc_slice",
    "alloc_uninit_slice_box",
    Box<[std::mem::MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_uninit_slice_box
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_ZEROED_SLICE_BOX,
    alloc_slice_alloc_zeroed_slice_box,
    "criterion_alloc/alloc_slice",
    "alloc_zeroed_slice_box",
    Box<[std::mem::MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_zeroed_slice_box
);
arena_slice_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_COPY_ARC,
    alloc_slice_alloc_slice_copy_arc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_copy_arc",
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_copy_arc
);
arena_slice_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_CLONE_ARC,
    alloc_slice_alloc_slice_clone_arc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_clone_arc",
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_clone_arc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_ARC,
    alloc_slice_alloc_slice_fill_with_arc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_with_arc",
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_with_arc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_ARC,
    alloc_slice_alloc_slice_fill_iter_arc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_iter_arc",
    Arc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_iter_arc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_UNINIT_SLICE_ARC,
    alloc_slice_alloc_uninit_slice_arc,
    "criterion_alloc/alloc_slice",
    "alloc_uninit_slice_arc",
    Arc<[std::mem::MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_uninit_slice_arc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_ZEROED_SLICE_ARC,
    alloc_slice_alloc_zeroed_slice_arc,
    "criterion_alloc/alloc_slice",
    "alloc_zeroed_slice_arc",
    Arc<[std::mem::MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_zeroed_slice_arc
);
arena_slice_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_COPY_RC,
    alloc_slice_alloc_slice_copy_rc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_copy_rc",
    Rc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_copy_rc
);
arena_slice_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_CLONE_RC,
    alloc_slice_alloc_slice_clone_rc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_clone_rc",
    Rc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_clone_rc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_RC,
    alloc_slice_alloc_slice_fill_with_rc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_with_rc",
    Rc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_with_rc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_RC,
    alloc_slice_alloc_slice_fill_iter_rc,
    "criterion_alloc/alloc_slice",
    "alloc_slice_fill_iter_rc",
    Rc<[u64]>,
    common::OWNED_SLICE_N,
    common::alloc_slice_fill_iter_rc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_UNINIT_SLICE_RC,
    alloc_slice_alloc_uninit_slice_rc,
    "criterion_alloc/alloc_slice",
    "alloc_uninit_slice_rc",
    Rc<[std::mem::MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_uninit_slice_rc
);
arena_collect_benchmark!(
    ALLOC_SLICE_ALLOC_ZEROED_SLICE_RC,
    alloc_slice_alloc_zeroed_slice_rc,
    "criterion_alloc/alloc_slice",
    "alloc_zeroed_slice_rc",
    Rc<[std::mem::MaybeUninit<u64>]>,
    common::OWNED_SLICE_N,
    common::alloc_zeroed_slice_rc
);

#[metabench::benchmark(STRING_BUILDER_ALLOC_STRING, "criterion_alloc/string_builder", "alloc_string")]
#[bench::run(&mut common::setup_arena_words())]
fn string_builder_alloc_string(state: &mut (Arena, Vec<String>)) {
    let (arena, words) = state;
    _ = common::alloc_string(arena, words);
}

#[metabench::benchmark(STRING_BUILDER_BUMPALO_STRING_NEW_IN, "criterion_alloc/string_builder", "bumpalo_string_new_in")]
#[bench::run(&mut common::setup_bump_words())]
fn string_builder_bumpalo_string_new_in(state: &mut (bumpalo::Bump, Vec<String>)) {
    let (bump, words) = state;
    _ = common::bumpalo_string_new_in(bump, words);
}

#[metabench::benchmark(
    STRING_BUILDER_ALLOC_STRING_WITH_CAPACITY,
    "criterion_alloc/string_builder",
    "alloc_string_with_capacity"
)]
#[bench::run(&mut common::setup_arena_words_with_len())]
fn string_builder_alloc_string_with_capacity(state: &mut (Arena, Vec<String>, usize)) {
    let (arena, words, len) = state;
    _ = common::alloc_string_with_capacity(arena, words, *len);
}

#[metabench::benchmark(
    STRING_BUILDER_BUMPALO_STRING_WITH_CAPACITY_IN,
    "criterion_alloc/string_builder",
    "bumpalo_string_with_capacity_in"
)]
#[bench::run(&mut common::setup_bump_words_with_len())]
fn string_builder_bumpalo_string_with_capacity_in(state: &mut (bumpalo::Bump, Vec<String>, usize)) {
    let (bump, words, len) = state;
    _ = common::bumpalo_string_with_capacity_in(bump, words, *len);
}

#[metabench::benchmark(VEC_BUILDER_ALLOC_VEC, "criterion_alloc/vec_builder", "alloc_vec")]
#[bench::run(&mut common::setup_arena_ints())]
fn vec_builder_alloc_vec(state: &mut (Arena, Vec<i32>)) {
    let (arena, ints) = state;
    _ = common::alloc_vec(arena, ints);
}

#[metabench::benchmark(VEC_BUILDER_BUMPALO_VEC_NEW_IN, "criterion_alloc/vec_builder", "bumpalo_vec_new_in")]
#[bench::run(&mut common::setup_bump_ints())]
fn vec_builder_bumpalo_vec_new_in(state: &mut (bumpalo::Bump, Vec<i32>)) {
    let (bump, ints) = state;
    _ = common::bumpalo_vec_new_in(bump, ints);
}

#[metabench::benchmark(VEC_BUILDER_ALLOC_VEC_WITH_CAPACITY, "criterion_alloc/vec_builder", "alloc_vec_with_capacity")]
#[bench::run(&mut common::setup_arena_ints())]
fn vec_builder_alloc_vec_with_capacity(state: &mut (Arena, Vec<i32>)) {
    let (arena, ints) = state;
    _ = common::alloc_vec_with_capacity(arena, ints);
}

#[metabench::benchmark(
    VEC_BUILDER_BUMPALO_VEC_WITH_CAPACITY_IN,
    "criterion_alloc/vec_builder",
    "bumpalo_vec_with_capacity_in"
)]
#[bench::run(&mut common::setup_bump_ints())]
fn vec_builder_bumpalo_vec_with_capacity_in(state: &mut (bumpalo::Bump, Vec<i32>)) {
    let (bump, ints) = state;
    _ = common::bumpalo_vec_with_capacity_in(bump, ints);
}

arena_only_benchmark!(
    ALLOCATOR_GROW_IN_PLACE,
    allocator_grow_in_place,
    "criterion_alloc/allocator_grow",
    "in_place",
    common::warm_arena_local(),
    common::allocator_grow_in_place
);
arena_only_benchmark!(
    ALLOCATOR_GROW_ZEROED_IN_PLACE,
    allocator_grow_zeroed_in_place,
    "criterion_alloc/allocator_grow",
    "zeroed_in_place",
    common::warm_arena_local(),
    common::allocator_grow_zeroed_in_place
);
arena_only_benchmark!(
    ALLOCATOR_SHRINK_IN_PLACE,
    allocator_shrink_in_place,
    "criterion_alloc/allocator_grow",
    "shrink_in_place",
    common::warm_arena_local(),
    common::allocator_shrink_in_place
);

fn criterion_alloc_benchmarks(criterion: &mut Criterion) {
    if !is_list_mode() {
        validate_allocation_contract();
    }

    let mut arena_lifecycle = criterion.benchmark_group(ARENA_LIFECYCLE_MULTITUDE_NEW.group_name());
    arena_lifecycle.bench_function(ARENA_LIFECYCLE_MULTITUDE_NEW.benchmark_name(), |bencher| {
        bencher.iter(arena_lifecycle_multitude_new);
    });
    arena_lifecycle.bench_function(ARENA_LIFECYCLE_BUMPALO_NEW.benchmark_name(), |bencher| {
        bencher.iter(arena_lifecycle_bumpalo_new);
    });
    arena_lifecycle.finish();

    let mut alloc_u64 = criterion.benchmark_group(ALLOC_U64_ALLOC.group_name());
    alloc_u64.bench_function(ALLOC_U64_ALLOC.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_arena_local, alloc_u64_alloc);
    });
    alloc_u64.bench_function(ALLOC_U64_BUMPALO_ALLOC.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_bump, alloc_u64_bumpalo_alloc);
    });
    alloc_u64.bench_function(ALLOC_U64_ALLOC_WITH.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_arena_local, alloc_u64_alloc_with);
    });
    alloc_u64.bench_function(ALLOC_U64_BUMPALO_ALLOC_WITH.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_bump, alloc_u64_bumpalo_alloc_with);
    });
    macro_rules! alloc_u64_collect {
        ($meta:ident, $func:ident, $ty:ty, $count:expr, $setup:expr) => {
            alloc_u64.bench_function($meta.benchmark_name(), |bencher| {
                iter_with_setup_alloc(bencher, $setup, |state: &mut (Arena, Vec<$ty>)| $func(state));
            });
        };
    }
    alloc_u64_collect!(ALLOC_U64_ALLOC_BOX, alloc_u64_alloc_box, Box<u64>, common::N, || {
        common::setup_arena_out::<Box<u64>>(common::N)
    });
    alloc_u64_collect!(ALLOC_U64_ALLOC_BOX_WITH, alloc_u64_alloc_box_with, Box<u64>, common::N, || {
        common::setup_arena_out::<Box<u64>>(common::N)
    });
    alloc_u64_collect!(
        ALLOC_U64_ALLOC_UNINIT_BOX,
        alloc_u64_alloc_uninit_box,
        Box<std::mem::MaybeUninit<u64>>,
        common::N,
        || common::setup_arena_out::<Box<std::mem::MaybeUninit<u64>>>(common::N)
    );
    alloc_u64_collect!(
        ALLOC_U64_ALLOC_ZEROED_BOX,
        alloc_u64_alloc_zeroed_box,
        Box<std::mem::MaybeUninit<u64>>,
        common::N,
        || common::setup_arena_out::<Box<std::mem::MaybeUninit<u64>>>(common::N)
    );
    alloc_u64_collect!(ALLOC_U64_ALLOC_ARC, alloc_u64_alloc_arc, Arc<u64>, common::N, || {
        common::setup_arena_out::<Arc<u64>>(common::N)
    });
    alloc_u64_collect!(ALLOC_U64_ALLOC_ARC_WITH, alloc_u64_alloc_arc_with, Arc<u64>, common::N, || {
        common::setup_arena_out::<Arc<u64>>(common::N)
    });
    alloc_u64_collect!(
        ALLOC_U64_ALLOC_UNINIT_ARC,
        alloc_u64_alloc_uninit_arc,
        Arc<std::mem::MaybeUninit<u64>>,
        common::N,
        || common::setup_arena_out::<Arc<std::mem::MaybeUninit<u64>>>(common::N)
    );
    alloc_u64_collect!(
        ALLOC_U64_ALLOC_ZEROED_ARC,
        alloc_u64_alloc_zeroed_arc,
        Arc<std::mem::MaybeUninit<u64>>,
        common::N,
        || common::setup_arena_out::<Arc<std::mem::MaybeUninit<u64>>>(common::N)
    );
    alloc_u64_collect!(ALLOC_U64_ALLOC_RC, alloc_u64_alloc_rc, Rc<u64>, common::N, || {
        common::setup_arena_out::<Rc<u64>>(common::N)
    });
    alloc_u64_collect!(ALLOC_U64_ALLOC_RC_WITH, alloc_u64_alloc_rc_with, Rc<u64>, common::N, || {
        common::setup_arena_out::<Rc<u64>>(common::N)
    });
    alloc_u64_collect!(
        ALLOC_U64_ALLOC_UNINIT_RC,
        alloc_u64_alloc_uninit_rc,
        Rc<std::mem::MaybeUninit<u64>>,
        common::N,
        || common::setup_arena_out::<Rc<std::mem::MaybeUninit<u64>>>(common::N)
    );
    alloc_u64_collect!(
        ALLOC_U64_ALLOC_ZEROED_RC,
        alloc_u64_alloc_zeroed_rc,
        Rc<std::mem::MaybeUninit<u64>>,
        common::N,
        || common::setup_arena_out::<Rc<std::mem::MaybeUninit<u64>>>(common::N)
    );
    alloc_u64.finish();

    let mut alloc_str = criterion.benchmark_group(ALLOC_STR_ALLOC_STR.group_name());
    alloc_str.bench_function(ALLOC_STR_ALLOC_STR.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_arena_words, alloc_str_alloc_str);
    });
    alloc_str.bench_function(ALLOC_STR_BUMPALO_ALLOC_STR.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_bump_words, alloc_str_bumpalo_alloc_str);
    });
    macro_rules! alloc_str_collect {
        ($meta:ident, $func:ident, $ty:ty) => {
            alloc_str.bench_function($meta.benchmark_name(), |bencher| {
                iter_with_setup_alloc(bencher, common::setup_arena_words_out::<$ty>, |state| $func(state));
            });
        };
    }
    alloc_str_collect!(ALLOC_STR_ALLOC_STR_BOX, alloc_str_alloc_str_box, Box<str>);
    alloc_str_collect!(ALLOC_STR_ALLOC_STR_ARC, alloc_str_alloc_str_arc, Arc<str>);
    alloc_str_collect!(ALLOC_STR_ALLOC_STR_RC, alloc_str_alloc_str_rc, Rc<str>);
    alloc_str.finish();

    let mut alloc_slice = criterion.benchmark_group(ALLOC_SLICE_ALLOC_SLICE_COPY.group_name());
    alloc_slice.bench_function(ALLOC_SLICE_ALLOC_SLICE_COPY.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, || common::setup_arena_slices(common::N), alloc_slice_alloc_slice_copy);
    });
    alloc_slice.bench_function(ALLOC_SLICE_BUMPALO_ALLOC_SLICE_COPY.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_bump_slices, alloc_slice_bumpalo_alloc_slice_copy);
    });
    alloc_slice.bench_function(ALLOC_SLICE_ALLOC_SLICE_CLONE.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, || common::setup_arena_slices(common::N), alloc_slice_alloc_slice_clone);
    });
    alloc_slice.bench_function(ALLOC_SLICE_BUMPALO_ALLOC_SLICE_CLONE.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_bump_slices, alloc_slice_bumpalo_alloc_slice_clone);
    });
    alloc_slice.bench_function(ALLOC_SLICE_ALLOC_SLICE_FILL_WITH.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_arena_local, alloc_slice_alloc_slice_fill_with);
    });
    alloc_slice.bench_function(ALLOC_SLICE_BUMPALO_ALLOC_SLICE_FILL_WITH.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_bump, alloc_slice_bumpalo_alloc_slice_fill_with);
    });
    alloc_slice.bench_function(ALLOC_SLICE_ALLOC_SLICE_FILL_ITER.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_arena_local, alloc_slice_alloc_slice_fill_iter);
    });
    alloc_slice.bench_function(ALLOC_SLICE_BUMPALO_ALLOC_SLICE_FILL_ITER.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_bump, alloc_slice_bumpalo_alloc_slice_fill_iter);
    });
    macro_rules! alloc_slice_collect {
        ($meta:ident, $func:ident, $ty:ty, $count:expr) => {
            alloc_slice.bench_function($meta.benchmark_name(), |bencher| {
                iter_with_setup_alloc(
                    bencher,
                    || common::setup_arena_slices_out::<$ty>($count),
                    |state| $func(state),
                );
            });
        };
    }
    macro_rules! alloc_slice_generated {
        ($meta:ident, $func:ident, $ty:ty, $count:expr) => {
            alloc_slice.bench_function($meta.benchmark_name(), |bencher| {
                iter_with_setup_alloc(bencher, || common::setup_arena_out::<$ty>($count), |state| $func(state));
            });
        };
    }
    alloc_slice_collect!(
        ALLOC_SLICE_ALLOC_SLICE_COPY_BOX,
        alloc_slice_alloc_slice_copy_box,
        Box<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_collect!(
        ALLOC_SLICE_ALLOC_SLICE_CLONE_BOX,
        alloc_slice_alloc_slice_clone_box,
        Box<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_BOX,
        alloc_slice_alloc_slice_fill_with_box,
        Box<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_BOX,
        alloc_slice_alloc_slice_fill_iter_box,
        Box<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_UNINIT_SLICE_BOX,
        alloc_slice_alloc_uninit_slice_box,
        Box<[std::mem::MaybeUninit<u64>]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_ZEROED_SLICE_BOX,
        alloc_slice_alloc_zeroed_slice_box,
        Box<[std::mem::MaybeUninit<u64>]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_collect!(
        ALLOC_SLICE_ALLOC_SLICE_COPY_ARC,
        alloc_slice_alloc_slice_copy_arc,
        Arc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_collect!(
        ALLOC_SLICE_ALLOC_SLICE_CLONE_ARC,
        alloc_slice_alloc_slice_clone_arc,
        Arc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_ARC,
        alloc_slice_alloc_slice_fill_with_arc,
        Arc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_ARC,
        alloc_slice_alloc_slice_fill_iter_arc,
        Arc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_UNINIT_SLICE_ARC,
        alloc_slice_alloc_uninit_slice_arc,
        Arc<[std::mem::MaybeUninit<u64>]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_ZEROED_SLICE_ARC,
        alloc_slice_alloc_zeroed_slice_arc,
        Arc<[std::mem::MaybeUninit<u64>]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_collect!(
        ALLOC_SLICE_ALLOC_SLICE_COPY_RC,
        alloc_slice_alloc_slice_copy_rc,
        Rc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_collect!(
        ALLOC_SLICE_ALLOC_SLICE_CLONE_RC,
        alloc_slice_alloc_slice_clone_rc,
        Rc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_RC,
        alloc_slice_alloc_slice_fill_with_rc,
        Rc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_RC,
        alloc_slice_alloc_slice_fill_iter_rc,
        Rc<[u64]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_UNINIT_SLICE_RC,
        alloc_slice_alloc_uninit_slice_rc,
        Rc<[std::mem::MaybeUninit<u64>]>,
        common::OWNED_SLICE_N
    );
    alloc_slice_generated!(
        ALLOC_SLICE_ALLOC_ZEROED_SLICE_RC,
        alloc_slice_alloc_zeroed_slice_rc,
        Rc<[std::mem::MaybeUninit<u64>]>,
        common::OWNED_SLICE_N
    );
    alloc_slice.finish();

    let mut string_builder = criterion.benchmark_group(STRING_BUILDER_ALLOC_STRING.group_name());
    string_builder.bench_function(STRING_BUILDER_ALLOC_STRING.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_arena_words, string_builder_alloc_string);
    });
    string_builder.bench_function(STRING_BUILDER_BUMPALO_STRING_NEW_IN.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_bump_words, string_builder_bumpalo_string_new_in);
    });
    string_builder.bench_function(STRING_BUILDER_ALLOC_STRING_WITH_CAPACITY.benchmark_name(), |bencher| {
        iter_with_setup_alloc(
            bencher,
            common::setup_arena_words_with_len,
            string_builder_alloc_string_with_capacity,
        );
    });
    string_builder.bench_function(STRING_BUILDER_BUMPALO_STRING_WITH_CAPACITY_IN.benchmark_name(), |bencher| {
        iter_with_setup_alloc(
            bencher,
            common::setup_bump_words_with_len,
            string_builder_bumpalo_string_with_capacity_in,
        );
    });
    string_builder.finish();

    let mut vec_builder = criterion.benchmark_group(VEC_BUILDER_ALLOC_VEC.group_name());
    vec_builder.bench_function(VEC_BUILDER_ALLOC_VEC.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_arena_ints, vec_builder_alloc_vec);
    });
    vec_builder.bench_function(VEC_BUILDER_BUMPALO_VEC_NEW_IN.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_bump_ints, vec_builder_bumpalo_vec_new_in);
    });
    vec_builder.bench_function(VEC_BUILDER_ALLOC_VEC_WITH_CAPACITY.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_arena_ints, vec_builder_alloc_vec_with_capacity);
    });
    vec_builder.bench_function(VEC_BUILDER_BUMPALO_VEC_WITH_CAPACITY_IN.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::setup_bump_ints, vec_builder_bumpalo_vec_with_capacity_in);
    });
    vec_builder.finish();

    let mut allocator_grow = criterion.benchmark_group(ALLOCATOR_GROW_IN_PLACE.group_name());
    allocator_grow.bench_function(ALLOCATOR_GROW_IN_PLACE.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_arena_local, allocator_grow_in_place);
    });
    allocator_grow.bench_function(ALLOCATOR_GROW_ZEROED_IN_PLACE.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_arena_local, allocator_grow_zeroed_in_place);
    });
    allocator_grow.bench_function(ALLOCATOR_SHRINK_IN_PLACE.benchmark_name(), |bencher| {
        iter_with_setup_alloc(bencher, common::warm_arena_local, allocator_shrink_in_place);
    });
    allocator_grow.finish();
}

const ARC_ARRAY_PROPERTIES: usize = 8;
const ARC_ARRAY_PROPERTY_SIZE: usize = 16;

type GlobalArcArray = std::sync::Arc<[std::sync::Arc<[u8]>]>;
type ArenaArcArrayOfArena = Arc<[Arc<[u8]>]>;
type ArenaArcArrayOfGlobal = Arc<[std::sync::Arc<[u8]>]>;

type GlobalRcArray = std::rc::Rc<[std::rc::Rc<[u8]>]>;
type ArenaRcArrayOfArena = Rc<[Rc<[u8]>]>;
type ArenaRcArrayOfGlobal = Rc<[std::rc::Rc<[u8]>]>;

fn build_global_arc_array(payload: &[u8]) -> GlobalArcArray {
    let mut properties = Vec::with_capacity(ARC_ARRAY_PROPERTIES);
    for _ in 0..ARC_ARRAY_PROPERTIES {
        properties.push(std::sync::Arc::<[u8]>::from(payload));
    }
    std::sync::Arc::from(properties)
}

fn build_global_arc_array_from_slice(properties: &[std::sync::Arc<[u8]>]) -> GlobalArcArray {
    std::sync::Arc::from(properties)
}

fn build_arena_arc_array(arena: &Arena, payload: &[u8]) -> ArenaArcArrayOfArena {
    let mut properties = arena.alloc_vec_with_capacity::<Arc<[u8]>>(ARC_ARRAY_PROPERTIES);
    for _ in 0..ARC_ARRAY_PROPERTIES {
        properties.push(arena.alloc_slice_copy_arc(payload));
    }
    properties.try_into_arc_slice().unwrap()
}

fn build_arena_arc_array_from_slice(arena: &Arena, properties: &[std::sync::Arc<[u8]>]) -> ArenaArcArrayOfGlobal {
    arena.alloc_slice_clone_arc(properties)
}

fn global_arc_properties(payload: &[u8]) -> Vec<std::sync::Arc<[u8]>> {
    (0..ARC_ARRAY_PROPERTIES).map(|_| std::sync::Arc::<[u8]>::from(payload)).collect()
}

fn warm_arc_arena() -> Arena {
    let arena = Arena::builder().with_capacity(128 * 1024).build();
    let _ = arena.alloc(0_u64);
    let _ = arena.alloc_arc(0_u64);
    arena
}

fn setup_arc_array_global() -> Vec<u8> {
    vec![0xAB_u8; ARC_ARRAY_PROPERTY_SIZE]
}

fn setup_arc_array_arena() -> (Arena, Vec<u8>) {
    (warm_arc_arena(), setup_arc_array_global())
}

fn setup_arc_array_global_from_slice() -> Vec<std::sync::Arc<[u8]>> {
    global_arc_properties(&setup_arc_array_global())
}

fn setup_arc_array_arena_from_slice() -> (Arena, Vec<std::sync::Arc<[u8]>>) {
    (warm_arc_arena(), setup_arc_array_global_from_slice())
}

#[metabench::benchmark(ARC_ARRAY_GLOBAL, "criterion_arc_array/arc_array", "global")]
#[bench::run(setup_arc_array_global())]
fn arc_array_global(payload: Vec<u8>) -> (GlobalArcArray, Vec<u8>) {
    let result = black_box(build_global_arc_array(black_box(&payload)));
    (result, payload)
}

#[metabench::benchmark(ARC_ARRAY_ARENA, "criterion_arc_array/arc_array", "arena")]
#[bench::run(setup_arc_array_arena())]
fn arc_array_arena(state: (Arena, Vec<u8>)) -> (ArenaArcArrayOfArena, Arena, Vec<u8>) {
    let (arena, payload) = state;
    let result = black_box(build_arena_arc_array(&arena, black_box(&payload)));
    (result, arena, payload)
}

#[metabench::benchmark(ARC_ARRAY_GLOBAL_FROM_SLICE, "criterion_arc_array/arc_array", "global_from_slice")]
#[bench::run(setup_arc_array_global_from_slice())]
fn arc_array_global_from_slice(properties: Vec<std::sync::Arc<[u8]>>) -> (GlobalArcArray, Vec<std::sync::Arc<[u8]>>) {
    let result = black_box(build_global_arc_array_from_slice(black_box(&properties)));
    (result, properties)
}

#[metabench::benchmark(ARC_ARRAY_ARENA_FROM_SLICE, "criterion_arc_array/arc_array", "arena_from_slice")]
#[bench::run(setup_arc_array_arena_from_slice())]
fn arc_array_arena_from_slice(state: (Arena, Vec<std::sync::Arc<[u8]>>)) -> (ArenaArcArrayOfGlobal, Arena, Vec<std::sync::Arc<[u8]>>) {
    let (arena, properties) = state;
    let result = black_box(build_arena_arc_array_from_slice(&arena, black_box(&properties)));
    (result, arena, properties)
}

fn criterion_arc_array_benchmarks(criterion: &mut Criterion) {
    let payload = vec![0xAB_u8; ARC_ARRAY_PROPERTY_SIZE];
    let mut group = criterion.benchmark_group(ARC_ARRAY_GLOBAL.group_name());

    group.bench_function(ARC_ARRAY_GLOBAL.benchmark_name(), |bencher| {
        bencher.iter_batched(setup_arc_array_global, arc_array_global, BatchSize::SmallInput);
    });
    group.bench_function(ARC_ARRAY_ARENA.benchmark_name(), |bencher| {
        bencher.iter_batched(setup_arc_array_arena, arc_array_arena, BatchSize::SmallInput);
    });

    let global_props = global_arc_properties(&payload);
    group.bench_function(ARC_ARRAY_GLOBAL_FROM_SLICE.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || (),
            |()| black_box(build_global_arc_array_from_slice(black_box(&global_props))),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(ARC_ARRAY_ARENA_FROM_SLICE.benchmark_name(), |bencher| {
        bencher.iter_batched(
            warm_arc_arena,
            |arena| {
                let result = black_box(build_arena_arc_array_from_slice(&arena, black_box(&global_props)));
                (result, arena)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn build_global_rc_array(payload: &[u8]) -> GlobalRcArray {
    let mut properties = Vec::with_capacity(ARC_ARRAY_PROPERTIES);
    for _ in 0..ARC_ARRAY_PROPERTIES {
        properties.push(std::rc::Rc::<[u8]>::from(payload));
    }
    std::rc::Rc::from(properties)
}

fn build_global_rc_array_from_slice(properties: &[std::rc::Rc<[u8]>]) -> GlobalRcArray {
    std::rc::Rc::from(properties)
}

fn build_arena_rc_array(arena: &Arena, payload: &[u8]) -> ArenaRcArrayOfArena {
    let mut properties = arena.alloc_vec_with_capacity::<Rc<[u8]>>(ARC_ARRAY_PROPERTIES);
    for _ in 0..ARC_ARRAY_PROPERTIES {
        properties.push(arena.alloc_slice_copy_rc(payload));
    }
    properties.try_into_rc_slice().unwrap()
}

fn build_arena_rc_array_from_slice(arena: &Arena, properties: &[std::rc::Rc<[u8]>]) -> ArenaRcArrayOfGlobal {
    arena.alloc_slice_clone_rc(properties)
}

fn global_rc_properties(payload: &[u8]) -> Vec<std::rc::Rc<[u8]>> {
    (0..ARC_ARRAY_PROPERTIES).map(|_| std::rc::Rc::<[u8]>::from(payload)).collect()
}

fn warm_rc_arena() -> Arena {
    let arena = Arena::builder().with_capacity(128 * 1024).build();
    let _ = arena.alloc(0_u64);
    let _ = arena.alloc_rc(0_u64);
    arena
}

fn setup_rc_array_global() -> Vec<u8> {
    vec![0xAB_u8; ARC_ARRAY_PROPERTY_SIZE]
}

fn setup_rc_array_arena() -> (Arena, Vec<u8>) {
    (warm_rc_arena(), setup_rc_array_global())
}

fn setup_rc_array_global_from_slice() -> Vec<std::rc::Rc<[u8]>> {
    global_rc_properties(&setup_rc_array_global())
}

fn setup_rc_array_arena_from_slice() -> (Arena, Vec<std::rc::Rc<[u8]>>) {
    (warm_rc_arena(), setup_rc_array_global_from_slice())
}

#[metabench::benchmark(RC_ARRAY_GLOBAL, "criterion_rc_array/rc_array", "global")]
#[bench::run(setup_rc_array_global())]
fn rc_array_global(payload: Vec<u8>) -> (GlobalRcArray, Vec<u8>) {
    let result = black_box(build_global_rc_array(black_box(&payload)));
    (result, payload)
}

#[metabench::benchmark(RC_ARRAY_ARENA, "criterion_rc_array/rc_array", "arena")]
#[bench::run(setup_rc_array_arena())]
fn rc_array_arena(state: (Arena, Vec<u8>)) -> (ArenaRcArrayOfArena, Arena, Vec<u8>) {
    let (arena, payload) = state;
    let result = black_box(build_arena_rc_array(&arena, black_box(&payload)));
    (result, arena, payload)
}

#[metabench::benchmark(RC_ARRAY_GLOBAL_FROM_SLICE, "criterion_rc_array/rc_array", "global_from_slice")]
#[bench::run(setup_rc_array_global_from_slice())]
fn rc_array_global_from_slice(properties: Vec<std::rc::Rc<[u8]>>) -> (GlobalRcArray, Vec<std::rc::Rc<[u8]>>) {
    let result = black_box(build_global_rc_array_from_slice(black_box(&properties)));
    (result, properties)
}

#[metabench::benchmark(RC_ARRAY_ARENA_FROM_SLICE, "criterion_rc_array/rc_array", "arena_from_slice")]
#[bench::run(setup_rc_array_arena_from_slice())]
fn rc_array_arena_from_slice(state: (Arena, Vec<std::rc::Rc<[u8]>>)) -> (ArenaRcArrayOfGlobal, Arena, Vec<std::rc::Rc<[u8]>>) {
    let (arena, properties) = state;
    let result = black_box(build_arena_rc_array_from_slice(&arena, black_box(&properties)));
    (result, arena, properties)
}

fn criterion_rc_array_benchmarks(criterion: &mut Criterion) {
    let payload = vec![0xAB_u8; ARC_ARRAY_PROPERTY_SIZE];
    let mut group = criterion.benchmark_group(RC_ARRAY_GLOBAL.group_name());

    group.bench_function(RC_ARRAY_GLOBAL.benchmark_name(), |bencher| {
        bencher.iter_batched(setup_rc_array_global, rc_array_global, BatchSize::SmallInput);
    });
    group.bench_function(RC_ARRAY_ARENA.benchmark_name(), |bencher| {
        bencher.iter_batched(setup_rc_array_arena, rc_array_arena, BatchSize::SmallInput);
    });

    let global_props = global_rc_properties(&payload);
    group.bench_function(RC_ARRAY_GLOBAL_FROM_SLICE.benchmark_name(), |bencher| {
        bencher.iter_batched(
            || (),
            |()| black_box(build_global_rc_array_from_slice(black_box(&global_props))),
            BatchSize::SmallInput,
        );
    });
    group.bench_function(RC_ARRAY_ARENA_FROM_SLICE.benchmark_name(), |bencher| {
        bencher.iter_batched(
            warm_rc_arena,
            |arena| {
                let result = black_box(build_arena_rc_array_from_slice(&arena, black_box(&global_props)));
                (result, arena)
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

#[derive(Clone, Copy, Default)]
struct MimallocAllocator;

// SAFETY: This adapter forwards every allocation operation to `mimalloc::MiMalloc`
// using the same `Layout`/pointer pairs it receives, so it upholds the allocator
// contract as long as callers pair allocate/deallocate correctly.
unsafe impl ApiAllocator for MimallocAllocator {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // SAFETY: `MiMalloc` implements `GlobalAlloc`; forwarding the requested
        // layout to `alloc` is valid and returns a pointer suitable for the same layout.
        let ptr = unsafe { GlobalAlloc::alloc(&mimalloc::MiMalloc, layout) };
        let ptr = NonNull::new(ptr).ok_or(AllocError)?;
        Ok(NonNull::slice_from_raw_parts(ptr, layout.size()))
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        // SAFETY: `ptr` came from `allocate` above and is deallocated with the
        // same allocator and layout, satisfying `GlobalAlloc::dealloc`.
        unsafe { GlobalAlloc::dealloc(&mimalloc::MiMalloc, ptr.as_ptr(), layout) };
    }
}

type MiBox<T> = HeapBox<T, MimallocAllocator>;
type MiVec<T> = HeapVec<T, MimallocAllocator>;

const ARENA_VS_ALLOCATOR_N: usize = 1_000;
const ARENA_VS_ALLOCATOR_SLICE_LEN: usize = 32;

struct ArenaVsAllocatorArenaState {
    arena: Arena,
    words: Vec<String>,
    payload: [u8; ARENA_VS_ALLOCATOR_SLICE_LEN],
}

struct ArenaVsAllocatorSystemState {
    words: Vec<String>,
    payload: [u8; ARENA_VS_ALLOCATOR_SLICE_LEN],
}

fn alloc_arena_workload(arena: &Arena, words: &[String], payload: &[u8]) {
    let mut vals = arena.alloc_vec_with_capacity::<Alloc<'_, u64>>(ARENA_VS_ALLOCATOR_N);
    let mut slices = arena.alloc_vec_with_capacity::<Alloc<'_, [u8]>>(ARENA_VS_ALLOCATOR_N);
    let mut strs = arena.alloc_vec_with_capacity::<Alloc<'_, str>>(ARENA_VS_ALLOCATOR_N);
    for (index, word) in words.iter().enumerate() {
        vals.push(arena.alloc(black_box(index as u64)));
        slices.push(arena.alloc_slice_copy(black_box(payload)));
        strs.push(arena.alloc_str(black_box(word.as_str())));
    }
    black_box((&vals, &slices, &strs));
}

fn alloc_system_workload(words: &[String], payload: &[u8]) {
    let mut vals: MiVec<MiBox<u64>> = MiVec::with_capacity_in(ARENA_VS_ALLOCATOR_N, MimallocAllocator);
    let mut slices: MiVec<MiBox<[u8]>> = MiVec::with_capacity_in(ARENA_VS_ALLOCATOR_N, MimallocAllocator);
    let mut strs: MiVec<MiBox<str>> = MiVec::with_capacity_in(ARENA_VS_ALLOCATOR_N, MimallocAllocator);
    for (index, word) in words.iter().enumerate() {
        vals.push(MiBox::new_in(black_box(index as u64), MimallocAllocator));
        slices.push(MiBox::<[u8]>::from(black_box(payload)));
        strs.push(MiBox::<str>::from(black_box(word.as_str())));
    }
    black_box((&vals, &slices, &strs));
}

fn setup_arena_vs_allocator_arena() -> ArenaVsAllocatorArenaState {
    let words: Vec<String> = (0..ARENA_VS_ALLOCATOR_N).map(|i| format!("item-{i:08}")).collect();
    let payload = [0xAB_u8; ARENA_VS_ALLOCATOR_SLICE_LEN];
    let mut arena = Arena::new();
    for _ in 0..32 {
        alloc_arena_workload(&arena, &words, payload.as_slice());
        arena.reset();
    }
    ArenaVsAllocatorArenaState { arena, words, payload }
}

fn setup_arena_vs_allocator_system() -> ArenaVsAllocatorSystemState {
    ArenaVsAllocatorSystemState {
        words: (0..ARENA_VS_ALLOCATOR_N).map(|i| format!("item-{i:08}")).collect(),
        payload: [0xAB_u8; ARENA_VS_ALLOCATOR_SLICE_LEN],
    }
}

#[metabench::benchmark(ARENA_VS_ALLOCATOR_ARENA, "criterion_arena_vs_allocator/arena_vs_allocator", "arena")]
#[bench::run(&mut setup_arena_vs_allocator_arena())]
fn arena_vs_allocator_arena(state: &mut ArenaVsAllocatorArenaState) {
    alloc_arena_workload(&state.arena, &state.words, black_box(state.payload.as_slice()));
    state.arena.reset();
}

#[metabench::benchmark(ARENA_VS_ALLOCATOR_SYSTEM, "criterion_arena_vs_allocator/arena_vs_allocator", "system")]
#[bench::run(&setup_arena_vs_allocator_system())]
fn arena_vs_allocator_system(state: &ArenaVsAllocatorSystemState) {
    alloc_system_workload(&state.words, black_box(state.payload.as_slice()));
}

fn criterion_arena_vs_allocator_benchmarks(criterion: &mut Criterion) {
    let words: Vec<String> = (0..ARENA_VS_ALLOCATOR_N).map(|i| format!("item-{i:08}")).collect();
    let payload = [0xAB_u8; ARENA_VS_ALLOCATOR_SLICE_LEN];
    let mut group = criterion.benchmark_group(ARENA_VS_ALLOCATOR_ARENA.group_name());

    group.bench_function(ARENA_VS_ALLOCATOR_ARENA.benchmark_name(), |bencher| {
        let mut arena = Arena::new();
        for _ in 0..32 {
            alloc_arena_workload(&arena, &words, payload.as_slice());
            arena.reset();
        }
        bencher.iter(|| {
            alloc_arena_workload(&arena, &words, black_box(payload.as_slice()));
            arena.reset();
        });
    });

    group.bench_function(ARENA_VS_ALLOCATOR_SYSTEM.benchmark_name(), |bencher| {
        bencher.iter(|| alloc_system_workload(&words, black_box(payload.as_slice())));
    });
    group.finish();
}

const DROP_N: usize = 1_000;
const DROP_SLICE_LEN: usize = 8;

type DroppyT = std::boxed::Box<u64>;

#[expect(clippy::unnecessary_box_returns, reason = "Box<u64> is the T: Drop probe")]
fn make_droppy(index: usize) -> DroppyT {
    std::boxed::Box::new(index as u64)
}

fn setup_drop_box_u64() -> (Vec<Box<u64>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_box(index as u64));
    }
    (handles, arena)
}

fn setup_drop_rc_u64() -> (Vec<Rc<u64>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_rc(index as u64));
    }
    (handles, arena)
}

fn setup_drop_arc_u64() -> (Vec<Arc<u64>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_arc(index as u64));
    }
    (handles, arena)
}

fn setup_drop_box_droppy() -> (Vec<Box<DroppyT>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_box(make_droppy(index)));
    }
    (handles, arena)
}

fn setup_drop_rc_droppy() -> (Vec<Rc<DroppyT>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_rc(make_droppy(index)));
    }
    (handles, arena)
}

fn setup_drop_arc_droppy() -> (Vec<Arc<DroppyT>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_arc(make_droppy(index)));
    }
    (handles, arena)
}

fn setup_drop_str_box() -> (Vec<Box<str>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_str_box(format!("word{index}")));
    }
    (handles, arena)
}

fn setup_drop_str_rc() -> (Vec<Rc<str>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_str_rc(format!("word{index}")));
    }
    (handles, arena)
}

fn setup_drop_str_arc() -> (Vec<Arc<str>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for index in 0..DROP_N {
        handles.push(arena.alloc_str_arc(format!("word{index}")));
    }
    (handles, arena)
}

fn setup_drop_slice_box_u64() -> (Vec<Box<[u64]>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for _ in 0..DROP_N {
        handles.push(arena.alloc_slice_fill_with_box::<u64, _>(DROP_SLICE_LEN, |index| index as u64));
    }
    (handles, arena)
}

fn setup_drop_slice_rc_u64() -> (Vec<Rc<[u64]>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for _ in 0..DROP_N {
        handles.push(arena.alloc_slice_fill_with_rc::<u64, _>(DROP_SLICE_LEN, |index| index as u64));
    }
    (handles, arena)
}

fn setup_drop_slice_arc_u64() -> (Vec<Arc<[u64]>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for _ in 0..DROP_N {
        handles.push(arena.alloc_slice_fill_with_arc::<u64, _>(DROP_SLICE_LEN, |index| index as u64));
    }
    (handles, arena)
}

fn setup_drop_slice_box_droppy() -> (Vec<Box<[DroppyT]>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for _ in 0..DROP_N {
        handles.push(arena.alloc_slice_fill_with_box::<DroppyT, _>(DROP_SLICE_LEN, make_droppy));
    }
    (handles, arena)
}

fn setup_drop_slice_rc_droppy() -> (Vec<Rc<[DroppyT]>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for _ in 0..DROP_N {
        handles.push(arena.alloc_slice_fill_with_rc::<DroppyT, _>(DROP_SLICE_LEN, make_droppy));
    }
    (handles, arena)
}

fn setup_drop_slice_arc_droppy() -> (Vec<Arc<[DroppyT]>>, Arena) {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    let mut handles = Vec::with_capacity(DROP_N);
    for _ in 0..DROP_N {
        handles.push(arena.alloc_slice_fill_with_arc::<DroppyT, _>(DROP_SLICE_LEN, make_droppy));
    }
    (handles, arena)
}

fn setup_drop_arena_drop() -> Arena {
    let arena = Arena::builder().with_capacity(64 * 1024).build();
    for index in 0..DROP_N {
        let _ = black_box(arena.alloc(black_box(index as u64)));
    }
    arena
}

fn setup_clone_rc_u64() -> (Rc<u64>, Arena, Vec<Rc<u64>>) {
    let arena = Arena::new();
    let source = arena.alloc_rc(42_u64);
    (source, arena, Vec::with_capacity(DROP_N))
}

fn setup_clone_arc_u64() -> (Arc<u64>, Arena, Vec<Arc<u64>>) {
    let arena = Arena::new();
    let source = arena.alloc_arc(42_u64);
    (source, arena, Vec::with_capacity(DROP_N))
}

macro_rules! drop_benchmark {
    ($const_name:ident, $fn_name:ident, $group:literal, $label:literal, $setup:expr, $ty:ty) => {
        #[metabench::benchmark($const_name, $group, $label)]
        #[bench::run($setup)]
        fn $fn_name(state: $ty) {
            black_box(state);
        }
    };
}

drop_benchmark!(
    DROP_BOX_U64,
    drop_box_u64,
    "criterion_drop/drop",
    "box_u64",
    setup_drop_box_u64(),
    (Vec<Box<u64>>, Arena)
);
drop_benchmark!(
    DROP_RC_U64,
    drop_rc_u64,
    "criterion_drop/drop",
    "rc_u64",
    setup_drop_rc_u64(),
    (Vec<Rc<u64>>, Arena)
);
drop_benchmark!(
    DROP_ARC_U64,
    drop_arc_u64,
    "criterion_drop/drop",
    "arc_u64",
    setup_drop_arc_u64(),
    (Vec<Arc<u64>>, Arena)
);
drop_benchmark!(
    DROP_BOX_DROPPY,
    drop_box_droppy,
    "criterion_drop/drop",
    "box_droppy",
    setup_drop_box_droppy(),
    (Vec<Box<DroppyT>>, Arena)
);
drop_benchmark!(
    DROP_RC_DROPPY,
    drop_rc_droppy,
    "criterion_drop/drop",
    "rc_droppy",
    setup_drop_rc_droppy(),
    (Vec<Rc<DroppyT>>, Arena)
);
drop_benchmark!(
    DROP_ARC_DROPPY,
    drop_arc_droppy,
    "criterion_drop/drop",
    "arc_droppy",
    setup_drop_arc_droppy(),
    (Vec<Arc<DroppyT>>, Arena)
);
drop_benchmark!(
    DROP_STR_BOX,
    drop_str_box,
    "criterion_drop/drop",
    "str_box",
    setup_drop_str_box(),
    (Vec<Box<str>>, Arena)
);
drop_benchmark!(
    DROP_STR_RC,
    drop_str_rc,
    "criterion_drop/drop",
    "str_rc",
    setup_drop_str_rc(),
    (Vec<Rc<str>>, Arena)
);
drop_benchmark!(
    DROP_STR_ARC,
    drop_str_arc,
    "criterion_drop/drop",
    "str_arc",
    setup_drop_str_arc(),
    (Vec<Arc<str>>, Arena)
);
drop_benchmark!(
    DROP_SLICE_BOX_U64,
    drop_slice_box_u64,
    "criterion_drop/drop",
    "slice_box_u64",
    setup_drop_slice_box_u64(),
    (Vec<Box<[u64]>>, Arena)
);
drop_benchmark!(
    DROP_SLICE_RC_U64,
    drop_slice_rc_u64,
    "criterion_drop/drop",
    "slice_rc_u64",
    setup_drop_slice_rc_u64(),
    (Vec<Rc<[u64]>>, Arena)
);
drop_benchmark!(
    DROP_SLICE_ARC_U64,
    drop_slice_arc_u64,
    "criterion_drop/drop",
    "slice_arc_u64",
    setup_drop_slice_arc_u64(),
    (Vec<Arc<[u64]>>, Arena)
);
drop_benchmark!(
    DROP_SLICE_BOX_DROPPY,
    drop_slice_box_droppy,
    "criterion_drop/drop",
    "slice_box_droppy",
    setup_drop_slice_box_droppy(),
    (Vec<Box<[DroppyT]>>, Arena)
);
drop_benchmark!(
    DROP_SLICE_RC_DROPPY,
    drop_slice_rc_droppy,
    "criterion_drop/drop",
    "slice_rc_droppy",
    setup_drop_slice_rc_droppy(),
    (Vec<Rc<[DroppyT]>>, Arena)
);
drop_benchmark!(
    DROP_SLICE_ARC_DROPPY,
    drop_slice_arc_droppy,
    "criterion_drop/drop",
    "slice_arc_droppy",
    setup_drop_slice_arc_droppy(),
    (Vec<Arc<[DroppyT]>>, Arena)
);
drop_benchmark!(
    DROP_ARENA_DROP,
    drop_arena_drop,
    "criterion_drop/drop",
    "arena_drop",
    setup_drop_arena_drop(),
    Arena
);

#[metabench::benchmark(CLONE_RC_U64, "criterion_drop/clone", "rc_u64")]
#[bench::run(setup_clone_rc_u64())]
fn clone_rc_u64(state: (Rc<u64>, Arena, Vec<Rc<u64>>)) -> (Rc<u64>, Arena, Vec<Rc<u64>>) {
    let (source, arena, mut clones) = state;
    for _ in 0..DROP_N {
        clones.push(black_box(source.clone()));
    }
    (source, arena, clones)
}

#[metabench::benchmark(CLONE_ARC_U64, "criterion_drop/clone", "arc_u64")]
#[bench::run(setup_clone_arc_u64())]
fn clone_arc_u64(state: (Arc<u64>, Arena, Vec<Arc<u64>>)) -> (Arc<u64>, Arena, Vec<Arc<u64>>) {
    let (source, arena, mut clones) = state;
    for _ in 0..DROP_N {
        clones.push(black_box(source.clone()));
    }
    (source, arena, clones)
}

fn criterion_drop_benchmarks(criterion: &mut Criterion) {
    let mut drop_group = criterion.benchmark_group(DROP_BOX_U64.group_name());
    macro_rules! drop_group_bench {
        ($meta:ident, $setup:expr, $func:path) => {
            drop_group.bench_function($meta.benchmark_name(), |bencher| {
                bencher.iter_batched(|| $setup, $func, BatchSize::SmallInput);
            });
        };
    }
    drop_group_bench!(DROP_BOX_U64, setup_drop_box_u64(), drop_box_u64);
    drop_group_bench!(DROP_RC_U64, setup_drop_rc_u64(), drop_rc_u64);
    drop_group_bench!(DROP_ARC_U64, setup_drop_arc_u64(), drop_arc_u64);
    drop_group_bench!(DROP_BOX_DROPPY, setup_drop_box_droppy(), drop_box_droppy);
    drop_group_bench!(DROP_RC_DROPPY, setup_drop_rc_droppy(), drop_rc_droppy);
    drop_group_bench!(DROP_ARC_DROPPY, setup_drop_arc_droppy(), drop_arc_droppy);
    drop_group_bench!(DROP_STR_BOX, setup_drop_str_box(), drop_str_box);
    drop_group_bench!(DROP_STR_RC, setup_drop_str_rc(), drop_str_rc);
    drop_group_bench!(DROP_STR_ARC, setup_drop_str_arc(), drop_str_arc);
    drop_group_bench!(DROP_SLICE_BOX_U64, setup_drop_slice_box_u64(), drop_slice_box_u64);
    drop_group_bench!(DROP_SLICE_RC_U64, setup_drop_slice_rc_u64(), drop_slice_rc_u64);
    drop_group_bench!(DROP_SLICE_ARC_U64, setup_drop_slice_arc_u64(), drop_slice_arc_u64);
    drop_group_bench!(DROP_SLICE_BOX_DROPPY, setup_drop_slice_box_droppy(), drop_slice_box_droppy);
    drop_group_bench!(DROP_SLICE_RC_DROPPY, setup_drop_slice_rc_droppy(), drop_slice_rc_droppy);
    drop_group_bench!(DROP_SLICE_ARC_DROPPY, setup_drop_slice_arc_droppy(), drop_slice_arc_droppy);
    drop_group_bench!(DROP_ARENA_DROP, setup_drop_arena_drop(), drop_arena_drop);
    drop_group.finish();

    let mut clone_group = criterion.benchmark_group(CLONE_RC_U64.group_name());
    clone_group.bench_function(CLONE_RC_U64.benchmark_name(), |bencher| {
        bencher.iter_batched(setup_clone_rc_u64, clone_rc_u64, BatchSize::SmallInput);
    });
    clone_group.bench_function(CLONE_ARC_U64.benchmark_name(), |bencher| {
        bencher.iter_batched(setup_clone_arc_u64, clone_arc_u64, BatchSize::SmallInput);
    });
    clone_group.finish();
}

#[metabench::benchmark(RECORD_BATCH_DECODE_STANDARD_VEC, "multitude_record_batch/decode", "standard_vec")]
#[bench::run(&record_batch_shared::unescaped_state())]
fn record_batch_decode_standard_vec(state: &RecordBatchState) {
    standard_vec_hot_path(&state.input);
}

#[metabench::benchmark(RECORD_BATCH_DECODE_ARENA_BOX_SLICE, "multitude_record_batch/decode", "arena_box_slice")]
#[bench::run(&mut record_batch_shared::unescaped_state())]
fn record_batch_decode_arena_box_slice(state: &mut RecordBatchState) {
    arena_box_slice_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(RECORD_BATCH_DECODE_ARENA_VEC_BASELINE, "multitude_record_batch/decode", "arena_vec_baseline")]
#[bench::run(&mut record_batch_shared::unescaped_state())]
fn record_batch_decode_arena_vec_baseline(state: &mut RecordBatchState) {
    arena_vec_baseline_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_STRINGS_STANDARD_VEC_UNESCAPED,
    "multitude_record_batch/strings",
    "standard_vec_unescaped"
)]
#[bench::run(&record_batch_shared::unescaped_state())]
fn record_batch_strings_standard_vec_unescaped(state: &RecordBatchState) {
    standard_vec_hot_path(&state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_STRINGS_STANDARD_VEC_ESCAPED,
    "multitude_record_batch/strings",
    "standard_vec_escaped"
)]
#[bench::run(&record_batch_shared::escaped_state())]
fn record_batch_strings_standard_vec_escaped(state: &RecordBatchState) {
    standard_vec_hot_path(&state.input);
}

#[metabench::benchmark(RECORD_BATCH_STRINGS_ARENA_VEC_UNESCAPED, "multitude_record_batch/strings", "arena_vec_unescaped")]
#[bench::run(&mut record_batch_shared::unescaped_state())]
fn record_batch_strings_arena_vec_unescaped(state: &mut RecordBatchState) {
    arena_vec_baseline_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(RECORD_BATCH_STRINGS_ARENA_VEC_ESCAPED, "multitude_record_batch/strings", "arena_vec_escaped")]
#[bench::run(&mut record_batch_shared::escaped_state())]
fn record_batch_strings_arena_vec_escaped(state: &mut RecordBatchState) {
    arena_vec_baseline_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(RECORD_BATCH_REUSE_REPEATED_NO_RESET, "multitude_record_batch/reuse", "repeated_no_reset")]
#[bench::run(&mut reusable_vector_state())]
fn record_batch_reuse_repeated_no_reset(state: &mut ReusableVectorState) {
    repeated_no_reset_iteration(state);
}

#[metabench::benchmark(RECORD_BATCH_REUSE_RESET_RECREATE, "multitude_record_batch/reuse", "reset_recreate")]
#[bench::run(&mut reset_recreate_state())]
fn record_batch_reuse_reset_recreate(state: &mut RecordBatchState) {
    reset_recreate_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_SPARSE_RETENTION_STANDARD_ONE_IN_EIGHT,
    "multitude_record_batch/sparse_retention",
    "standard_one_in_eight"
)]
#[bench::run(&record_batch_shared::unescaped_state())]
fn record_batch_sparse_retention_standard_one_in_eight(state: &RecordBatchState) {
    sparse_standard_hot_path(&state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_SPARSE_RETENTION_ARENA_ONE_IN_EIGHT,
    "multitude_record_batch/sparse_retention",
    "arena_one_in_eight"
)]
#[bench::run(&mut record_batch_shared::unescaped_state())]
fn record_batch_sparse_retention_arena_one_in_eight(state: &mut RecordBatchState) {
    sparse_arena_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_LAZY_RAW_STRINGS_EAGER_SPARSE_ESCAPED,
    "multitude_record_batch/lazy_raw_strings",
    "eager_sparse_escaped"
)]
#[bench::run(&record_batch_shared::escaped_state())]
fn record_batch_lazy_raw_strings_eager_sparse_escaped(state: &RecordBatchState) {
    sparse_standard_hot_path(&state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_LAZY_RAW_STRINGS_LAZY_SPARSE_ESCAPED,
    "multitude_record_batch/lazy_raw_strings",
    "lazy_sparse_escaped"
)]
#[bench::run(&record_batch_shared::escaped_state())]
fn record_batch_lazy_raw_strings_lazy_sparse_escaped(state: &RecordBatchState) {
    sparse_lazy_standard_hot_path(&state.input);
}

#[metabench::benchmark(RECORD_BATCH_ERRORS_MALFORMED_STANDARD, "multitude_record_batch/errors", "malformed_standard")]
#[bench::run(&record_batch_shared::malformed_state())]
fn record_batch_errors_malformed_standard(state: &RecordBatchState) {
    malformed_standard_hot_path(&state.input);
}

#[metabench::benchmark(RECORD_BATCH_ERRORS_MALFORMED_ARENA, "multitude_record_batch/errors", "malformed_arena")]
#[bench::run(&mut record_batch_shared::malformed_state())]
fn record_batch_errors_malformed_arena(state: &mut RecordBatchState) {
    malformed_arena_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_ERRORS_RESOURCE_LIMITED_ARENA,
    "multitude_record_batch/errors",
    "resource_limited_arena"
)]
#[bench::run(&mut record_batch_shared::unescaped_state())]
fn record_batch_errors_resource_limited_arena(state: &mut RecordBatchState) {
    resource_limited_hot_path(&mut state.arena, &state.input);
}

#[metabench::benchmark(
    RECORD_BATCH_REFRESH_WORKLOAD_STANDARD_GLOBAL_SELECT,
    "multitude_record_batch/refresh_workload",
    "standard_global_select"
)]
#[bench::run(&mut standard_refresh_state())]
fn record_batch_refresh_workload_standard_global_select(state: &mut StandardRefreshState) {
    standard_refresh_iteration(state);
}

#[metabench::benchmark(
    RECORD_BATCH_REFRESH_WORKLOAD_ARENA_VEC_RESET_GLOBAL_SELECT,
    "multitude_record_batch/refresh_workload",
    "arena_vec_reset_global_select"
)]
#[bench::run(&mut arena_vec_refresh_state())]
fn record_batch_refresh_workload_arena_vec_reset_global_select(state: &mut ArenaRefreshState) {
    arena_vec_refresh_iteration(state);
}

#[metabench::benchmark(
    RECORD_BATCH_REFRESH_WORKLOAD_ARENA_EACH_RESET_GLOBAL_SELECT,
    "multitude_record_batch/refresh_workload",
    "arena_each_reset_global_select"
)]
#[bench::run(&mut arena_each_refresh_state())]
fn record_batch_refresh_workload_arena_each_reset_global_select(state: &mut ArenaRefreshState) {
    arena_each_refresh_iteration(state);
}

#[metabench::benchmark(
    RECORD_BATCH_REFRESH_WORKLOAD_ARENA_RAW_EACH_RESET_GLOBAL_SELECT,
    "multitude_record_batch/refresh_workload",
    "arena_raw_each_reset_global_select"
)]
#[bench::run(&mut arena_raw_each_refresh_state())]
fn record_batch_refresh_workload_arena_raw_each_reset_global_select(state: &mut ArenaRefreshState) {
    arena_raw_each_refresh_iteration(state);
}

#[metabench::benchmark(
    RECORD_BATCH_REFRESH_WORKLOAD_ARENA_RAW_INDEX_RESET_GLOBAL_SELECT,
    "multitude_record_batch/refresh_workload",
    "arena_raw_index_reset_global_select"
)]
#[bench::run(&mut arena_raw_index_refresh_state())]
fn record_batch_refresh_workload_arena_raw_index_reset_global_select(state: &mut ArenaRefreshState) {
    arena_raw_index_refresh_iteration(state);
}

#[cfg(feature = "stats")]
fn record_batch_diagnostics(criterion: &mut Criterion) {
    let input = workload_json(false);
    let (arena, live, released) = record_batch_shared::diagnostic_stats(&input);
    if !is_list_mode() {
        eprintln!("multitude_record_batch ArenaStats with live batch: {live:?}; after drop: {released:?}");
    }

    let mut group = criterion.benchmark_group("multitude_record_batch/diagnostics");
    group.bench_function("arena_stats_snapshot", |bencher| bencher.iter(|| black_box(arena.stats())));
    group.finish();
}

#[cfg(not(feature = "stats"))]
fn record_batch_diagnostics(_: &mut Criterion) {}

fn criterion_record_batch_benchmarks(criterion: &mut Criterion) {
    let input = workload_json(false);
    let allocations = Session::new();
    let standard_allocations = allocations.operation("multitude_record_batch_standard_vec");
    let box_allocations = allocations.operation("multitude_record_batch_arena_box_slice");
    let vec_allocations = allocations.operation("multitude_record_batch_arena_vec_baseline");
    let mut decode = criterion.benchmark_group(RECORD_BATCH_DECODE_STANDARD_VEC.group_name());
    decode.bench_function(RECORD_BATCH_DECODE_STANDARD_VEC.benchmark_name(), |bencher| {
        bencher.iter_custom(|iters| {
            let _span = standard_allocations.measure_thread().iterations(iters);
            time_sample(iters, || standard_vec_hot_path(&input))
        });
    });
    decode.bench_function(RECORD_BATCH_DECODE_ARENA_BOX_SLICE.benchmark_name(), |bencher| {
        let mut arena = warm_record_batch_arena();
        arena_box_slice_hot_path(&mut arena, &input);
        bencher.iter_custom(|iters| {
            let _span = box_allocations.measure_thread().iterations(iters);
            time_sample(iters, || arena_box_slice_hot_path(&mut arena, &input))
        });
    });
    decode.bench_function(RECORD_BATCH_DECODE_ARENA_VEC_BASELINE.benchmark_name(), |bencher| {
        let mut arena = warm_record_batch_arena();
        arena_vec_baseline_hot_path(&mut arena, &input);
        bencher.iter_custom(|iters| {
            let _span = vec_allocations.measure_thread().iterations(iters);
            time_sample(iters, || arena_vec_baseline_hot_path(&mut arena, &input))
        });
    });
    decode.finish();

    let unescaped = workload_json(false);
    let escaped = workload_json(true);
    let mut strings = criterion.benchmark_group(RECORD_BATCH_STRINGS_STANDARD_VEC_UNESCAPED.group_name());
    strings.bench_function(RECORD_BATCH_STRINGS_STANDARD_VEC_UNESCAPED.benchmark_name(), |bencher| {
        bencher.iter(|| standard_vec_hot_path(&unescaped));
    });
    strings.bench_function(RECORD_BATCH_STRINGS_STANDARD_VEC_ESCAPED.benchmark_name(), |bencher| {
        bencher.iter(|| standard_vec_hot_path(&escaped));
    });
    strings.bench_function(RECORD_BATCH_STRINGS_ARENA_VEC_UNESCAPED.benchmark_name(), |bencher| {
        let mut arena = warm_record_batch_arena();
        arena_vec_baseline_hot_path(&mut arena, &unescaped);
        bencher.iter(|| arena_vec_baseline_hot_path(&mut arena, &unescaped));
    });
    strings.bench_function(RECORD_BATCH_STRINGS_ARENA_VEC_ESCAPED.benchmark_name(), |bencher| {
        let mut arena = warm_record_batch_arena();
        arena_vec_baseline_hot_path(&mut arena, &escaped);
        bencher.iter(|| arena_vec_baseline_hot_path(&mut arena, &escaped));
    });
    strings.finish();

    let reuse_allocations = Session::new();
    let repeated_allocations = reuse_allocations.operation("multitude_record_batch_repeated_no_reset");
    let reset_allocations = reuse_allocations.operation("multitude_record_batch_reset_recreate");
    let mut reuse = criterion.benchmark_group(RECORD_BATCH_REUSE_REPEATED_NO_RESET.group_name());
    reuse.bench_function(RECORD_BATCH_REUSE_REPEATED_NO_RESET.benchmark_name(), |bencher| {
        let mut state = reusable_vector_state();
        bencher.iter_custom(|iters| {
            let _span = repeated_allocations.measure_thread().iterations(iters);
            time_sample(iters, || repeated_no_reset_iteration(&mut state))
        });
    });
    reuse.bench_function(RECORD_BATCH_REUSE_RESET_RECREATE.benchmark_name(), |bencher| {
        let mut state = reset_recreate_state();
        bencher.iter_custom(|iters| {
            let _span = reset_allocations.measure_thread().iterations(iters);
            time_sample(iters, || reset_recreate_hot_path(&mut state.arena, &state.input))
        });
    });
    reuse.finish();

    let input = workload_json(false);
    let mut sparse_retention = criterion.benchmark_group(RECORD_BATCH_SPARSE_RETENTION_STANDARD_ONE_IN_EIGHT.group_name());
    sparse_retention.bench_function(RECORD_BATCH_SPARSE_RETENTION_STANDARD_ONE_IN_EIGHT.benchmark_name(), |bencher| {
        bencher.iter(|| sparse_standard_hot_path(&input));
    });
    sparse_retention.bench_function(RECORD_BATCH_SPARSE_RETENTION_ARENA_ONE_IN_EIGHT.benchmark_name(), |bencher| {
        let mut arena = warm_record_batch_arena();
        sparse_arena_hot_path(&mut arena, &input);
        bencher.iter(|| sparse_arena_hot_path(&mut arena, &input));
    });
    sparse_retention.finish();

    let escaped = workload_json(true);
    let mut lazy_raw_strings = criterion.benchmark_group(RECORD_BATCH_LAZY_RAW_STRINGS_EAGER_SPARSE_ESCAPED.group_name());
    lazy_raw_strings.bench_function(RECORD_BATCH_LAZY_RAW_STRINGS_EAGER_SPARSE_ESCAPED.benchmark_name(), |bencher| {
        bencher.iter(|| sparse_standard_hot_path(&escaped));
    });
    lazy_raw_strings.bench_function(RECORD_BATCH_LAZY_RAW_STRINGS_LAZY_SPARSE_ESCAPED.benchmark_name(), |bencher| {
        bencher.iter(|| sparse_lazy_standard_hot_path(&escaped));
    });
    lazy_raw_strings.finish();

    let malformed = malformed_json();
    let valid = workload_json(false);
    let mut errors = criterion.benchmark_group(RECORD_BATCH_ERRORS_MALFORMED_STANDARD.group_name());
    errors.bench_function(RECORD_BATCH_ERRORS_MALFORMED_STANDARD.benchmark_name(), |bencher| {
        bencher.iter(|| malformed_standard_hot_path(&malformed));
    });
    errors.bench_function(RECORD_BATCH_ERRORS_MALFORMED_ARENA.benchmark_name(), |bencher| {
        let mut arena = warm_record_batch_arena();
        malformed_arena_hot_path(&mut arena, &malformed);
        bencher.iter(|| malformed_arena_hot_path(&mut arena, &malformed));
    });
    errors.bench_function(RECORD_BATCH_ERRORS_RESOURCE_LIMITED_ARENA.benchmark_name(), |bencher| {
        let mut arena = warm_record_batch_arena();
        resource_limited_hot_path(&mut arena, &valid);
        bencher.iter(|| resource_limited_hot_path(&mut arena, &valid));
    });
    errors.finish();

    record_batch_diagnostics(criterion);

    let refresh_allocations = Session::new();
    let standard_allocations = refresh_allocations.operation("multitude_record_batch_refresh_standard");
    let vector_allocations = refresh_allocations.operation("multitude_record_batch_refresh_arena_vec");
    let streaming_allocations = refresh_allocations.operation("multitude_record_batch_refresh_arena_each");
    let raw_streaming_allocations = refresh_allocations.operation("multitude_record_batch_refresh_arena_raw_each");
    let raw_index_allocations = refresh_allocations.operation("multitude_record_batch_refresh_arena_raw_index");
    let mut refresh = criterion.benchmark_group(RECORD_BATCH_REFRESH_WORKLOAD_STANDARD_GLOBAL_SELECT.group_name());
    refresh.sample_size(20);
    refresh.bench_function(RECORD_BATCH_REFRESH_WORKLOAD_STANDARD_GLOBAL_SELECT.benchmark_name(), |bencher| {
        let mut state = standard_refresh_state();
        bencher.iter_custom(|iters| {
            let _span = standard_allocations.measure_thread().iterations(iters);
            time_sample(iters, || standard_refresh_iteration(&mut state))
        });
    });
    refresh.bench_function(
        RECORD_BATCH_REFRESH_WORKLOAD_ARENA_VEC_RESET_GLOBAL_SELECT.benchmark_name(),
        |bencher| {
            let mut state = arena_vec_refresh_state();
            bencher.iter_custom(|iters| {
                let _span = vector_allocations.measure_thread().iterations(iters);
                time_sample(iters, || arena_vec_refresh_iteration(&mut state))
            });
        },
    );
    refresh.bench_function(
        RECORD_BATCH_REFRESH_WORKLOAD_ARENA_EACH_RESET_GLOBAL_SELECT.benchmark_name(),
        |bencher| {
            let mut state = arena_each_refresh_state();
            bencher.iter_custom(|iters| {
                let _span = streaming_allocations.measure_thread().iterations(iters);
                time_sample(iters, || arena_each_refresh_iteration(&mut state))
            });
        },
    );
    refresh.bench_function(
        RECORD_BATCH_REFRESH_WORKLOAD_ARENA_RAW_EACH_RESET_GLOBAL_SELECT.benchmark_name(),
        |bencher| {
            let mut state = arena_raw_each_refresh_state();
            bencher.iter_custom(|iters| {
                let _span = raw_streaming_allocations.measure_thread().iterations(iters);
                time_sample(iters, || arena_raw_each_refresh_iteration(&mut state))
            });
        },
    );
    refresh.bench_function(
        RECORD_BATCH_REFRESH_WORKLOAD_ARENA_RAW_INDEX_RESET_GLOBAL_SELECT.benchmark_name(),
        |bencher| {
            let mut state = arena_raw_index_refresh_state();
            bencher.iter_custom(|iters| {
                let _span = raw_index_allocations.measure_thread().iterations(iters);
                time_sample(iters, || arena_raw_index_refresh_iteration(&mut state))
            });
        },
    );
    refresh.finish();
}

#[metabench::benchmark(SERDE_TYPED_ARENA_OWNED, "multitude_serde/typed", "arena_owned")]
#[bench::run(&mut arena_output())]
fn serde_typed_arena_owned(output: &mut ArenaOutput<SerdeArenaRecord>) {
    typed_arena_hot_path(output);
}

#[metabench::benchmark(SERDE_TYPED_SERDE_JSON_OWNED, "multitude_serde/typed", "serde_json_owned")]
#[bench::run(&mut None)]
fn serde_typed_serde_json_owned(output: &mut Option<SerdeStandardRecord>) {
    typed_standard_hot_path(output);
}

#[metabench::benchmark(SERDE_DYNAMIC_ARENA_VALUE, "multitude_serde/dynamic", "arena_value")]
#[bench::run(&mut arena_output())]
fn serde_dynamic_arena_value(output: &mut ArenaOutput<multitude::de::Value>) {
    dynamic_arena_hot_path(output);
}

#[metabench::benchmark(SERDE_DYNAMIC_SERDE_JSON_VALUE, "multitude_serde/dynamic", "serde_json_value")]
#[bench::run(&mut None)]
fn serde_dynamic_serde_json_value(output: &mut Option<serde_json::Value>) {
    dynamic_standard_hot_path(output);
}

#[metabench::benchmark(SERDE_TYPED_LIFECYCLE_SERDE_JSON, "multitude_serde/typed_lifecycle", "serde_json")]
#[bench::run(&mut ())]
fn serde_typed_lifecycle_serde_json(state: &mut ()) {
    typed_standard_lifecycle(state);
}

#[metabench::benchmark(SERDE_TYPED_LIFECYCLE_MULTITUDE, "multitude_serde/typed_lifecycle", "multitude")]
#[bench::run(&mut warm_reset_arena())]
fn serde_typed_lifecycle_multitude(arena: &mut Arena) {
    typed_multitude_lifecycle(arena);
}

#[metabench::benchmark(SERDE_TYPED_LIFECYCLE_BUMPALO, "multitude_serde/typed_lifecycle", "bumpalo")]
#[bench::run(&mut warm_serde_bump())]
fn serde_typed_lifecycle_bumpalo(bump: &mut bumpalo::Bump) {
    typed_bumpalo_lifecycle(bump);
}

#[metabench::benchmark(SERDE_BATCH_LIFECYCLE_SERDE_JSON, "multitude_serde/batch_lifecycle", "serde_json")]
#[bench::run(&mut ())]
fn serde_batch_lifecycle_serde_json(state: &mut ()) {
    batch_standard_lifecycle(state);
}

#[metabench::benchmark(SERDE_BATCH_LIFECYCLE_MULTITUDE, "multitude_serde/batch_lifecycle", "multitude")]
#[bench::run(&mut warm_reset_arena())]
fn serde_batch_lifecycle_multitude(arena: &mut Arena) {
    batch_multitude_lifecycle(arena);
}

#[metabench::benchmark(SERDE_BATCH_LIFECYCLE_BUMPALO, "multitude_serde/batch_lifecycle", "bumpalo")]
#[bench::run(&mut warm_serde_bump())]
fn serde_batch_lifecycle_bumpalo(bump: &mut bumpalo::Bump) {
    batch_bumpalo_lifecycle(bump);
}

fn criterion_serde_benchmarks(criterion: &mut Criterion) {
    let mut typed = criterion.benchmark_group(SERDE_TYPED_ARENA_OWNED.group_name());
    typed.bench_function(SERDE_TYPED_ARENA_OWNED.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(arena_output, typed_arena_hot_path, BatchSize::PerIteration);
    });
    typed.bench_function(SERDE_TYPED_SERDE_JSON_OWNED.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(
            || None,
            |output: &mut Option<SerdeStandardRecord>| typed_standard_hot_path(output),
            BatchSize::PerIteration,
        );
    });
    typed.finish();

    let mut dynamic = criterion.benchmark_group(SERDE_DYNAMIC_ARENA_VALUE.group_name());
    dynamic.bench_function(SERDE_DYNAMIC_ARENA_VALUE.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(arena_output, dynamic_arena_hot_path, BatchSize::PerIteration);
    });
    dynamic.bench_function(SERDE_DYNAMIC_SERDE_JSON_VALUE.benchmark_name(), |bencher| {
        bencher.iter_batched_ref(
            || None,
            |output: &mut Option<serde_json::Value>| dynamic_standard_hot_path(output),
            BatchSize::PerIteration,
        );
    });
    dynamic.finish();

    let mut typed_lifecycle = criterion.benchmark_group(SERDE_TYPED_LIFECYCLE_SERDE_JSON.group_name());
    typed_lifecycle.bench_function(SERDE_TYPED_LIFECYCLE_SERDE_JSON.benchmark_name(), |bencher| {
        let mut state = ();
        bencher.iter(|| typed_standard_lifecycle(&mut state));
    });
    typed_lifecycle.bench_function(SERDE_TYPED_LIFECYCLE_MULTITUDE.benchmark_name(), |bencher| {
        let mut arena = warm_reset_arena();
        bencher.iter(|| typed_multitude_lifecycle(&mut arena));
    });
    typed_lifecycle.bench_function(SERDE_TYPED_LIFECYCLE_BUMPALO.benchmark_name(), |bencher| {
        let mut bump = warm_serde_bump();
        bencher.iter(|| typed_bumpalo_lifecycle(&mut bump));
    });
    typed_lifecycle.finish();

    let mut batch_lifecycle = criterion.benchmark_group(SERDE_BATCH_LIFECYCLE_SERDE_JSON.group_name());
    batch_lifecycle.bench_function(SERDE_BATCH_LIFECYCLE_SERDE_JSON.benchmark_name(), |bencher| {
        let mut state = ();
        bencher.iter(|| batch_standard_lifecycle(&mut state));
    });
    batch_lifecycle.bench_function(SERDE_BATCH_LIFECYCLE_MULTITUDE.benchmark_name(), |bencher| {
        let mut arena = warm_reset_arena();
        bencher.iter(|| batch_multitude_lifecycle(&mut arena));
    });
    batch_lifecycle.bench_function(SERDE_BATCH_LIFECYCLE_BUMPALO.benchmark_name(), |bencher| {
        let mut bump = warm_serde_bump();
        bencher.iter(|| batch_bumpalo_lifecycle(&mut bump));
    });
    batch_lifecycle.finish();
}

const INPUTS_PER_BATCH: u64 = 16;

fn iter_with_setup_teardown<T>(bencher: &mut Bencher<'_>, mut setup: impl FnMut() -> T, mut routine: impl FnMut(&mut T)) {
    bencher.iter_custom(|iters| {
        let mut remaining = iters;
        let mut elapsed = Duration::ZERO;

        while remaining != 0 {
            let batch_len = remaining.min(INPUTS_PER_BATCH);
            let mut inputs = (0..batch_len).map(|_| setup()).collect::<Vec<_>>();
            let start = Instant::now();
            for input in &mut inputs {
                routine(input);
            }
            elapsed += start.elapsed();
            drop(inputs);
            remaining -= batch_len;
        }

        elapsed
    });
}

fn assert_teardown_allocation_free<T>(name: &str, mut input: T, routine: impl FnOnce(&mut T)) {
    let session = Session::new().no_stdout().no_file();
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

macro_rules! teardown_benchmark_count {
    ($count:ident, $standard_const:ident, $standard_fn:ident, $multitude_const:ident, $multitude_fn:ident, $bumpalo_const:ident, $bumpalo_fn:ident, $multitude_reset_const:ident, $multitude_reset_fn:ident, $bumpalo_reset_const:ident, $bumpalo_reset_fn:ident, $label:literal) => {
        #[metabench::benchmark($standard_const, $label, "standard")]
        #[bench::run(&mut standard_state::<$count>())]
        fn $standard_fn(state: &mut StandardState<$count>) {
            free_standard(state);
        }

        #[metabench::benchmark($multitude_const, $label, "multitude")]
        #[bench::run(&mut multitude_state::<$count>())]
        fn $multitude_fn(arena: &mut Arena) {
            reset_multitude(arena);
        }

        #[metabench::benchmark($bumpalo_const, $label, "bumpalo")]
        #[bench::run(&mut bumpalo_state::<$count>())]
        fn $bumpalo_fn(bump: &mut bumpalo::Bump) {
            reset_bumpalo(bump);
        }

        #[metabench::benchmark($multitude_reset_const, $label, "multitude_reset_allocate")]
        #[bench::run(&mut multitude_state::<$count>())]
        fn $multitude_reset_fn(arena: &mut Arena) {
            reset_allocate_multitude(arena);
        }

        #[metabench::benchmark($bumpalo_reset_const, $label, "bumpalo_reset_allocate")]
        #[bench::run(&mut bumpalo_state::<$count>())]
        fn $bumpalo_reset_fn(bump: &mut bumpalo::Bump) {
            reset_allocate_bumpalo(bump);
        }
    };
}

teardown_benchmark_count!(
    SMALL,
    TEARDOWN_FREE_1_STANDARD,
    teardown_free_1_standard,
    TEARDOWN_FREE_1_MULTITUDE,
    teardown_free_1_multitude,
    TEARDOWN_FREE_1_BUMPALO,
    teardown_free_1_bumpalo,
    TEARDOWN_FREE_1_MULTITUDE_RESET_ALLOCATE,
    teardown_free_1_multitude_reset_allocate,
    TEARDOWN_FREE_1_BUMPALO_RESET_ALLOCATE,
    teardown_free_1_bumpalo_reset_allocate,
    "multitude_teardown/free_1"
);

teardown_benchmark_count!(
    MEDIUM,
    TEARDOWN_FREE_32_STANDARD,
    teardown_free_32_standard,
    TEARDOWN_FREE_32_MULTITUDE,
    teardown_free_32_multitude,
    TEARDOWN_FREE_32_BUMPALO,
    teardown_free_32_bumpalo,
    TEARDOWN_FREE_32_MULTITUDE_RESET_ALLOCATE,
    teardown_free_32_multitude_reset_allocate,
    TEARDOWN_FREE_32_BUMPALO_RESET_ALLOCATE,
    teardown_free_32_bumpalo_reset_allocate,
    "multitude_teardown/free_32"
);

teardown_benchmark_count!(
    LARGE,
    TEARDOWN_FREE_1000_STANDARD,
    teardown_free_1000_standard,
    TEARDOWN_FREE_1000_MULTITUDE,
    teardown_free_1000_multitude,
    TEARDOWN_FREE_1000_BUMPALO,
    teardown_free_1000_bumpalo,
    TEARDOWN_FREE_1000_MULTITUDE_RESET_ALLOCATE,
    teardown_free_1000_multitude_reset_allocate,
    TEARDOWN_FREE_1000_BUMPALO_RESET_ALLOCATE,
    teardown_free_1000_bumpalo_reset_allocate,
    "multitude_teardown/free_1000"
);

fn verify_teardown_allocation_contract<const N: usize>(name: &str) {
    assert_teardown_allocation_free(&format!("{name}/multitude_reset"), multitude_state::<N>(), reset_multitude);
    assert_teardown_allocation_free(&format!("{name}/bumpalo_reset"), bumpalo_state::<N>(), reset_bumpalo);
    assert_teardown_allocation_free(
        &format!("{name}/multitude_reset_allocate"),
        multitude_state::<N>(),
        reset_allocate_multitude,
    );
    assert_teardown_allocation_free(
        &format!("{name}/bumpalo_reset_allocate"),
        bumpalo_state::<N>(),
        reset_allocate_bumpalo,
    );
}

fn criterion_teardown_benchmarks(criterion: &mut Criterion) {
    if !is_list_mode() {
        verify_teardown_allocation_contract::<SMALL>("free_1");
    }
    let mut free_1 = criterion.benchmark_group(TEARDOWN_FREE_1_STANDARD.group_name());
    free_1.bench_function(TEARDOWN_FREE_1_STANDARD.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, standard_state::<SMALL>, free_standard);
    });
    free_1.bench_function(TEARDOWN_FREE_1_MULTITUDE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, multitude_state::<SMALL>, reset_multitude);
    });
    free_1.bench_function(TEARDOWN_FREE_1_BUMPALO.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, bumpalo_state::<SMALL>, reset_bumpalo);
    });
    free_1.bench_function(TEARDOWN_FREE_1_MULTITUDE_RESET_ALLOCATE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, multitude_state::<SMALL>, reset_allocate_multitude);
    });
    free_1.bench_function(TEARDOWN_FREE_1_BUMPALO_RESET_ALLOCATE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, bumpalo_state::<SMALL>, reset_allocate_bumpalo);
    });
    free_1.finish();

    if !is_list_mode() {
        verify_teardown_allocation_contract::<MEDIUM>("free_32");
    }
    let mut free_32 = criterion.benchmark_group(TEARDOWN_FREE_32_STANDARD.group_name());
    free_32.bench_function(TEARDOWN_FREE_32_STANDARD.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, standard_state::<MEDIUM>, free_standard);
    });
    free_32.bench_function(TEARDOWN_FREE_32_MULTITUDE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, multitude_state::<MEDIUM>, reset_multitude);
    });
    free_32.bench_function(TEARDOWN_FREE_32_BUMPALO.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, bumpalo_state::<MEDIUM>, reset_bumpalo);
    });
    free_32.bench_function(TEARDOWN_FREE_32_MULTITUDE_RESET_ALLOCATE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, multitude_state::<MEDIUM>, reset_allocate_multitude);
    });
    free_32.bench_function(TEARDOWN_FREE_32_BUMPALO_RESET_ALLOCATE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, bumpalo_state::<MEDIUM>, reset_allocate_bumpalo);
    });
    free_32.finish();

    if !is_list_mode() {
        verify_teardown_allocation_contract::<LARGE>("free_1000");
    }
    let mut free_1000 = criterion.benchmark_group(TEARDOWN_FREE_1000_STANDARD.group_name());
    free_1000.bench_function(TEARDOWN_FREE_1000_STANDARD.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, standard_state::<LARGE>, free_standard);
    });
    free_1000.bench_function(TEARDOWN_FREE_1000_MULTITUDE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, multitude_state::<LARGE>, reset_multitude);
    });
    free_1000.bench_function(TEARDOWN_FREE_1000_BUMPALO.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, bumpalo_state::<LARGE>, reset_bumpalo);
    });
    free_1000.bench_function(TEARDOWN_FREE_1000_MULTITUDE_RESET_ALLOCATE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, multitude_state::<LARGE>, reset_allocate_multitude);
    });
    free_1000.bench_function(TEARDOWN_FREE_1000_BUMPALO_RESET_ALLOCATE.benchmark_name(), |bencher| {
        iter_with_setup_teardown(bencher, bumpalo_state::<LARGE>, reset_allocate_bumpalo);
    });
    free_1000.finish();
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    criterion_alloc_benchmarks(criterion);
    criterion_arc_array_benchmarks(criterion);
    criterion_rc_array_benchmarks(criterion);
    criterion_arena_vs_allocator_benchmarks(criterion);
    criterion_drop_benchmarks(criterion);
    criterion_record_batch_benchmarks(criterion);
    criterion_serde_benchmarks(criterion);
    criterion_teardown_benchmarks(criterion);
}

metabench::main!(
    criterion = criterion_benchmarks,
    groups = {
        CRITERION_ALLOC {
            benchmarks = [
                ARENA_LIFECYCLE_MULTITUDE_NEW,
                ARENA_LIFECYCLE_BUMPALO_NEW,
                ALLOC_U64_ALLOC,
                ALLOC_U64_BUMPALO_ALLOC,
                ALLOC_U64_ALLOC_WITH,
                ALLOC_U64_BUMPALO_ALLOC_WITH,
                ALLOC_U64_ALLOC_BOX,
                ALLOC_U64_ALLOC_BOX_WITH,
                ALLOC_U64_ALLOC_UNINIT_BOX,
                ALLOC_U64_ALLOC_ZEROED_BOX,
                ALLOC_U64_ALLOC_ARC,
                ALLOC_U64_ALLOC_ARC_WITH,
                ALLOC_U64_ALLOC_UNINIT_ARC,
                ALLOC_U64_ALLOC_ZEROED_ARC,
                ALLOC_U64_ALLOC_RC,
                ALLOC_U64_ALLOC_RC_WITH,
                ALLOC_U64_ALLOC_UNINIT_RC,
                ALLOC_U64_ALLOC_ZEROED_RC,
                ALLOC_STR_ALLOC_STR,
                ALLOC_STR_BUMPALO_ALLOC_STR,
                ALLOC_STR_ALLOC_STR_BOX,
                ALLOC_STR_ALLOC_STR_ARC,
                ALLOC_STR_ALLOC_STR_RC,
                ALLOC_SLICE_ALLOC_SLICE_COPY,
                ALLOC_SLICE_BUMPALO_ALLOC_SLICE_COPY,
                ALLOC_SLICE_ALLOC_SLICE_CLONE,
                ALLOC_SLICE_BUMPALO_ALLOC_SLICE_CLONE,
                ALLOC_SLICE_ALLOC_SLICE_FILL_WITH,
                ALLOC_SLICE_BUMPALO_ALLOC_SLICE_FILL_WITH,
                ALLOC_SLICE_ALLOC_SLICE_FILL_ITER,
                ALLOC_SLICE_BUMPALO_ALLOC_SLICE_FILL_ITER,
                ALLOC_SLICE_ALLOC_SLICE_COPY_BOX,
                ALLOC_SLICE_ALLOC_SLICE_CLONE_BOX,
                ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_BOX,
                ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_BOX,
                ALLOC_SLICE_ALLOC_UNINIT_SLICE_BOX,
                ALLOC_SLICE_ALLOC_ZEROED_SLICE_BOX,
                ALLOC_SLICE_ALLOC_SLICE_COPY_ARC,
                ALLOC_SLICE_ALLOC_SLICE_CLONE_ARC,
                ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_ARC,
                ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_ARC,
                ALLOC_SLICE_ALLOC_UNINIT_SLICE_ARC,
                ALLOC_SLICE_ALLOC_ZEROED_SLICE_ARC,
                ALLOC_SLICE_ALLOC_SLICE_COPY_RC,
                ALLOC_SLICE_ALLOC_SLICE_CLONE_RC,
                ALLOC_SLICE_ALLOC_SLICE_FILL_WITH_RC,
                ALLOC_SLICE_ALLOC_SLICE_FILL_ITER_RC,
                ALLOC_SLICE_ALLOC_UNINIT_SLICE_RC,
                ALLOC_SLICE_ALLOC_ZEROED_SLICE_RC,
                STRING_BUILDER_ALLOC_STRING,
                STRING_BUILDER_BUMPALO_STRING_NEW_IN,
                STRING_BUILDER_ALLOC_STRING_WITH_CAPACITY,
                STRING_BUILDER_BUMPALO_STRING_WITH_CAPACITY_IN,
                VEC_BUILDER_ALLOC_VEC,
                VEC_BUILDER_BUMPALO_VEC_NEW_IN,
                VEC_BUILDER_ALLOC_VEC_WITH_CAPACITY,
                VEC_BUILDER_BUMPALO_VEC_WITH_CAPACITY_IN,
                ALLOCATOR_GROW_IN_PLACE,
                ALLOCATOR_GROW_ZEROED_IN_PLACE,
                ALLOCATOR_SHRINK_IN_PLACE,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        ARC_ARRAY {
            benchmarks = [ARC_ARRAY_GLOBAL, ARC_ARRAY_ARENA, ARC_ARRAY_GLOBAL_FROM_SLICE, ARC_ARRAY_ARENA_FROM_SLICE],
            gungraun_config = callgrind_branch_config(),
        },
        RC_ARRAY {
            benchmarks = [RC_ARRAY_GLOBAL, RC_ARRAY_ARENA, RC_ARRAY_GLOBAL_FROM_SLICE, RC_ARRAY_ARENA_FROM_SLICE],
            gungraun_config = callgrind_branch_config(),
        },
        ARENA_VS_ALLOCATOR {
            benchmarks = [ARENA_VS_ALLOCATOR_ARENA, ARENA_VS_ALLOCATOR_SYSTEM],
            gungraun_config = callgrind_branch_config(),
        },
        DROP {
            benchmarks = [
                DROP_BOX_U64,
                DROP_RC_U64,
                DROP_ARC_U64,
                DROP_BOX_DROPPY,
                DROP_RC_DROPPY,
                DROP_ARC_DROPPY,
                DROP_STR_BOX,
                DROP_STR_RC,
                DROP_STR_ARC,
                DROP_SLICE_BOX_U64,
                DROP_SLICE_RC_U64,
                DROP_SLICE_ARC_U64,
                DROP_SLICE_BOX_DROPPY,
                DROP_SLICE_RC_DROPPY,
                DROP_SLICE_ARC_DROPPY,
                DROP_ARENA_DROP,
                CLONE_RC_U64,
                CLONE_ARC_U64,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        RECORD_BATCH {
            benchmarks = [
                RECORD_BATCH_DECODE_STANDARD_VEC,
                RECORD_BATCH_DECODE_ARENA_BOX_SLICE,
                RECORD_BATCH_DECODE_ARENA_VEC_BASELINE,
                RECORD_BATCH_STRINGS_STANDARD_VEC_UNESCAPED,
                RECORD_BATCH_STRINGS_STANDARD_VEC_ESCAPED,
                RECORD_BATCH_STRINGS_ARENA_VEC_UNESCAPED,
                RECORD_BATCH_STRINGS_ARENA_VEC_ESCAPED,
                RECORD_BATCH_REUSE_REPEATED_NO_RESET,
                RECORD_BATCH_REUSE_RESET_RECREATE,
                RECORD_BATCH_SPARSE_RETENTION_STANDARD_ONE_IN_EIGHT,
                RECORD_BATCH_SPARSE_RETENTION_ARENA_ONE_IN_EIGHT,
                RECORD_BATCH_LAZY_RAW_STRINGS_EAGER_SPARSE_ESCAPED,
                RECORD_BATCH_LAZY_RAW_STRINGS_LAZY_SPARSE_ESCAPED,
                RECORD_BATCH_ERRORS_MALFORMED_STANDARD,
                RECORD_BATCH_ERRORS_MALFORMED_ARENA,
                RECORD_BATCH_ERRORS_RESOURCE_LIMITED_ARENA,
                RECORD_BATCH_REFRESH_WORKLOAD_STANDARD_GLOBAL_SELECT,
                RECORD_BATCH_REFRESH_WORKLOAD_ARENA_VEC_RESET_GLOBAL_SELECT,
                RECORD_BATCH_REFRESH_WORKLOAD_ARENA_EACH_RESET_GLOBAL_SELECT,
                RECORD_BATCH_REFRESH_WORKLOAD_ARENA_RAW_EACH_RESET_GLOBAL_SELECT,
                RECORD_BATCH_REFRESH_WORKLOAD_ARENA_RAW_INDEX_RESET_GLOBAL_SELECT,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        SERDE {
            benchmarks = [
                SERDE_TYPED_ARENA_OWNED,
                SERDE_TYPED_SERDE_JSON_OWNED,
                SERDE_DYNAMIC_ARENA_VALUE,
                SERDE_DYNAMIC_SERDE_JSON_VALUE,
                SERDE_TYPED_LIFECYCLE_SERDE_JSON,
                SERDE_TYPED_LIFECYCLE_MULTITUDE,
                SERDE_TYPED_LIFECYCLE_BUMPALO,
                SERDE_BATCH_LIFECYCLE_SERDE_JSON,
                SERDE_BATCH_LIFECYCLE_MULTITUDE,
                SERDE_BATCH_LIFECYCLE_BUMPALO,
            ],
            gungraun_config = callgrind_branch_config(),
        },
        TEARDOWN {
            benchmarks = [
                TEARDOWN_FREE_1_STANDARD,
                TEARDOWN_FREE_1_MULTITUDE,
                TEARDOWN_FREE_1_BUMPALO,
                TEARDOWN_FREE_1_MULTITUDE_RESET_ALLOCATE,
                TEARDOWN_FREE_1_BUMPALO_RESET_ALLOCATE,
                TEARDOWN_FREE_32_STANDARD,
                TEARDOWN_FREE_32_MULTITUDE,
                TEARDOWN_FREE_32_BUMPALO,
                TEARDOWN_FREE_32_MULTITUDE_RESET_ALLOCATE,
                TEARDOWN_FREE_32_BUMPALO_RESET_ALLOCATE,
                TEARDOWN_FREE_1000_STANDARD,
                TEARDOWN_FREE_1000_MULTITUDE,
                TEARDOWN_FREE_1000_BUMPALO,
                TEARDOWN_FREE_1000_MULTITUDE_RESET_ALLOCATE,
                TEARDOWN_FREE_1000_BUMPALO_RESET_ALLOCATE,
            ],
            gungraun_config = callgrind_branch_config(),
        },
    },
);
