// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker clocks advance timers and remain usable after relocation.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::time::Duration;

use arty::runtime::{Builtins, Runtime};
use testing_aids::execute_or_terminate_process;
use tick::FutureExt;

fn workers() -> usize {
    #[cfg(miri)]
    {
        6
    }
    #[cfg(not(miri))]
    {
        many_cpus::SystemHardware::current().processors().len()
    }
}

#[test]
fn many_timers_with_relocation_ensure_advanced() {
    execute_or_terminate_process(|| {
        let runtime = Runtime::new().unwrap();
        let scheduler = runtime.task_scheduler();
        let count = workers();
        runtime
            .block_on(async move |builtins: Builtins| {
                // ensure clock works across all threads
                let handles: Vec<_> = (0..count)
                    .map(|_| {
                        scheduler.spawn_anywhere(builtins.clone(), move |b| {
                            let builtins = b;
                            async move {
                                println!("delay(pending) - clock: {:?}, thread: {:?}", builtins.clock(), builtins.thread());

                                let watch = builtins.clock().stopwatch();
                                builtins.clock().delay(std::time::Duration::from_millis(50)).await;

                                println!("delay(done) - clock: {:?}, thread: {:?}", builtins.clock(), builtins.thread());

                                assert!(watch.elapsed().as_millis() >= 50);
                            }
                        })
                    })
                    .collect();

                for handle in handles {
                    handle.timeout(builtins.clock(), Duration::from_secs(10)).await.unwrap().unwrap();
                }
            })
            .unwrap();
    });
}

#[test]
fn many_timers_ensure_advanced() {
    let runtime = Runtime::new().unwrap();

    let count = workers();
    let handles: Vec<_> = (0..count)
        .map(|_| {
            runtime
                .task_scheduler()
                .spawn(async |builtins| builtins.clock().delay(Duration::from_millis(1)).await)
        })
        .collect();

    for handle in handles {
        handle.wait().unwrap();
    }
}

#[test]
fn timer_with_relocated_builtins() {
    execute_or_terminate_process(|| {
        let runtime = Runtime::new().unwrap();
        runtime
            .block_on(async |builtins: Builtins| {
                builtins
                    .scheduler()
                    .spawn_anywhere(builtins.clone(), |c| c.clock().delay(Duration::from_millis(10)))
                    .await
                    .unwrap();
            })
            .unwrap();
    });
}
