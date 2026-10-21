// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(not(miri))]

use std::future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use anyspawn::Spawner;
use arty::runtime::Runtime;
use thread_aware::ThreadAware;

static_assertions::assert_impl_all!(Spawner: Send, Sync, ThreadAware);

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

fn runtime_and_spawner() -> (Runtime, Spawner) {
    let runtime = Runtime::new().unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let spawner = Spawner::new_arty(scheduler);
    (runtime, spawner)
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap()
}

#[test]
fn arty_spawn_and_clone_dispatch_tasks() {
    let (runtime, spawner) = runtime_and_spawner();
    let clone = spawner.clone();

    let result = futures::executor::block_on(clone.spawn(async { String::from("Arty") }));

    assert_eq!(result, "Arty");
    runtime.stop().unwrap();
}

#[test]
fn dropping_arty_join_handle_does_not_cancel_task() {
    let (runtime, spawner) = runtime_and_spawner();
    let (sender, receiver) = std::sync::mpsc::channel();
    let (release, released) = futures::channel::oneshot::channel();

    let handle = spawner.spawn(async move {
        released.await.unwrap();
        sender.send(42).unwrap();
    });
    drop(handle);
    release.send(()).unwrap();

    assert_eq!(receiver.recv_timeout(TEST_TIMEOUT).unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn arty_spawn_anywhere_returns_send_output() {
    struct SendOnly(u32);

    let (runtime, spawner) = runtime_and_spawner();

    let result = futures::executor::block_on(spawner.spawn_anywhere(21_u32, |value| async move { SendOnly(value * 2) }));

    assert_eq!(result.0, 42);
    runtime.stop().unwrap();
}

#[test]
fn arty_spawn_blocking_returns_output() {
    let (runtime, spawner) = runtime_and_spawner();

    let result = futures::executor::block_on(spawner.spawn_blocking(|| 6 * 7));

    assert_eq!(result, 42);
    runtime.stop().unwrap();
}

#[test]
fn arty_task_panic_panics_when_joined() {
    let (runtime, spawner) = runtime_and_spawner();
    let (started, task_ran) = std::sync::mpsc::channel();
    let handle = spawner.spawn(async move {
        started.send(()).unwrap();
        panic!("task failed");
    });

    let panic = catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(handle))).unwrap_err();

    task_ran.recv_timeout(TEST_TIMEOUT).unwrap();
    assert!(panic_message(&*panic).contains("spawned task did not complete"));
    runtime.stop().unwrap();
}

#[test]
fn arty_shutdown_cancels_pending_task() {
    let (runtime, spawner) = runtime_and_spawner();
    let (started, task_started) = std::sync::mpsc::channel();
    let handle = spawner.spawn(async move {
        started.send(()).unwrap();
        future::pending::<()>().await;
    });
    task_started.recv_timeout(TEST_TIMEOUT).unwrap();
    runtime.stop().unwrap();

    let panic = catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(handle))).unwrap_err();

    assert!(panic_message(&*panic).contains("spawned task did not complete"));
}

#[test]
fn arty_scheduler_does_not_keep_runtime_alive() {
    let (runtime, spawner) = runtime_and_spawner();
    runtime.stop().unwrap();

    let handle = spawner.spawn(async { 42 });
    let panic = catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(handle)));

    assert!(panic.is_err());
}

#[test]
fn arty_spawner_debug_names_the_runtime() {
    let (runtime, spawner) = runtime_and_spawner();

    let debug = format!("{spawner:?}");

    assert!(debug.contains("arty"));
    runtime.stop().unwrap();
}
