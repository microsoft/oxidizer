// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared wall-clock, allocation, and instruction-count executor benchmarks.
//!
//! Decomposed cases measure one operation on prepared state: executor creation,
//! warm-up, wake delivery (for consumption cases), and destruction are excluded.
//! Composite cases include task registration, polling, and join-handle cleanup,
//! but reuse a warmed executor. The large composites retain the sequential-1000
//! and burst-10000 workloads. Add `--gungraun-arg '*::one_of_32'` to isolate
//! wake consumption without measuring the large composites.
//!
//! Run with `cargo bench -p arty_executor --features test-util --bench ae_basic_operations
//! -- --gungraun` on Linux, or select `--criterion` / `--allocations` on any platform.
//! Use `--allocations --test` for one-shot allocation probes rather than full
//! Criterion timing runs for every case.
//! Spawn measurements include allocator instructions, not just scheduler work.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::cell::RefCell;
use std::future::{pending, poll_fn};
use std::hint::black_box;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use arty_executor::testing::new_guarded_executor;
use arty_executor::{CycleOutcome, Executor, JoinHandle, TaskSet};
use criterion::{BatchSize, BenchmarkId, Criterion};
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
use scopeguard::{Always, ScopeGuard};
use testing_aids::YieldFuture;

const BASIC: &str = "ae_basic_operations/basic";
const DECOMPOSED: &str = "ae_basic_operations/decomposed";
const SLOW: &str = "ae_basic_operations/slow";
const SEQUENTIAL_COUNT: usize = 1_000;
const BURST_COUNT: usize = 10_000;
const MODERATE_OCCUPANCY: usize = 32;
// Prime both sides of double-buffer reuse before measuring steady-state work.
const WARM_UP_OPERATIONS: usize = 2;
// Exceed the current 1024-entry wake queue without requiring thousands of tasks.
const OVERFLOW_WAKE_COUNT: usize = 1_025;

// Bound live executors independently of Criterion's sample size, while
// amortizing the timer overhead across several elementary operations.
const BATCH_SIZE: BatchSize = BatchSize::NumIterations(32);

// Fields drop in declaration order: all external wakers and join handles must
// disappear before the guarded executor starts its shutdown loop.
struct State {
    wakers: Vec<Waker>,
    added: Option<JoinHandle<()>>,
    handles: Vec<JoinHandle<()>>,
    tasks: TaskSet,
    executor: ScopeGuard<Executor, fn(Executor), Always>,
}

fn executor_state() -> State {
    let executor = new_guarded_executor(Waker::noop().clone());
    let tasks = executor.tasks();
    State {
        wakers: Vec::new(),
        added: None,
        handles: Vec::new(),
        tasks,
        executor,
    }
}

fn spawn_complete_once(state: &State) -> Poll<()> {
    // Stack-pin to avoid allocator noise on the measured path.
    let mut handle = pin!(state.tasks.add(async {}));
    black_box(state.executor.execute_cycle());
    handle.as_mut().poll(&mut Context::from_waker(Waker::noop()))
}

fn yield_once(state: &State) -> Poll<()> {
    // Stack-pin to avoid allocator noise on the measured path.
    let mut handle = pin!(state.tasks.add(YieldFuture::default()));
    black_box(state.executor.execute_cycle());
    black_box(state.executor.execute_cycle());
    handle.as_mut().poll(&mut Context::from_waker(Waker::noop()))
}

fn warmed_state() -> State {
    let state = executor_state();
    for _ in 0..WARM_UP_OPERATIONS {
        assert_eq!(spawn_complete_once(&state), Poll::Ready(()));
    }
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state
}

fn warmed_yield_state() -> State {
    let state = executor_state();
    for _ in 0..WARM_UP_OPERATIONS {
        assert_eq!(yield_once(&state), Poll::Ready(()));
    }
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state
}

fn ready_state(count: usize) -> State {
    let mut state = warmed_state();
    state.handles = (0..count).map(|_| state.tasks.add(async {})).collect();
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state.handles.clear();
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state.handles.extend((0..count).map(|_| state.tasks.add(async {})));
    state
}

fn completed_state() -> State {
    let state = ready_state(1);
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state
}

fn pending_state() -> State {
    let mut state = warmed_yield_state();
    state.added = Some(state.tasks.add(pending::<()>()));
    state
}

fn inactive_state() -> State {
    let state = pending_state();
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state
}

