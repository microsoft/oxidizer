// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

//! Bolero property tests for the end-to-end [`Thunker`] pipeline.
//!
//! Bolero generates random sequences of operations that build a [`Thunker`],
//! dispatch work through `#[thunk]` methods, optionally cancel mid-flight,
//! and finally drop everything. Each iteration asserts:
//!
//! - Every awaited thunk returns the expected transform of its input. A
//!   stale, swapped, or missing value would indicate a state-handoff bug.
//! - The runtime, `Thunker`, and `Service` can all be dropped at any point
//!   without panic, leak, or deadlock — even when futures were cancelled
//!   pre- or post-dispatch.
//!
//! The whole file is gated out of Miri because bolero's corpus replay needs
//! filesystem isolation that Miri does not provide, and the macro/runtime
//! machinery dominates Miri's MIR translation cost even when iterations are
//! skipped. The publish/wake/drop race is covered under loom in `loom.rs`.

#![cfg(not(loom))]
#![cfg(not(miri))]
#![allow(clippy::std_instead_of_core, reason = "test code uses std")]
#![allow(clippy::missing_panics_doc, reason = "test code")]
#![allow(clippy::unwrap_used, reason = "test code")]
#![allow(clippy::cast_possible_truncation, reason = "test indices are bounded")]

use std::sync::Arc;

use bolero::TypeGenerator;
use sync_thunk::{Thunker, thunk};
use tokio::runtime::Builder;

/// Service exposing a simple compute method via `#[thunk]`.
///
/// Wrapped in `Arc<Self>` so cancellation tests can move it into spawned
/// tasks without colliding with the borrow checker.
struct Service {
    thunker: Thunker,
}

impl Service {
    fn new(max_threads: usize, capacity: usize) -> Self {
        Self {
            thunker: Thunker::builder()
                .max_thread_count(max_threads.max(1))
                .channel_capacity(capacity.max(1))
                .build(),
        }
    }

    /// Round-trip transform the property test verifies.
    #[thunk(from = me.thunker)]
    async fn compute(me: Arc<Self>, x: u64) -> u64 {
        let _ = me;
        x.wrapping_add(1)
    }
}

/// Single bolero-generated step. `arg` is the input passed to `compute`; the
/// reply (when produced) must equal `arg.wrapping_add(1)`.
#[derive(Debug, Clone, Copy, TypeGenerator)]
enum Op {
    /// Dispatch and await: the standard happy path.
    Await { arg: u64 },
    /// Build the future but drop it before polling. Should be a no-op:
    /// the macro lazily dispatches on first poll.
    DropBeforePoll { arg: u64 },
    /// Dispatch via `tokio::spawn`, then immediately `abort`. Exercises
    /// the cancellation path: the future is polled at least once (so the
    /// work item is enqueued) and then dropped while the worker may still
    /// be running it. Verifies that `StackState`'s drop guard prevents
    /// use-after-free of the caller's stack frame.
    SpawnAndAbort { arg: u64 },
    /// Sequential `Await` after the previous op so we periodically drain
    /// the queue and synchronise with the worker.
    Drain { arg: u64 },
}

/// Pool sizing knobs, also fuzzed. Bounded to keep iterations cheap.
#[derive(Debug, Clone, Copy, TypeGenerator)]
struct PoolConfig {
    /// `max_thread_count` for the [`Thunker`] (clamped to 1..=4).
    max_threads_raw: u8,
    /// `channel_capacity` for the [`Thunker`] (clamped to 1..=16).
    capacity_raw: u8,
}

impl PoolConfig {
    fn max_threads(self) -> usize {
        (self.max_threads_raw % 4) as usize + 1
    }
    fn capacity(self) -> usize {
        (self.capacity_raw % 16) as usize + 1
    }
}

/// Full per-iteration scenario: pool config + a (bounded) sequence of ops.
#[derive(Debug, Clone, TypeGenerator)]
struct Scenario {
    config: PoolConfig,
    #[generator(bolero::generator::produce::<Vec<Op>>().with().len(0_usize..=32))]
    ops: Vec<Op>,
}

fn run(scenario: &Scenario) {
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async {
        let service = Arc::new(Service::new(scenario.config.max_threads(), scenario.config.capacity()));

        for op in &scenario.ops {
            match *op {
                Op::Await { arg } => {
                    let got = Service::compute(Arc::clone(&service), arg).await;
                    assert_eq!(got, arg.wrapping_add(1), "compute() round-trip mismatch");
                }
                Op::DropBeforePoll { arg } => {
                    // The future hasn't been polled, so no work item has
                    // been enqueued. Dropping must be a no-op.
                    let fut = Service::compute(Arc::clone(&service), arg);
                    drop(fut);
                }
                Op::SpawnAndAbort { arg } => {
                    let svc = Arc::clone(&service);
                    let handle = tokio::spawn(async move { Service::compute(Arc::clone(&svc), arg).await });
                    handle.abort();
                    // Await the handle so the task is fully torn down before
                    // we proceed. We expect either a cancellation error or
                    // (if abort raced the completion) the correct value.
                    match handle.await {
                        Ok(got) => assert_eq!(got, arg.wrapping_add(1)),
                        Err(e) => assert!(e.is_cancelled(), "unexpected task error: {e:?}"),
                    }
                }
                Op::Drain { arg } => {
                    let got = Service::compute(Arc::clone(&service), arg).await;
                    assert_eq!(got, arg.wrapping_add(1), "drain compute() round-trip mismatch");
                }
            }
        }

        // Drop the Service (and its Thunker) while the runtime is still
        // alive. Worker threads must clean up without deadlock.
        drop(service);
    });
    // Runtime drop must also be clean.
    drop(runtime);
}

#[test]
fn thunker_pipeline_property() {
    bolero::check!().with_type::<Scenario>().for_each(run);
}
