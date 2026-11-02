// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public API benchmarks for the core `arty_executor` lifecycle.
//!
//! Run with `cargo bench -p arty_executor --bench ae_basic_operations -- --gungraun`
//! on Linux, or select `--criterion` / `--allocations` on any platform.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::borrow;
use std::cell::RefCell;
use std::future::{pending, poll_fn};
use std::hint::black_box;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use arty_executor::{
    CycleOutcome, Executor, ShutdownCause, TaskCompletion, TaskOptions, TaskPanicPayload, TaskSet, discard_task_panic_payload,
};
use criterion::{BatchSize, BenchmarkId, Criterion};
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};

const CYCLE: &str = "ae_basic_operations/execute_cycle";
const TASK: &str = "ae_basic_operations/task";
const WAKER: &str = "ae_basic_operations/waker";
const SELF_WAKING_POLL_COUNT: usize = 4;

/// Avoids result receiver storage in measured operations.
///
/// Successful results and cancellation need no action. Panic payloads still
/// require explicit disposal.
#[derive(Debug)]
struct DiscardCompletion;

impl TaskCompletion<()> for DiscardCompletion {
    fn completed(self, _result: ()) {}

    fn panicked(self, panic: TaskPanicPayload, _cause: Option<ShutdownCause>) {
        discard_task_panic_payload(panic);
    }

    fn cancelled(self, _cause: ShutdownCause) {}
}

struct ExecutorState {
    tasks: TaskSet,
    _executor: Executor,
}

fn executor_state() -> ExecutorState {
    let executor = Executor::builder().build();
    let tasks = executor.tasks();
    ExecutorState {
        tasks,
        _executor: executor,
    }
}

struct CycleState {
    executor: Executor,
}

fn cycle_state(pending_task: bool) -> CycleState {
    let executor = Executor::builder().build();
    if pending_task {
        executor.tasks().add(pending::<()>(), TaskOptions::default(), DiscardCompletion);
    }
    CycleState { executor }
}

struct CloneWakerState {
    _captured: Rc<RefCell<Option<Waker>>>,
    executor: Executor,
}

struct SelfWakingFuture {
    remaining_polls: usize,
    clone_waker: bool,
}