fn yield_state() -> State {
    let mut state = warmed_yield_state();
    state.added = Some(state.tasks.add(YieldFuture::default()));
    state
}

fn self_woken_state() -> State {
    let state = yield_state();
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Continue);
    state
}

fn waiting_state(count: usize) -> State {
    let mut state = warmed_yield_state();
    let slots = (0..count)
        .map(|_| {
            let slot = Rc::new(RefCell::new(None));
            let mut capture = Some(Rc::clone(&slot));
            state.handles.push(state.tasks.add(poll_fn(move |cx| {
                // Capture only on the setup poll. Later measured polls just return
                // Pending, without cloning wakers or touching a RefCell.
                if let Some(slot) = capture.take() {
                    *slot.borrow_mut() = Some(cx.waker().clone());
                }
                Poll::<()>::Pending
            })));
            slot
        })
        .collect::<Vec<_>>();
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state.wakers = slots
        .into_iter()
        .map(|slot| {
            slot.borrow_mut()
                .take()
                .expect("the setup cycle polled every newly registered task")
        })
        .collect();
    // Prime wake-processing metrics and active capacity at this occupancy.
    for waker in &state.wakers {
        waker.wake_by_ref();
    }
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    state
}

fn woken_state(occupancy: usize, wake_count: usize) -> State {
    let state = waiting_state(occupancy);
    for waker in &state.wakers[..wake_count] {
        waker.wake_by_ref();
    }
    state
}

fn fill_wake_queue(state: &State) {
    for _ in 0..OVERFLOW_WAKE_COUNT {
        state.wakers[0].wake_by_ref();
    }
}

fn overflow_wake_state() -> State {
    let mut state = waiting_state(MODERATE_OCCUPANCY);
    // Prime overflow, spurious-wake, and fallback-probe metrics, then restore
    // the same saturated-queue precondition for the measured operation.
    fill_wake_queue(&state);
    state.wakers[1].wake_by_ref();
    assert_eq!(state.executor.execute_cycle(), CycleOutcome::Suspend);
    fill_wake_queue(&state);
    // The measured wake targets a second task; the remaining tasks stay inactive.
    state.wakers.swap(0, 1);
    state
}

fn overflow_cycle_state() -> State {
    let state = overflow_wake_state();
    state.wakers[0].wake_by_ref();
    state
}

#[metabench::benchmark(BASIC_NOOP, BASIC, "noop")]
#[bench::cold(&executor_state())]
#[bench::warm(&warmed_state())]
fn basic_noop(state: &State) -> CycleOutcome {
    state.executor.execute_cycle()
}

#[metabench::benchmark(DECOMPOSED_TASK_ADD, DECOMPOSED, "task_add")]
#[bench::cold(&mut executor_state())]
#[bench::warm(&mut warmed_state())]
fn decomposed_task_add(state: &mut State) {
    state.added = Some(black_box(state.tasks.add(async {})));
}

#[metabench::benchmark(DECOMPOSED_CYCLE_READY, DECOMPOSED, "cycle_ready")]
#[bench::tasks_1(&ready_state(1))]
#[bench::tasks_32(&ready_state(MODERATE_OCCUPANCY))]
fn decomposed_cycle_ready(state: &State) -> CycleOutcome {
    state.executor.execute_cycle()
}

#[metabench::benchmark(DECOMPOSED_POLL_COMPLETED, DECOMPOSED, "poll_completed")]
#[bench::ready(&mut completed_state())]
fn decomposed_poll_completed(state: &mut State) -> Poll<()> {
    Pin::new(&mut state.handles[0]).poll(&mut Context::from_waker(Waker::noop()))
}

#[metabench::benchmark(DECOMPOSED_CYCLE_PENDING, DECOMPOSED, "cycle_pending")]
#[bench::first_poll(&pending_state())]
#[bench::inactive(&inactive_state())]
fn decomposed_cycle_pending(state: &State) -> CycleOutcome {
    state.executor.execute_cycle()
}

#[metabench::benchmark(DECOMPOSED_YIELD_CYCLE, DECOMPOSED, "yield_cycle")]
#[bench::self_wake(&yield_state())]
#[bench::completion(&self_woken_state())]
fn decomposed_yield_cycle(state: &State) -> CycleOutcome {
    state.executor.execute_cycle()
}

