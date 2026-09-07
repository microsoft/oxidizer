// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::{Context as TaskContext, Poll, Waker};

use crate::{Parker, Shutdown, ThreadAware};

/// One async worker's adapter to an I/O subsystem.
///
/// A driver is created on the thread that owns it and remains on that thread for its entire
/// lifetime. It deliberately has no [`Send`] or [`Sync`] requirement. A driver may delegate all
/// work to threads owned by its provider, use runtime blocking workers, or expose a [`Parker`] so
/// the runtime worker drives completions while waiting.
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
pub trait Driver: 'static {
    /// The handle through which consumers start operations on this driver.
    type Context: Clone + ThreadAware + 'static;

    /// Returns a context bound to this driver instance.
    fn context(&self) -> Self::Context;

    /// Returns the waiting point this driver chose to provide, if any.
    ///
    /// A driver returns `Some` only when creation reported that a waiting point was
    /// [`Available`](crate::WaitingPoint::Available). Returning `None` means the driver arranges
    /// progress through runtime blocking workers or threads of its own.
    fn parker(&mut self) -> Option<&mut dyn Parker> {
        None
    }

    /// Prevents new operations from starting and begins graceful cleanup.
    ///
    /// In-flight operations may continue. This method is idempotent and returns promptly without
    /// waiting for external progress.
    fn begin_shutdown(&mut self);

    /// Polls graceful cleanup to completion.
    ///
    /// [`Poll::Ready`] means the driver has released everything it held on behalf of consumers and
    /// the operating system. While returning [`Poll::Pending`], the driver arranges for
    /// `cx.waker()` to be woken when shutdown can make progress.
    ///
    /// The runtime calls [`begin_shutdown`](Self::begin_shutdown) before the first poll and bounds
    /// the total shutdown duration. Repeated calls after completion return [`Poll::Ready`].
    fn poll_shutdown(&mut self, cx: &mut TaskContext<'_>) -> Poll<()>;

    /// Returns a future that begins and then polls graceful shutdown.
    ///
    /// Runtime implementations that erase driver types can call
    /// [`begin_shutdown`](Self::begin_shutdown) and [`poll_shutdown`](Self::poll_shutdown)
    /// directly. This adapter is the ergonomic form for callers holding a concrete driver.
    fn shutdown(&mut self) -> Shutdown<'_, Self>
    where
        Self: Sized,
    {
        Shutdown::new(self)
    }

    /// Returns a handle that wakes this driver when it does not expose a [`Parker`].
    ///
    /// Drivers whose progress is entirely self-scheduled may return a no-op waker. The returned
    /// waker remains safe to invoke after the driver is dropped.
    fn waker(&self) -> Waker;
}
