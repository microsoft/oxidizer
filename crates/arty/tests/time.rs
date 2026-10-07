// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Worker clocks advance timers and remain usable after relocation.

#![cfg(all(feature = "rt", feature = "macros"))]

testing_aids::init_tracing!();

use std::time::Duration;

use arty::task::Builtins;
use tick::FutureExt;

const TIMER_COUNT: usize = 8;

#[arty::test]
async fn many_timers_with_relocation_ensure_advanced(builtins: Builtins) {
    let scheduler = builtins.scheduler();
    // Ensure each relocated task keeps access to the runtime clock.
    let handles: Vec<_> = (0..TIMER_COUNT)
        .map(|_| {
            scheduler.spawn_anywhere(builtins.clone(), move |builtins| async move {
                println!("delay(pending) - clock: {:?}, thread: {:?}", builtins.clock(), builtins.thread());

                let watch = builtins.clock().stopwatch();
                builtins.clock().delay(Duration::from_millis(50)).await;

                println!("delay(done) - clock: {:?}, thread: {:?}", builtins.clock(), builtins.thread());

                assert!(watch.elapsed().as_millis() >= 50);
            })
        })
        .collect();

    for handle in handles {
        handle.timeout(builtins.clock(), Duration::from_secs(10)).await.unwrap().unwrap();
    }
}

#[arty::test]
async fn many_timers_ensure_advanced(builtins: Builtins) {
    let handles: Vec<_> = (0..TIMER_COUNT)
        .map(|_| {
            builtins
                .scheduler()
                .spawn_anywhere(builtins.clone(), |builtins| builtins.clock().delay(Duration::from_millis(1)))
        })
        .collect();

    for handle in handles {
        handle.await.unwrap();
    }
}

#[arty::test]
async fn timer_with_relocated_builtins(builtins: Builtins) {
    builtins
        .scheduler()
        .spawn_anywhere(builtins.clone(), |builtins| builtins.clock().delay(Duration::from_millis(10)))
        .await
        .unwrap();
}
