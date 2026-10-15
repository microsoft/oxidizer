// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Live task inspection with recording initially disabled.
//!
//! Run `cargo run -p arty --features seismograph --example arty_seismograph -- --seconds 300`, then
//! `cargo run -p seismograph_cli -- monitor` in another terminal on the same OS.
//! Select `arty-live-tasks`, press Enter, then `s` to request current task data
//! and `6` to select Runtime. Repeated captures show new worker-local children.
//! Configure Runtime tasks with `c` to see poll events collected after enable.
//! The same feature compiles `performables` hooks. Enable General events and/or
//! Arc dereferences in the monitor to record them; allocation recording still
//! requires an instrumented allocator.
//! Without arguments it requests shutdown after five seconds, then joins outstanding work.

use std::hint::black_box;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use arty::runtime::{MonitorOptions, Runtime, WorkersPolicy};
use arty::task::Builtins;
use seismograph::recorder::Configuration;
use thread_aware::Unaware;

const RECURRING_TASKS: usize = 6;

fn main() -> Result<(), ohno::AppError> {
    let duration = demo_duration()?;
    seismograph::recorder(Configuration::default());
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::at_most(2))
        .monitor(MonitorOptions::default().name("arty-live-tasks"))
        .build()?;
    let stopped = Arc::new(AtomicBool::new(false));
    let tasks: Vec<_> = (0..RECURRING_TASKS)
        .map(|index| {
            // Only an atomic stop flag and numeric input are shared between workers.
            runtime
                .scheduler()
                .spawn_anywhere(Unaware((Arc::clone(&stopped), index)), recurring)
        })
        .collect();
    let spawner = runtime.scheduler().spawn_anywhere(Unaware(Arc::clone(&stopped)), spawn_children);

    println!(
        "arty-live-tasks: PID {}, running for {} seconds",
        std::process::id(),
        duration.as_secs()
    );
    println!("Six recurring tasks plus a spawner and its child remain visible.");
    println!("Recording is OFF. In another terminal run:");
    println!("  rtk proxy cargo +1.95 run --locked -p seismograph_cli -- monitor");
    println!("Select arty-live-tasks, Enter, s to capture, then 6 for Runtime.");
    println!("Press s again to refresh. Timing stays unavailable until recording is enabled.");
    println!("For poll timelines: c, enable Runtime tasks, apply, wait, then s.");

    // This is the outside caller, not an async worker.
    std::thread::sleep(duration);
    stopped.store(true, Ordering::Release);
    for task in tasks {
        futures::executor::block_on(task)?;
    }
    futures::executor::block_on(spawner)??;
    runtime.stop()?;
    println!("Demo stopped cleanly.");
    Ok(())
}

fn demo_duration() -> io::Result<Duration> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let seconds = match args.as_slice() {
        [] => 5,
        [option, value] if option == "--seconds" => value
            .parse::<u64>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid demo duration: {error}")))?,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: arty_seismograph [--seconds 1..3600]",
            ));
        }
    };
    if !(1..=3600).contains(&seconds) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "demo duration must be 1..=3600 seconds",
        ));
    }
    Ok(Duration::from_secs(seconds))
}

async fn recurring(cx: Builtins, Unaware((stopped, index)): Unaware<(Arc<AtomicBool>, usize)>) {
    let interval = Duration::from_millis(200 + index as u64 * 50);
    while !stopped.load(Ordering::Acquire) {
        work(index as u64);
        cx.clock().delay(interval).await;
    }
}

async fn spawn_children(cx: Builtins, Unaware(stopped): Unaware<Arc<AtomicBool>>) -> Result<(), arty::task::JoinError> {
    let mut generation = 0_u64;
    while !stopped.load(Ordering::Acquire) {
        cx.scheduler()
            .spawn(async move |child| {
                work(generation);
                child.clock().delay(Duration::from_secs(2)).await;
            })
            .await?;
        generation = generation.wrapping_add(1);
    }
    Ok(())
}

fn work(seed: u64) {
    let mut value = seed;
    for index in 0..2000_u64 {
        value = value.wrapping_mul(31).wrapping_add(index);
    }
    black_box(value);
}
