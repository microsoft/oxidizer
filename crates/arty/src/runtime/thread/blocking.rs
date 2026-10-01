// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! There exist some Arty runtime APIs that block. These APIs are intended for compatibility and
//! convenience purposes when called from a non-runtime thread. They are not safe to call from
//! threads that are marked as non-blocking threads and attempting to do so will panic.
//!
//! Arty marks all asynchronous worker threads as non-blocking threads. Runtime internals may
//! wait for notifications, but public API entry points meant for intentional
//! blocking on results are forbidden on these threads.

use std::cell::Cell;

/// Flags the current thread as a non-blocking thread. Attempting to call blocking Arty runtime
/// APIs on this thread will result in a panic.
pub(in crate::runtime) fn flag_current_thread() {
    IS_FLAGGED.with(|x| {
        x.set(true);
    });
}

pub(crate) fn assert_not_flagged() {
    IS_FLAGGED.with(|x| {
        assert!(
            !x.get(),
            "blocking Arty runtime APIs must not be called from threads owned by Arty runtime"
        );
    });
}

pub(crate) fn is_flagged() -> bool {
    IS_FLAGGED.with(Cell::get)
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
