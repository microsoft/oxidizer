// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::task::Waker;
use std::time::Duration;

use crate::{IoContext, ShutdownError};

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
/// [`process_completions`](Self::process_completions) access that state through `&mut self`
/// without interior mutability.
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
    type Context: IoContext;

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

    /// Shuts down the driver.
    ///
    /// This method consumes the driver, closes admission to new operations, and blocks while
    /// existing operations and operating-system callbacks drain. Context handles do not
    /// themselves delay shutdown because they remain valid in a closed state.
    ///
    /// The implementation remains responsible for making progress on its own completions and for
    /// bounding the wait. It must not wait indefinitely or depend on work that can run only after
    /// this call returns, such as another driver on the same runtime thread. When graceful cleanup
    /// cannot complete within the driver's policy, it returns an error.
    ///
    /// [`Drop::drop`] still runs on the consumed value after this method returns. Cleanup shared by
    /// shutdown and `Drop` must therefore be idempotent or guarded. A driver that needs ownership
    /// of one of its fields during shutdown can store that field in an [`Option`] and take it
    /// before waiting.
    ///
    /// Regardless of the result, the consumed driver must remain safe to drop and contexts must
    /// reject new operations.
    ///
    /// # Errors
    ///
    /// Returns an error when graceful cleanup exceeds the driver's liveness policy or an
    /// underlying cleanup operation fails.
    fn shutdown(self) -> Result<(), ShutdownError>;
}
