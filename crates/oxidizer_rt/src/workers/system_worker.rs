// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::rc::Rc;

use threadpool::ThreadPool;
use tracing::warn;

use crate::once_event::shared::new_inefficient;
use crate::{RemoteJoinHandle, SystemTaskMeta};

/// Worker for system tasks. Meant to be created for each async worker thread to allow for scheduling of system tasks.
#[derive(Debug)]
pub struct SystemWorker {
    // Naive implementation using a per-async-worker thread pool
    pool: WorkerPool,
    is_shutting_down: RefCell<bool>,
}

impl SystemWorker {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            pool: WorkerPool::default(),
            is_shutting_down: RefCell::new(false),
        })
    }

    /// Submits a system task to the worker. If the worker is shutting down, the task will be ignored.
    pub fn spawn_system<F, R>(&self, _meta: SystemTaskMeta, body: F) -> RemoteJoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        if *self.is_shutting_down.borrow() {
            return RemoteJoinHandle::new_never();
        }

        let (tx, rx) = new_inefficient();
        // We ignore meta for now
        self.pool.execute(|| {
            let result = body();
            tx.set(result);
        });

        if self.pool.is_overloaded() {
            self.pool.grow();
        }

        RemoteJoinHandle::new_unplaced(rx)
    }

    /// After shutting down, no new tasks will be accepted, but existing tasks will continue to run.
    pub fn shutdown(&self) {
        *self.is_shutting_down.borrow_mut() = true;
    }

    /// Waits for the currently running system tasks to complete.
    pub fn join(&self) {
        self.pool.join();
    }
}

#[derive(Debug)]
struct WorkerPool {
    pool: RefCell<ThreadPool>,
    max_thread_count: usize,
}

impl WorkerPool {
    /// Initial number of threads in the pool.
    /// It should be reasonable to start at one thread and ramp up the number if necessary.
    const INITIAL_THREAD_COUNT: usize = 1;

    /// Constant taken from similar use case in smol's [blocking}(https://github.com/smol-rs/blocking/blob/master/src/lib.rs) crate
    const MAX_TASKS_PER_THREAD: usize = 5;

    /// Maximum number of spawned threads. This number is currently more a guesstimate
    /// vaguely based on a default limits used by Tokio and Smol (512 threads globally).
    /// As the purpose of this thread pool is to run io bound tasks and not cpu bound tasks,
    /// higher thread number shouldn't significantly affect performance.
    /// For now, we set it to 64 threads, but this should be revisited in the future when we get some data from usage.
    const MAX_THREAD_COUNT_DEFAULT: usize = 64;
    fn new(initial_thread_count: usize, max_thread_count: usize) -> Self {
        Self {
            pool: RefCell::new(ThreadPool::new(initial_thread_count)),
            max_thread_count,
        }
    }

    fn execute<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.pool.borrow().execute(f);
    }

    fn join(&self) {
        self.pool.borrow().join();
    }

    /// Grows the pool by one thread if possible
    fn grow(&self) {
        let mut pool = self.pool.borrow_mut();
        let new_thread_count = pool.max_count().saturating_add(1);

        if new_thread_count > self.max_thread_count {
            warn!(
                "System scheduler thread pool can't grow - it's overloaded and already at maximum permitted size"
            );
            return;
        }
        pool.set_num_threads(new_thread_count);
    }

    fn is_overloaded(&self) -> bool {
        let pool = self.pool.borrow();
        pool.active_count().saturating_add(pool.queued_count())
            > pool.max_count().saturating_mul(Self::MAX_TASKS_PER_THREAD)
    }
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::new(Self::INITIAL_THREAD_COUNT, Self::MAX_THREAD_COUNT_DEFAULT)
    }
}

