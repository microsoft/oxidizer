// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! System-task admission and blocking-pool execution.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use observed::{Sink, emit};
use threadpool::ThreadPool;

use crate::rt::task::execution::prepare_system;
use crate::rt::task::join::JoinHandle;
use crate::rt::telemetry::events::{SystemMetricCount, SystemWorkerPoolSaturated};

const ERR_POISONED_LOCK: &str = "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";

/// Worker for system tasks. Meant to be created for each async worker thread to allow for scheduling of system tasks.
#[derive(Debug)]
pub(crate) struct SystemWorker {
    // Naive implementation using a per-async-worker thread pool
    pool: WorkerPool,
    is_shutting_down: AtomicBool,
    sink: Sink,
}

impl SystemWorker {
    pub(in crate::rt::runtime) fn new(pool: WorkerPool, sink: Sink) -> Arc<Self> {
        Arc::new(Self {
            pool,
            is_shutting_down: AtomicBool::new(false),
            sink,
        })
    }

    /// Submits a system task to the worker. If the worker is shutting down, the task will be ignored.
    pub(crate) fn spawn_system<F, R>(&self, body: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (task, join_handle) = prepare_system(body);

        if !self.is_shutting_down.load(Ordering::Acquire) && self.pool.execute(task) && self.pool.is_overloaded() && !self.pool.grow() {
            emit!(
                &self.sink,
                SystemWorkerPoolSaturated {
                    max_threads: SystemMetricCount::from(self.pool.max_thread_count()),
                }
            );
        }

        join_handle
    }

    /// After shutting down, no new tasks will be accepted, but existing tasks will continue to run.
    pub(in crate::rt::runtime) fn shutdown(&self) {
        self.is_shutting_down.store(true, Ordering::Release);
    }

    /// Waits for the currently running system tasks to complete.
    #[cfg_attr(test, mutants::skip)] // Impractical to test without overly-expensive timeout logic.
    pub(in crate::rt::runtime) fn join(&self) {
        self.pool.join();
    }
}

#[derive(Debug, Clone)]
pub(in crate::rt::runtime) struct WorkerPool {
    pool: Arc<Mutex<Option<ThreadPool>>>,
    max_thread_count: usize,
}

