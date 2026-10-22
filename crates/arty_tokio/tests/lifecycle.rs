// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests ownership and shutdown through the public compatibility API.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

mod support;

use std::future::pending;
use std::sync::{Arc, mpsc};

use performables::sync::channel::oneshot;
use support::{TEST_TIMEOUT, runtime, stop_bounded};
use tokio::runtime::Builder;

struct DropSignal(mpsc::SyncSender<()>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        let _send_result = self.0.send(());
    }
}

#[test]
fn default_runtime_shutdown_drops_pending_tokio_tasks() {
    let runtime = runtime(1);
    let (dropped, observed_drop) = mpsc::sync_channel(1);
    let (started, ready) = oneshot();

    runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap();
            drop(tokio::spawn(async move {
                let _drop_signal = DropSignal(dropped);
                started.send(()).unwrap();
                pending::<()>().await;
            }));
            ready.await.unwrap();
        })
        .unwrap();

    stop_bounded(runtime);
    observed_drop.recv_timeout(TEST_TIMEOUT).unwrap();
}

#[test]
fn default_runtime_shutdown_completes_after_blocking_work_is_released() {
    let runtime = runtime(1);
    let (release, gate) = mpsc::sync_channel(1);
    let (blocking_started, blocking_ready) = oneshot();

    runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, arty_tokio::TokioHandle::default()).await.unwrap();
            drop(tokio::task::spawn_blocking(move || {
                blocking_started.send(()).unwrap();
                gate.recv().unwrap();
            }));
            blocking_ready.await.unwrap();
        })
        .unwrap();

    let (stopped, completion) = mpsc::sync_channel(1);
    let shutdown = std::thread::spawn(move || {
        let result = runtime.stop();
        stopped.send(result).unwrap();
    });

    release.send(()).unwrap();
    completion.recv_timeout(TEST_TIMEOUT).unwrap().unwrap();
    shutdown.join().unwrap();
}

#[test]
fn external_runtime_is_not_retained_when_it_shuts_down_first() {
    let tokio = Arc::new(Builder::new_multi_thread().enable_all().build().unwrap());
    let weak = Arc::downgrade(&tokio);
    let runtime = runtime(1);

    runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, tokio.handle().clone()).await.unwrap();
        })
        .unwrap();

    let (dropped, completion) = mpsc::sync_channel(1);
    let drop_runtime = std::thread::spawn(move || {
        drop(tokio);
        dropped.send(()).unwrap();
    });
    completion.recv_timeout(TEST_TIMEOUT).unwrap();
    drop_runtime.join().unwrap();
    assert!(weak.upgrade().is_none());

    let still_running = runtime.scheduler().block_on(async |_| 42).unwrap();
    assert_eq!(still_running, 42);
    stop_bounded(runtime);
}

#[test]
fn cross_runtime_ownership_cycle_is_broken_during_shutdown() {
    let tokio = Arc::new(Builder::new_multi_thread().enable_all().build().unwrap());
    let weak = Arc::downgrade(&tokio);
    let runtime = runtime(1);
    let (started, ready) = oneshot();

    runtime
        .scheduler()
        .block_on(async |cx| {
            arty_tokio::init(&cx, tokio.handle().clone()).await.unwrap();
            let never_completes = cx.scheduler().spawn(async |_| pending::<()>().await);
            drop(tokio::spawn(async move {
                started.send(()).unwrap();
                let () = never_completes.await.unwrap();
            }));
            ready.await.unwrap();
        })
        .unwrap();

    drop(tokio);
    assert!(weak.upgrade().is_none());
    stop_bounded(runtime);
}
