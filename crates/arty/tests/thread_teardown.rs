// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public handles remain usable during thread-local destruction.

#![cfg(feature = "rt")]

mod support;

use std::cell::RefCell;
use std::pin::pin;
use std::sync::mpsc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use arty::runtime::{Runtime, RuntimeControl, WorkersPolicy};
use arty::task::{JoinHandle, Scheduler};
use support::JoinHandleExt as _;

thread_local! {
    static SPAWN_ON_DROP: RefCell<Option<SpawnOnDrop>> = const { RefCell::new(None) };
    static SHUTDOWN_GUARD: RefCell<Option<ShutdownGuard>> = const { RefCell::new(None) };
}

struct SpawnOnDrop {
    scheduler: Scheduler,
    spawned: mpsc::Sender<JoinHandle<()>>,
}

impl Drop for SpawnOnDrop {
    fn drop(&mut self) {
        self.spawned.send(self.scheduler.spawn(async |_| {})).unwrap();
    }
}

struct ShutdownGuard(RuntimeControl);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.0.request_stop();
    }
}

#[test]
fn external_spawn_survives_scheduler_slot_teardown() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let (spawned, received) = mpsc::channel();

    std::thread::spawn(move || {
        SPAWN_ON_DROP.with(|owner| {
            assert!(
                owner
                    .replace(Some(SpawnOnDrop {
                        scheduler: scheduler.clone(),
                        spawned,
                    }))
                    .is_none()
            );
        });
        scheduler.spawn(async |_| {}).join().unwrap();
    })
    .join()
    .unwrap();

    received.recv_timeout(Duration::from_secs(5)).unwrap().join().unwrap();
    runtime.stop().unwrap();
}

#[test]
fn dispatcher_shutdown_rejects_direct_submissions_immediately() {
    let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
    let rejected = runtime
        .scheduler()
        .block_on(async |cx| {
            let control = cx.control().clone();
            // Publish dispatcher shutdown without stopping this worker's executor in this poll.
            std::thread::spawn(move || control.request_stop()).join().unwrap();
            let mut task = pin!(cx.scheduler().spawn(async |_| 42));

            matches!(
                task.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(Err(error)) if error.is_shutdown()
            )
        })
        .unwrap();

    assert!(rejected);
    runtime.stop().unwrap();
}

#[test]
fn thread_local_shutdown_guard_survives_worker_slot_teardown() {
    std::thread::spawn(|| {
        let runtime = Runtime::builder().workers(WorkersPolicy::exactly(1)).build().unwrap();
        let (control, scheduler) = runtime
            .scheduler()
            .block_on(async |cx| (cx.control().clone(), cx.scheduler().clone()))
            .unwrap();
        SHUTDOWN_GUARD.with(|guard| {
            assert!(guard.replace(Some(ShutdownGuard(control))).is_none());
        });
        scheduler.spawn(async |_| {}).join().unwrap();
        runtime.stop().unwrap();
    })
    .join()
    .unwrap();
}
