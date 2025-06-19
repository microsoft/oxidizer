// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(not(miri))] // The runtime talks to the real OS, which Miri cannot do.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oxidizer_rt::{BasicThreadState, Runtime};
use oxidizer_rt_testing::CanaryFuture;
use oxidizer_testing::{TEST_TIMEOUT, execute_or_abandon};

#[test]
fn stop_via_runtime() {
    let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

    let (canary, started, observer) = CanaryFuture::new_with_start_notification_and_observer();

    runtime.spawn(|_| canary);
    started.recv_timeout(TEST_TIMEOUT).unwrap();

    runtime.stop();

    execute_or_abandon(move || runtime.wait()).unwrap();

    // We expect the canary to have died. Otherwise, the runtime is still running!
    assert!(observer.upgrade().is_none());
}

#[test]
fn stop_via_async_task() {
    let runtime = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");

    let (canary, started, observer) = CanaryFuture::new_with_start_notification_and_observer();

    runtime.spawn(|_| canary);
    started.recv_timeout(TEST_TIMEOUT).unwrap();

    runtime.spawn(async move |cx| {
        cx.runtime_ops().stop();
    });

    execute_or_abandon(move || runtime.wait()).unwrap();

    // We expect the canary to have died. Otherwise, the runtime is still running!
    assert!(observer.upgrade().is_none());
}

#[test]
fn stop_scoped() {
    execute_or_abandon(move || {
        _ = Runtime::<BasicThreadState>::new().expect("Failed to create runtime");
    })
    .unwrap();
}

#[test]
fn stop_in_run() {
    let task_was_executed = Arc::new(AtomicBool::new(false));

    execute_or_abandon({
        let task_was_executed = Arc::clone(&task_was_executed);

        move || {
            Runtime::<BasicThreadState>::new()
                .expect("Failed to create runtime")
                .run(async move |_| {
                    task_was_executed.store(true, Ordering::Relaxed);
                });
        }
    })
    .unwrap();

    assert!(task_was_executed.load(Ordering::Relaxed));
}