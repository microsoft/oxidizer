// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::{Context as TaskContext, Poll};

use crate::{DriverContext, Parker, Shutdown};

/// One async worker's adapter to an I/O subsystem.
///
/// A driver is created on the thread that owns it and remains on that thread for its entire
/// lifetime. It deliberately has no [`Send`] or [`Sync`] requirement. A driver may delegate work
/// to threads owned by its provider, use runtime system workers, or process completions directly
/// through its [`Parker`].
///
/// Every runtime callback takes `&self`. A driver uses thread-local interior mutability when a
/// callback changes state, so the runtime never needs mutable driver access or a synchronization
/// wrapper merely to invoke the contract.
///
/// # Context
///
/// A context is the handle through which consumers start I/O operations. Contexts may move
/// between runtime workers and may outlive the driver. Operations attempted after driver shutdown
/// must fail safely.
///
/// # Shutdown safety
///
/// A driver must always be safe to drop, even when shutdown has not completed. If external code or
/// the operating system can still access a resource, dropping the driver must retain that resource
/// rather than invalidate it. [`poll_shutdown`](Self::poll_shutdown) reports graceful cleanup
/// progress; it is never a memory-safety gate.
///
/// Contexts and in-flight operations should own reference-counted handles or pool leases for the
/// state they access. Shutdown closes admission, then waits for those owners to drain. Any unsafe
/// code needed by a platform driver stays private to that implementation rather than spreading
/// into this stable contract or its runtime caller.
pub trait Driver: 'static {
    /// The handle through which consumers start operations on this driver.
    type Context: DriverContext;

    /// Returns a context bound to this driver instance.
    fn context(&self) -> Self::Context;

    /// Returns the completion-aware waiting point for this driver.
    fn parker(&self) -> &dyn Parker;

    /// Prevents new operations from starting and begins graceful cleanup.
    ///
    /// In-flight operations may continue. The runtime calls this method exactly once for each
    /// driver and never calls it again. Implementations do not need to tolerate repeated shutdown
    /// initiation. The method returns promptly without waiting for external progress.
    fn begin_shutdown(&self);

    /// Polls graceful cleanup to completion.
    ///
    /// [`Poll::Ready`] means the driver has released everything it held on behalf of consumers and
    /// the operating system. While returning [`Poll::Pending`], the driver arranges for
    /// `cx.waker()` to be woken when shutdown can make progress.
    ///
    /// The runtime calls [`begin_shutdown`](Self::begin_shutdown) before the first poll and bounds
    /// the total shutdown duration. It may poll repeatedly until completion; calls after completion
    /// return [`Poll::Ready`].
    fn poll_shutdown(&self, cx: &mut TaskContext<'_>) -> Poll<()>;

    /// Returns a future that begins and then polls graceful shutdown.
    ///
    /// Runtime implementations that erase driver types can call
    /// [`begin_shutdown`](Self::begin_shutdown) and [`poll_shutdown`](Self::poll_shutdown)
    /// directly. This adapter is the ergonomic form for callers holding a concrete driver. The
    /// caller ensures no previous shutdown was started for the same driver.
    fn shutdown(&self) -> Shutdown<'_, Self>
    where
        Self: Sized,
    {
        Shutdown::new(self)
    }
}
