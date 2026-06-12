// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! The [`Runtime`] async-runtime adapter.

use std::future::ready;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyspawn::{JoinHandle, Spawner};
use azure_core::async_runtime::{AbortableTask, AsyncRuntime, SpawnedTask, TaskFuture};
use azure_core::time::Duration;
use futures::future::{AbortHandle, Abortable};
use tick::Clock;

/// An [`AsyncRuntime`] that spawns work on an [`anyspawn::Spawner`] and sleeps
/// on a [`tick::Clock`].
///
/// Construct one from an existing [`Spawner`] and [`Clock`] with
/// [`Runtime::new`] (or via [`From`]), then convert it into an
/// `Arc<dyn AsyncRuntime>` via [`From`] / [`Into`] and install it with
/// [`azure_core::async_runtime::set_async_runtime`].
#[derive(Debug, Clone)]
pub struct Runtime {
    spawner: Spawner,
    clock: Clock,
}

impl Runtime {
    /// Creates a new runtime that spawns work on `spawner` and sleeps on `clock`.
    #[must_use]
    pub const fn new(spawner: Spawner, clock: Clock) -> Self {
        Self { spawner, clock }
    }

    /// Returns a reference to the wrapped [`Spawner`].
    pub const fn spawner(&self) -> &Spawner {
        &self.spawner
    }

    /// Returns a reference to the wrapped [`Clock`].
    #[must_use]
    pub const fn clock(&self) -> &Clock {
        &self.clock
    }
}

impl From<(Spawner, Clock)> for Runtime {
    fn from((spawner, clock): (Spawner, Clock)) -> Self {
        Self::new(spawner, clock)
    }
}

impl From<Runtime> for Arc<dyn AsyncRuntime> {
    fn from(runtime: Runtime) -> Self {
        Arc::new(runtime)
    }
}

impl AsyncRuntime for Runtime {
    fn spawn(&self, f: TaskFuture) -> SpawnedTask {
        // Wrap the task so that `abort` cancels it through `futures`: aborting
        // wakes the spawned task, which resolves, which in turn wakes anyone
        // awaiting the returned `SpawnedTask`.
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        let task = Abortable::new(f, abort_registration);
        let handle = self.spawner.spawn(async move {
            // The `Aborted` result is expected when cancelled and carries no value.
            let _ = task.await;
        });
        Box::pin(RuntimeTask { handle, abort_handle })
    }

    fn sleep(&self, duration: Duration) -> TaskFuture {
        let clock = self.clock.clone();
        Box::pin(async move {
            // `time::Duration` can be negative; clamp such values to zero.
            let duration = std::time::Duration::try_from(duration).unwrap_or_default();
            clock.delay(duration).await;
        })
    }

    fn yield_now(&self) -> TaskFuture {
        std::thread::yield_now();
        Box::pin(ready(()))
    }
}

/// Adapts an [`anyspawn::JoinHandle`] into an [`AbortableTask`].
///
/// Holds the [`AbortHandle`] of the spawned [`Abortable`] task so [`abort`]
/// can cancel work that is still pending and wake anyone awaiting it.
///
/// [`abort`]: AbortableTask::abort
struct RuntimeTask {
    handle: JoinHandle<()>,
    abort_handle: AbortHandle,
}

impl Future for RuntimeTask {
    type Output = Result<(), Box<dyn std::error::Error + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().handle).poll(cx).map(Ok)
    }
}

impl AbortableTask for RuntimeTask {
    fn abort(&self) {
        // Cancels the `Abortable` task, which wakes its executor, completes the
        // join handle, and unblocks any pending waiter on this task.
        self.abort_handle.abort();
    }
}

/// Runs developer-credential commands (e.g. the Azure CLI) on the blocking
/// pool of the [`Spawner`], so credentials like `DeveloperToolsCredential`
/// work on the same runtime as the rest of the SDK.
#[cfg(feature = "azure-identity")]
#[async_trait::async_trait]
impl azure_identity::Executor for Runtime {
    async fn run(&self, program: &std::ffi::OsStr, args: &[&std::ffi::OsStr]) -> std::io::Result<std::process::Output> {
        // The program and arguments are borrowed, so own them before moving the
        // blocking work onto the spawner's pool.
        let program = program.to_os_string();
        let args: Vec<std::ffi::OsString> = args.iter().map(|arg| (*arg).to_os_string()).collect();

        self.spawner
            .spawn_blocking(move || std::process::Command::new(&program).args(&args).output())
            .await
    }
}
