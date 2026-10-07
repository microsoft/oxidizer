// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tracks asynchronous worker threads where public blocking calls must not wait.
//!
//! `RuntimeScheduler::block_on` and `Runtime::stop` return errors in this context.
//! `JoinHandle::wait` uses a panic-on-misuse guard. Implicit runtime destruction
//! requests shutdown without waiting when this flag is set.
//! Runtime internals may still wait for their own coordination notifications.

use std::cell::Cell;

use crate::task::Scheduler;

/// Flags the current thread so public APIs reject waits that would stall a worker.
pub(in crate::runtime) fn flag_current_thread() {
    IS_FLAGGED.with(|x| {
        x.set(true);
    });
}

pub(crate) fn assert_not_flagged() {
    assert!(
        !is_async_worker_thread(),
        "blocking Arty runtime APIs must not be called from threads owned by Arty runtime"
    );
}

pub(crate) fn is_flagged() -> bool {
    IS_FLAGGED.with(Cell::get)
}

pub(crate) fn is_async_worker_thread() -> bool {
    is_flagged() || Scheduler::is_current_worker_thread()
}

thread_local! {
    /// The functions in this module may be called from any thread (via `Runtime`) and do not have
    /// access to the Arty runtime task context, as the functions are not necessarily executing
    /// as part of a task managed by the runtime. Therefore, we use a regular thread-local variable.
    static IS_FLAGGED: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "blocking Arty runtime APIs must not be called")]
    fn test_flagged_thread() {
        flag_current_thread();
        assert_not_flagged();
    }

    #[test]
    fn test_unflagged_thread() {
        assert_not_flagged();
    }
}
