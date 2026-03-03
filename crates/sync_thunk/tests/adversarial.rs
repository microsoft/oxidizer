// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for adversarial usage patterns — `mem::forget`, cancellation, etc.

use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::time::Duration;

use sync_thunk::{Thunker, thunk};

/// Creates a no-op waker for manual polling.
fn noop_waker() -> Waker {
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    // SAFETY: The vtable functions are valid no-ops.
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

struct Service {
    thunker: Thunker,
}

impl Service {
    #[thunk(from = self.thunker)]
    async fn blocking_work(&self, flag: &AtomicBool) -> u64 {
        // Signal that the worker started executing.
        flag.store(true, Ordering::Release);
        std::thread::sleep(Duration::from_millis(10));
        42
    }
}

/// Calling `mem::forget` on a thunked future after it has been polled must not cause UB.
///
/// When a future is forgotten after the work item has been dispatched, its
/// destructor (including `StackState::Drop`) never runs. The worker thread
/// still completes and writes its result into the leaked — but still valid —
/// memory. The result is a memory leak, not use-after-free.
#[tokio::test]
async fn mem_forget_on_thunked_future_does_not_cause_ub() {
    let service = Service {
        thunker: Thunker::builder()
            .max_thread_count(2)
            .cool_down_interval(Duration::from_secs(1))
            .build(),
    };

    let started = AtomicBool::new(false);

    // Box::pin the future so we can forget the Box (and thus the StackState inside).
    let mut future = Box::pin(service.blocking_work(&started));

    // Poll once — this dispatches the WorkItem to the thunker and returns Pending.
    let waker = noop_waker();
    let mut cx = std::task::Context::from_waker(&waker);
    let _ = std::future::Future::poll(future.as_mut(), &mut cx);

    // Forget the boxed future. Its destructor (StackState::Drop) never runs.
    // The heap allocation is leaked, but the worker still has a valid pointer.
    std::mem::forget(future);

    // Give the worker time to complete. It writes to the leaked StackState.
    std::thread::sleep(Duration::from_millis(100));

    // The worker should have executed.
    assert!(started.load(Ordering::Acquire), "worker should have executed");
}
