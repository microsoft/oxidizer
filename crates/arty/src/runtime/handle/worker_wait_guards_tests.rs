// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consuming a runtime on a worker leaves no public shutdown-completion handle.
//! These tests retain the private dispatcher so every owned worker is joined.
//! See docs/INTERNALS.md, "Shutdown guard tests".

use std::future::pending;
use std::sync::mpsc;

use thread_aware::Unaware;

use crate::runtime::{BlockingPoolPolicy, Runtime, WorkersPolicy};

/// Exercises both wait guards while cancellation destroys a task on its worker.
struct Probe {
    block_on_runtime: Runtime,
    stop_runtime: Option<Runtime>,
    outcomes: mpsc::Sender<(bool, bool)>,
}

impl Drop for Probe {
    #[mutants::skip] // Test stimulus, not runtime behavior.
    fn drop(&mut self) {
        let block_on_rejected = self.block_on_runtime.scheduler().block_on(async |_| ()).is_err();
        let stop_rejected = self.stop_runtime.take().unwrap().stop().is_err();
        self.outcomes.send((block_on_rejected, stop_rejected)).unwrap();
    }
}

#[test]
fn async_worker_stop_returns_without_self_joining() {
    let runtime = runtime();
    let dispatcher = runtime.scheduler.dispatcher.clone();
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let result = scheduler.spawn(async move |_| runtime.stop()).join().unwrap();

    assert!(dispatcher.is_shutting_down());
    dispatcher.wait().unwrap();
    assert!(result.is_err());
}

#[test]
fn cancellation_destructors_remain_worker_guarded() {
    let source = runtime();
    let block_on_runtime = runtime();
    let block_on_dispatcher = block_on_runtime.scheduler.dispatcher.clone();
    let stop_runtime = runtime();
    let stop_dispatcher = stop_runtime.scheduler.dispatcher.clone();
    let (outcomes, received) = mpsc::channel();
    let pending_task = source.scheduler().spawn_anywhere(
        Unaware(Probe {
            block_on_runtime,
            stop_runtime: Some(stop_runtime),
            outcomes,
        }),
        |_, Unaware(probe)| async move {
            let _probe = probe;
            pending::<()>().await;
        },
    );

    source.stop().unwrap();
    assert!(block_on_dispatcher.is_shutting_down());
    assert!(stop_dispatcher.is_shutting_down());
    // Source cancellation requests shutdown of the nested runtimes, but does not join them.
    block_on_dispatcher.wait().unwrap();
    stop_dispatcher.wait().unwrap();

    assert_eq!(received.try_recv().unwrap(), (true, true));
    assert!(pending_task.join().unwrap_err().is_shutdown());
}

/// Uses the minimum worker topology needed to exercise cross-runtime wait guards.
#[mutants::skip] // Test configuration, not runtime behavior.
fn runtime() -> Runtime {
    Runtime::builder()
        .workers(WorkersPolicy::exactly(1))
        .blocking_pool(BlockingPoolPolicy::shared().max(1))
        .build()
        .unwrap()
}
