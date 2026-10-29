// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures primary event dispatch and secondary event transfer.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(clippy::unwrap_used, reason = "benchmark fixture failures provide sufficient context")]

use std::collections::{HashMap, VecDeque};
use std::hint::black_box;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use performables::arc::Arc;
use performables::sync::mutex::Mutex;

const BATCH_SIZES: [usize; 6] = [1, 4, 16, 64, 256, 1_024];
const COMPLETION_BUDGET: usize = 256;
const BUDGETS: [usize; 3] = [64, 256, 1_024];
const BUDGET_GROUP: &str = "arty_mio/event_dispatch/budget";
const PRIMARY_GROUP: &str = "arty_mio/event_dispatch/primary";
const READINESS_GROUP: &str = "arty_mio/event_dispatch/readiness";
const SECONDARY_GROUP: &str = "arty_mio/event_dispatch/secondary";

#[derive(Clone, Copy, Debug, Default)]
struct Metrics {
    events: usize,
    owner_wakes: usize,
    driver_cycles: usize,
    registration_locks: usize,
    shared_locks: usize,
    allocations: usize,
    pauses: usize,
    resumes: usize,
}

struct Dispatcher {
    registrations: Mutex<HashMap<usize, Arc<AtomicUsize>>>,
}

impl Dispatcher {
    fn new(count: usize) -> Self {
        let registrations = (0..count).map(|token| (token, Arc::new(AtomicUsize::new(0)))).collect();
        Self {
            registrations: Mutex::new(registrations),
        }
    }

