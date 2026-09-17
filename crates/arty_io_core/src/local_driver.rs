// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::{CompletionBudget, Driver, DriverError, ServiceStatus, Shutdown, WaitStatus};

/// An exclusively owned driver that cannot be moved or shared across threads.
///
/// Providers return this handle on the final owning thread. Its thread affinity is enforced
/// even when the concrete driver's fields happen to implement `Send` or `Sync`.
/// Consumers receive only the independent context returned by [`context`](Self::context).
///
/// The runtime can privately erase unrelated context types by forwarding this handle's methods.
/// Consuming [`shutdown`](Self::shutdown) transfers ownership into a local drain exactly once.
pub struct LocalDriver<D: Driver + ?Sized> {
    driver: Box<D>,
    _local: PhantomData<Rc<()>>,
}

impl<D: Driver> LocalDriver<D> {
    /// Owns a newly created driver on its final owning thread.
    #[must_use]
    pub fn new(driver: D) -> Self {
        Self::from_box(Box::new(driver))
    }
}

impl<D: Driver + ?Sized> LocalDriver<D> {
    /// Owns a boxed driver, including a driver trait object with a specified context type.
    #[must_use]
    pub const fn from_box(driver: Box<D>) -> Self {
        Self {
            driver,
            _local: PhantomData,
        }
    }

    /// Returns the driver-defined consumer context.
    #[must_use]
    pub fn context(&self) -> D::Context {
        self.driver.context()
    }

    /// Performs a bounded, non-blocking completion-service turn.
    ///
    /// # Errors
    ///
    /// Returns the driver's service failure without converting it into an idle result.
    pub fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        self.driver.service(budget)
    }

    /// Arms notifications and rechecks work before the runtime considers sleeping.
    ///
    /// # Errors
    ///
    /// Returns the driver's notification-preparation failure.
    pub fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        self.driver.prepare_wait()
    }

    /// Closes admission and transfers the driver into cooperative draining.
    pub fn shutdown(self) -> Shutdown {
        self.driver.shutdown()
    }
}

impl<D: Driver + ?Sized> fmt::Debug for LocalDriver<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}
