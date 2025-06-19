// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.

use oxidizer_rt::{BasicThreadState, Runtime};
use oxidizer_testing::execute_or_abandon;
use oxidizer_time::{Delay, Stopwatch};

#[test]
fn timers_ok() {
    let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

    runtime
        .spawn(async move |cx| {
            let watch = Stopwatch::with_clock(cx.clock());
            Delay::with_clock(cx.clock(), std::time::Duration::from_millis(50)).await;
            assert!(watch.elapsed().as_millis() >= 50);
        })
        .wait();

    runtime.stop();

    execute_or_abandon(move || runtime.wait()).unwrap();
}