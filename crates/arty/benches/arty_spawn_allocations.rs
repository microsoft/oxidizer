// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Counts process-wide allocations for warmed async spawning.
//!
//! Run each case in a separate process. Select `local` or `remote`, followed by
//! `idle` or `disabled`. To exclude monitoring dependencies, build this source
//! in a separate package with Arty as a normal, runtime-only dependency.
//! Arty's own dev-dependencies can enable monitoring through feature unification.
//! Runtime setup and warm-up are excluded.
//! Unrelated process threads can contribute allocations. The local case measures
//! 4096 sequential children in one parent. The remote case measures 64 batches
//! of 64 tasks on four workers, including dispatch and joins.

use std::alloc::System;
use std::hint::black_box;

use alloc_tracker::{Allocator, Session};
use arty::runtime::{MonitorPolicy, Runtime, WorkersPolicy};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() -> Result<(), &'static str> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let [workload, mode] = arguments.as_slice() else {
        return Err("pass a workload (local or remote) and a monitoring mode (idle or disabled)");
    };
    let local = match workload.as_str() {
        "local" => true,
        "remote" => false,
        _ => return Err("workload must be local or remote"),
    };
    let monitor = match mode.as_str() {
        "idle" => MonitorPolicy::enabled(),
        "disabled" => MonitorPolicy::disabled(),
        _ => return Err("monitoring mode must be idle or disabled"),
    };
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(if local { 1 } else { 4 }))
        .monitor(monitor)
        .build()
        .expect("benchmark requires the configured worker count");
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("spawn");
    let mut handles = Vec::with_capacity(64);
    for measured in [false, true] {
        let measurement = measured.then(|| operation.measure_process().iterations(4096));
        if local {
            let parent = runtime.scheduler().spawn_anywhere((), |cx, ()| async move {
                for _ in 0..4096 {
                    cx.scheduler()
                        .spawn(async |_| black_box(()))
                        .await
                        .expect("benchmark child succeeds");
                }
            });
            futures::executor::block_on(parent).expect("benchmark parent succeeds");
        } else {
            for _ in 0..64 {
                handles.clear();
                for _ in 0..64 {
                    handles.push(runtime.scheduler().spawn_anywhere((), |_, ()| async { black_box(()) }));
                }
                for task in &mut handles {
                    futures::executor::block_on(task).expect("benchmark remote task succeeds");
                }
            }
        }
        drop(measurement);
    }
    let report = session.to_report();
    let (_, operation) = report
        .operations()
        .find(|(name, _)| *name == "spawn")
        .expect("the measured operation exists");
    println!(
        "{workload},{mode},{},{},{}",
        operation.total_iterations(),
        operation.total_allocations_count(),
        operation.total_bytes_allocated()
    );
    runtime.stop().expect("benchmark runtime stops");
    Ok(())
}