#[metabench::benchmark(DECOMPOSED_WAKE_BY_REF, DECOMPOSED, "wake_by_ref")]
#[bench::delivered(&waiting_state(1))]
#[bench::duplicate(&woken_state(1, 1))]
#[bench::overflow(&overflow_wake_state())]
fn decomposed_wake_by_ref(state: &State) {
    black_box(&state.wakers[0]).wake_by_ref();
}

#[metabench::benchmark(DECOMPOSED_CYCLE_WOKEN, DECOMPOSED, "cycle_woken")]
#[bench::one_of_1(&woken_state(1, 1))]
#[bench::one_of_32(&woken_state(MODERATE_OCCUPANCY, 1))]
#[bench::all_32(&woken_state(MODERATE_OCCUPANCY, MODERATE_OCCUPANCY))]
#[bench::overflow(&overflow_cycle_state())]
fn decomposed_cycle_woken(state: &State) -> CycleOutcome {
    state.executor.execute_cycle()
}

#[metabench::benchmark(BASIC_SPAWN_AND_COMPLETE_ONE, BASIC, "spawn_and_complete_one")]
#[bench::warm(&warmed_state())]
fn basic_spawn_and_complete_one(state: &State) -> Poll<()> {
    spawn_complete_once(state)
}

#[metabench::benchmark(BASIC_YIELD_ONE, BASIC, "yield_one")]
#[bench::warm(&warmed_yield_state())]
fn basic_yield_one(state: &State) -> Poll<()> {
    yield_once(state)
}

#[metabench::benchmark(SLOW_SPAWN_AND_COMPLETE_ONE_TIMES_MANY, SLOW, "spawn_and_complete_one_times_many")]
#[bench::sequential_1000(&warmed_state())]
fn slow_spawn_and_complete_one_times_many(state: &State) -> usize {
    (0..SEQUENTIAL_COUNT)
        .filter(|_| black_box(spawn_complete_once(state)).is_ready())
        .count()
}

fn complete_burst<F>(state: &mut State, future: impl Fn() -> F, cycles: usize) -> usize
where
    F: Future<Output = ()> + 'static,
{
    state.handles.clear();
    for _ in 0..BURST_COUNT {
        state.handles.push(state.tasks.add(future()));
    }
    for _ in 0..cycles {
        black_box(state.executor.execute_cycle());
    }
    let mut cx = Context::from_waker(Waker::noop());
    state
        .handles
        .iter_mut()
        .map(|handle| usize::from(black_box(Pin::new(handle).poll(&mut cx)).is_ready()))
        .sum()
}

fn ready_burst(state: &mut State) -> usize {
    complete_burst(state, || async {}, 1)
}

fn yield_burst(state: &mut State) -> usize {
    complete_burst(state, YieldFuture::default, 2)
}

fn warmed_burst_state() -> State {
    let mut state = executor_state();
    state.handles = Vec::with_capacity(BURST_COUNT);
    for _ in 0..WARM_UP_OPERATIONS {
        assert_eq!(ready_burst(&mut state), BURST_COUNT);
    }
    state
}

fn warmed_yield_burst_state() -> State {
    let mut state = executor_state();
    state.handles = Vec::with_capacity(BURST_COUNT);
    for _ in 0..WARM_UP_OPERATIONS {
        assert_eq!(yield_burst(&mut state), BURST_COUNT);
    }
    state
}

#[metabench::benchmark(SLOW_SPAWN_AND_COMPLETE_10K, SLOW, "spawn_and_complete_10k")]
#[bench::burst_10000(&mut warmed_burst_state())]
fn slow_spawn_and_complete_10k(state: &mut State) -> usize {
    ready_burst(state)
}

