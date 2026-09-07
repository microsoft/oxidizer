// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

use crate::{DriverContext, Parker};

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
/// rather than invalidate it. The future returned by [`begin_shutdown`](Self::begin_shutdown)
/// reports graceful cleanup progress; it is never a memory-safety gate.
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
    /// initiation. Calling the method closes admission before it returns.
    ///
    /// The returned future resolves once the driver has released everything it held on behalf of
    /// consumers and the operating system. While pending, it arranges for the task waker to be
    /// notified when shutdown can make progress. The runtime bounds the total shutdown duration.
    /// The boxed return keeps this method object-safe, so runtimes can store
    /// `Box<dyn Driver<Context = C>>`.
    fn begin_shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + '_>>;
}
