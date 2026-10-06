// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker clocks advance timers and remain usable after relocation.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::time::Duration;

use arty::runtime::Runtime;
use arty::task::Builtins;
use testing_aids::execute_or_terminate_process;
use tick::FutureExt;

fn workers() -> usize {
    #[cfg(miri)]
    {
        2
    }
    #[cfg(not(miri))]
    {
        many_cpus::SystemHardware::current().processors().len()
    }
}

#[test]
fn many_timers_with_relocation_ensure_advanced() {
    execute_or_terminate_process(|| {
        let builder = Runtime::builder();
        // Two workers preserve cross-worker timer relocation under the interpreter.
        #[cfg(miri)]
        let builder = builder.cpu_policy(arty::runtime::CpuPolicy::exactly(workers()));
        let runtime = builder.build().unwrap();
        let count = workers();
        runtime
            .scheduler()
            .block_on(async move |builtins: Builtins| {
                let scheduler = builtins.scheduler();
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
    let builder = Runtime::builder();
    #[cfg(miri)]
    let builder = builder.cpu_policy(arty::runtime::CpuPolicy::exactly(workers()));
    let runtime = builder.build().unwrap();

    let count = workers();
    let handles: Vec<_> = (0..count)
        .map(|_| {
            runtime.scheduler().spawn_anywhere(
                (),
                |builtins, ()| async move { builtins.clock().delay(Duration::from_millis(1)).await },
            )
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
            .scheduler()
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
