// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shutdown cancels pending work and releases its captured resources.

#![cfg(feature = "rt")]

testing_aids::init_tracing!();

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arty::runtime::Runtime;
use events_once::{BoxedReceiver, Event};
use testing_aids::execute_or_abandon;

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
    execute_or_abandon(|| {
        let runtime = Runtime::new().unwrap();

        let (canary, started, observer) = canary();

        runtime.task_scheduler().spawn(async |_| canary.await);
        futures::executor::block_on(started).unwrap();

        runtime.stop();
        runtime.wait();

        // We expect the canary to have died. Otherwise, the runtime is still running!
        assert!(observer.upgrade().is_none());
    })
    .unwrap();
}

#[test]
fn stop_via_async_task() {
    execute_or_abandon(|| {
        let runtime = Runtime::new().unwrap();

        let (canary, started, observer) = canary();

        runtime.task_scheduler().spawn(async |_| canary.await);

        futures::executor::block_on(started).unwrap();

        runtime.task_scheduler().spawn(async move |cx| {
            cx.runtime_operations().stop();
        });

        runtime.wait();

        // We expect the canary to have died. Otherwise, the runtime is still running!
        assert!(observer.upgrade().is_none());
    })
    .unwrap();
}

#[test]
fn stop_scoped() {
    execute_or_abandon(move || {
        _ = Runtime::new().unwrap();
    })
    .unwrap();
}

#[test]
fn stop_in_run() {
    let task_was_executed = Arc::new(AtomicBool::new(false));

    execute_or_abandon({
        let task_was_executed = Arc::clone(&task_was_executed);

        move || {
            Runtime::new()
                .unwrap()
                .run(async move |_| {
                    task_was_executed.store(true, Ordering::Relaxed);
                })
                .unwrap();
        }
    })
    .unwrap();

    assert!(task_was_executed.load(Ordering::Relaxed));
}
