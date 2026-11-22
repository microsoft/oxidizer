// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A macro entry point, a worker-driven delay, and a simple message.

use std::mem::DropGuard;
use std::thread;
use std::time::Duration;

use arty::runtime::hook::{HookFn, TaskSpawn, WorkerSpawn};
use arty::task::Context;

#[arty::main(workers = 2)]
async fn main(cx: Context) {
    let scheduler = cx.scheduler().clone();

    cx.control().on(HookFn::new((), |s: &mut TaskSpawn, _| {
        println!("Task spawn hook triggered, location: {}", s.location());
    }));

    cx.control().on(HookFn::new(scheduler, |_d: &mut WorkerSpawn, scheduler| {
        _ = scheduler.spawn(|cx| async move {
            let _guard = DropGuard::new(cx.clock().stopwatch(), |w| {
                println!("Dropping background task at worker {:?} after {:?}", cx.thread().id(), w.elapsed());
            });

            println!("starting background task at worker {:?}", cx.thread().id());
            loop {
                cx.clock().delay(Duration::from_millis(1000)).await;

                println!(
                    "background task running at worker {:?} on thread {:?}",
                    cx.thread().id(),
                    thread::current().id()
                );
            }
        });
    }));

    let _ = cx.scheduler().spawn(|_| async move {}).await;

    cx.clock().delay(Duration::from_millis(10000)).await;
    println!("Hello from Arty!");
}