impl WorkerPool {
    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)]
    pub(in crate::rt::runtime) fn shares_pool_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pool, &other.pool)
    }

    /// Initial number of threads in the pool.
    /// It should be reasonable to start at one thread and ramp up the number if necessary.
    const INITIAL_THREAD_COUNT: usize = 1;

    /// Queue-growth heuristic following the [`blocking`](https://github.com/smol-rs/blocking/blob/master/src/lib.rs) pool.
    const MAX_TASKS_PER_THREAD: usize = 5;

    /// Default per-pool thread limit for blocking system tasks.
    const MAX_THREAD_COUNT_DEFAULT: usize = 64;

    pub(in crate::rt::runtime) fn new(max_thread_count: Option<usize>) -> Self {
        // Start with one thread and let the pool grow as needed.
        let thread_pool = ThreadPool::with_name("oxidizer-sys".to_string(), Self::INITIAL_THREAD_COUNT);

        Self {
            pool: Arc::new(Mutex::new(Some(thread_pool))),
            max_thread_count: max_thread_count.unwrap_or(Self::MAX_THREAD_COUNT_DEFAULT),
        }
    }

    pub(in crate::rt::runtime) fn execute<F>(&self, f: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        let pool = self.pool.lock().expect(ERR_POISONED_LOCK);
        let Some(pool) = pool.as_ref() else {
            // Shutdown closed the shared pool before this racing submission arrived.
            return false;
        };
        pool.execute(f);
        true
    }

    #[cfg_attr(test, mutants::skip)] // Impractical to test without overly-expensive timeout logic.
    fn join(&self) {
        // Do not keep the pool alive through retained scheduler handles, and do not
        // hold its submission lock while waiting for user work to finish.
        let pool = self.pool.lock().expect(ERR_POISONED_LOCK).take();
        if let Some(pool) = pool {
            pool.join();
        }
    }

    /// Grows the pool by one thread if possible, returning whether it grew.
    ///
    /// Returns `false` when the pool is already at its maximum permitted size.
    fn grow(&self) -> bool {
        let mut pool = self.pool.lock().expect(ERR_POISONED_LOCK);
        let Some(pool) = pool.as_mut() else {
            return false;
        };
        let new_thread_count = pool.max_count().saturating_add(1);

        if new_thread_count > self.max_thread_count {
            return false;
        }
        pool.set_num_threads(new_thread_count);
        true
    }

    fn max_thread_count(&self) -> usize {
        self.max_thread_count
    }

    fn is_overloaded(&self) -> bool {
        let pool = self.pool.lock().expect(ERR_POISONED_LOCK);
        pool.as_ref().is_some_and(|pool| {
            pool.active_count().saturating_add(pool.queued_count()) > pool.max_count().saturating_mul(Self::MAX_TASKS_PER_THREAD)
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
pub(super) mod system_worker_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use events_once::Event;
    use observed::{Sink, Value};
    use observed_testing::{CapturedEvent, TEST_ID, test_emitter};
    use testing_aids::execute_or_abandon;

    use crate::rt::runtime::system_worker::{SystemWorker, WorkerPool};

    #[cfg_attr(test, mutants::skip)]
    pub(in crate::rt::runtime) fn is_system_worker_shutting_down(worker: &SystemWorker) -> bool {
        worker.is_shutting_down.load(Ordering::Acquire)
    }

    #[test]
    fn system_worker_join_waits_for_tasks_to_complete() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (task_start_tx, task_start_rx) = channel();

        let events_clone = Arc::clone(&events);
        let thread_join_handle = thread::spawn(move || {
            let worker = SystemWorker::new(WorkerPool::new(None), Sink::noop());

            let events_clone2 = Arc::clone(&events_clone);
            drop(worker.spawn_system(move || {
                events_clone2.lock().unwrap().push("task started");
                task_start_tx.send(()).unwrap();
                events_clone2.lock().unwrap().push("task finished");
            }));

            // The sender lives only inside the system task. A worker that never runs
            // the task drops it (and its sender) instead, so `recv` observes an
            // immediate disconnect and this test fails deterministically rather than
            // hanging.
            task_start_rx.recv().expect("the worker must run the system task");

            worker.shutdown();
            worker.join();
            events_clone.lock().unwrap().push("worker joined");
        });

        execute_or_abandon(|| {
            thread_join_handle.join().unwrap();
        })
        .unwrap();

        assert_eq!(
            events.lock().unwrap().as_slice(),
            &["task started", "task finished", "worker joined"]
        );
    }

    #[test]
    fn system_worker_shutdown_prevents_new_tasks() {
        struct WorkItem {
            dropped: Arc<AtomicBool>,
            work_done: Arc<AtomicBool>,
        }

        impl WorkItem {
            fn do_work(&self) {
                self.work_done.store(true, Ordering::Release);
            }
        }

        impl Drop for WorkItem {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let work_done = Arc::new(AtomicBool::new(false));

        let worker = SystemWorker::new(WorkerPool::new(None), Sink::noop());

        worker.shutdown();

        let work_item = WorkItem {
            dropped: Arc::clone(&dropped),
            work_done: Arc::clone(&work_done),
        };

        let task = worker.spawn_system(move || {
            work_item.do_work();
        });

        assert!(dropped.load(Ordering::Acquire));
        assert!(!work_done.load(Ordering::Acquire));
        drop(task);
    }

    #[test]
    fn worker_pool_grow_increases_thread_count() {
        let worker_pool = WorkerPool::new(None);
        assert!(worker_pool.grow());
        assert_eq!(
            worker_pool.pool.lock().unwrap().as_ref().unwrap().max_count(),
            WorkerPool::INITIAL_THREAD_COUNT + 1,
            "Number of worker threads should be increased by 1"
        );
    }

    #[test]
    fn worker_pool_grow_to_maximum() {
        let worker_pool = WorkerPool::new(Some(5));

        // It says "max" but it is effectively the "current" count because it grows asynchronously.
        assert_eq!(worker_pool.pool.lock().unwrap().as_ref().unwrap().max_count(), 1);
        assert!(worker_pool.grow()); // 2
        assert!(worker_pool.grow()); // 3
        assert!(worker_pool.grow()); // 4
        assert!(worker_pool.grow()); // 5
        assert!(!worker_pool.grow()); // 5 - should not grow further

        // It says "max" but it is effectively the "current" count because it grows asynchronously.
        assert_eq!(worker_pool.pool.lock().unwrap().as_ref().unwrap().max_count(), 5);
    }

    #[test]
    fn worker_pool_reports_configured_max_thread_count() {
        // A configured value distinct from both 1 and the initial thread count keeps
        // this assertion honest: the getter must return the exact maximum it was
        // built with, and the default path must fall back to the crate default.
        assert_eq!(WorkerPool::new(Some(7)).max_thread_count(), 7);
        assert_eq!(WorkerPool::new(None).max_thread_count(), WorkerPool::MAX_THREAD_COUNT_DEFAULT);
    }

    #[test]
    fn closed_pool_rejects_work_and_cannot_grow() {
        let pool = WorkerPool::new(Some(2));
        pool.join();
        let invoked = Arc::new(AtomicBool::new(false));
        let captured = Arc::clone(&invoked);
        assert!(!pool.execute(move || captured.store(true, Ordering::Relaxed)));
        assert!(!invoked.load(Ordering::Relaxed));
        assert!(!pool.grow());
        assert!(!pool.is_overloaded());
    }

    #[cfg(not(miri))]
    #[test]
    fn runtime_releases_pool_even_when_a_scheduler_is_retained() {
        execute_or_abandon(|| {
            let runtime = crate::rt::Runtime::builder()
                .processor_count(crate::rt::config::ProcessorCount::exactly(std::num::NonZeroUsize::MIN))
                .build()
                .unwrap();
            let scheduler = runtime.task_scheduler().spawn(async |cx| cx.scheduler().clone()).wait();
            let worker = Arc::clone(scheduler.system_worker());
            drop(runtime);
            assert!(worker.pool.pool.lock().unwrap().is_none());
        })
        .unwrap();
    }

    #[test]
    fn spawn_on_worker() {
        let worker_pool = WorkerPool::new(None);
        let (sender, receiver) = channel();
        worker_pool.execute(move || {
            sender.send("joined").unwrap();
        });

        // The sender lives only inside the dispatched task. A pool that never runs
        // the task drops it (and its sender) instead, so `recv` observes an
        // immediate disconnect and this test fails deterministically rather than
        // hanging.
        let res = receiver.recv().expect("the worker pool must run the dispatched task");
        assert_eq!(res, "joined");
    }

    #[test]
    fn overload_worker() {
        execute_or_abandon(move || {
            let worker_pool = WorkerPool::new(None);
            let (worker_thread_started_tx, worker_thread_started_rx) = channel();
            let (worker_thread_finished_tx, worker_thread_finished_rx) = Event::<()>::boxed();

            // These are only used by the first thread but the compiler does not know that, so we
            // wrap them in Arc + Mutex + Option to make it safe.
            let worker_thread_finished_rx = Arc::new(Mutex::new(Some(worker_thread_finished_rx)));

            for i in 0..WorkerPool::MAX_TASKS_PER_THREAD {
                worker_pool.execute({
                    let worker_thread_started_tx = worker_thread_started_tx.clone();
                    let worker_thread_finished_rx = Arc::clone(&worker_thread_finished_rx);
                    move || {
                        if i == 0 {
                            // This is the first spawned task on a single threaded pool.
                            // We need to wait for it to start and then
                            // block it, so we can schedule further tasks and overload the pool.
                            worker_thread_started_tx.send(()).unwrap();

                            futures::executor::block_on(async {
                                let rx = worker_thread_finished_rx.lock().unwrap().take().unwrap();
                                rx.await.unwrap();
                            });
                        }
                    }
                });
            }

            // Drop every sender we still hold so a pool that never runs the first task
            // leaves the receiver with no senders at all. `recv` then observes an
            // immediate disconnect and this test fails deterministically rather than
            // hanging.
            drop(worker_thread_started_tx);
            drop(worker_thread_finished_rx);

            // First barrier: wait for the thread pool to start the first task.
            worker_thread_started_rx.recv().expect("the worker pool must start the first task");

            assert!(
                !worker_pool.is_overloaded(),
                "Worker pool should be at the task limit but not overloaded"
            );

            // This task can be empty, the blocked thread pool won't have a chance to start it anyway
            worker_pool.execute(move || {});

            assert!(worker_pool.is_overloaded(), "Worker pool should be overloaded");

            // Now we can release the second barrier and clean up
            worker_thread_finished_tx.send(());
            worker_pool.join();
        })
        .unwrap();
    }

    #[cfg_attr(test, mutants::skip)] // Test-only helper.
    fn dimension(event: &CapturedEvent, key: &str) -> Option<Value> {
        event.dimensions().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Runs `spawn_system` against a pool of the given size, holding one worker
    /// thread busy while `queued_task_count` further tasks are spawned, and returns
    /// the telemetry captured while the saturation guard was evaluated on each spawn.
    #[cfg_attr(test, mutants::skip)] // Test-only helper.
    fn spawn_system_telemetry(max_thread_count: Option<usize>, queued_task_count: usize) -> Vec<CapturedEvent> {
        let (sink, processor) = test_emitter(TEST_ID);
        let worker = SystemWorker::new(WorkerPool::new(max_thread_count), sink);

        // Occupy the pool's single initial thread with a task that blocks until released,
        // so every task spawned afterwards stays queued and counts towards the overload
        // threshold instead of completing and disappearing.
        let (blocker_started_tx, blocker_started_rx) = channel();
        let (release_tx, release_rx) = Event::<()>::boxed();
        let release_rx = Arc::new(Mutex::new(Some(release_rx)));

        drop(worker.spawn_system({
            let release_rx = Arc::clone(&release_rx);
            move || {
                blocker_started_tx.send(()).unwrap();
                futures::executor::block_on(async {
                    let release_rx = release_rx.lock().unwrap().take().unwrap();
                    release_rx.await.unwrap();
                });
            }
        }));

        // The sender lives only inside the blocker task. A worker that never runs the
        // task drops it (and its sender) instead, so `recv` observes an immediate
        // disconnect and the saturation tests fail deterministically rather than
        // hanging.
        blocker_started_rx.recv().expect("the worker must run the blocker task");

        // Each spawn re-evaluates the saturation guard; capture whatever it emits.
        for _ in 0..queued_task_count {
            drop(worker.spawn_system(|| {}));
        }

        let events = processor.events();

        // Release the blocker so the queued tasks drain, then wait for the pool to settle.
        release_tx.send(());
        worker.shutdown();
        worker.join();

        events
    }

    #[test]
    fn spawn_system_reports_saturation_when_overloaded_pool_cannot_grow() {
        // A pool capped at one thread cannot grow, so overloading it must surface saturation.
        let events = spawn_system_telemetry(Some(1), WorkerPool::MAX_TASKS_PER_THREAD + 1);

        let saturated: Vec<_> = events
            .iter()
            .filter(|event| event.name() == "oxidizer.rt.system_worker.pool_saturated")
            .collect();
        assert!(!saturated.is_empty(), "an overloaded pool that cannot grow must report saturation");
        assert_eq!(
            dimension(saturated[0], "system_worker_pool.max_threads"),
            Some("1".into()),
            "the saturation event reports the pool's maximum thread count"
        );
    }

    #[test]
    fn spawn_system_stays_quiet_when_maxed_out_pool_is_not_overloaded() {
        // A pool at its maximum size that is not overloaded must stay quiet: saturation is
        // only reported when an overload coincides with an inability to grow, never on the
        // inability to grow alone.
        let events = spawn_system_telemetry(Some(1), 1);

        assert!(
            events
                .iter()
                .all(|event| event.name() != "oxidizer.rt.system_worker.pool_saturated"),
            "a pool that is merely at its size limit, without being overloaded, must stay quiet"
        );
    }
}
