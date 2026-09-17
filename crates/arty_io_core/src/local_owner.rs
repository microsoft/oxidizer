// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::{CompletionBudget, Drain, DrainStatus, Driver, DriverError, ServiceStatus, WaitStatus};

/// An inline, exclusively owned driver confined to its construction thread.
///
/// This wrapper does not allocate or erase the concrete driver. It is neither [`Send`] nor
/// [`Sync`] even when the driver is. It does not expose the inner value or references through
/// which installed state could escape. A runtime may independently choose private erasure for
/// heterogeneous storage.
///
/// Thread confinement is not pinning: the owner may move within its thread. Native code must
/// retain independently stable, pinned, or otherwise safely owned callback-visible storage.
///
/// ```compile_fail,E0277
/// use arty_io_core::{Driver, LocalDriver};
///
/// fn move_to_another_thread<D: Driver + Send>(driver: LocalDriver<D>) {
///     std::thread::spawn(move || drop(driver));
/// }
/// ```
pub struct LocalDriver<D: Driver> {
    inner: D,
    local: PhantomData<Rc<()>>,
}

impl<D: Driver> LocalDriver<D> {
    /// Installs a concrete driver on its final owning thread without allocating.
    #[must_use]
    pub const fn new(inner: D) -> Self {
        Self { inner, local: PhantomData }
    }

    /// Performs one bounded, non-blocking service turn.
    ///
    /// # Errors
    ///
    /// Returns the driver's failure unchanged.
    pub fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError> {
        Driver::service(&mut self.inner, budget)
    }

    /// Arms notifications and rechecks work before a possible wait.
    ///
    /// # Errors
    ///
    /// Returns the driver's notification-preparation failure unchanged.
    pub fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        Driver::prepare_wait(&mut self.inner)
    }

    /// Closes admission synchronously and transfers ownership into a local drain.
    ///
    /// The driver implements admission closure and safe independent resource ownership. This
    /// consuming transition preserves the owning thread and cannot be restarted on this owner.
    ///
    /// ```compile_fail,E0382
    /// use arty_io_core::{Driver, LocalDriver};
    ///
    /// fn restart<D: Driver>(driver: LocalDriver<D>) {
    ///     let _drain = driver.shutdown();
    ///     let _again = driver.shutdown();
    /// }
    /// ```
    #[must_use = "the drain must be serviced to completion or reported as abandoned"]
    pub fn shutdown(self) -> LocalDrain<D::Drain> {
        LocalDrain {
            inner: self.inner.shutdown(),
            local: PhantomData,
        }
    }
}

impl<D: Driver> fmt::Debug for LocalDriver<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

/// An inline drain confined to its original driver-owning thread.
///
/// Created by consuming [`LocalDriver::shutdown`], not by extracting the running state. Its
/// separate phase type keeps running and draining methods unambiguous even when one concrete
/// type implements both [`Driver`] and [`Drain`].
///
/// This wrapper neither allocates nor erases its value. It is neither [`Send`] nor [`Sync`],
/// but it is not a pinning primitive. The drain retains independent native resource ownership.
/// After completion or either error, the runtime drops this owner and calls neither method
/// again; this wrapper does not add a terminal-state guard.
///
/// ```compile_fail,E0277
/// use arty_io_core::{Drain, LocalDrain};
///
/// fn move_to_another_thread<S: Drain + Send>(drain: LocalDrain<S>) {
///     std::thread::spawn(move || drop(drain));
/// }
/// ```
#[must_use = "the drain must be serviced to completion or reported as abandoned"]
pub struct LocalDrain<S: Drain> {
    inner: S,
    local: PhantomData<Rc<()>>,
}

impl<S: Drain> LocalDrain<S> {
    /// Performs one bounded, non-blocking shutdown-service turn.
    ///
    /// # Errors
    ///
    /// Returns the drain's terminal failure unchanged.
    pub fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        Drain::service(&mut self.inner, budget)
    }

    /// Arms notifications and rechecks shutdown progress before a possible wait.
    ///
    /// # Errors
    ///
    /// Returns the drain's terminal notification-preparation failure unchanged.
    pub fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        Drain::prepare_wait(&mut self.inner)
    }
}

impl<S: Drain> fmt::Debug for LocalDrain<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}
