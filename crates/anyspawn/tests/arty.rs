// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(missing_docs, reason = "test code")]
#![cfg(not(miri))]
#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

testing_aids::init_tracing!();

use std::future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread::{self, ThreadId};
use std::time::Duration;

use anyspawn::Spawner;
use arty::runtime::{Runtime, WorkersPolicy};
use futures::future::join_all;
use thread_aware::{Thread, ThreadAware};

static_assertions::assert_impl_all!(Spawner: Send, Sync, ThreadAware);

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

struct RelocationProbe(Option<ThreadId>);

impl ThreadAware for RelocationProbe {
    fn relocate(&mut self, _source: Option<&Thread>, destination: &Thread) {
        self.0 = Some(destination.id());
    }
}

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

    let original_result = futures::executor::block_on(spawner.spawn(async { String::from("original") }));
    let clone_result = futures::executor::block_on(clone.spawn(async { String::from("clone") }));

    assert_eq!((original_result.as_str(), clone_result.as_str()), ("original", "clone"));
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
fn arty_spawn_anywhere_returns_thread_aware_output() {
    let (runtime, spawner) = runtime_and_spawner();

    let result = futures::executor::block_on(spawner.spawn_anywhere(21_u32, |value| async move { value * 2 }));

    assert_eq!(result, 42);
    runtime.stop().unwrap();
}

#[test]
fn arty_relocation_round_trip_rebinds_to_destination_worker() {
    if many_cpus::SystemHardware::current().processors().len() < 2 {
        return;
    }
    let runtime_a = Runtime::builder().workers(WorkersPolicy::exactly(2)).build().unwrap();
    let (mut spawner, source, destination) = runtime_a
        .scheduler()
        .block_on(async |cx| {
            let source = cx.thread().clone();
            let scheduler = cx.scheduler().clone();
            let threads =
                join_all((0..2).map(|_| scheduler.spawn_anywhere(cx.clone(), |worker| async move { worker.thread().clone() }))).await;
            let destination = threads.into_iter().map(Result::unwrap).find(|thread| thread != &source).unwrap();
            (Spawner::new_arty(scheduler), source, destination)
        })
        .unwrap();
    let original = spawner.clone();

    let runtime_b = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let foreign = runtime_b.scheduler().block_on(async |cx| cx.thread().clone()).unwrap();

    spawner.relocate(Some(&source), &foreign);
    spawner.relocate(Some(&foreign), &destination);
    let original_worker = futures::executor::block_on(original.spawn(async { thread::current().id() }));
    let relocated_worker = futures::executor::block_on(spawner.spawn(async { thread::current().id() }));

    assert_eq!((original_worker, relocated_worker), (source.id(), destination.id()));
    runtime_b.stop().unwrap();
    runtime_a.stop().unwrap();
}

#[test]
fn arty_spawn_anywhere_relocates_payload_before_factory() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let spawner = Spawner::new_arty(scheduler);

    let relocated = futures::executor::block_on(spawner.spawn_anywhere(RelocationProbe(None), |probe| async move {
        probe.0 == Some(thread::current().id())
    }));

    assert!(relocated);
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

    drop(panic.unwrap_err());
}

#[test]
fn arty_spawner_debug_names_the_runtime() {
    let (runtime, spawner) = runtime_and_spawner();

    let debug = format!("{spawner:?}");

    assert!(debug.contains("arty"));
    runtime.stop().unwrap();
}
