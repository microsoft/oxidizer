// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public shutdown guards must prevent an async worker from waiting on itself.

#![cfg(feature = "rt")]
#![cfg(test)]

mod panic_support;

mod support;

use std::future::pending;
use std::sync::mpsc;

use arty::runtime::Runtime;
use panic_support::runtime;
use support::JoinHandleExt as _;
use testing_aids::TEST_TIMEOUT;
use thread_aware::Unaware;

testing_aids::init_tracing!();

#[test]
#[cfg_attr(
    miri,
    ignore = "self-stop intentionally returns before the worker can finish; native and careful suites retain the guard contract"
)]
fn async_worker_stop_returns_without_self_joining() {
    let runtime = runtime();
    let scheduler = runtime
        .scheduler()
        .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
        .join()
        .unwrap();
    let result = scheduler.spawn(async move |_| runtime.stop()).join().unwrap();
    assert!(result.is_err());
}

#[test]
fn cancellation_destructors_remain_worker_guarded() {
    struct Probe {
        block_on_runtime: Runtime,
        stop_runtime: Option<Runtime>,
        outcomes: mpsc::Sender<(bool, bool)>,
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            let block_on_rejected = self.block_on_runtime.scheduler().block_on(async |_| ()).is_err();
            let stop_rejected = self.stop_runtime.take().unwrap().stop().is_err();
            self.outcomes.send((block_on_rejected, stop_rejected)).unwrap();
        }
    }

    let source = runtime();
    let (outcomes, received) = mpsc::channel();
    let pending_task = source.scheduler().spawn_anywhere(
        Unaware(Probe {
            block_on_runtime: runtime(),
            stop_runtime: Some(runtime()),
            outcomes,
        }),
        |_, Unaware(probe)| async move {
            let _probe = probe;
            pending::<()>().await;
        },
    );

    source.stop().unwrap();
    assert_eq!(received.recv_timeout(TEST_TIMEOUT).unwrap(), (true, true));
    assert!(pending_task.join().unwrap_err().is_shutdown());
}
