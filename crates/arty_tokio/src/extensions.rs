// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty::runtime::hook::{HookFn, WorkerInit};
use arty::runtime::{RuntimeBuilder, RuntimeControl};
use thread_aware::ThreadAware;
use tokio::runtime::Handle;

use crate::TokioHandle;

#[derive(ThreadAware)]
struct TokioSupport {
    #[thread_aware(skip)]
    handle: Handle,
    #[thread_aware(skip)]
    owned_runtime: Option<tokio::runtime::Runtime>,
}

impl Clone for TokioSupport {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            owned_runtime: None,
        }
    }
}

impl Drop for TokioSupport {
    fn drop(&mut self) {
        if let Some(runtime) = self.owned_runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Adds Tokio support to an Arty runtime.
///
/// The implementation for [`RuntimeBuilder`] configures workers before
/// startup. The implementation for a [`RuntimeControl`] reference configures
/// live workers and workers that start later.
pub trait TokioCompatExt: private::Sealed {
    /// The result of adding Tokio support.
    type Output;

    /// Enters a new default Tokio runtime context on each applicable Arty
    /// worker.
    fn tokio_compat(self) -> Self::Output
    where
        Self: Sized,
    {
        self.tokio_compat_with(TokioHandle::default())
    }

    /// Enters the Tokio runtime context on each applicable Arty worker.
    ///
    /// The first initialization wins on each worker. Later calls do nothing
    /// on that worker, even if they supply a different Tokio runtime.
    /// An existing Tokio context is never replaced.
    ///
    /// If you supply a borrowed handle, keep its Tokio runtime alive while
    /// Arty tasks use Tokio. If you supply an owned runtime, Arty owns it until
    /// shutdown starts.
    /// For a current-thread runtime, also run
    /// [`Runtime::block_on`](tokio::runtime::Runtime::block_on) on another thread
    /// while Arty tasks use Tokio. Entering its handle does not drive Tokio
    /// tasks, timers, or I/O.
    fn tokio_compat_with(self, tokio: impl Into<TokioHandle>) -> Self::Output;
}

impl TokioCompatExt for RuntimeBuilder {
    type Output = Self;

    fn tokio_compat_with(self, tokio: impl Into<TokioHandle>) -> Self::Output {
        self.on(worker_init(tokio.into()))
    }
}

impl TokioCompatExt for &RuntimeControl {
    type Output = ();

    fn tokio_compat_with(self, tokio: impl Into<TokioHandle>) {
        self.on(worker_init(tokio.into()));
    }
}

fn worker_init(tokio: TokioHandle) -> HookFn<WorkerInit, TokioSupport> {
    HookFn::new(
        TokioSupport {
            handle: tokio.handle,
            owned_runtime: tokio.owned_runtime,
        },
        enter_worker,
    )
}

fn enter_worker(_event: &mut WorkerInit, tokio: &TokioSupport) {
    if Handle::try_current().is_ok() {
        return;
    }

    // The worker thread owns this context until it exits. Tokio drops the
    // thread-local handle during thread teardown.
    std::mem::forget(tokio.handle.enter());
}

mod private {
    use arty::runtime::{RuntimeBuilder, RuntimeControl};

    pub trait Sealed {}

    impl Sealed for RuntimeBuilder {}
    impl Sealed for &RuntimeControl {}
}
