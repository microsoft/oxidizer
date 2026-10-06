// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Same-pool blocking joins must be rejected instead of starving their pool.

#![cfg(feature = "rt")]
#![cfg(not(miri))]

mod panic_support;

use std::error::Error as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::pin;
use std::sync::{Arc, mpsc};
use std::task::Poll;

use arty::runtime::{BlockingPoolPolicy, CpuPolicy, Runtime};
use arty::task::JoinError;
use panic_support::{isolated, isolated_with_timeout, runtime as shared_runtime};
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

fn isolated_deadlock(name: &str, body: fn()) {
    isolated_with_timeout(name, TEST_TIMEOUT + std::time::Duration::from_secs(5), body);
}

fn completes_without_deadlock(body: impl FnOnce() + Send + 'static) {
    let (completed, completion) = mpsc::channel();
    std::thread::spawn(move || {
        body();
        _ = completed.send(());
    });
    completion
        .recv_timeout(TEST_TIMEOUT)
        .expect("the supported operation must complete without a same-pool deadlock");
}

#[test]
fn blocking_join_future_rejects_its_current_pool() {
    isolated_deadlock("blocking_join_future_rejects_its_current_pool", || {
        completes_without_deadlock(|| {
            let runtime = shared_runtime();
            let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
            let nested = scheduler.clone();
            let rejected = scheduler
                .spawn_blocking(move || {
                    let inner = nested.spawn_blocking(|| 42);
                    catch_unwind(AssertUnwindSafe(|| futures::executor::block_on(inner))).is_err()
                })
                .wait()
                .unwrap();

            assert!(rejected);
            runtime.stop().unwrap();
        });
    });
}

#[test]
fn blocking_block_on_rejects_an_async_dependency_on_its_pool() {
    isolated_deadlock("blocking_block_on_rejects_an_async_dependency_on_its_pool", || {
        completes_without_deadlock(|| {
            let runtime = shared_runtime();
            let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
            let task = scheduler.spawn_blocking(move || {
                runtime
                    .scheduler()
                    .block_on(async |cx| cx.scheduler().spawn_blocking(|| 42).await.unwrap())
                    .unwrap_err()
                    .source()
                    .and_then(|error| error.downcast_ref::<JoinError>())
                    .is_some_and(JoinError::is_panic)
            });

            assert!(task.wait().unwrap());
        });
    });
}

#[test]
fn blocking_block_on_rejects_an_async_dependency_on_its_isolated_pool() {
    isolated_deadlock("blocking_block_on_rejects_an_async_dependency_on_its_isolated_pool", || {
        completes_without_deadlock(|| {
            let runtime = Runtime::builder()
                .cpu_policy(CpuPolicy::exactly(1))
                .blocking_pool_policy(BlockingPoolPolicy::isolated())
                .build()
                .unwrap();
            let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
            let task = scheduler.spawn_blocking(move || {
                runtime
                    .scheduler()
                    .block_on(async |cx| cx.scheduler().spawn_blocking(|| 42).await.unwrap())
                    .unwrap_err()
                    .source()
                    .and_then(|error| error.downcast_ref::<JoinError>())
                    .is_some_and(JoinError::is_panic)
            });

            assert!(task.wait().unwrap());
        });
    });
}

#[test]
fn blocking_wait_rejects_an_async_dependency_on_its_pool() {
    isolated_deadlock("blocking_wait_rejects_an_async_dependency_on_its_pool", || {
        completes_without_deadlock(|| {
            let runtime = shared_runtime();
            let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
            let nested = scheduler.clone();
            let task = scheduler.spawn_blocking(move || {
                nested
                    .spawn({
                        let nested = nested.clone();
                        async move |_| nested.spawn_blocking(|| 42).await.unwrap()
                    })
                    .wait()
                    .unwrap_err()
                    .is_panic()
            });

            assert!(task.wait().unwrap());
            runtime.stop().unwrap();
        });
    });
}

#[test]
fn blocking_wait_reaches_descendants_created_before_the_wait() {
    isolated_deadlock("blocking_wait_reaches_descendants_created_before_the_wait", || {
        completes_without_deadlock(|| {
            let runtime = shared_runtime();
            let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
            let (release, released) = events_once::Event::boxed();
            let (ready, child_ready) = mpsc::channel();
            let (parked, child_parked) = mpsc::channel();
            let parent = scheduler.spawn({
                let scheduler = scheduler.clone();
                async move |_| {
                    scheduler
                        .spawn(async move |cx| {
                            ready.send(()).unwrap();
                            released.await.unwrap();
                            let blocking = cx.scheduler().spawn_blocking(|| 42);
                            let mut blocking = pin!(blocking);
                            let mut parked = Some(parked);
                            futures::future::poll_fn(move |context| match blocking.as_mut().poll(context) {
                                Poll::Pending => {
                                    if let Some(parked) = parked.take() {
                                        parked.send(()).unwrap();
                                    }
                                    Poll::Pending
                                }
                                Poll::Ready(outcome) => Poll::Ready(outcome),
                            })
                            .await
                            .unwrap()
                        })
                        .await
                        .unwrap()
                }
            });
            child_ready.recv_timeout(TEST_TIMEOUT).unwrap();

            let (started, waiter_started) = mpsc::channel();
            let (begin_wait, wait_permitted) = mpsc::channel();
            let waiter = scheduler.spawn_blocking(move || {
                started.send(()).unwrap();
                wait_permitted.recv_timeout(TEST_TIMEOUT).unwrap();
                parent.wait().unwrap_err().is_panic()
            });
            waiter_started.recv_timeout(TEST_TIMEOUT).unwrap();
            release.send(());
            child_parked.recv_timeout(TEST_TIMEOUT).unwrap();
            begin_wait.send(()).unwrap();

            assert!(waiter.wait().unwrap());
            runtime.stop().unwrap();
        });
    });
}

#[test]
fn blocking_wait_allows_async_work_without_a_same_pool_dependency() {
    let runtime = shared_runtime();
    let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
    let nested = scheduler.clone();
    let task = scheduler.spawn_blocking(move || nested.spawn(async |_| 42).wait().unwrap());

    assert_eq!(task.wait().unwrap(), 42);
    runtime.stop().unwrap();
}

#[test]
fn detached_async_work_drops_expired_blocking_wait_provenance() {
    isolated("detached_async_work_drops_expired_blocking_wait_provenance", || {
        let runtime = Arc::new(shared_runtime());
        let scheduler = runtime.scheduler().block_on(async |cx| cx.scheduler().clone()).unwrap();
        let callback_runtime = Arc::clone(&runtime);
        let (release, released) = events_once::Event::boxed();
        let (detached, detached_task) = mpsc::channel();
        let task = scheduler.spawn_blocking(move || {
            let Unaware(child) = callback_runtime
                .scheduler()
                .block_on(async move |cx| {
                    Unaware(cx.scheduler().spawn(async move |cx| {
                        released.await.unwrap();
                        cx.scheduler().spawn_blocking(|| 42).await.unwrap()
                    }))
                })
                .unwrap();
            detached.send(child).unwrap();
        });

        task.wait().unwrap();
        let detached_task = detached_task.recv_timeout(TEST_TIMEOUT).unwrap();
        release.send(());
        assert_eq!(detached_task.wait().unwrap(), 42);
        Arc::try_unwrap(runtime).unwrap().stop().unwrap();
    });
}
