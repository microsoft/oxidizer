// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! There exist some Oxidizer Runtime APIs that block. These APIs are intended for compatibility and
//! convenience purposes when called from a non-Oxidizer thread. They are not safe to call from
//! threads that are marked as non-blocking threads and attempting to do so will panic.
//!
//! Oxidizer marks all worker threads as non-blocking threads. To be clear, Oxidizer itself is
//! allowed to run blocking code on them but Oxidizer public API entrypoints meant for intentional
//! blocking on results are forbidden on these threads.

use std::cell::Cell;

/// Flags the current thread as a non-blocking thread. Attempting to call blocking Oxidizer Runtime
/// APIs on this thread will result in a panic.
pub fn flag_current_thread() {
    IS_FLAGGED.with(|x| {
        x.set(true);
    });
}

pub fn assert_not_flagged() {
    IS_FLAGGED.with(|x| {
        assert!(!x.get(), "blocking Oxidizer Runtime APIs must not be called from threads owned by Oxidizer Runtime");
    });
}

thread_local! {
    /// The functions in this module may be called from any thread (via `Runtime`) and do not have
    /// access to the Oxidizer Runtime task context, as the functions are not necessarily executing
    /// as part of a task managed by the runtime. Therefore, we use a regular thread-local variable.
    static IS_FLAGGED: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn test_flagged_thread() {
        flag_current_thread();
        assert_not_flagged();
    }

    #[test]
    fn test_unflagged_thread() {
        assert_not_flagged();
    }
}