// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared support code for benchmark targets.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::BatchSize;

/// Times a Criterion sample by running the benchmark body `iters` times.
pub fn time_sample<R>(iters: u64, mut bench: impl FnMut() -> R) -> Duration {
    let start = Instant::now();
    for _ in 0..iters {
        _ = black_box(bench());
    }
    start.elapsed()
}

fn div_ceil(numerator: u64, denominator: u64) -> u64 {
    numerator.div_ceil(denominator.max(1))
}

fn iters_per_batch(batch_size: BatchSize, iters: u64) -> u64 {
    match batch_size {
        BatchSize::SmallInput => div_ceil(iters, 10),
        BatchSize::LargeInput => div_ceil(iters, 1000),
        BatchSize::PerIteration => 1,
        BatchSize::NumBatches(batches) => div_ceil(iters, batches),
        BatchSize::NumIterations(batch_iters) => batch_iters.max(1),
        _ => 1,
    }
}

/// Times a Criterion sample over prepared per-iteration inputs.
pub fn time_sample_with_batched_inputs<T, R, M>(
    iters: u64,
    batch_size: BatchSize,
    mut setup: impl FnMut() -> T,
    mut measure: impl FnMut(u64) -> M,
    mut bench: impl FnMut(&mut T) -> R,
) -> Duration {
    let mut elapsed = Duration::ZERO;
    let mut remaining = iters;
    let batch_iters = iters_per_batch(batch_size, iters);

    while remaining > 0 {
        let current_batch = remaining.min(batch_iters);
        let mut inputs = (0..current_batch).map(|_| setup()).collect::<Vec<_>>();
        let mut outputs = Vec::with_capacity(inputs.len());

        let _measurement = measure(current_batch);
        let start = Instant::now();

        for input in &mut inputs {
            outputs.push(black_box(bench(input)));
        }

        elapsed += start.elapsed();
        drop(_measurement);
        drop(outputs);
        drop(inputs);

        remaining -= current_batch;
    }

    elapsed
}

/// Times a Criterion sample over an already-prepared per-iteration input vector.
pub fn time_sample_with_inputs<T, R>(inputs: Vec<T>, mut bench: impl FnMut(T) -> R) -> Duration {
    let start = Instant::now();
    for input in inputs {
        _ = black_box(bench(input));
    }
    start.elapsed()
}

#[cfg(test)]
mod tests {
    use criterion::BatchSize;

    use super::{time_sample, time_sample_with_batched_inputs, time_sample_with_inputs};

    #[test]
    fn time_sample_runs_all_iterations() {
        let mut calls = 0_u64;
        _ = time_sample(11, || {
            calls += 1;
        });

        assert_eq!(calls, 11);
    }

    #[test]
    fn time_sample_with_inputs_processes_all_inputs() {
        let mut seen = Vec::new();
        _ = time_sample_with_inputs(vec![1_u8, 2, 3, 4], |input| {
            seen.push(input);
        });

        assert_eq!(seen, vec![1, 2, 3, 4]);
    }

    #[test]
    fn time_sample_with_batched_inputs_runs_setup_measure_and_bench_for_each_iteration() {
        let mut setup_calls = 0_u64;
        let mut measured_iterations = Vec::new();
        let mut bench_calls = 0_u64;

        _ = time_sample_with_batched_inputs(
            25,
            BatchSize::SmallInput,
            || {
                setup_calls += 1;
                7_u8
            },
            |batch_iters| measured_iterations.push(batch_iters),
            |value| {
                bench_calls += 1;
                *value += 1;
            },
        );

        assert_eq!(setup_calls, 25);
        assert_eq!(bench_calls, 25);
        assert_eq!(measured_iterations, vec![3, 3, 3, 3, 3, 3, 3, 3, 1]);
    }
}
