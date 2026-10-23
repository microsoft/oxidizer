// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Measures clock timer registration and advancement.

use std::hint::black_box;
use std::pin::Pin;
use std::task::{Context, Waker};
use std::time::{Duration, Instant};

use criterion::{BatchSize, BenchmarkId, Criterion};
use tick::runtime::{ClockDriver, ClockGone, InactiveClock};
use tick::{Clock, Delay};

const CLOCK: &str = "tick_clock/clock";
const ADVANCE: &str = "tick_clock/advance_timers";

struct ClockState {
    clock: Clock,
    driver: ClockDriver,
}

fn clock_state() -> ClockState {
    let (clock, driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    ClockState { clock, driver }
}

struct AdvanceState {
    _clock: Clock,
    driver: ClockDriver,
    now: Instant,
    _delays: Vec<Delay>,
}

fn advance_state(timer_count: usize, ready: bool) -> AdvanceState {
    let (clock, driver) = InactiveClock::new_isolated().activate(Waker::noop().clone());
    let now = Instant::now();
    let duration = Duration::from_secs(if ready { 1 } else { 60 });
    let mut delays = Vec::with_capacity(timer_count);
    let mut context = Context::from_waker(Waker::noop());

    for _ in 0..timer_count {
        let mut delay = Delay::new(&clock, duration);
        _ = Pin::new(&mut delay).poll(&mut context);
        delays.push(delay);
    }

    AdvanceState {
        _clock: clock,
        driver,
        now,
        _delays: delays,
    }
}

#[metabench::benchmark(CLOCK_OPERATIONS, CLOCK, "clock_operations")]
#[bench::run(&mut clock_state())]
fn clock_operations(state: &mut ClockState) {
    let mut context = Context::from_waker(Waker::noop());
    let now = Instant::now();
    let mut delays = [
        Delay::new(&state.clock, Duration::from_secs(1)),
        Delay::new(&state.clock, Duration::from_secs(2)),
        Delay::new(&state.clock, Duration::from_secs(3)),
        Delay::new(&state.clock, Duration::from_secs(4)),
        Delay::new(&state.clock, Duration::from_secs(5)),
    ];

    for delay in &mut delays {
        _ = Pin::new(delay).poll(&mut context);
    }

    _ = state.driver.advance_timers(now + Duration::from_secs(2));
    for delay in &mut delays[..2] {
        _ = Pin::new(delay).poll(&mut context);
    }

    _ = state.driver.advance_timers(now + Duration::from_secs(4));
    for delay in &mut delays[2..] {
        _ = Pin::new(delay).poll(&mut context);
    }
}

#[metabench::benchmark(ADVANCE_TIMERS, ADVANCE, "advance")]
#[bench::idle(&mut advance_state(0, false))]
#[bench::pending_5(&mut advance_state(5, false))]
#[bench::ready_5(&mut advance_state(5, true))]
fn advance_timers(state: &mut AdvanceState) -> Result<Option<Instant>, ClockGone> {
    let at = state.now + Duration::from_secs(2);
    state.driver.advance_timers(black_box(at))
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut clock = criterion.benchmark_group(CLOCK);
    let mut state = clock_state();
    clock.bench_function(CLOCK_OPERATIONS.benchmark_name(), |bencher| {
        bencher.iter(|| clock_operations(black_box(&mut state)));
    });
    clock.finish();

    let mut advance = criterion.benchmark_group(ADVANCE);
    for (name, timer_count, ready) in [("idle", 0, false), ("pending_5", 5, false), ("ready_5", 5, true)] {
        advance.bench_function(BenchmarkId::new(ADVANCE_TIMERS.benchmark_name(), name), |bencher| {
            bencher.iter_batched_ref(
                || advance_state(timer_count, ready),
                |state| black_box(advance_timers(black_box(state))),
                BatchSize::SmallInput,
            );
        });
    }
    advance.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [CLOCK_OPERATIONS, ADVANCE_TIMERS],);