    fn dispatch_one(&self, token: usize) {
        let registration = self.registrations.lock().get(&token).cloned();
        if let Some(registration) = registration {
            registration.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn dispatch_batch(&self, tokens: &[usize], resolved: &mut Vec<Option<Arc<AtomicUsize>>>) {
        resolved.clear();
        let registrations = self.registrations.lock();
        resolved.extend(tokens.iter().map(|token| registrations.get(token).cloned()));
        drop(registrations);
        for registration in resolved.iter().flatten() {
            registration.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct PrimaryState {
    pending: VecDeque<usize>,
    dispatcher: Dispatcher,
    batch: Vec<usize>,
    resolved: Vec<Option<Arc<AtomicUsize>>>,
}

impl PrimaryState {
    fn new(count: usize) -> Self {
        Self {
            pending: (0..count).collect(),
            dispatcher: Dispatcher::new(count),
            batch: Vec::with_capacity(COMPLETION_BUDGET),
            resolved: Vec::with_capacity(COMPLETION_BUDGET),
        }
    }
}

fn primary_per_event(state: &mut PrimaryState) -> Metrics {
    let total = state.pending.len();
    let mut metrics = Metrics::default();
    while !state.pending.is_empty() {
        metrics.driver_cycles += 1;
        let count = COMPLETION_BUDGET.min(state.pending.len());
        for token in state.pending.drain(..count) {
            state.dispatcher.dispatch_one(token);
            metrics.registration_locks += 1;
        }
        if !state.pending.is_empty() {
            metrics.owner_wakes += 1;
        }
    }
    metrics.events = total;
    metrics
}

fn primary_batched(state: &mut PrimaryState) -> Metrics {
    primary_batched_with_budget(state, COMPLETION_BUDGET)
}

fn primary_batched_with_budget(state: &mut PrimaryState, budget: usize) -> Metrics {
    let total = state.pending.len();
    let mut metrics = Metrics::default();
    while !state.pending.is_empty() {
        metrics.driver_cycles += 1;
        let count = budget.min(state.pending.len());
        state.batch.clear();
        state.batch.extend(state.pending.drain(..count));
        state.dispatcher.dispatch_batch(&state.batch, &mut state.resolved);
        metrics.registration_locks += 1;
        if !state.pending.is_empty() {
            metrics.owner_wakes += 1;
        }
    }
    metrics.events = total;
    metrics
}

struct SecondaryQueue {
    events: VecDeque<usize>,
    paused: bool,
}

struct SecondaryState {
    queue: Mutex<SecondaryQueue>,
    dispatcher: Dispatcher,
    events: Vec<usize>,
    resolved: Vec<Option<Arc<AtomicUsize>>>,
}

impl SecondaryState {
    fn new(count: usize) -> Self {
        Self {
            queue: Mutex::new(SecondaryQueue {
                events: (0..count).collect(),
                paused: count != 0,
            }),
            dispatcher: Dispatcher::new(count),
            events: Vec::with_capacity(COMPLETION_BUDGET),
            resolved: Vec::with_capacity(COMPLETION_BUDGET),
        }
    }
}

fn secondary_allocating(state: &SecondaryState) -> Metrics {
    let total = state.queue.lock().events.len();
    let mut metrics = Metrics {
        owner_wakes: usize::from(total != 0),
        pauses: usize::from(total != 0),
        ..Metrics::default()
    };
    loop {
        metrics.driver_cycles += 1;
        metrics.shared_locks += 1;
        let (events, resume) = {
            let mut queue = state.queue.lock();
            let count = COMPLETION_BUDGET.min(queue.events.len());
            let events: Vec<_> = queue.events.drain(..count).collect();
            let resume = queue.events.is_empty();
            if resume {
                queue.paused = false;
            }
            (events, resume)
        };
        metrics.allocations += 1;
        for token in events {
            state.dispatcher.dispatch_one(token);
            metrics.registration_locks += 1;
        }
        metrics.shared_locks += 1;
        let has_events = !state.queue.lock().events.is_empty();
        if resume {
            metrics.resumes += 1;
        }
        if !has_events {
            break;
        }
        metrics.owner_wakes += 1;
    }
    metrics.events = total;
    metrics
}

fn secondary_reused_batch(state: &mut SecondaryState) -> Metrics {
    secondary_reused_batch_with_budget(state, COMPLETION_BUDGET)
}

fn secondary_reused_batch_with_budget(state: &mut SecondaryState, budget: usize) -> Metrics {
    let total = state.queue.lock().events.len();
    let mut metrics = Metrics {
        owner_wakes: usize::from(total != 0),
        pauses: usize::from(total != 0),
        ..Metrics::default()
    };
    loop {
        metrics.driver_cycles += 1;
        metrics.shared_locks += 1;
        state.events.clear();
        let (resume, has_events) = {
            let mut queue = state.queue.lock();
            let count = budget.min(queue.events.len());
            state.events.extend(queue.events.drain(..count));
            let has_events = !queue.events.is_empty();
            let resume = !has_events;
            if resume {
                queue.paused = false;
            }
            (resume, has_events)
        };
        state.dispatcher.dispatch_batch(&state.events, &mut state.resolved);
        metrics.registration_locks += 1;
        if resume {
            metrics.resumes += 1;
        }
        if !has_events {
            break;
        }
        metrics.owner_wakes += 1;
    }
    metrics.events = total;
    metrics
}

struct StdReadiness {
    inner: StdMutex<ReadinessInner>,
}

impl StdReadiness {
    fn new() -> Self {
        Self {
            inner: StdMutex::new(ReadinessInner { ready: true }),
        }
    }
}

struct PerformablesReadiness {
    inner: Mutex<ReadinessInner>,
}

impl PerformablesReadiness {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ReadinessInner { ready: true }),
        }
    }
}

struct ReadinessInner {
    ready: bool,
}

#[metabench::benchmark(READINESS_STD, READINESS_GROUP, "std")]
#[bench::state(Box::new(StdReadiness::new()))]
fn readiness_std<S: std::borrow::Borrow<StdReadiness>>(owned_input: S) -> (bool, S) {
    let result = std::mem::take(&mut owned_input.borrow().inner.lock().unwrap().ready);
    (result, owned_input)
}

#[metabench::benchmark(READINESS_PERFORMABLES, READINESS_GROUP, "performables")]
#[bench::state(Box::new(PerformablesReadiness::new()))]
fn readiness_performables<S: std::borrow::Borrow<PerformablesReadiness>>(owned_input: S) -> (bool, S) {
    let result = std::mem::take(&mut owned_input.borrow().inner.lock().ready);
    (result, owned_input)
}

#[metabench::benchmark(PRIMARY_PER_EVENT_1024, PRIMARY_GROUP, "per_event_1024")]
#[bench::events(Box::new(PrimaryState::new(1_024)))]
fn primary_per_event_1024<S: std::borrow::BorrowMut<PrimaryState>>(mut owned_input: S) -> (Metrics, S) {
    let result = primary_per_event(owned_input.borrow_mut());
    (result, owned_input)
}

#[metabench::benchmark(PRIMARY_BATCHED_1024, PRIMARY_GROUP, "batched_1024")]
#[bench::events(Box::new(PrimaryState::new(1_024)))]
fn primary_batched_1024<S: std::borrow::BorrowMut<PrimaryState>>(mut owned_input: S) -> (Metrics, S) {
    let result = primary_batched(owned_input.borrow_mut());
    (result, owned_input)
}

#[metabench::benchmark(SECONDARY_ALLOCATING_1024, SECONDARY_GROUP, "allocating_1024")]
#[bench::events(Box::new(SecondaryState::new(1_024)))]
fn secondary_allocating_1024<S: std::borrow::Borrow<SecondaryState>>(owned_input: S) -> (Metrics, S) {
    let result = secondary_allocating(owned_input.borrow());
    (result, owned_input)
}

#[metabench::benchmark(SECONDARY_REUSED_BATCH_1024, SECONDARY_GROUP, "reused_batch_1024")]
#[bench::events(Box::new(SecondaryState::new(1_024)))]
fn secondary_reused_batch_1024<S: std::borrow::BorrowMut<SecondaryState>>(mut owned_input: S) -> (Metrics, S) {
    let result = secondary_reused_batch(owned_input.borrow_mut());
    (result, owned_input)
}

fn print_operation_counts() {
    eprintln!("size,path,events,owner_wakes,driver_cycles,registration_locks,shared_locks,allocations,pauses,resumes");
    for size in BATCH_SIZES {
        for (path, metrics) in [
            ("primary_per_event", primary_per_event(&mut PrimaryState::new(size))),
            ("primary_batched", primary_batched(&mut PrimaryState::new(size))),
            ("secondary_allocating", secondary_allocating(&SecondaryState::new(size))),
            ("secondary_reused_batch", secondary_reused_batch(&mut SecondaryState::new(size))),
        ] {
            eprintln!(
                "{size},{path},{},{},{},{},{},{},{},{}",
                metrics.events,
                metrics.owner_wakes,
                metrics.driver_cycles,
                metrics.registration_locks,
                metrics.shared_locks,
                metrics.allocations,
                metrics.pauses,
                metrics.resumes
            );
        }
    }
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    print_operation_counts();

    let mut primary = criterion.benchmark_group(PRIMARY_GROUP);
    for size in BATCH_SIZES {
        primary.throughput(Throughput::Elements(size as u64));
        primary.bench_with_input(BenchmarkId::new("per_event", size), &size, |bencher, size| {
            bencher.iter_batched(
                || PrimaryState::new(*size),
                |mut state| black_box(primary_per_event(black_box(&mut state))),
                BatchSize::SmallInput,
            );
        });
        primary.bench_with_input(BenchmarkId::new("batched", size), &size, |bencher, size| {
            bencher.iter_batched(
                || PrimaryState::new(*size),
                |mut state| black_box(primary_batched(black_box(&mut state))),
                BatchSize::SmallInput,
            );
        });
    }
    primary.finish();

    let mut secondary = criterion.benchmark_group(SECONDARY_GROUP);
    for size in BATCH_SIZES {
        secondary.throughput(Throughput::Elements(size as u64));
        secondary.bench_with_input(BenchmarkId::new("allocating", size), &size, |bencher, size| {
            bencher.iter_batched(
                || SecondaryState::new(*size),
                |state| black_box(secondary_allocating(black_box(&state))),
                BatchSize::SmallInput,
            );
        });
        secondary.bench_with_input(BenchmarkId::new("reused_batch", size), &size, |bencher, size| {
            bencher.iter_batched(
                || SecondaryState::new(*size),
                |mut state| black_box(secondary_reused_batch(black_box(&mut state))),
                BatchSize::SmallInput,
            );
        });
    }
    secondary.finish();

    let mut readiness = criterion.benchmark_group(READINESS_GROUP);
    readiness.bench_function(READINESS_STD.benchmark_name(), |bencher| {
        bencher.iter_batched(
            StdReadiness::new,
            |state| black_box(readiness_std(black_box(&state)).0),
            BatchSize::SmallInput,
        );
    });
    readiness.bench_function(READINESS_PERFORMABLES.benchmark_name(), |bencher| {
        bencher.iter_batched(
            PerformablesReadiness::new,
            |state| black_box(readiness_performables(black_box(&state)).0),
            BatchSize::SmallInput,
        );
    });
    readiness.finish();

    let mut budgets = criterion.benchmark_group(BUDGET_GROUP);
    budgets.throughput(Throughput::Elements(1_024));
    for budget in BUDGETS {
        budgets.bench_with_input(BenchmarkId::new("primary", budget), &budget, |bencher, budget| {
            bencher.iter_batched(
                || PrimaryState::new(1_024),
                |mut state| black_box(primary_batched_with_budget(black_box(&mut state), *budget)),
                BatchSize::SmallInput,
            );
        });
        budgets.bench_with_input(BenchmarkId::new("secondary", budget), &budget, |bencher, budget| {
            bencher.iter_batched(
                || SecondaryState::new(1_024),
                |mut state| black_box(secondary_reused_batch_with_budget(black_box(&mut state), *budget)),
                BatchSize::SmallInput,
            );
        });
    }
    budgets.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        PRIMARY_PER_EVENT_1024,
        PRIMARY_BATCHED_1024,
        SECONDARY_ALLOCATING_1024,
        SECONDARY_REUSED_BATCH_1024,
        READINESS_STD,
        READINESS_PERFORMABLES
    ],
    allocator = std::alloc::System,
);
