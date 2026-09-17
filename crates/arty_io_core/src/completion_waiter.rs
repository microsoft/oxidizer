// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;

use crate::{CompletionBudget, CompletionDomain, DriverError, ServiceStatus};

/// The host side of a completion domain: native collection, routing, and blocking wait.
///
/// The runtime owns this separately from its drivers. For example, a Windows adapter can
/// collect one shared IOCP and route records into driver-owned mailboxes; a Linux adapter
/// can observe independent ring descriptors and signal the drivers that must drain them.
/// Native client services supplied to drivers carry this waiter's [`domain`](Self::domain).
///
/// Collection delivers records or readiness, not recursive calls to driver service or
/// application futures. Native routing must select the owning registration before interpreting
/// driver-private operation storage. Registrations and native buffers retain their own
/// ownership through cancellation, failure, and late notifications.
///
/// A waiter is used only on its configured owning thread. It has no `Send` or `Sync`
/// requirement. Configuring extra domains, dedicated hosts, or external observers is explicit
/// runtime policy; this trait does not create threads.
pub trait CompletionWaiter: 'static {
    /// Returns the identity shared by this collector and its native client services.
    fn domain(&self) -> &CompletionDomain;

    /// Returns the interruption handle used by the runtime for this collection domain.
    ///
    /// This is distinct from a driver's readiness waker. It latches task, control, or source
    /// activity before or during the next blocking collection. Same-thread wakes are honored,
    /// redundant wakes may coalesce, and the handle remains memory-safe after waiter destruction.
    fn waker(&self) -> Waker;

    /// Collects and routes available activity, optionally waiting for its first arrival.
    ///
    /// `Duration::ZERO` never blocks and never consumes a pending interruption. A positive wait
    /// observes any latched interruption before sleeping. Finite waits may round up to native
    /// precision but must not become unbounded; only `Duration::MAX` permits an unbounded wait.
    ///
    /// Charge the budget before each bounded collection/routing step. An exhausted budget must
    /// not enter a wait. Return runnable when collection needs another turn, including when a
    /// native batch remains without another notification. Do not wait again to fill a batch
    /// after activity has already been collected.
    ///
    /// The runtime calls this without blocking while other work is runnable, and permits a positive
    /// wait only after servicing and arming its drivers and rechecking task and source readiness.
    /// It limits the wait by all relevant deadlines. The implementation must not hold resources
    /// across a wait that submitting or notifying threads need.
    ///
    /// # Errors
    ///
    /// Returns native collection or routing failures. A normal timeout without activity is an
    /// idle result, not a driver failure. Individual operation errors remain in their native
    /// records for the owning driver to deliver.
    fn collect(&mut self, max_wait: Duration, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError>;
}
