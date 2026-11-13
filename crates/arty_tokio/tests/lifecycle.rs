// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests ownership and shutdown through the public compatibility API.

#![expect(clippy::unwrap_used, reason = "test failures use unwrap for concise backtraces")]

mod support;

use std::future::pending;
use std::sync::{Arc, mpsc};

use arty::runtime::{Runtime, WorkersPolicy};
use arty_tokio::TokioCompatExt as _;
use support::{TEST_TIMEOUT, stop_bounded};
use tokio::runtime::{Builder, Handle};

#[test]
fn owned_runtime_shutdown_is_safe_inside_a_tokio_task() {
    let tokio = Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let handle = tokio.handle().clone();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .tokio_compat_with(tokio)
        .build()
        .unwrap();
    let control = runtime.scheduler().block_on(async |cx| cx.control().clone()).unwrap();
    let caller = Builder::new_current_thread().build().unwrap();

    caller.block_on(async { control.request_stop() });

    let error = caller.block_on(handle.spawn(async { 42 })).unwrap_err();
    assert!(error.is_cancelled());
    stop_bounded(runtime);
}

#[test]
fn worker_shutdown_cancels_arty_task_awaited_by_tokio_blocking_task() {
    let (finished, completion) = mpsc::sync_channel(1);
    let shutdown = std::thread::spawn(move || {
        let tokio = Builder::new_multi_thread().worker_threads(1).build().unwrap();
        let runtime = Runtime::builder()
            .workers(WorkersPolicy::exactly(1))
            .tokio_compat_with(tokio)
            .build()
            .unwrap();
        let (started, ready) = mpsc::sync_channel(1);
        let (cancelled, cancellation) = mpsc::sync_channel(1);
        let control = runtime
            .scheduler()
            .block_on(async move |cx| {
                let task = cx.scheduler().spawn(async |_| pending::<()>().await);
                drop(tokio::task::spawn_blocking(move || {
                    started.send(()).unwrap();
                    let error = Handle::current().block_on(task).unwrap_err();
                    cancelled.send(error.is_shutdown()).unwrap();
                }));
                ready.recv_timeout(TEST_TIMEOUT).unwrap();
                cx.control().clone()
            })
            .unwrap();

        let _shutdown_result = runtime.scheduler().block_on(async move |_| control.request_stop());

        let cancelled = cancellation.recv_timeout(TEST_TIMEOUT).unwrap();
        runtime.stop().unwrap();
        finished.send(cancelled).unwrap();
    });

    assert!(completion.recv_timeout(TEST_TIMEOUT).unwrap());
    shutdown.join().unwrap();
}

#[test]
fn spawning_fails_after_external_tokio_runtime_is_dropped() {
    let tokio = Builder::new_multi_thread().worker_threads(1).build().unwrap();
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .tokio_compat_with(tokio.handle())
        .build()
        .unwrap();

    let value = runtime
        .scheduler()
        .block_on(async |_| tokio::spawn(async { 42 }).await.unwrap())
        .unwrap();
    assert_eq!(value, 42);

    drop(tokio);

    let error = runtime
        .scheduler()
        .block_on(async |_| tokio::spawn(async { 42 }).await.unwrap_err())
        .unwrap();
    assert!(error.is_cancelled());

    stop_bounded(runtime);
}

#[test]
fn external_runtime_is_not_retained_when_it_shuts_down_first() {
    let tokio = Arc::new(Builder::new_multi_thread().enable_all().build().unwrap());
    let weak = Arc::downgrade(&tokio);
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .tokio_compat_with(tokio.handle())
        .build()
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
    let runtime = Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .tokio_compat_with(tokio.handle())
        .build()
        .unwrap();
    let (started, ready) = mpsc::sync_channel(1);

    runtime
        .scheduler()
        .block_on(async move |cx| {
            let never_completes = cx.scheduler().spawn(async |_| pending::<()>().await);
            drop(tokio::spawn(async move {
                started.send(()).unwrap();
                let () = never_completes.await.unwrap();
            }));
            ready.recv_timeout(TEST_TIMEOUT).unwrap();
        })
        .unwrap();

    drop(tokio);
    assert!(weak.upgrade().is_none());
    stop_bounded(runtime);
}
