// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

/// The wait limit for one driver invocation.
#[derive(Debug)]
pub struct Cycle {
    max_wait: Duration,
    // A cycle belongs to one invocation on one async worker. The !Send and !Sync markers prevent
    // the runtime from moving or sharing this state across workers.
    _not_send: PhantomData<Rc<()>>,
}

impl Cycle {
    /// Creates the wait input for one driver invocation.
    ///
    /// The runtime must use [`Duration::ZERO`] when the driver must not wait. This requirement
    /// applies to every secondary driver.
    #[must_use]
    #[inline]
    pub const fn new(max_wait: Duration) -> Self {
        Self {
            max_wait,
            _not_send: PhantomData,
        }
    }

    /// Returns the maximum wait duration.
    ///
    /// A [primary driver](crate::DriverRole::Primary) can wait on its worker for up to this
    /// duration. A [secondary driver](crate::DriverRole::Secondary) must not wait on its worker.
    /// [`Duration::ZERO`] means no waiting.
    #[must_use]
    #[inline]
    pub const fn max_wait(&self) -> Duration {
        self.max_wait
    }
}
