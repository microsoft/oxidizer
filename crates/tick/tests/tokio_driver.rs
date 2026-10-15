// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public Tokio-driver timing behavior.

#![cfg(feature = "tokio")]

use std::future::poll_fn;
use std::task::Poll;
use std::time::Duration;

use thread_aware::Relocator;
use tick::Clock;

#[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
#[tokio::test]
async fn tokio_clock_advances_timers() {
    let clock = Clock::new_tokio();

    clock.delay(Duration::from_millis(15)).await;
}

#[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
#[tokio::test]
async fn tokio_clock_advances_timers_after_relocation() {
    let mut clock = Clock::new_tokio();
    _ = Relocator::between_numa_nodes().relocate(&mut clock);

    clock.delay(Duration::from_millis(15)).await;
}

#[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn earlier_timer_registration_interrupts_the_tokio_driver_wait() {
    let clock = Clock::new_tokio();
    let long_clock = clock.clone();
    let (registered, registration) = tokio::sync::oneshot::channel();
    let long_timer = tokio::spawn(async move {
        let delay = long_clock.delay(Duration::from_secs(5));
        tokio::pin!(delay);
        poll_fn(|cx| {
            assert!(delay.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        registered.send(()).unwrap();
        delay.await;
    });

    registration.await.unwrap();

    tokio::time::timeout(Duration::from_secs(1), clock.delay(Duration::from_millis(20)))
        .await
        .unwrap();

    long_timer.abort();
    _ = long_timer.await;
}
