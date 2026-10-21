// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Cancellation destructors must retain the public async-worker wait guard.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

mod support;

use std::cell::RefCell;
use std::future::pending;
use std::sync::{Arc, mpsc};

use arty::runtime::Runtime;
use panic_support::runtime;
use support::JoinHandleExt as _;
use thread_aware::Unaware;

testing_aids::init_tracing!();

thread_local! {
    static BLOCKING_THREAD_EXIT: RefCell<Option<BlockingThreadExit>> = const { RefCell::new(None) };
}

struct BlockingThreadExit(mpsc::Sender<()>);

impl Drop for BlockingThreadExit {
    fn drop(&mut self) {
        self.0.send(()).unwrap();
    }
}

/// Exercises the wait guard while cancellation destroys a task on its worker.
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
