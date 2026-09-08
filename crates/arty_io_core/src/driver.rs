// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use crate::DriverContext;

/// One async worker's adapter to an I/O subsystem.
///
/// A driver is created on the thread that owns it and remains on that thread for its entire
/// lifetime. It deliberately has no [`Send`] or [`Sync`] requirement. A driver may delegate work
/// to threads owned by its provider, use runtime system workers, or process completions directly
/// on its owning thread.
///
/// # Context
///
/// A context is the handle through which consumers start I/O operations. Contexts may move
/// between runtime workers and may outlive the driver. Operations attempted after driver shutdown
/// must fail safely.
///
/// # Driver state
///
/// State reached by contexts, wakers, background threads, or operating-system callbacks is shared
/// independently of the driver and uses appropriate reference counting and synchronization. Each
/// in-flight operation owns every resource it uses through a reference count, pool lease, or
/// equivalent handle.
///
/// Completion buffers, queue-reader state, batching state, and lifecycle state used only on the
/// owning thread remain ordinary driver fields. Exclusive runtime ownership lets
/// [`process_completions`](Self::process_completions),
/// [`begin_shutdown`](Self::begin_shutdown), and [`poll_shutdown`](Self::poll_shutdown) access
/// that state through `&mut self` without interior mutability.
///
/// # Shutdown safety
///
/// A driver must always be safe to drop, even when shutdown has not completed. Dropping a driver
/// closes admission if necessary. If external code or the operating system can still access a
/// resource, dropping the driver must retain that resource rather than invalidate it.
///
/// Contexts remain valid after their driver is gone and reject new operations once admission is
/// closed. In-flight operations own reference-counted handles, pool leases, or equivalent safe
/// ownership for every resource they access. When an operating system retains only a raw pointer
/// into pooled storage, the implementation must retain the storage owner independently of the
/// driver. Any unsafe platform code stays private to that implementation rather than spreading
/// into this stable contract or its runtime caller.
pub trait Driver: 'static {
    /// The handle through which consumers start operations on this driver.
    type Context: DriverContext;

    /// Returns a context bound to this driver instance.
    ///
    /// After shutdown starts, the returned context is closed and rejects new operations.
    #[must_use]
    fn context(&self) -> Self::Context;

    /// Waits for and processes completion events.
    ///
    /// [`Duration::ZERO`] requests a non-blocking poll and [`Duration::MAX`] an unbounded wait.
    /// A mechanism with coarser timing rounds finite waits up without converting one into an
    /// unbounded wait.
    ///
    /// Operations may be submitted from other threads while this method waits. The implementation
    /// must not hold anything across the wait that a submitter needs.
    fn process_completions(&mut self, max_wait: Duration);

    /// Returns a handle that causes the current or next completion wait to return.
    ///
    /// Wake-ups are latched: a wake raised before a wait makes the next wait return immediately.
    /// Same-thread wake-ups are honored. Redundant wake-ups may be coalesced, but a wake-up is never
    /// dropped. The returned waker remains safe to invoke after the driver is dropped.
    #[must_use]
    fn waker(&self) -> Waker;

    /// Prevents new operations from starting and begins graceful cleanup.
    ///
    /// In-flight operations may continue. This method is idempotent and closes admission before
    /// returning.
    fn begin_shutdown(&mut self);

    /// Reports graceful drain progress.
    ///
    /// [`Poll::Ready`] means the driver has released everything it held on behalf of active
    /// operations and the operating system. Context handles do not by themselves prevent
    /// completion because they remain valid in a closed state.
    ///
    /// Calling this method before [`begin_shutdown`](Self::begin_shutdown) begins shutdown as if
    /// that method had been called first.
    ///
    /// While returning [`Poll::Pending`], the driver arranges for `cx.waker()` to be woken when
    /// shutdown can make progress. Calls after completion continue to return [`Poll::Ready`].
    /// The runtime bounds the total shutdown duration. Completion is a liveness signal and never
    /// authorizes otherwise-unsafe destruction.
    fn poll_shutdown(&mut self, cx: &mut TaskContext<'_>) -> Poll<()>;
}
