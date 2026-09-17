// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;

use crate::{CompletionBudget, DriverContext, DriverError, ServiceStatus};

/// The host side of native collection: routing, the blocking wait, and client assembly.
///
/// The runtime owns this separately from its drivers. For example, a Windows adapter can collect
/// one shared completion port and route records into driver-owned mailboxes; a Linux adapter can
/// observe independent ring descriptors and signal the drivers that must drain them.
///
/// Collection delivers records or readiness, not recursive calls to driver service or application
/// futures. Native routing must select the owning registration before interpreting driver-private
/// operation storage. Registrations and native buffers retain their own ownership through
/// cancellation, failure, and late notifications.
///
/// A collector is used only on its configured owning thread. It has no `Send` or `Sync`
/// requirement. Configuring extra collectors, dedicated hosts, or external observers is explicit
/// runtime policy; this trait does not create threads.
pub trait CompletionWaiter: 'static {
    /// Returns the interruption handle used by the runtime for this collector.
    ///
    /// This is distinct from a driver's readiness waker. It latches task, control, or source
    /// activity before or during the next blocking collection. Same-thread wakes are honored,
    /// redundant wakes may coalesce, and the handle remains memory-safe after destruction.
    fn waker(&self) -> Waker;

    /// Collects and routes available activity, optionally waiting for its first arrival.
    ///
    /// [`Duration::ZERO`] never blocks and never consumes a pending interruption. A positive wait
    /// observes any latched interruption before sleeping. Finite waits may round up to native
    /// precision but must not become unbounded; only [`Duration::MAX`] permits an unbounded wait.
    ///
    /// Charge the budget before each bounded collection or routing step. An exhausted budget must
    /// not enter a wait. Return [`ServiceStatus::Runnable`] when collection needs another turn,
    /// including when a native batch remains without another notification. Do not wait again to
    /// fill a batch after activity has already been collected.
    ///
    /// Discovery must be fair across native sources. When several registrations are continuously
    /// actionable, a hot peer must not indefinitely delay delivery to or notification of another:
    /// preserve a cursor or equivalent continuation across turns so every actionable source is
    /// eventually served within the budgets the runtime grants.
    ///
    /// The runtime calls this without blocking while other work is runnable, and permits a
    /// positive wait only after servicing and arming its drivers and rechecking task and source
    /// readiness. It limits the wait by all relevant deadlines. The implementation must not hold
    /// resources across a wait that submitting or notifying threads need.
    ///
    /// # Errors
    ///
    /// Returns native collection or routing failures. A normal timeout without activity is an
    /// idle result, not a driver failure. Individual operation errors remain in their native
    /// records for the owning driver to deliver. A collection failure has a different scope than
    /// one driver's failure; the runtime applies an explicit policy to each.
    fn collect(&mut self, max_wait: Duration, budget: &mut CompletionBudget) -> Result<ServiceStatus, DriverError>;

    /// Supplies this collector's own client capabilities to a driver context.
    ///
    /// A runtime holding only `Box<dyn CompletionWaiter>` uses this to let the collector that will
    /// service a worker attach its clients, without naming any native type. The default attaches
    /// nothing, which suits a runtime that performs no native collection.
    ///
    /// This seam establishes the coherent construction path and removes the need for callers to
    /// route opaque client values by hand; it cannot prove native provenance, because clients can
    /// also be supplied directly through
    /// [`DriverContext::with_completion_service`](crate::DriverContext::with_completion_service)
    /// and an adapter's implementation is arbitrary. Native ownership and registration rules
    /// remain the adapter's explicit responsibility.
    ///
    /// # Errors
    ///
    /// Returns a duplicate-client error if a client type was already supplied to this context,
    /// or an adapter failure raised while producing its clients.
    fn attach_clients(&self, context: DriverContext) -> Result<DriverContext, DriverError> {
        Ok(context)
    }
}
