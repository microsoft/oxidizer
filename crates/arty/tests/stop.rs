// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shutdown cancels pending work and releases its captured resources.

#![cfg(feature = "rt")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use arty::runtime::Runtime;
use arty::runtime::policy::WorkersPolicy;
use events_once::{BoxedReceiver, Event};
use testing_aids::{TEST_TIMEOUT, execute_or_abandon};
use thread_aware::Unaware;

fn canary() -> (impl Future<Output = ()>, BoxedReceiver<()>, std::sync::Weak<()>) {
    let lifetime = Arc::new(());
    let observer = Arc::downgrade(&lifetime);
    let (started, notification) = Event::boxed();
    let future = async move {
        started.send(());
        std::future::pending::<()>().await;
        drop(lifetime);
    };
    (future, notification, observer)
}

#[test]
fn stop_via_runtime() {
    let (done, completion) = mpsc::channel();
    let owner = std::thread::spawn(move || {
        let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let (canary, started, observer) = canary();
        runtime.scheduler().spawn_anywhere(Unaware(canary), |_, Unaware(canary)| canary);
        futures::executor::block_on(started).unwrap();
        runtime.stop().unwrap();
        assert!(observer.upgrade().is_none());
        done.send(()).unwrap();
    });
    // Keep this receive bounded under mutation too. If stop never publishes
    // shutdown or wakes the worker, joining the runtime owner could deadlock.
    match completion.recv_timeout(TEST_TIMEOUT) {
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => owner.join().unwrap(),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("runtime did not stop and release pending work"),
    }
}

#[test]
fn stop_via_async_task() {
    execute_or_abandon(|| {
        let runtime = Runtime::builder().build().unwrap();

        let (canary, started, observer) = canary();

        runtime.scheduler().spawn_anywhere(Unaware(canary), |_, Unaware(canary)| canary);

        futures::executor::block_on(started).unwrap();

        let (requested, stop_requested) = Event::boxed();
        runtime
            .scheduler()
            .spawn_anywhere(Unaware(requested), |cx, Unaware(requested)| async move {
                cx.control().shutdown();
                requested.send(());
            });

        futures::executor::block_on(stop_requested).unwrap();
        runtime.stop().unwrap();

        // The runtime stopped only if it dropped the canary.
        assert!(observer.upgrade().is_none());
    })
    .unwrap();
}

#[test]
fn stop_scoped() {
    execute_or_abandon(move || {
        _ = Runtime::builder().build().unwrap();
    })
    .unwrap();
}

#[test]
fn stop_in_run() {
    let task_was_executed = Arc::new(AtomicBool::new(false));

    execute_or_abandon({
        let task_was_executed = Arc::clone(&task_was_executed);

        move || {
            Runtime::builder()
                .build()
                .unwrap()
                .scheduler()
                .block_on(async move |_| {
                    task_was_executed.store(true, Ordering::Relaxed);
                })
                .unwrap();
        }
    })
    .unwrap();

    assert!(task_was_executed.load(Ordering::Relaxed));
}
