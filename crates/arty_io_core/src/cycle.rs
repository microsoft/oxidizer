// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::time::Duration;
use std::{marker::PhantomData, rc::Rc};

/// The wait budget for one driver invocation.
pub struct Cycle {
    max_wait: Duration,
    _not_send: PhantomData<Rc<()>>,
}

impl Cycle {
    /// Creates a cycle with the maximum permitted wait.
    #[must_use]
    pub const fn new(max_wait: Duration) -> Self {
        Self {
            max_wait,
            _not_send: PhantomData,
        }
    }

    /// Returns the maximum wait duration.
    ///
    /// A [primary driver](crate::DriverRole::Primary) may wait on its worker for up to this
    /// duration. A [secondary driver](crate::DriverRole::Secondary) must return promptly from its
    /// worker-local cycle and may use this value for work processed independently of the worker.
    /// [`Duration::ZERO`] means no waiting.
    #[must_use]
    pub const fn max_wait(&self) -> Duration {
        self.max_wait
    }
}

impl fmt::Debug for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cycle").field("max_wait", &self.max_wait).finish()
    }
}
