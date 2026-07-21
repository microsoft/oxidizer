// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared support code for benchmark targets.

use std::hint::black_box;
use std::time::{Duration, Instant};

/// Times a Criterion sample by running the benchmark body `iters` times.
pub fn time_sample<R>(mut bench: impl FnMut() -> R) -> impl FnMut(u64) -> Duration {
    move |iters| {
        let start = Instant::now();
        for _ in 0..iters {
            _ = black_box(bench());
        }
        start.elapsed()
    }
}

/// Times a Criterion sample after preparing per-iteration inputs ahead of time.
pub fn time_sample_with_inputs<T, R>(mut setup: impl FnMut() -> T, mut bench: impl FnMut(T) -> R) -> impl FnMut(u64) -> Duration {
    move |iters| {
        let inputs = (0..iters).map(|_| setup()).collect::<Vec<_>>();

        let start = Instant::now();
        for input in inputs {
            _ = black_box(bench(input));
        }
        start.elapsed()
    }
}
