// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;

/// Timing for one driver invocation.
#[derive(Debug)]
pub struct Cycle {
    max_wait: Duration,
    _not_send: PhantomData<Rc<()>>,
}

impl Cycle {
    /// Creates the inputs for one driver invocation.
    ///
    /// The runtime must use [`Duration::ZERO`] for `max_wait` when no waiting is allowed,
    /// including during initialization.
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
    /// A [primary driver](crate::DriverRole::Primary) may wait on its worker for up to this
    /// duration. A [secondary driver](crate::DriverRole::Secondary) must not wait on its worker.
    /// [`Duration::ZERO`] means no waiting.
    #[must_use]
    #[inline]
    pub const fn max_wait(&self) -> Duration {
        self.max_wait
    }
}
