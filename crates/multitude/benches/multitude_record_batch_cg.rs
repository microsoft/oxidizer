// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for a synthetic wide-record batch deserialization
//! workload.
//!
//! Paired with `multitude_record_batch.rs`, which covers the same operations
//! under wall-clock measurement.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![allow(dead_code, reason = "wide deserialized records are consumed as whole values")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only.
}

#[cfg(target_os = "linux")]
#[path = "multitude_record_batch/shared.rs"]
mod shared;

#[cfg(target_os = "linux")]
mod linux {
    use gungraun::{library_benchmark, library_benchmark_group};

    use crate::shared::{
        ArenaRefreshState, RecordBatchState, ReusableVectorState, StandardRefreshState, arena_box_slice_hot_path,
        arena_each_refresh_iteration, arena_each_refresh_state, arena_raw_each_refresh_iteration, arena_raw_each_refresh_state,
        arena_vec_baseline_hot_path, arena_vec_refresh_iteration, arena_vec_refresh_state, escaped_state, malformed_arena_hot_path,
        malformed_standard_hot_path, malformed_state, repeated_no_reset_iteration, reset_recreate_hot_path, reset_recreate_state,
        resource_limited_hot_path, reusable_vector_state, sparse_arena_hot_path, sparse_lazy_standard_hot_path, sparse_standard_hot_path,
        standard_refresh_iteration, standard_refresh_state, standard_vec_hot_path, unescaped_state,
    };

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn decode_standard_vec(state: &RecordBatchState) {
        standard_vec_hot_path(&state.input);
    }

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn decode_arena_box_slice(state: &RecordBatchState) {
        arena_box_slice_hot_path(&state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn decode_arena_vec_baseline(state: &RecordBatchState) {
        arena_vec_baseline_hot_path(&state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn strings_standard_vec_unescaped(state: &RecordBatchState) {
        standard_vec_hot_path(&state.input);
    }

    #[library_benchmark]
    #[bench::run(&escaped_state())]
    fn strings_standard_vec_escaped(state: &RecordBatchState) {
        standard_vec_hot_path(&state.input);
    }

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn strings_arena_vec_unescaped(state: &RecordBatchState) {
        arena_vec_baseline_hot_path(&state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&escaped_state())]
    fn strings_arena_vec_escaped(state: &RecordBatchState) {
        arena_vec_baseline_hot_path(&state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&mut reusable_vector_state())]
    fn reuse_repeated_no_reset(state: &mut ReusableVectorState) {
        repeated_no_reset_iteration(state);
    }

    #[library_benchmark]
    #[bench::run(&mut reset_recreate_state())]
    fn reuse_reset_recreate(state: &mut RecordBatchState) {
        reset_recreate_hot_path(&mut state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn sparse_retention_standard_one_in_eight(state: &RecordBatchState) {
        sparse_standard_hot_path(&state.input);
    }

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn sparse_retention_arena_one_in_eight(state: &RecordBatchState) {
        sparse_arena_hot_path(&state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&escaped_state())]
    fn lazy_raw_strings_eager_sparse_escaped(state: &RecordBatchState) {
        sparse_standard_hot_path(&state.input);
    }

    #[library_benchmark]
    #[bench::run(&escaped_state())]
    fn lazy_raw_strings_lazy_sparse_escaped(state: &RecordBatchState) {
        sparse_lazy_standard_hot_path(&state.input);
    }

    #[library_benchmark]
    #[bench::run(&malformed_state())]
    fn errors_malformed_standard(state: &RecordBatchState) {
        malformed_standard_hot_path(&state.input);
    }

    #[library_benchmark]
    #[bench::run(&malformed_state())]
    fn errors_malformed_arena(state: &RecordBatchState) {
        malformed_arena_hot_path(&state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&unescaped_state())]
    fn errors_resource_limited_arena(state: &RecordBatchState) {
        resource_limited_hot_path(&state.arena, &state.input);
    }

    #[library_benchmark]
    #[bench::run(&mut standard_refresh_state())]
    fn refresh_workload_standard_selective(state: &mut StandardRefreshState) {
        standard_refresh_iteration(state);
    }

    #[library_benchmark]
    #[bench::run(&mut arena_vec_refresh_state())]
    fn refresh_workload_arena_vec_reset_selective(state: &mut ArenaRefreshState) {
        arena_vec_refresh_iteration(state);
    }

    #[library_benchmark]
    #[bench::run(&mut arena_each_refresh_state())]
    fn refresh_workload_arena_each_reset_selective(state: &mut ArenaRefreshState) {
        arena_each_refresh_iteration(state);
    }

    #[library_benchmark]
    #[bench::run(&mut arena_raw_each_refresh_state())]
    fn refresh_workload_arena_raw_each_reset_index_selective(state: &mut ArenaRefreshState) {
        arena_raw_each_refresh_iteration(state);
    }

    library_benchmark_group!(
        name = decode;
        benchmarks = decode_standard_vec, decode_arena_box_slice, decode_arena_vec_baseline
    );
    library_benchmark_group!(
        name = strings;
        benchmarks =
            strings_standard_vec_unescaped,
            strings_standard_vec_escaped,
            strings_arena_vec_unescaped,
            strings_arena_vec_escaped
    );
    library_benchmark_group!(
        name = reuse;
        benchmarks = reuse_repeated_no_reset, reuse_reset_recreate
    );
    library_benchmark_group!(
        name = sparse_retention;
        benchmarks = sparse_retention_standard_one_in_eight, sparse_retention_arena_one_in_eight
    );
    library_benchmark_group!(
        name = lazy_raw_strings;
        benchmarks = lazy_raw_strings_eager_sparse_escaped, lazy_raw_strings_lazy_sparse_escaped
    );
    library_benchmark_group!(
        name = errors;
        benchmarks = errors_malformed_standard, errors_malformed_arena, errors_resource_limited_arena
    );
    library_benchmark_group!(
        name = refresh_workload;
        benchmarks =
            refresh_workload_standard_selective,
            refresh_workload_arena_vec_reset_selective,
            refresh_workload_arena_each_reset_selective,
            refresh_workload_arena_raw_each_reset_index_selective
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{decode, errors, lazy_raw_strings, refresh_workload, reuse, sparse_retention, strings};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default()
        .tool(Callgrind::with_args(["--branch-sim=yes"]));
    library_benchmark_groups =
        decode,
        strings,
        reuse,
        sparse_retention,
        lazy_raw_strings,
        errors,
        refresh_workload
);
