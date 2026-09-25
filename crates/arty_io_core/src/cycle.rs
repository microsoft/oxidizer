// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::task::Waker;
use std::time::{Duration, Instant};

use crate::{CoordinationToken, Coordinator};

/// Runtime inputs and coordination operations for one driver completion cycle.
///
/// A cycle is worker-local and cannot be sent to another thread. Move
/// [`CoordinationToken`] values to background work instead.
pub struct Cycle<'a> {
    started_at: Instant,
    max_wait: Duration,
    coordinator: &'a Coordinator,
    _not_send: PhantomData<Rc<()>>,
}

impl<'a> Cycle<'a> {
    /// Creates a cycle view for one driver invocation.
    #[must_use]
    pub const fn new(started_at: Instant, max_wait: Duration, coordinator: &'a Coordinator) -> Self {
        Self {
            started_at,
            max_wait,
            coordinator,
            _not_send: PhantomData,
        }
    }

    /// Returns the snapshot captured once for all drivers visited in this runtime cycle.
    #[must_use]
    pub const fn started_at(&self) -> Instant {
        self.started_at
    }

    /// Returns the maximum duration this driver may wait in the current cycle.
    ///
    /// A primary may wait directly on the worker. A secondary may apply this duration only to
    /// background work represented by coordination tokens.
    #[must_use]
    pub const fn max_wait(&self) -> Duration {
        self.max_wait
    }

    /// Returns a stable waker that interrupts the current coordinator cycle.
    ///
    /// Contexts, submissions, and long-lived observers may retain this waker across cycles.
    #[must_use]
    pub fn interrupt_waker(&self) -> Waker {
        self.coordinator.interrupt_waker()
    }

    /// Creates a non-cloneable token for work that can outlive this call.
    ///
    /// Drivers may create multiple tokens. Use [`CoordinationToken::on_interrupted`] to attach a
    /// native wait's waker. After publishing work, call [`CoordinationToken::work_completed`]. If the
    /// work ends without publishing anything, drop the token. The runtime does not begin the next
    /// cycle until all tokens are completed or dropped.
    #[must_use]
    pub fn start_work(&self) -> CoordinationToken {
        self.coordinator.start_work()
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
