// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CompletionBudget, DriverError, ServiceStatus, WaitStatus};

/// Driver-owned state retained while graceful shutdown makes cooperative progress.
///
/// [`Driver::shutdown`](crate::Driver::shutdown) consumes the running driver and returns this
/// state as a boxed local trait object. Admission is already closed when the runtime receives it.
/// A drain keeps the running driver's source identities, readiness waker, native registrations,
/// and active-operation ownership, and does not require another initiation call.
///
/// A new drain is initially runnable: the runtime gives it a service turn before it can park,
/// even if no native notification has arrived.
///
/// The runtime owns terminal removal. After [`DrainStatus::Complete`] or an error from either
/// method, the runtime drops the drain and calls neither method again; the core provides no
/// wrapper that re-checks this.
///
/// Every method runs on the original owning thread. Dropping this state must remain safe even if
/// service fails or the overall shutdown deadline expires. Context clones are not drain
/// participants; operations and callbacks retain ownership of the resources they may still
/// access. Destruction does not wait for I/O, another participant, or an external callback.
pub trait Drain: 'static {
    /// Performs bounded shutdown progress without waiting for new activity.
    ///
    /// Charge the budget before each bounded completion, cancellation, or cleanup step.
    /// Return [`DrainStatus::Pending`] with [`ServiceStatus::Runnable`] if the allowance expires
    /// while work remains. Pending without a deadline requires an armed notification when
    /// progress becomes possible.
    ///
    /// # Errors
    ///
    /// Returns a graceful-cleanup failure. The runtime reports it and continues servicing other
    /// participants rather than treating it as successful completion.
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
