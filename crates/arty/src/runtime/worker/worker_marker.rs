// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

thread_local! {
    static ATTACHED: Cell<bool> = const { Cell::new(false) };
}

/// Marks the current thread as owned by an Arty worker.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkerMarker;

impl WorkerMarker {
    pub(crate) fn attach() {
        assert!(!ATTACHED.replace(true), "a local worker is already attached to this thread");
    }

    pub(crate) fn is_current() -> bool {
        ATTACHED.get()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;
    use crate::runtime::worker::asynchronous;

    thread_local! {
        static OWNER: Owner = const { Owner };
    }

    struct Owner;

    impl Drop for Owner {
        fn drop(&mut self) {
            assert!(WorkerMarker::is_current());
        }
    }

    #[test]
    fn worker_marker_attaches_to_the_current_thread() {
        WorkerMarker::attach();
        assert!(WorkerMarker::is_current());
    }

    #[test]
    fn marker_remains_accessible_after_destructible_worker_state() {
        std::thread::spawn(|| {
            OWNER.with(|_| {});
            asynchronous::LocalWorker::attach();
        })
        .join()
        .unwrap();
    }
}
