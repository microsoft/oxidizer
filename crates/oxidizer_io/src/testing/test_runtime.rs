// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use futures::StreamExt;
use futures::executor::LocalSpawner;
use futures::task::LocalSpawnExt;

use crate::{AsyncTask, Runtime, SystemTaskCategory};

/// I/O subsystem runtime environment adapter for the runtime environment used
/// in the crate's unit tests.
///
/// The `TestRuntime` is the test harness side, with `.client()` used to get one or more clients
/// that implement the I/O trait. This distinction is mostly to guarantee that the test harness
/// is the one that stops/waits on the runtime, matching real world behavior (as opposed to the
/// last `Rc` in wherever doing that, which might happen in some unexpected place like a system
/// task itself).
///
/// On drop, it waits for system tasks to complete and forwards any assertion failures.
/// Note that this treatment does not extend to local async tasks, which are detached.
#[derive(Debug)]
pub struct TestRuntime {
    // We clone this whenever anyone wants one more client. We do not use its functionality
    // ourselves, just keep it for cloning and drop when done (setting to None to release
    // our sender to the work channel).
    client: Option<TestRuntimeClient>,

    worker: Option<JoinHandle<()>>,

    stats: Arc<TestRuntimeStats>,
}

impl TestRuntime {
    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    #[must_use]
    ///
    /// # Panics
    ///
    /// TODO: Document panics
    pub fn new(async_executor: &LocalSpawner) -> Self {
        let (sender, receiver) = mpsc::channel();

        let stats = Arc::new(TestRuntimeStats::default());

        let worker = thread::Builder::new()
            .name("oxidizer-io-test-worker".to_string())
            .spawn({
                let stats = Arc::clone(&stats);
                move || worker_thread_entrypoint(receiver, &stats)
            })
            .expect("Failed to spawn worker thread");

        // We make a thread-safe task queue for spawning because we do not want to (cannot)
        // access the LocalSpawner itself from another thread. This is because the Runtime
        // trait requires Sync - it is possible to spawn tasks from any thread.
        let (spawn_tx, spawn_rx) = async_channel::unbounded::<AsyncTask>();

        let spawner = async_executor.clone();

        async_executor
            .spawn_local(async move {
                let mut spawn_rx = pin!(spawn_rx);

                while let Some(task) = spawn_rx.next().await {
                    spawner.spawn_local(Box::into_pin(task)).unwrap();
                }
            })
            .expect("Failed to spawn async executor thread");

        let client = TestRuntimeClient {
            async_spawn_tx: spawn_tx,
            sender: Some(sender),
            stats: Arc::clone(&stats),
        };

        Self {
            worker: Some(worker),
            client: Some(client),
            stats,
        }
    }

    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    #[must_use]
    pub fn stats(&self) -> Arc<TestRuntimeStats> {
        Arc::clone(&self.stats)
    }

    ///
    /// # Panics
    ///
    /// TODO: Document panics
    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    #[must_use]
    pub fn client(&self) -> TestRuntimeClient {
        self.client
            .as_ref()
            .expect("TestRuntime has already been dropped")
            .clone()
    }
}

impl Drop for TestRuntime {
    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    fn drop(&mut self) {
        // Disconnect our sender (via client) to signal the worker thread to exit.
        // If other clients still exist in memory, they might keep the worker thread alive.
        // In general, a test is expected to stop "doing stuff" so needs to drop that reference.
        self.client.take().expect("double drop");

        // We wait for the worker to exit so we can forward any assertions that happened on the
        // worker thread. In a real app, there is no expectation that the runtime wait for anything
        // like this, this is purely a test-specific behavior to ensure panic messages are visible.
        // Some tests may also take advantage of this for extra depth in whitebox testing.
        self.worker
            .take()
            .expect("double drop")
            .join()
            .expect("Failed to join worker thread");
    }
}

#[derive(Clone, Debug)]
pub struct TestRuntimeClient {
    async_spawn_tx: async_channel::Sender<AsyncTask>,

    // System tasks are executed on a dedicated thread, which consumes entries from this queue.
    // The worker thread exits when the queue becomes disconnected from senders and empty.
    sender: Option<mpsc::Sender<Box<dyn FnOnce() + Send + 'static>>>,

    stats: Arc<TestRuntimeStats>,
}

impl Runtime for TestRuntimeClient {
    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    fn enqueue_system_task(
        &self,
        _category: SystemTaskCategory,
        body: Box<dyn FnOnce() + Send + 'static>,
    ) {
        self.stats
            .system_tasks_enqueued
            .fetch_add(1, Ordering::Relaxed);

        self.sender
            .as_ref()
            .expect("sender is only cleared on drop")
            .send(body)
            .expect("test runtime worker thread exited before test runtime was dropped");
    }

    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    fn enqueue_task(&self, body: AsyncTask) {
        self.async_spawn_tx
            .send_blocking(body)
            .expect("failed to send async task to runtime - did it disappear before we did");
    }
}

#[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
fn worker_thread_entrypoint(
    receiver: mpsc::Receiver<Box<dyn FnOnce() + Send + 'static>>,
    stats: &Arc<TestRuntimeStats>,
) {
    for task in receiver {
        task();

        stats.system_tasks_executed.fetch_add(1, Ordering::Relaxed);
    }
}

/// Exposes data about the work done by the test runtime.
/// Inspecting this data may be useful in tests.
#[derive(Debug, Default)]
pub struct TestRuntimeStats {
    pub system_tasks_enqueued: AtomicUsize,
    pub system_tasks_executed: AtomicUsize,
}