#[metabench::benchmark(SLOW_YIELD_10K, SLOW, "yield_10k")]
#[bench::burst_10000(&mut warmed_yield_burst_state())]
fn slow_yield_10k(state: &mut State) -> usize {
    yield_burst(state)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut basic = criterion.benchmark_group(BASIC);
    macro_rules! repeated {
        ($group:ident, $identity:ident, $case:literal, $setup:expr, $body:ident) => {
            $group.bench_function(BenchmarkId::new($identity.benchmark_name(), $case), |bencher| {
                let mut state = $setup;
                bencher.iter(|| $body(black_box(&mut state)));
            });
        };
    }
    basic.bench_function(BenchmarkId::new(BASIC_NOOP.benchmark_name(), "cold"), |bencher| {
        bencher.iter_batched_ref(executor_state, |state| basic_noop(black_box(state)), BATCH_SIZE);
    });
    repeated!(basic, BASIC_NOOP, "warm", warmed_state(), basic_noop);
    repeated!(
        basic,
        BASIC_SPAWN_AND_COMPLETE_ONE,
        "warm",
        warmed_state(),
        basic_spawn_and_complete_one
    );
    repeated!(basic, BASIC_YIELD_ONE, "warm", warmed_yield_state(), basic_yield_one);
    basic.finish();

    let mut decomposed = criterion.benchmark_group(DECOMPOSED);
    macro_rules! prepared {
        ($identity:ident, $case:literal, $setup:expr, $body:ident) => {
            decomposed.bench_function(BenchmarkId::new($identity.benchmark_name(), $case), |bencher| {
                bencher.iter_batched_ref(|| $setup, |state| $body(black_box(state)), BATCH_SIZE);
            });
        };
    }
    prepared!(DECOMPOSED_TASK_ADD, "cold", executor_state(), decomposed_task_add);
    prepared!(DECOMPOSED_TASK_ADD, "warm", warmed_state(), decomposed_task_add);
    prepared!(DECOMPOSED_CYCLE_READY, "tasks_1", ready_state(1), decomposed_cycle_ready);
    prepared!(
        DECOMPOSED_CYCLE_READY,
        "tasks_32",
        ready_state(MODERATE_OCCUPANCY),
        decomposed_cycle_ready
    );
    prepared!(DECOMPOSED_POLL_COMPLETED, "ready", completed_state(), decomposed_poll_completed);
    prepared!(DECOMPOSED_CYCLE_PENDING, "first_poll", pending_state(), decomposed_cycle_pending);
    prepared!(DECOMPOSED_CYCLE_PENDING, "inactive", inactive_state(), decomposed_cycle_pending);
    prepared!(DECOMPOSED_YIELD_CYCLE, "self_wake", yield_state(), decomposed_yield_cycle);
    prepared!(DECOMPOSED_YIELD_CYCLE, "completion", self_woken_state(), decomposed_yield_cycle);
    prepared!(DECOMPOSED_WAKE_BY_REF, "delivered", waiting_state(1), decomposed_wake_by_ref);
    prepared!(DECOMPOSED_WAKE_BY_REF, "duplicate", woken_state(1, 1), decomposed_wake_by_ref);
    prepared!(DECOMPOSED_WAKE_BY_REF, "overflow", overflow_wake_state(), decomposed_wake_by_ref);
    prepared!(DECOMPOSED_CYCLE_WOKEN, "one_of_1", woken_state(1, 1), decomposed_cycle_woken);
    prepared!(
        DECOMPOSED_CYCLE_WOKEN,
        "one_of_32",
        woken_state(MODERATE_OCCUPANCY, 1),
        decomposed_cycle_woken
    );
    prepared!(
        DECOMPOSED_CYCLE_WOKEN,
        "all_32",
        woken_state(MODERATE_OCCUPANCY, MODERATE_OCCUPANCY),
        decomposed_cycle_woken
    );
    prepared!(DECOMPOSED_CYCLE_WOKEN, "overflow", overflow_cycle_state(), decomposed_cycle_woken);
    decomposed.finish();

    let mut slow = criterion.benchmark_group(SLOW);
    repeated!(
        slow,
        SLOW_SPAWN_AND_COMPLETE_ONE_TIMES_MANY,
        "sequential_1000",
        warmed_state(),
        slow_spawn_and_complete_one_times_many
    );
    repeated!(
        slow,
        SLOW_SPAWN_AND_COMPLETE_10K,
        "burst_10000",
        warmed_burst_state(),
        slow_spawn_and_complete_10k
    );
    repeated!(slow, SLOW_YIELD_10K, "burst_10000", warmed_yield_burst_state(), slow_yield_10k);
    slow.finish();
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
        BASIC_NOOP,
        DECOMPOSED_TASK_ADD,
        DECOMPOSED_CYCLE_READY,
        DECOMPOSED_POLL_COMPLETED,
        DECOMPOSED_CYCLE_PENDING,
        DECOMPOSED_YIELD_CYCLE,
        DECOMPOSED_WAKE_BY_REF,
        DECOMPOSED_CYCLE_WOKEN,
        BASIC_SPAWN_AND_COMPLETE_ONE,
        BASIC_YIELD_ONE,
        SLOW_SPAWN_AND_COMPLETE_ONE_TIMES_MANY,
        SLOW_SPAWN_AND_COMPLETE_10K,
        SLOW_YIELD_10K,
    ],
);