impl Future for SelfWakingFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.remaining_polls -= 1;
        if self.remaining_polls == 0 {
            return Poll::Ready(());
        }

        if self.clone_waker {
            cx.waker().clone().wake_by_ref();
        } else {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

struct SelfWakingState {
    executor: Executor,
}

fn self_waking_state(clone_waker: bool) -> SelfWakingState {
    let executor = Executor::builder().build();
    executor.tasks().add(
        SelfWakingFuture {
            remaining_polls: SELF_WAKING_POLL_COUNT,
            clone_waker,
        },
        TaskOptions::default(),
        DiscardCompletion,
    );
    SelfWakingState { executor }
}

fn clone_waker_state() -> CloneWakerState {
    let executor = Executor::builder().build();
    let captured = Rc::new(RefCell::new(None));
    executor.tasks().add(
        {
            let captured = Rc::clone(&captured);
            poll_fn(move |cx| {
                *captured.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending::<()>
            })
        },
        TaskOptions::default(),
        DiscardCompletion,
    );
    CloneWakerState {
        _captured: captured,
        executor,
    }
}

fn retained_waker_after_executor_drop() -> Waker {
    let mut executor = Executor::builder().build();
    let tasks = executor.tasks();
    let captured = Rc::new(RefCell::new(None));
    tasks.add(
        {
            let captured = Rc::clone(&captured);
            poll_fn(move |cx| {
                *captured.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending::<()>
            })
        },
        TaskOptions::default(),
        DiscardCompletion,
    );

    assert!(matches!(executor.execute_cycle(), CycleOutcome::Suspend));
    let retained = captured.borrow_mut().take().expect("the task captured its waker");
    drop(executor);
    drop(tasks);
    retained
}

#[metabench::benchmark(EXECUTE_CYCLE, CYCLE, "execute_cycle")]
#[bench::idle(Box::new(cycle_state(false)))]
#[bench::pending_task(Box::new(cycle_state(true)))]
fn execute_cycle<S: borrow::BorrowMut<CycleState>>(mut state: S) -> (CycleOutcome, S) {
    let outcome = state.borrow_mut().executor.execute_cycle();
    (outcome, state)
}

#[metabench::benchmark(ADD_TASK, TASK, "add_task")]
#[bench::ready(Box::new(executor_state()))]
fn add_task<S: borrow::Borrow<ExecutorState>>(state: S) -> S {
    black_box(&state.borrow().tasks).add(async {}, TaskOptions::default(), DiscardCompletion);
    state
}

#[metabench::benchmark(SELF_WAKING_TASK, TASK, "self_waking_task")]
#[bench::borrowed_waker_polls_4(Box::new(self_waking_state(false)))]
#[bench::cloned_waker_polls_4(Box::new(self_waking_state(true)))]
fn self_waking_task<S: borrow::BorrowMut<SelfWakingState>>(mut state: S) -> (CycleOutcome, S) {
    let mut outcome = CycleOutcome::Suspend;
    for _ in 0..SELF_WAKING_POLL_COUNT {
        outcome = state.borrow_mut().executor.execute_cycle();
    }
    (outcome, state)
}

#[metabench::benchmark(CLONE_WAKER_DURING_CYCLE, WAKER, "clone_during_cycle")]
#[bench::pending_task(Box::new(clone_waker_state()))]
fn clone_waker_during_cycle<S: borrow::BorrowMut<CloneWakerState>>(mut state: S) -> (CycleOutcome, S) {
    let outcome = state.borrow_mut().executor.execute_cycle();
    (outcome, state)
}

#[metabench::benchmark(RETAINED_WAKER_AFTER_DROP, WAKER, "retained_after_executor_drop")]
#[bench::wake_by_ref(Box::new(retained_waker_after_executor_drop()))]
fn retained_waker_after_drop<W: borrow::Borrow<Waker>>(waker: W) -> W {
    black_box(waker.borrow()).wake_by_ref();
    waker
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut cycle = criterion.benchmark_group(CYCLE);
    cycle.bench_function(BenchmarkId::new(EXECUTE_CYCLE.benchmark_name(), "pending_task"), |bencher| {
        bencher.iter_batched_ref(
            || cycle_state(true),
            |state| execute_cycle(black_box(state)).0,
            BatchSize::PerIteration,
        );
    });
    {
        let mut idle = cycle_state(false);
        cycle.bench_function(BenchmarkId::new(EXECUTE_CYCLE.benchmark_name(), "idle"), |bencher| {
            bencher.iter(|| execute_cycle(black_box(&mut idle)).0);
        });
    }
    cycle.finish();

    let mut task = criterion.benchmark_group(TASK);
    task.bench_function(BenchmarkId::new(ADD_TASK.benchmark_name(), "ready"), |bencher| {
        bencher.iter_batched_ref(
            executor_state,
            |state| {
                _ = add_task(black_box(&*state));
            },
            BatchSize::PerIteration,
        );
    });
    for (case, clone_waker) in [("borrowed_waker_polls_4", false), ("cloned_waker_polls_4", true)] {
        task.bench_function(BenchmarkId::new(SELF_WAKING_TASK.benchmark_name(), case), |bencher| {
            bencher.iter_batched_ref(
                || self_waking_state(clone_waker),
                |state| self_waking_task(black_box(state)).0,
                BatchSize::PerIteration,
            );
        });
    }
    task.finish();

    let mut waker = criterion.benchmark_group(WAKER);
    waker.bench_function(
        BenchmarkId::new(CLONE_WAKER_DURING_CYCLE.benchmark_name(), "pending_task"),
        |bencher| {
            bencher.iter_batched_ref(
                clone_waker_state,
                |state| clone_waker_during_cycle(black_box(state)).0,
                BatchSize::PerIteration,
            );
        },
    );
    let retained = retained_waker_after_executor_drop();
    waker.bench_function(
        BenchmarkId::new(RETAINED_WAKER_AFTER_DROP.benchmark_name(), "wake_by_ref"),
        |bencher| {
            bencher.iter(|| retained_waker_after_drop(black_box(&retained)));
        },
    );
    waker.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    gungraun = {
        config = LibraryBenchmarkConfig::default().tool(
            Callgrind::default()
                .args(["--branch-sim=yes"])
                .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
        );
    },
    benchmarks = [
        EXECUTE_CYCLE,
        ADD_TASK,
        SELF_WAKING_TASK,
        CLONE_WAKER_DURING_CYCLE,
        RETAINED_WAKER_AFTER_DROP,
    ],
);
