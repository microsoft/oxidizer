// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Callgrind benchmarks for basic operations of the executor.
//!
//! Paired with `ae_basic_operations.rs` which covers the same operations under
//! wall-clock measurement. Two scenario groups:
//!
//! * Decomposed scenarios isolate a single executor operation (cycle, task
//!   spawn, join-handle poll) for clean instruction-count attribution.
//! * Composite scenarios mirror the existing Criterion benches (round-trip
//!   spawn-and-complete, yield-round-trip) so totals can be compared across
//!   wall-clock and Callgrind measurements.
//!
//! Scenarios that call `tasks.add()` measure executor + allocator overhead
//! together (Callgrind counts allocator instructions like any other), which
//! is what the wall-clock benches also include.

#![allow(missing_docs, reason = "no need for API documentation on benchmark code")]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Upstream tracking issues are pending."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only. On other platforms this
    // bench target compiles to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;
    use std::pin::{Pin, pin};
    use std::task::{Context, Waker};

    use arty_executor::testing::new_guarded_executor;
    use arty_executor::{CycleOutcome, Executor, JoinHandle, TaskSet};
    use gungraun::prelude::*;
    use scopeguard::{Always, ScopeGuard};
    use testing_aids::YieldFuture;

    // Bundles the executor, a pre-acquired `TaskSet` handle, and an optional
    // boxed-pinned join handle. The executor is wrapped in a `ScopeGuard` so its
    // drop performs the proper shutdown loop. Field declaration order controls
    // drop order: `join_handle` -> `tasks` -> `executor` (last). The executor's
    // shutdown loop refuses to terminate while a `JoinHandle` is outstanding,
    // so the join handle MUST be dropped before the executor. All cleanup
    // happens *after* the measurement window closes.
    struct State {
        join_handle: Option<Pin<Box<JoinHandle<()>>>>,
        tasks: TaskSet,
        executor: ScopeGuard<Executor, fn(Executor), Always>,
    }

    fn make_executor_only() -> State {
        let executor = new_guarded_executor(Waker::noop().clone());
        let tasks = executor.tasks();
        State {
            join_handle: None,
            tasks,
            executor,
        }
    }

    fn make_with_unrun_async() -> State {
        let mut state = make_executor_only();
        let handle = Box::pin(state.tasks.add(async move {}));
        state.join_handle = Some(handle);
        state
    }

    fn make_with_completed_async() -> State {
        let state = make_with_unrun_async();
        // Drive the task to completion so the join handle is Ready before the bench fn runs.
        assert_ne!(state.executor.execute_cycle(), CycleOutcome::Shutdown);
        state
    }

    fn make_with_unrun_yield() -> State {
        let mut state = make_executor_only();
        let handle = Box::pin(state.tasks.add(YieldFuture::default()));
        state.join_handle = Some(handle);
        state
    }

    fn make_with_yield_re_awakened() -> State {
        let state = make_with_unrun_yield();
        // First cycle polls YieldFuture once (it self-wakes and returns Pending).
        assert_eq!(state.executor.execute_cycle(), CycleOutcome::Continue);
        state
    }

    // -----------------------------------------------------------------------------
    // Decomposed group: one measured operation per scenario.
    // -----------------------------------------------------------------------------

    #[library_benchmark]
    #[bench::no_tasks(setup = make_executor_only)]
    fn cycle_empty(state: State) -> State {
        // No tasks. Should return Suspend after walking an empty task list.
        let _outcome = black_box(state.executor.execute_cycle());
        state
    }

    #[library_benchmark]
    #[bench::default(setup = make_executor_only)]
    fn task_add(state: State) -> State {
        let handle = black_box(state.tasks.add(async move {}));
        // Drop the handle inside the measured region just like the wall-clock bench does;
        // detaching it from `state` keeps the state struct moveable for the return.
        drop(black_box(handle));
        state
    }

    #[library_benchmark]
    #[bench::async_unrun(setup = make_with_unrun_async)]
    fn cycle_one_ready(state: State) -> State {
        // The pre-spawned `async {}` task is polled once and completes inside this cycle.
        let _outcome = black_box(state.executor.execute_cycle());
        state
    }

    #[library_benchmark]
    #[bench::async_completed(setup = make_with_completed_async)]
    fn poll_completed(mut state: State) -> State {
        let mut cx = Context::from_waker(Waker::noop());
        let handle = state.join_handle.as_mut().expect("setup populated a completed join handle");
        let _result = black_box(handle.as_mut().poll(&mut cx));
        state
    }

    #[library_benchmark]
    #[bench::yield_unrun(setup = make_with_unrun_yield)]
    fn yield_cycle_first(state: State) -> State {
        // YieldFuture's first poll: self-wakes, returns Pending => cycle reports Continue.
        let _outcome = black_box(state.executor.execute_cycle());
        state
    }

    #[library_benchmark]
    #[bench::yield_re_awakened(setup = make_with_yield_re_awakened)]
    fn yield_cycle_second(state: State) -> State {
        // YieldFuture's second poll: returns Ready => task completes, cycle reports Suspend.
        let _outcome = black_box(state.executor.execute_cycle());
        state
    }

    library_benchmark_group!(
        name = decomposed_group,
        benchmarks = [
            cycle_empty,
            task_add,
            cycle_one_ready,
            poll_completed,
            yield_cycle_first,
            yield_cycle_second,
        ]
    );

    // -----------------------------------------------------------------------------
    // Composite group: parity with the wall-clock `_one` Criterion benches.
    // -----------------------------------------------------------------------------

    #[library_benchmark]
    #[bench::async_round_trip(setup = make_executor_only)]
    fn spawn_and_complete(state: State) -> State {
        let mut join_handle = pin!(state.tasks.add(async move {}));
        assert_ne!(state.executor.execute_cycle(), CycleOutcome::Shutdown);
        let mut cx = Context::from_waker(Waker::noop());
        let result = black_box(join_handle.as_mut().poll(&mut cx));
        assert!(result.is_ready());
        state
    }

    #[library_benchmark]
    #[bench::yield_round_trip(setup = make_executor_only)]
    fn yield_round_trip(state: State) -> State {
        let mut join_handle = pin!(state.tasks.add(YieldFuture::default()));
        // First cycle: YieldFuture self-wakes; cycle reports Continue.
        assert_eq!(state.executor.execute_cycle(), CycleOutcome::Continue);
        // Second cycle: YieldFuture returns Ready; cycle reports Suspend.
        assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
        let mut cx = Context::from_waker(Waker::noop());
        let result = black_box(join_handle.as_mut().poll(&mut cx));
        assert!(result.is_ready());
        state
    }

    library_benchmark_group!(name = composite_group, benchmarks = [spawn_and_complete, yield_round_trip]);
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::{composite_group, decomposed_group};

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = decomposed_group, composite_group
);
