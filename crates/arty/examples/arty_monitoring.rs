// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Run async tasks, blocking tasks, and allocations for a Seismograph showcase.
//!
//! Start the workload:
//!
//! ```text
//! cargo run -p arty --example arty_monitoring
//! ```
//!
//! Start the monitor in another terminal:
//!
//! ```text
//! cargo run -p seismograph_cli -- monitor
//! ```
//!
//! The workload runs until you stop it. The Anvil example check builds the
//! example, prints a skip message, and exits without running the workload.

#![forbid(unsafe_code)]

use std::env;
use std::time::Duration;

use arty::task::{Context, JoinError};

rallocator::rallocator!(backend = dynamic, default = system);

const ASYNC_STEPS: usize = 8;
const ALLOCATIONS_PER_STEP: usize = 64;
const ALLOCATION_SIZE: usize = 8 * 1024;
const BLOCKING_ALLOCATION_COUNT: usize = 32;
const BLOCKING_ALLOCATION_SIZE: usize = 16 * 1024;
const SESSION_COUNT: usize = 256;
const ANALYSIS_SAMPLE_COUNT: usize = 250_000;

const ASYNC_STEP_DELAY: Duration = Duration::from_millis(250);
const BLOCKING_TASK_DELAY: Duration = Duration::from_millis(750);
const BLOCKING_ALLOCATION_DELAY: Duration = Duration::from_millis(1_250);
const CYCLE_DELAY: Duration = Duration::from_millis(500);

struct CycleSummary {
    allocated_bytes: usize,
    active_sessions: usize,
    percentile: u64,
}

#[arty::main(workers = 2, monitor = enabled)]
async fn main(cx: Context) -> Result<(), JoinError> {
    if env::var_os("ANVIL_EXAMPLE").is_some() {
        println!("Arty monitoring is an interactive example and does not run during automated builds.");
        return Ok(());
    }

    let mut cycle = 0_u64;

    println!("Arty monitoring showcase started. Open seismograph_cli and select this process.");

    loop {
        cycle += 1;
        let cycle_task = cx.scheduler().spawn(move |cx| async move { run_cycle(cx, cycle).await });
        let summary = cycle_task.await??;

        println!(
            "cycle {cycle}: allocated {} KiB, retained {} sessions, percentile sample {}",
            summary.allocated_bytes / 1024,
            summary.active_sessions,
            summary.percentile
        );

        cx.clock().delay(CYCLE_DELAY).await;
    }
}

async fn run_cycle(cx: Context, cycle: u64) -> Result<CycleSummary, JoinError> {
    let allocation_task = cx
        .scheduler()
        .spawn(move |cx| async move { run_allocation_bursts(cx, cycle).await });
    let session_task = cx.scheduler().spawn(move |cx| async move { churn_sessions(cx, cycle).await });
    let analysis_task = cx.scheduler().spawn_blocking(move || run_blocking_analysis(cycle));
    let blocking_allocation_task = cx.scheduler().spawn_blocking(move || run_blocking_allocations(cycle));

    Ok(CycleSummary {
        allocated_bytes: allocation_task.await? + blocking_allocation_task.await?,
        active_sessions: session_task.await?,
        percentile: analysis_task.await?,
    })
}

async fn run_allocation_bursts(cx: Context, cycle: u64) -> usize {
    let mut allocated_bytes = 0;

    for step in 0..ASYNC_STEPS {
        let fill = cycle.wrapping_add(step as u64) as u8;
        let payloads: Vec<_> = (0..ALLOCATIONS_PER_STEP)
            .map(|allocation| vec![fill.wrapping_add(allocation as u8); ALLOCATION_SIZE])
            .collect();
        allocated_bytes += payloads.iter().map(Vec::len).sum::<usize>();

        cx.clock().delay(ASYNC_STEP_DELAY).await;
        std::hint::black_box(payloads);
    }

    allocated_bytes
}

async fn churn_sessions(cx: Context, cycle: u64) -> usize {
    let mut sessions: Vec<_> = (0..SESSION_COUNT).map(|session| format!("session-{cycle}-{session}")).collect();

    cx.clock().delay(ASYNC_STEP_DELAY).await;
    sessions.retain(|session| session.len().is_multiple_of(2));
    cx.clock().delay(ASYNC_STEP_DELAY).await;

    sessions.len()
}

fn run_blocking_analysis(cycle: u64) -> u64 {
    let mut samples: Vec<_> = (0..ANALYSIS_SAMPLE_COUNT as u64)
        .map(|sample| sample.wrapping_mul(31).wrapping_add(cycle) % 10_000)
        .collect();
    samples.sort_unstable();

    std::thread::sleep(BLOCKING_TASK_DELAY);

    samples[samples.len() * 95 / 100]
}

fn run_blocking_allocations(cycle: u64) -> usize {
    let allocations: Vec<_> = (0..BLOCKING_ALLOCATION_COUNT)
        .map(|allocation| {
            let fill = cycle.wrapping_add(allocation as u64) as u8;
            vec![fill; BLOCKING_ALLOCATION_SIZE].into_boxed_slice()
        })
        .collect();
    let allocated_bytes = allocations.iter().map(|allocation| allocation.len()).sum();

    std::thread::sleep(BLOCKING_ALLOCATION_DELAY);
    std::hint::black_box(allocations);

    allocated_bytes
}
