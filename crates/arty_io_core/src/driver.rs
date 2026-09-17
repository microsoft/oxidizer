// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{CompletionBudget, Drain, DriverError, ServiceStatus, WaitStatus};

/// One worker's non-blocking participant in an I/O completion arrangement.
///
/// The runtime owns the concrete driver through [`LocalDriver`](crate::LocalDriver) on its final
/// owning thread. That inline owner is neither [`Send`] nor [`Sync`] even when the implementation
/// is thread-safe. Service is statically dispatched; a heterogeneous runtime may choose its own
/// private erasure boundary without imposing allocation or virtual dispatch on other runtimes.
///
/// The consumer handle is not obtained from the driver:
/// [`DriverProvider::create`](crate::DriverProvider::create) returns it together with the local
/// owner. Contexts may move between workers and outlive the driver, and
/// relocation is an optimization rather than a correctness condition. Native client capabilities
/// and the readiness waker arrive through [`DriverContext`](crate::DriverContext).
///
/// Driver implementations need neither `Send` nor `Sync`.
///
/// # Ownership
///
/// Operation and buffer representations stay private to the driver. Native adapters route a
/// record to its owning registration before private decoding, or notify a driver that must drain
/// its own queue. In-flight operations, callbacks, and native registrations independently own
/// every resource they may still access.
/// Thread confinement is not pinning: inline owners may move within their owning thread.
/// Callback-visible addresses must belong to independently stable, pinned, or otherwise safely
/// owned storage rather than relying on the address of this driver.
///
/// Dropping a driver is always memory-safe and closes admission if needed. If native code still
/// holds raw pointers, retain their backing owners rather than invalidate them on early drop.
/// Destruction must not wait for I/O, other drivers, or callbacks; transfer or retain ownership
/// for independently driven cleanup when necessary.
/// Retained contexts remain closed handles; their existence does not itself delay shutdown.
/// Private platform code may use unsafe mechanisms, but callers of this contract have no unsafe
/// implementation or inertness-checking obligation.
pub trait Driver: Sized + 'static {
    /// The concrete state that makes cooperative progress after admission closes.
    type Drain: Drain;

    /// Performs one bounded service turn without waiting for new I/O.
    ///
    /// Charge the budget before each completion or other bounded progress step. Perform required
    /// submission flushing and native task work; do not assume that checking queue entries alone
    /// makes every backend progress. Keep operation completion and task wakes on their supported
    /// execution paths.
    ///
    /// Return [`ServiceStatus::Runnable`] if work remains after the budget is exhausted, even if
    /// no new notification will arrive. Otherwise report a required service deadline or
    /// [`ServiceStatus::Idle`]. New activity after an idle result must signal the supplied
    /// readiness waker. The runtime schedules an initial service turn after creation even without
    /// a notification.
    ///
    /// # Errors
    ///
    /// Returns submission-progress, completion-processing, or driver failures. A failure is not
    /// an idle result; the runtime reports it and applies its source failure policy. Individual
    /// I/O results, including operation errors, stay on the context's result path and do not by
    /// themselves fail this service participant.
    fn service(&mut self, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError>;

    /// Arms notifications and rechecks private work immediately before a possible wait.
    ///
    /// [`WaitStatus::Armed`] means notifications are enabled and the driver rechecked that no
    /// immediate work remains. Activity appearing afterward signals the supplied readiness waker.
    /// Return [`WaitStatus::WorkReady`] if work is already present or preparation needs another
    /// budgeted service turn.
    ///
    /// This method only performs bounded notification preparation. Draining and cancellation use
    /// budgeted service. The runtime separately latches source readiness, arms its own wait, and
    /// rechecks task and control activity before blocking.
    ///
    /// # Errors
    ///
    /// Returns native notification-preparation failures.
    fn prepare_wait(&mut self) -> Result<WaitStatus, DriverError>;

    /// Closes admission synchronously and transfers ownership into cooperative draining.
    ///
    /// Admission is already closed when this method returns, before the first drain-service turn.
    /// Racing operations are either admitted and included in the drain or rejected. Do not block
    /// or perform an unbounded cancellation loop here.
    ///
    /// The returned drain retains the same registrations and readiness waker. The runtime keeps
    /// collecting activity and drives all drains fairly with completion budgets until they
    /// complete, fail, or reach its overall shutdown deadline. System work remains available
    /// throughout this phase.
    ///
    /// Consuming the driver makes initiation exactly once. Its local owner wraps the returned
    /// concrete state in [`LocalDrain`](crate::LocalDrain) before returning it to the runtime.
    /// Dropping the returned drain early must remain memory-safe; graceful cleanup is never a
    /// precondition for safe destruction.
    #[must_use = "the drain must be serviced to completion or reported as abandoned"]
    fn shutdown(self) -> Self::Drain;
}
