// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Synchronization primitives.

use std::error::Error;
use std::fmt;

/// Reusable participant barriers.
pub mod barrier;
/// Queue, oneshot, and latest-value channels.
pub mod channel;
/// Condition-variable synchronization.
pub mod condition;
/// Reader-writer synchronization.
pub mod lock;
/// Mutual-exclusion synchronization.
pub mod mutex;
/// One-time and lazy initialization.
pub mod once;
mod wait_queue;

/// Error returned when a lock is acquired after another thread poisoned it.
///
/// The error retains the acquired guard so callers can inspect or repair the
/// protected value. Dropping the guard does not clear the poison state; call
/// the lock's `clear_poison` method explicitly after recovery is complete.
#[derive(Debug)]
pub struct PoisonError<G> {
    guard: G,
}

impl<G> PoisonError<G> {
    pub(crate) const fn new(guard: G) -> Self {
        Self { guard }
    }

    /// Returns a shared reference to the acquired guard.
    #[must_use]
    pub const fn get_ref(&self) -> &G {
        &self.guard
    }

    /// Returns a mutable reference to the acquired guard.
    #[must_use]
    pub const fn get_mut(&mut self) -> &mut G {
        &mut self.guard
    }

    /// Consumes the error and returns the acquired guard.
    #[must_use]
    pub fn into_inner(self) -> G {
        self.guard
    }
}

impl<G> fmt::Display for PoisonError<G> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("poisoned lock: another thread panicked while holding the lock")
    }
}

impl<G: fmt::Debug> Error for PoisonError<G> {}

#[cold]
#[track_caller]
#[expect(clippy::panic, reason = "default lock acquisition intentionally follows std poisoning semantics")]
pub(super) fn panic_poisoned<G>(error: &PoisonError<G>) -> ! {
    panic!("{error}");
}