#[cfg(test)]
pub mod system_worker_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;
    use std::time::Duration;

    use futures::channel::oneshot;
    use oxidizer_testing::execute_or_abandon;

    use crate::workers::system_worker::WorkerPool;
    use crate::{SystemTaskMeta, SystemWorker};

    pub fn is_system_worker_shutting_down(worker: &SystemWorker) -> bool {
        *worker.is_shutting_down.borrow()
    }

    #[test]
    fn system_worker_join_waits_for_tasks_to_complete() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let (task_start_tx, mut task_start_rx) = oneshot::channel();

        let events_clone = Arc::clone(&events);
        let thread_join_handle = thread::spawn(move || {
            let worker = SystemWorker::new();

            let events_clone2 = Arc::clone(&events_clone);
            drop(worker.spawn_system(SystemTaskMeta::default(), move || {
                events_clone2.lock().unwrap().push("task started");
                task_start_tx.send(()).unwrap();

                // Ideally, this would wait for the join call to happen, but join blocks the thread
                // which means we cannot send a signal after the join call. So we just sleep for hopefully
                // long enough to give join a chance to run first.
                thread::sleep(Duration::from_millis(50));

                events_clone2.lock().unwrap().push("task finished");
            }));

            task_start_rx.try_recv().unwrap();
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

        let worker = SystemWorker::new();

        worker.shutdown();

        let work_item = WorkItem {
            dropped: Arc::clone(&dropped),
            work_done: Arc::clone(&work_done),
        };

        let task = worker.spawn_system(SystemTaskMeta::default(), move || {
            work_item.do_work();
        });

        assert!(dropped.load(Ordering::Acquire));
        assert!(!work_done.load(Ordering::Acquire));
        drop(task);
    }

    #[test]
    fn worker_pool_grow_increases_thread_count() {
        let worker_pool = WorkerPool::default();
        worker_pool.grow();
        assert_eq!(
            worker_pool.pool.borrow().max_count(),
            WorkerPool::INITIAL_THREAD_COUNT + 1,
            "Number of worker threads should be increased by 1"
        );
    }

    #[test]
    fn worker_pool_grow_maximum() {
        let worker_pool = WorkerPool::new(1, 1);
        assert_eq!(worker_pool.pool.borrow().max_count(), 1);
        worker_pool.grow();
        assert_eq!(
            worker_pool.pool.borrow().max_count(),
            1,
            "Number of worker threads should not exceed the maximum limit"
        );
    }

    #[test]
    fn spawn_on_worker() {
        let worker_pool = WorkerPool::default();
        let (rx, tx) = channel();
        worker_pool.execute(move || {
            rx.send("joined").unwrap();
        });
        let res = execute_or_abandon(move || tx.recv().unwrap()).unwrap();
        assert_eq!(res, "joined");
    }

    #[test]
    fn overload_worker() {
        let worker_pool = WorkerPool::default();
        let worker_thread_started = Arc::new(Barrier::new(2));
        let worker_thread_finished = Arc::new(Barrier::new(2));

        execute_or_abandon(move || {
            for i in 0..WorkerPool::MAX_TASKS_PER_THREAD {
                worker_pool.execute({
                    let worker_thread_started = Arc::clone(&worker_thread_started);
                    let worker_thread_finished = Arc::clone(&worker_thread_finished);
                    move || {
                        if i == 0 {
                            // This is the first spawned task on a single threaded pool.
                            // We need to wait for it to start and then
                            // block it, so we can schedule further tasks and overload the pool.
                            worker_thread_started.wait();
                            worker_thread_finished.wait();
                        }
                    }
                });
            }
            // First barrier will be waiting for the thread pool to start the first task
            worker_thread_started.wait();

            assert!(
                !worker_pool.is_overloaded(),
                "Worker pool should be at the task limit but not overloaded"
            );

            // This task can be empty, the blocked thread pool won't have a chance to start it anyway
            worker_pool.execute(move || {});

            assert!(
                worker_pool.is_overloaded(),
                "Worker pool should be overloaded"
            );

            // Now we can release the second barrier and clean up
            worker_thread_finished.wait();
            worker_pool.join();
        })
        .unwrap();
    }
}