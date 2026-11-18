// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Mutex, Weak};

use arty::runtime::hook::{HookFn, WorkerInit};
use arty::runtime::{RuntimeBuilder, RuntimeControl};
use thread_aware::ThreadAware;
use tokio::runtime::{Handle, Runtime};

use crate::TokioHandle;

enum RuntimeState {
    Uninitialized,
    Running(Runtime),
    ShutDown,
}

#[derive(ThreadAware)]
struct TokioSupport {
    #[thread_aware(skip)]
    handle: Option<Handle>,
    #[thread_aware(skip)]
    owned_runtime: Option<Arc<Mutex<RuntimeState>>>,
    #[thread_aware(skip)]
    runtime: Weak<Mutex<RuntimeState>>,
}

impl Clone for TokioSupport {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
            owned_runtime: None,
            runtime: Weak::clone(&self.runtime),
        }
    }
}

impl Drop for TokioSupport {
    fn drop(&mut self) {
        let Some(owned_runtime) = self.owned_runtime.take() else {
            return;
        };
        let runtime = {
            let mut state = owned_runtime.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            match std::mem::replace(&mut *state, RuntimeState::ShutDown) {
                RuntimeState::Running(runtime) => Some(runtime),
                RuntimeState::Uninitialized | RuntimeState::ShutDown => None,
            }
        };
        if let Some(runtime) = runtime {
            runtime.shutdown_background();
        }
    }
}

impl TokioSupport {
    fn new() -> Self {
        let owned_runtime = Arc::new(Mutex::new(RuntimeState::Uninitialized));
        Self {
            handle: None,
            runtime: Arc::downgrade(&owned_runtime),
            owned_runtime: Some(owned_runtime),
        }
    }

    fn from_handle(tokio: TokioHandle) -> Self {
        let state = tokio.owned_runtime.map_or(RuntimeState::Uninitialized, RuntimeState::Running);
        let owned_runtime = Arc::new(Mutex::new(state));
        Self {
            handle: Some(tokio.handle),
            runtime: Arc::downgrade(&owned_runtime),
            owned_runtime: Some(owned_runtime),
        }
    }

    #[expect(
        clippy::panic,
        reason = "tokio_compat documents runtime creation failure as a panic because its extension contract cannot return Result"
    )]
    fn handle(&self) -> Option<Handle> {
        if let Some(handle) = &self.handle {
            return Some(handle.clone());
        }
        let owned_runtime = self.runtime.upgrade()?;
        let mut state = owned_runtime.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *state {
            RuntimeState::Uninitialized => {
                let runtime = Runtime::new().unwrap_or_else(|error| panic!("failed to create the default Tokio runtime: {error}"));
                let handle = runtime.handle().clone();
                *state = RuntimeState::Running(runtime);
                Some(handle)
            }
            RuntimeState::Running(runtime) => Some(runtime.handle().clone()),
            RuntimeState::ShutDown => None,
        }
    }

    fn enter(&self) {
        if Handle::try_current().is_ok() {
            return;
        }

        // The worker thread owns this context until it exits. Tokio drops the
        // thread-local handle during thread teardown.
        if let Some(handle) = self.handle() {
            std::mem::forget(handle.enter());
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
    ///
    /// If a Tokio context already exists, this method does not create another
    /// runtime.
    ///
    /// # Panics
    ///
    /// Panics if Tokio cannot create the runtime. To handle this failure,
    /// create the runtime with [`tokio::runtime::Builder::build`], then pass it
    /// to [`tokio_compat_with`](Self::tokio_compat_with).
    fn tokio_compat(self) -> Self::Output;

    /// Enters the Tokio runtime context on each applicable Arty worker.
    ///
    /// The first initialization wins on each worker. Later calls do nothing
    /// on that worker, even if they supply a different Tokio runtime.
    /// An existing Tokio context is never replaced.
    ///
    /// If you supply a borrowed handle, keep its Tokio runtime alive while Arty
    /// tasks use Tokio. If you supply an owned runtime, Arty releases it after
    /// [`Runtime::stop`](arty::runtime::Runtime::stop) or runtime destruction
    /// finishes waiting for the workers.
    ///
    /// This crate does not support current-thread Tokio runtimes. If you supply
    /// one, you are responsible for full interoperability with Arty.
    fn tokio_compat_with(self, tokio: impl Into<TokioHandle>) -> Self::Output;
}

impl TokioCompatExt for RuntimeBuilder {
    type Output = Self;

    fn tokio_compat(self) -> Self::Output {
        self.on(worker_init(TokioSupport::new()))
    }

    fn tokio_compat_with(self, tokio: impl Into<TokioHandle>) -> Self::Output {
        self.on(worker_init(TokioSupport::from_handle(tokio.into())))
    }
}

impl TokioCompatExt for &RuntimeControl {
    type Output = ();

    fn tokio_compat(self) {
        self.on(worker_init(TokioSupport::new()));
    }

    fn tokio_compat_with(self, tokio: impl Into<TokioHandle>) {
        self.on(worker_init(TokioSupport::from_handle(tokio.into())));
    }
}

fn worker_init(tokio: TokioSupport) -> HookFn<WorkerInit, TokioSupport> {
    HookFn::new(tokio, enter_worker)
}

fn enter_worker(_event: &mut WorkerInit, tokio: &TokioSupport) {
    tokio.enter();
}

mod private {
    use arty::runtime::{RuntimeBuilder, RuntimeControl};

    pub trait Sealed {}

    impl Sealed for RuntimeBuilder {}
    impl Sealed for &RuntimeControl {}
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{RuntimeState, TokioSupport};
    use crate::TokioHandle;

    #[test]
    fn default_support_does_not_create_another_runtime_inside_tokio() {
        let existing = tokio::runtime::Runtime::new().unwrap();
        let _entered = existing.enter();
        let support = TokioSupport::new();

        support.enter();

        assert!(matches!(
            *support.owned_runtime.as_ref().unwrap().lock().unwrap(),
            RuntimeState::Uninitialized
        ));
    }

    #[test]
    fn worker_clone_does_not_own_the_runtime_state() {
        let support = TokioSupport::new();
        let worker = support.clone();

        assert!(worker.owned_runtime.is_none());
        drop(support);
        assert!(worker.runtime.upgrade().is_none());
        assert!(worker.handle().is_none());
    }

    #[test]
    fn shutdown_prevents_late_default_runtime_creation() {
        let support = TokioSupport::new();
        let worker = support.clone();
        let runtime = worker.runtime.upgrade().unwrap();

        drop(support);

        worker.enter();
        assert!(worker.handle().is_none());
        assert!(matches!(*runtime.lock().unwrap(), RuntimeState::ShutDown));
    }

    #[test]
    fn owned_runtime_drop_does_not_wait_for_blocking_tasks() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (started, task_started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        runtime.spawn_blocking(move || {
            started.send(()).unwrap();
            released.recv().unwrap();
        });
        task_started.recv_timeout(Duration::from_secs(5)).unwrap();
        let support = TokioSupport::from_handle(TokioHandle::from(runtime));
        let (completed, completion) = mpsc::sync_channel(1);
        let drop_thread = std::thread::spawn(move || {
            drop(support);
            completed.send(()).unwrap();
        });

        let result = completion.recv_timeout(Duration::from_secs(1));
        release.send(()).unwrap();
        drop_thread.join().unwrap();

        result.unwrap();
    }
}
