// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cancellation destructors must retain the public async-worker wait guard.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

mod support;

use std::cell::RefCell;
use std::future::pending;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Context, Poll, Waker};

use arty::runtime::Runtime;
use panic_support::runtime;
use support::JoinHandleExt as _;
use testing_aids::{TEST_TIMEOUT, is_mutation_testing};
use thread_aware::Unaware;

thread_local! {
    static BLOCKING_THREAD_EXIT: RefCell<Option<BlockingThreadExit>> = const { RefCell::new(None) };
}

struct BlockingThreadExit(mpsc::Sender<()>);

impl Drop for BlockingThreadExit {
    fn drop(&mut self) {
        self.0.send(()).unwrap();
    }
}

/// A probe that exercises the wait guard while cancellation destroys a task on its worker.
struct Probe {
    block_on_runtime: Arc<Runtime>,
    outcome: mpsc::Sender<bool>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        let block_on_rejected = self.block_on_runtime.scheduler().block_on(async |_| ()).is_err();
        self.outcome.send(block_on_rejected).unwrap();
    }
}

#[test]
fn a_remote_task_wake_is_retained_before_the_runtime_worker_parks() {
    struct RemoteWake {
        ready: Arc<AtomicBool>,
        waker: Option<mpsc::Sender<Waker>>,
    }

    impl Future for RemoteWake {
        type Output = u64;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
            // Observe the published ready state independently from wake delivery.
            if self.ready.load(Ordering::Acquire) {
                return Poll::Ready(7);
            }
            if let Some(sender) = self.waker.take() {
                sender.send(cx.waker().clone()).unwrap();
            }
            Poll::Pending
        }
    }

    let runtime = runtime();
    for _ in 0..if cfg!(miri) { 2 } else { 20 } {
        let ready = Arc::new(AtomicBool::new(false));
        let (sent, received) = mpsc::channel();
        let task = runtime
            .scheduler()
            .spawn_anywhere(Unaware((Arc::clone(&ready), sent)), |_, Unaware((ready, sender))| RemoteWake {
                ready,
                waker: Some(sender),
            });
        let waker = if is_mutation_testing() {
            received.recv().unwrap()
        } else {
            received.recv_timeout(TEST_TIMEOUT).unwrap()
        };
        // Publish the predicate before the wake. The test does not use the wake as a memory barrier.
        ready.store(true, Ordering::Release);
        waker.wake_by_ref();
        waker.wake_by_ref();
        assert_eq!(task.join().unwrap(), 7);
    }
    runtime.stop().unwrap();
}

#[test]
fn stop_waits_for_blocking_worker_thread_teardown() {
    let runtime = runtime();
    let (exited, received) = mpsc::channel();

    runtime
        .scheduler()
        .spawn_blocking(move || {
            BLOCKING_THREAD_EXIT.with(|exit| {
                assert!(exit.replace(Some(BlockingThreadExit(exited))).is_none());
            });
        })
        .join()
        .unwrap();

    runtime.stop().unwrap();
    received.try_recv().unwrap();
}

#[test]
fn cancellation_destructors_remain_worker_guarded() {
    let source = runtime();
    // Keep the owner off the worker so the test can join it after cancellation.
    // Consuming stop guards need the private join capability tested in runtime::handle.
    let block_on_runtime = Arc::new(runtime());
    let (outcome, received) = mpsc::channel();
    let pending_task = source.scheduler().spawn_anywhere(
        Unaware(Probe {
            block_on_runtime: Arc::clone(&block_on_runtime),
            outcome,
        }),
        |_, Unaware(probe)| async move {
            let _probe = probe;
            pending::<()>().await;
        },
    );

    source.stop().unwrap();
    Arc::try_unwrap(block_on_runtime).unwrap().stop().unwrap();
    assert!(received.try_recv().unwrap());
    assert!(pending_task.join().unwrap_err().is_shutdown());
}
