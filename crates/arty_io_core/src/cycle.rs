// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::task::Waker;
use std::time::{Duration, Instant};

use crate::{PendingWork, PendingWorkTracker};

/// Timing and work registration for one driver invocation.
///
/// A cycle borrows a runtime-owned [`PendingWorkTracker`] on the owning worker. For background work,
/// move the [`PendingWork`] returned by [`start_work`](Self::start_work), not the cycle.
pub struct Cycle<'a> {
    started_at: Instant,
    max_wait: Duration,
    tracker: &'a mut dyn PendingWorkTracker,
    _not_send: PhantomData<Rc<()>>,
}

impl<'a> Cycle<'a> {
    /// Creates the inputs for one driver invocation.
    ///
    /// The runtime must supply the current logical cycle's `tracker` and use [`Duration::ZERO`]
    /// for `max_wait` when no waiting is allowed, including during initialization.
    #[must_use]
    pub const fn new(started_at: Instant, max_wait: Duration, tracker: &'a mut dyn PendingWorkTracker) -> Self {
        Self {
            started_at,
            max_wait,
            tracker,
            _not_send: PhantomData,
        }
    }

    /// Returns the time snapshot shared by all drivers in this runtime cycle.
    #[must_use]
    pub const fn started_at(&self) -> Instant {
        self.started_at
    }

    /// Returns the maximum wait duration.
    ///
    /// A [primary driver](crate::DriverRole::Primary) may wait on its worker for up to this
    /// duration. A [secondary driver](crate::DriverRole::Secondary) may apply it only to
    /// background waits represented by [`PendingWork`]. [`Duration::ZERO`] means no waiting.
    #[must_use]
    pub const fn max_wait(&self) -> Duration {
        self.max_wait
    }

    /// Synchronously registers pending work and its interruption waker.
    ///
    /// Before entering or scheduling a native wait, the driver must call this method with a
    /// waker whose signal remains latched until the wait observes it. The runtime may invoke
    /// the waker inline on any thread; it must return promptly without panicking, joining work,
    /// or acquiring locks held by the completing work.
    ///
    /// Keep the returned handle until the work ends, publishing any results before
    /// [completing](PendingWork::complete) or dropping it. Both actions notify the runtime.
    /// Each call registers a separate participant in the completion barrier.
    /// Use a no-op waker for work that does not wait.
    #[inline]
    pub fn start_work(&mut self, interrupt: Waker) -> PendingWork {
        self.tracker.start_work(interrupt)
    }
}

impl fmt::Debug for Cycle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cycle")
            .field("started_at", &self.started_at)
            .field("max_wait", &self.max_wait)
            .finish_non_exhaustive()
    }
}
