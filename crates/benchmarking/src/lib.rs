// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]

//! Shared timing for Criterion benchmarks that use `iter_custom`.
//!
//! Use [`time_sample`] for synchronous operations without per-iteration input,
//! [`time_sample_async`] for asynchronous operations, and
//! [`time_sample_with_inputs`] when every iteration needs prepared mutable state.
//!
//! # Allocation-tracked samples
//!
//! Open one allocation measurement per Criterion sample so the measurement
//! overhead is amortized across every requested iteration:
//!
//! ```no_run
//! use std::alloc::System;
//! use std::hint::black_box;
//!
//! use alloc_tracker::{Allocator, Session};
//! use benchmarking::time_sample;
//! use criterion::Criterion;
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn register(criterion: &mut Criterion) {
//!     let session = Session::new();
//!     let operation = session.operation("answer");
//!
//!     criterion.bench_function("answer", |bencher| {
//!         bencher.iter_custom(|iters| {
//!             let _measurement = operation.measure_thread().iterations(iters);
//!             time_sample(iters, || black_box(42))
//!         });
//!     });
//! }
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};

/// Times a Criterion sample by running the benchmark body `iters` times.
///
/// The callback runs exactly `iters` times. Each output is passed through
/// [`black_box`] and dropped before the next iteration. Zero iterations return
/// [`Duration::ZERO`] without invoking the callback.
#[must_use]
pub fn time_sample<R>(iters: u64, mut bench: impl FnMut() -> R) -> Duration {
    if iters == 0 {
        return Duration::ZERO;
    }

    let start = Instant::now();
    for _ in 0..iters {
        _ = black_box(bench());
    }
    start.elapsed()
}

/// Times an asynchronous Criterion sample by running the body `iters` times.
///
/// The callback receives the zero-based iteration index. Each future is awaited
/// before the next callback invocation. Each output is passed through
/// [`black_box`] and dropped before the next iteration. Zero iterations return
/// [`Duration::ZERO`] without invoking the callback.
#[must_use]
pub async fn time_sample_async<F, Fut, R>(iters: u64, mut bench: F) -> Duration
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = R>,
{
    if iters == 0 {
        return Duration::ZERO;
    }

    let start = Instant::now();
    for iteration in 0..iters {
        _ = black_box(bench(iteration).await);
    }
    start.elapsed()
}

/// Times a sample over caller-prepared mutable inputs.
///
/// `inputs` must contain one value per Criterion iteration. The helper invokes
/// `measure` once, after taking ownership of the prepared vector and before
/// starting the timer. It passes the exact input count to `measure`, then
/// invokes `bench` once for each input.
///
/// Inputs and outputs remain alive until timing and measurement have ended.
/// The value returned by `measure` is treated as an RAII guard and dropped
/// before any input or output teardown. Empty input returns [`Duration::ZERO`]
/// without invoking `measure` or `bench`.
///
/// # Panics
///
/// Panics if the platform's `usize` cannot be represented as `u64`. Every
/// currently supported Rust target represents `usize` with at most 64 bits.
///
/// ```
/// use std::alloc::System;
/// use std::hint::black_box;
///
/// use alloc_tracker::{Allocator, Session};
/// use benchmarking::time_sample_with_inputs;
/// use criterion::Criterion;
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<System> = Allocator::system();
///
/// fn register(criterion: &mut Criterion) {
///     let session = Session::new();
///     let operation = session.operation("increment");
///
///     criterion.bench_function("increment", |bencher| {
///         bencher.iter_custom(|iters| {
///             let inputs = (0..iters).collect::<Vec<_>>();
///             time_sample_with_inputs(
///                 inputs,
///                 |sample_iters| operation.measure_thread().iterations(sample_iters),
///                 |value| black_box(*value + 1),
///             )
///         });
///     });
/// }
/// ```
#[must_use]
pub fn time_sample_with_inputs<T, R, M>(
    mut inputs: Vec<T>,
    measure: impl FnOnce(u64) -> M,
    mut bench: impl FnMut(&mut T) -> R,
) -> Duration {
    if inputs.is_empty() {
        return Duration::ZERO;
    }

    let iterations = u64::try_from(inputs.len()).expect("benchmark input vector length fits in u64 on every supported target");
    let mut outputs = Vec::with_capacity(inputs.len());
    let measurement = measure(iterations);
    let start = Instant::now();

    for input in &mut inputs {
        outputs.push(black_box(bench(input)));
    }

    let elapsed = start.elapsed();
    drop(measurement);
    drop(outputs);
    drop(inputs);
    elapsed
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::pin::pin;
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use super::{time_sample, time_sample_async, time_sample_with_inputs};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());

        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn time_sample_runs_all_iterations() {
        let mut calls = 0_u64;
        _ = time_sample(11, || {
            calls += 1;
        });

        assert_eq!(calls, 11);
    }

    #[test]
    fn time_sample_async_receives_each_iteration_index() {
        let mut seen = Vec::new();
        _ = block_on(time_sample_async(4, |iteration| {
            seen.push(iteration);
            std::future::ready(())
        }));

        assert_eq!(seen, vec![0, 1, 2, 3]);
    }

    struct DropRecorder {
        event: &'static str,
        events: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Drop for DropRecorder {
        fn drop(&mut self) {
            self.events.borrow_mut().push(self.event);
        }
    }

    #[test]
    fn time_sample_with_inputs_ends_measurement_before_teardown() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let input = DropRecorder {
            event: "input dropped",
            events: Rc::clone(&events),
        };

        _ = time_sample_with_inputs(
            vec![input],
            |iterations| {
                assert_eq!(iterations, 1);
                events.borrow_mut().push("measurement started");
                DropRecorder {
                    event: "measurement ended",
                    events: Rc::clone(&events),
                }
            },
            |_| {
                events.borrow_mut().push("benchmark invoked");
                DropRecorder {
                    event: "output dropped",
                    events: Rc::clone(&events),
                }
            },
        );

        assert_eq!(
            *events.borrow(),
            [
                "measurement started",
                "benchmark invoked",
                "measurement ended",
                "output dropped",
                "input dropped",
            ]
        );
    }

    #[test]
    fn empty_samples_do_not_start_measurement() {
        let mut measured = false;
        let mut invoked = false;

        let elapsed = time_sample_with_inputs(
            Vec::<u8>::new(),
            |_| {
                measured = true;
            },
            |_| {
                invoked = true;
            },
        );

        assert_eq!(elapsed, Duration::ZERO);
        assert!(!measured);
        assert!(!invoked);
    }

    #[test]
    fn time_sample_with_inputs_processes_all_inputs() {
        let mut seen = Vec::new();
        _ = time_sample_with_inputs(
            vec![1_u8, 2, 3, 4],
            |_| (),
            |input| {
                seen.push(*input);
                *input += 1;
            },
        );

        assert_eq!(seen, vec![1, 2, 3, 4]);
    }

    #[test]
    fn zero_iteration_samples_do_not_invoke_callbacks() {
        let mut sync_invoked = false;
        let mut async_invoked = false;

        assert_eq!(
            time_sample(0, || {
                sync_invoked = true;
            }),
            Duration::ZERO
        );
        assert_eq!(
            block_on(time_sample_async(0, |_| {
                async_invoked = true;
                std::future::ready(())
            })),
            Duration::ZERO
        );

        assert!(!sync_invoked);
        assert!(!async_invoked);
    }
}
