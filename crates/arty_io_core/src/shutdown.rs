// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::type_name;
use std::fmt;

use crate::{CompletionBudget, DriverError, ServiceStatus, WaitStatus};

/// Driver-owned state retained while graceful shutdown makes cooperative progress.
///
/// Admission is already closed when the runtime receives this state. Keep the running driver's
/// source identities, readiness waker, native registrations, and active-operation ownership.
/// Do not require another shutdown call to initiate cancellation.
///
/// Every method runs on the original owning thread. Dropping this state must remain safe even
/// if service fails or the overall shutdown deadline expires. Context clones are not drain participants;
/// operations and callbacks retain ownership of the resources they may still access.
/// Destruction does not wait for I/O, another participant, or an external callback.
pub trait Drain: 'static {
    /// Performs bounded shutdown progress without waiting for new activity.
    ///
    /// Charge the budget before each bounded completion, cancellation, or cleanup step.
    /// Return pending with a runnable status if the allowance expires while work remains.
    /// Pending without a deadline requires an armed notification when progress becomes possible.
    ///
    /// # Errors
    ///
    /// Returns a graceful-cleanup failure. The runtime reports it and continues servicing
    /// other participants rather than treating it as successful completion.
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError>;

    /// Arms notification and rechecks progress before a possible shared wait.
    ///
    /// This is bounded preparation, not a hidden completion or cancellation loop.
    ///
    /// # Errors
    ///
    /// Returns a notification-preparation failure.
    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError>;
}

/// The outcome of one bounded graceful-shutdown turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrainStatus {
    /// Shutdown needs further service, notifications, or a deadline.
    Pending(ServiceStatus),
    /// Admitted operations and callbacks reached the adapter's safe retirement boundary.
    Complete,
}

/// An owned, thread-local graceful-shutdown operation.
///
/// This is driven by the coordinator, not a blocking call or a `Future`. It shares the
/// running-driver budget and arming protocol, so one drain cannot monopolize the worker.
/// The runtime keeps collecting native activity and servicing other drivers and drains.
/// A new shutdown is initially runnable, even if no native notification has arrived.
///
/// Completion or an error from either operation releases the drain object. Abandoning
/// this handle before that point is also memory-safe by the [`Drain`] contract, but does not
/// claim graceful cleanup. The runtime bounds the aggregate drain with its shutdown policy.
#[must_use = "shutdown must be driven to completion or reported as abandoned"]
pub struct Shutdown {
    drain: Option<Box<dyn Drain>>,
}

impl Shutdown {
    /// Owns a drain whose driver has already closed admission synchronously.
    pub fn new(drain: impl Drain) -> Self {
        Self {
            drain: Some(Box::new(drain)),
        }
    }

    /// Performs one bounded, non-blocking shutdown-service turn.
    ///
    /// # Errors
    ///
    /// Returns and preserves the drain's terminal failure.
    ///
    /// # Panics
    ///
    /// Panics if called after service returned `Complete`, or either operation returned an error.
    pub fn service(&mut self, budget: &mut CompletionBudget) -> Result<DrainStatus, DriverError> {
        let result = self
            .drain
            .as_mut()
            .expect("shutdown service was called after a terminal result")
            .service(budget);
        if !matches!(result, Ok(DrainStatus::Pending(_))) {
            drop(self.drain.take());
        }
        result
    }

    /// Arms notifications and rechecks progress before a possible shared wait.
    ///
    /// # Errors
    ///
    /// Returns a terminal notification-preparation failure and releases the drain. The runtime
    /// reports the failure while preserving the independent ownership of native state.
    ///
    /// # Panics
    ///
    /// Panics if called after service returned `Complete`, or either operation returned an error.
    pub fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError> {
        let result = self
            .drain
            .as_mut()
            .expect("shutdown preparation was called after a terminal result")
            .prepare_wait();
        if result.is_err() {
            drop(self.drain.take());
        }
        result
    }
}

impl fmt::Debug for Shutdown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("active", &self.drain.is_some())
            .finish_non_exhaustive()
    }
}
