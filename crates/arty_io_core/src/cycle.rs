// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::time::{Duration, Instant};

use crate::Interruptor;

/// Runtime inputs shared by every driver invocation in one completion cycle.
///
/// The runtime owns one stable [`Interruptor`] per worker. For each logical cycle it resets the
/// interruptor exactly once before checking task, control, or driver work; captures one
/// `started_at` and one `max_wait`; then invokes every secondary and finally the primary with
/// those same values. The initialization cycle for a newly created driver participates in the
/// current interruption round and does not reset the interruptor.
#[derive(Clone, Debug)]
pub struct Cycle {
    started_at: Instant,
    max_wait: Duration,
    interruptor: Interruptor,
}

impl Cycle {
    /// Creates a cycle with a shared time snapshot, maximum wait, and worker interruptor.
    #[must_use]
    #[inline]
    pub const fn new(started_at: Instant, max_wait: Duration, interruptor: Interruptor) -> Self {
        Self {
            started_at,
            max_wait,
            interruptor,
        }
    }

    /// Returns the snapshot captured once for all drivers visited in this runtime cycle.
    ///
    /// This is not a completion timestamp or the time after a native wait.
    #[must_use]
    #[inline]
    pub const fn started_at(&self) -> Instant {
        self.started_at
    }

    /// Returns the maximum duration this driver may wait in the current cycle.
    ///
    /// The runtime supplies exactly the same value to every driver in the logical cycle. A
    /// primary may wait directly on the worker. A secondary may use this duration only for a
    /// wait scheduled on a background thread; its worker-local cycle remains non-blocking.
    /// Registration initialization uses [`Duration::ZERO`].
    ///
    /// [`Duration::ZERO`] requires a non-blocking poll. [`Duration::MAX`] permits an unbounded
    /// wait that can still be interrupted. Implementations with a coarser native clock may round
    /// a finite duration up, but must not convert it into an unbounded wait. A pending
    /// interruption must make the wait return promptly.
    #[must_use]
    #[inline]
    pub const fn max_wait(&self) -> Duration {
        self.max_wait
    }

    /// Returns the stable notification and native-wait interruption handle for this worker.
    ///
    /// Code that discovers work outside [`Driver::execute_cycle`](crate::Driver::execute_cycle)
    /// retains a clone. After publishing work, it calls [`Interruptor::request`].
    ///
    /// The runtime resets the shared request latch before checking work in each logical cycle;
    /// retained clones then notify the new cycle. Drivers register the waker for each current
    /// native wait again after that reset. A secondary must use driver-private synchronization
    /// to arm or replace its off-worker wait; the interruptor notifies the runtime and the
    /// native waits whose wakers were registered for the current cycle.
    #[must_use]
    #[inline]
    pub const fn interruptor(&self) -> &Interruptor {
        &self.interruptor
    }
}
