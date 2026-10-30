// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Blocking worker admission and pool execution implementation.

use std::num::NonZeroUsize;
use std::sync::atomic::Ordering;
use std::{fmt, thread};

use arty_executor::discard_task_panic_payload;
use performables::arc::Arc;
use performables::sync::channel;
use performables::sync::mutex::Mutex;

use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::runtime::worker::blocking::{LocalWorker, PanicPolicy, PoolIdentity};
use crate::task::handle::{JoinHandle, JoinHandlePool};
use crate::task::preparation::prepare_blocking;

const ERR_POISONED_LOCK: &str = "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";

type Job = Box<dyn FnOnce() + Send + 'static>;

#[cfg_attr(test, mutants::skip)] // Missing pool identity disables the same-pool join deadlock guard.
fn attach_thread(pool: PoolIdentity) {
    LocalWorker::attach(pool);
}

/// Grow-only blocking pool that retains worker handles for deterministic shutdown.
struct JoinableThreadPool {
    jobs: Option<channel::Sender<Job>>,
    workers: Vec<thread::JoinHandle<()>>,
    identity: PoolIdentity,
    receiver: channel::Receiver<Job>,
    queued_count: Arc<std::sync::atomic::AtomicUsize>,
    active_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl JoinableThreadPool {
    fn new(thread_count: usize, identity: PoolIdentity) -> Self {
        let (jobs, receiver) = channel::unbounded();
        let mut pool = Self {
            jobs: Some(jobs),
            workers: Vec::with_capacity(thread_count),
            identity,
            receiver,
            queued_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            active_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };
        pool.set_num_threads(thread_count);
        pool
    }

    fn execute<F>(&self, job: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        let Some(jobs) = &self.jobs else {
            return false;
        };
        self.queued_count.fetch_add(1, Ordering::SeqCst);
        if jobs.send(Box::new(job)).is_err() {
            self.queued_count.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    #[cfg_attr(test, mutants::skip)] // Mutating worker construction creates non-terminating pools.
    fn set_num_threads(&mut self, thread_count: usize) {
        while self.workers.len() < thread_count {
            let receiver = self.receiver.clone();
            let queued_count = Arc::clone(&self.queued_count);
            let active_count = Arc::clone(&self.active_count);
            let identity = Arc::clone(&self.identity);
            let worker = thread::Builder::new()
                .name("arty-blocking".to_string())
                .spawn(move || {
                    attach_thread(identity);
                    while let Ok(job) = receiver.recv() {
                        active_count.fetch_add(1, Ordering::SeqCst);
                        queued_count.fetch_sub(1, Ordering::SeqCst);
                        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                        active_count.fetch_sub(1, Ordering::SeqCst);
                        if let Err(panic) = outcome {
                            discard_task_panic_payload(panic);
                        }
                    }
                })
                .expect("blocking worker thread creation must succeed");
            self.workers.push(worker);
        }
    }

    fn max_count(&self) -> usize {
        self.workers.len()
    }

    fn active_count(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    fn queued_count(&self) -> usize {
        self.queued_count.load(Ordering::SeqCst)
    }

    #[cfg_attr(test, mutants::skip)] // Equivalent mutation: consuming self still runs Drop.
    fn join(mut self) {
        self.shutdown_and_join();
    }

    fn shutdown_and_join(&mut self) {
        drop(self.jobs.take());
        for worker in self.workers.drain(..) {
            if let Err(panic) = worker.join() {
                discard_task_panic_payload(panic);
            }
        }
    }
}

impl Drop for JoinableThreadPool {
    fn drop(&mut self) {
        self.shutdown_and_join();
    }
}

impl fmt::Debug for JoinableThreadPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinableThreadPool")
            .field("queued_count", &self.queued_count())
            .field("active_count", &self.active_count())
            .field("max_count", &self.max_count())
            .finish()
    }
}

/// Worker for blocking tasks.
///
/// Each async worker thread has one blocking worker for scheduling blocking
/// tasks.
#[derive(Debug)]
pub(crate) struct Worker {
    pool: Pool,
    remote_results: JoinHandlePool,
    panic_policy: PanicPolicy,
}

impl Worker {
    pub(crate) fn new(pool: Pool, runtime_shutdown: Shutdown, panic_policy: PanicPolicy) -> Arc<Self> {
        Arc::new(Self {
            pool,
            remote_results: JoinHandlePool::new(runtime_shutdown),
            panic_policy,
        })
    }

    /// Submits a blocking task to the worker. If the worker is shutting down, the task will be ignored.
    pub(crate) fn spawn_blocking<F, R>(&self, body: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let runtime_shutdown = self.remote_results.shutdown().clone();
        let (task, join_handle) = prepare_blocking(&self.remote_results, body, self.panic_policy.clone());
        if matches!(runtime_shutdown.state(), ShutdownState::Started) {
            drop(task);
            return join_handle;
        }
        let join_handle = join_handle.blocking_pool(Arc::clone(&self.pool.identity));
        let task = move || {
            if matches!(runtime_shutdown.state(), ShutdownState::Started) {
                drop(task);
            } else {
                task();
            }
        };

        if matches!(self.remote_results.shutdown().state(), ShutdownState::Running) && self.pool.execute(task) {
            self.pool.grow_if_overloaded();
        }

        join_handle
    }

    /// Waits for the currently running blocking tasks to complete.
    #[cfg_attr(test, mutants::skip)] // Impractical to test without overly-expensive timeout logic.
    pub(crate) fn join(&self) {
        self.pool.join();
    }

    #[cfg(test)]
    pub(crate) fn is_shutting_down(&self) -> bool {
        matches!(self.remote_results.shutdown().state(), ShutdownState::Started)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Pool {
    inner: Arc<Mutex<Option<JoinableThreadPool>>>,
    identity: PoolIdentity,
    max_thread_count: NonZeroUsize,
}

impl Pool {
    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn shares_pool_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Initial number of threads in the pool.
    ///
    /// The pool starts with one thread and grows when the queue is overloaded.
    const INITIAL_THREAD_COUNT: usize = 1;

    /// Queue-growth heuristic following the [`blocking`](https://github.com/smol-rs/blocking/blob/master/src/lib.rs) pool.
    const MAX_TASKS_PER_THREAD: usize = 5;

    /// Default per-pool thread limit for blocking tasks.
    const MAX_THREAD_COUNT_DEFAULT: NonZeroUsize = NonZeroUsize::new(64).expect("the default blocking thread limit is nonzero");

    pub(crate) fn new(max_thread_count: Option<NonZeroUsize>) -> Self {
        // Start with one thread and let the pool grow as needed.
        let identity = Arc::new(());
        let thread_pool = JoinableThreadPool::new(Self::INITIAL_THREAD_COUNT, Arc::clone(&identity));

        Self {
            inner: Arc::new(Mutex::new(Some(thread_pool))),
            identity,
            max_thread_count: max_thread_count.unwrap_or(Self::MAX_THREAD_COUNT_DEFAULT),
        }
    }

    pub(crate) fn execute<F>(&self, f: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        let pool = self.inner.lock_result().expect(ERR_POISONED_LOCK);
        let Some(pool) = pool.as_ref() else {
            // Shutdown closed the shared pool before this racing submission arrived.
            return false;
        };
        pool.execute(f)
    }

    #[cfg_attr(test, mutants::skip)] // Impractical to test without overly-expensive timeout logic.
    fn join(&self) {
        // Do not keep the pool alive through retained scheduler handles, and do not
        // hold its submission lock while waiting for user work to finish.
        let pool = self.inner.lock_result().expect(ERR_POISONED_LOCK).take();
        if let Some(pool) = pool {
            pool.join();
        }
    }

    /// Grows the pool by one thread if possible, returning whether it grew.
    ///
    /// Returns `false` when the pool is already at its maximum permitted size.
    #[cfg(test)]
    fn grow(&self) -> bool {
        let mut pool = self.inner.lock_result().expect(ERR_POISONED_LOCK);
        let Some(pool) = pool.as_mut() else {
            return false;
        };
        let new_thread_count = pool.max_count().saturating_add(1);

        if new_thread_count > self.max_thread_count.get() {
            return false;
        }
        pool.set_num_threads(new_thread_count);
        true
    }

    #[cfg(test)]
    pub(crate) fn max_thread_count(&self) -> usize {
        self.max_thread_count.get()
    }

    #[cfg(test)]
    fn is_overloaded(&self) -> bool {
        let pool = self.inner.lock_result().expect(ERR_POISONED_LOCK);
        pool.as_ref().is_some_and(Self::thread_pool_is_overloaded)
    }

    fn grow_if_overloaded(&self) {
        let mut pool = self.inner.lock_result().expect(ERR_POISONED_LOCK);
        let Some(pool) = pool.as_mut() else {
            return;
        };
        if !Self::thread_pool_is_overloaded(pool) {
            return;
        }

        let new_thread_count = pool.max_count().saturating_add(1);
        if new_thread_count <= self.max_thread_count.get() {
            pool.set_num_threads(new_thread_count);
        }
    }

    fn thread_pool_is_overloaded(pool: &JoinableThreadPool) -> bool {
        pool.active_count().saturating_add(pool.queued_count()) > pool.max_count().saturating_mul(Self::MAX_TASKS_PER_THREAD)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::thread;

    use arty_executor::ShutdownCause;
    use events_once::Event;
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;
    use testing_aids::{TEST_TIMEOUT, execute_or_abandon};

    use super::{Job, JoinableThreadPool, LocalWorker, PanicPolicy, Pool, Worker, attach_thread};
    use crate::runtime::shutdown::Shutdown;

    fn limit(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    fn joinable_pool(thread_count: usize) -> JoinableThreadPool {
        JoinableThreadPool::new(thread_count, Arc::new(()))
    }

    #[test]
    fn blocking_pool_grow_increases_thread_count() {
        let blocking_pool = Pool::new(None);
        assert!(blocking_pool.grow());
        assert_eq!(
            blocking_pool.inner.lock_result().unwrap().as_ref().unwrap().max_count(),
            Pool::INITIAL_THREAD_COUNT + 1,
            "worker thread count must increase by one"
        );
    }

    #[test]
    fn blocking_pool_grow_to_maximum() {
        let blocking_pool = Pool::new(Some(limit(5)));

        // It says "max" but it is effectively the "current" count because growth is async.
        assert_eq!(blocking_pool.inner.lock_result().unwrap().as_ref().unwrap().max_count(), 1);
        assert!(blocking_pool.grow()); // 2
        assert!(blocking_pool.grow()); // 3
        assert!(blocking_pool.grow()); // 4
        assert!(blocking_pool.grow()); // 5
        assert!(!blocking_pool.grow()); // 5 - should not grow further

        // It says "max" but it is effectively the "current" count because growth is async.
        assert_eq!(blocking_pool.inner.lock_result().unwrap().as_ref().unwrap().max_count(), 5);
    }

    #[test]
    fn overloaded_pool_grows_only_to_configured_thread_limit() {
        let pool = Pool::new(Some(limit(2)));
        let queued_task_count = Pool::MAX_TASKS_PER_THREAD * 2 + 1;
        pool.inner
            .lock_result()
            .unwrap()
            .as_ref()
            .unwrap()
            .queued_count
            .store(queued_task_count, Ordering::SeqCst);

        pool.grow_if_overloaded();
        assert_eq!(pool.inner.lock_result().unwrap().as_ref().unwrap().max_count(), 2);

        pool.grow_if_overloaded();
        assert_eq!(pool.inner.lock_result().unwrap().as_ref().unwrap().max_count(), 2);
    }

    #[test]
    fn blocking_pool_reports_configured_max_thread_count() {
        // The configured maximum differs from the initial count and the crate
        // default. The getter must return it without substituting either.
        assert_eq!(Pool::new(Some(limit(7))).max_thread_count(), 7);
        assert_eq!(Pool::new(None).max_thread_count(), Pool::MAX_THREAD_COUNT_DEFAULT.get());
    }

    #[test]
    fn closed_pool_rejects_work_and_cannot_grow() {
        let pool = Pool::new(Some(limit(2)));
        pool.join();
        let invoked = Arc::new(AtomicBool::new(false));
        assert!(!pool.execute({
            let invoked = Arc::clone(&invoked);
            move || invoked.store(true, Ordering::Relaxed)
        }));
        assert!(!invoked.load(Ordering::Relaxed));
        assert!(!pool.grow());
        assert!(!pool.is_overloaded());
    }

    #[cfg(not(miri))]
    #[test]
    fn runtime_releases_pool_even_when_a_scheduler_is_retained() {
        execute_or_abandon(|| {
            let runtime = crate::runtime::Runtime::builder()
                .workers(crate::runtime::WorkersPolicy::exactly(1))
                .build()
                .unwrap();
            let scheduler = runtime
                .scheduler()
                .spawn_anywhere((), |cx, ()| async move { cx.scheduler().clone() })
                .join()
                .unwrap();
            let worker = Arc::clone(scheduler.blocking_worker());
            drop(runtime);
            assert!(worker.pool.inner.lock_result().unwrap().is_none());
        })
        .unwrap();
    }

    #[test]
    fn spawn_on_worker() {
        let blocking_pool = Pool::new(None);
        let (sender, receiver) = channel();
        blocking_pool.execute(move || {
            sender.send("joined").unwrap();
        });

        // The sender exists only in the task. If the pool drops the task,
        // `recv` returns a disconnect instead of waiting.
        let res = receiver.recv().expect("the worker pool must run the dispatched task");
        assert_eq!(res, "joined");
    }

    #[test]
    fn closed_pool_rejects_worker_submission_after_admission() {
        let pool = Pool::new(NonZeroUsize::new(1));
        let worker = Worker::new(pool.clone(), Shutdown::default(), PanicPolicy::Ignore);
        pool.join();

        let invoked = Arc::new(AtomicBool::new(false));
        let join = worker.spawn_blocking({
            let invoked = Arc::clone(&invoked);
            move || invoked.store(true, Ordering::Relaxed)
        });

        assert!(join.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Relaxed));
        pool.grow_if_overloaded();
    }

    #[test]
    fn joinable_pool_attaches_blocking_worker_identity() {
        let current = Arc::new(());
        let other = Arc::new(());
        let pool = JoinableThreadPool::new(1, Arc::clone(&current));
        let (observed, received) = channel();

        assert!(pool.execute(move || {
            observed
                .send((LocalWorker::is_current_pool(&current), LocalWorker::is_current_pool(&other)))
                .unwrap();
        }));

        assert_eq!(received.recv_timeout(TEST_TIMEOUT).unwrap(), (true, false));
        pool.join();
    }

    #[test]
    fn joinable_pool_reports_active_work() {
        let pool = joinable_pool(1);
        let (started, ready) = channel();
        let (release, released) = channel();
        assert!(pool.execute(move || {
            started.send(()).unwrap();
            released.recv().unwrap();
        }));
        ready.recv_timeout(TEST_TIMEOUT).unwrap();

        assert_eq!(pool.active_count(), 1);
        release.send(()).unwrap();
        pool.join();
    }

    #[test]
    fn joinable_pool_rejects_work_after_shutdown() {
        let mut pool = joinable_pool(1);
        pool.shutdown_and_join();

        assert!(!pool.execute(|| panic!("a shut down pool must reject work")));
    }

    #[test]
    fn joinable_pool_reports_disconnected_submission() {
        let (jobs, disconnected) = performables::sync::channel::unbounded::<Job>();
        drop(disconnected);
        let (_, receiver) = performables::sync::channel::unbounded();
        let mut pool = JoinableThreadPool {
            jobs: Some(jobs),
            workers: Vec::new(),
            identity: Arc::new(()),
            receiver,
            queued_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            active_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        assert!(!pool.execute(|| {}));
        assert_eq!(pool.queued_count(), 0);
        pool.shutdown_and_join();
    }

    #[test]
    fn joinable_pool_contains_job_panics() {
        let pool = joinable_pool(1);
        assert!(pool.execute(|| panic!("direct pool job panic")));
        let (completed, received) = channel();
        assert!(pool.execute(move || completed.send(()).unwrap()));

        received.recv_timeout(TEST_TIMEOUT).unwrap();
        pool.join();
    }

    #[test]
    fn joinable_pool_discards_worker_panic() {
        let worker = thread::spawn(|| panic!("worker failure"));
        let (_, receiver) = performables::sync::channel::unbounded();
        let mut pool = JoinableThreadPool {
            jobs: None,
            workers: vec![worker],
            identity: Arc::new(()),
            receiver,
            queued_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            active_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        pool.shutdown_and_join();
    }

    #[test]
    fn joinable_pool_debug_reports_counts() {
        let pool = joinable_pool(1);

        let debug = format!("{pool:?}");
        assert!(debug.contains("queued_count: 0"));
        assert!(debug.contains("active_count: 0"));
        assert!(debug.contains("max_count: 1"));
        pool.join();
    }

    #[test]
    fn overload_worker() {
        execute_or_abandon(move || {
            let blocking_pool = Pool::new(None);
            let (worker_thread_started_tx, worker_thread_started_rx) = channel();
            let (worker_thread_finished_tx, worker_thread_finished_rx) = Event::<()>::boxed();

            // Only the first thread uses this receiver. The wrapper provides
            // the thread-safe type that the task boundary requires.
            let worker_thread_finished_rx = Arc::new(Mutex::<Option<_>>::new(Some(worker_thread_finished_rx)));

            for i in 0..Pool::MAX_TASKS_PER_THREAD {
                blocking_pool.execute({
                    let worker_thread_started_tx = worker_thread_started_tx.clone();
                    let worker_thread_finished_rx = Arc::clone(&worker_thread_finished_rx);
                    move || {
                        if i == 0 {
                            // This is the first spawned task on a single threaded pool.
                            // We need to wait for it to start and then
                            // block it, so we can schedule further tasks and overload the pool.
                            worker_thread_started_tx.send(()).unwrap();

                            futures::executor::block_on(async {
                                let rx = worker_thread_finished_rx.lock_result().unwrap().take().unwrap();
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
                !blocking_pool.is_overloaded(),
                "worker pool must be at the task limit but not overloaded"
            );

            // The blocked thread pool cannot start this task before the assertion.
            blocking_pool.execute(move || {});

            assert!(blocking_pool.is_overloaded(), "worker pool must be overloaded");

            // Now we can release the second barrier and clean up
            worker_thread_finished_tx.send(());
            blocking_pool.join();
        })
        .unwrap();
    }

    #[test]
    fn blocking_worker_join_waits_for_tasks_to_complete() {
        let events = Arc::new(Mutex::<Vec<&str>>::new(Vec::new()));
        let (task_start_tx, task_start_rx) = channel();

        let thread_join_handle = thread::spawn({
            let events = Arc::clone(&events);
            move || {
                let worker = Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore);

                drop(worker.spawn_blocking({
                    let events = Arc::clone(&events);
                    move || {
                        events.lock_result().unwrap().push("task started");
                        task_start_tx.send(()).unwrap();
                        events.lock_result().unwrap().push("task finished");
                    }
                }));

                // The sender exists only in the blocking task. If the worker
                // drops the task, `recv` returns a disconnect instead of waiting.
                task_start_rx.recv().expect("the worker must run the blocking task");

                worker.remote_results.shutdown().begin(ShutdownCause::default());
                worker.join();
                events.lock_result().unwrap().push("worker joined");
            }
        });

        execute_or_abandon(|| {
            thread_join_handle.join().unwrap();
        })
        .unwrap();

        assert_eq!(
            events.lock_result().unwrap().as_slice(),
            &["task started", "task finished", "worker joined"]
        );
    }

    #[test]
    fn blocking_worker_shutdown_prevents_new_tasks() {
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

        let worker = Worker::new(Pool::new(None), Shutdown::default(), PanicPolicy::Ignore);

        worker.remote_results.shutdown().begin(ShutdownCause::default());

        let work_item = WorkItem {
            dropped: Arc::clone(&dropped),
            work_done: Arc::clone(&work_done),
        };

        let task = worker.spawn_blocking(move || {
            work_item.do_work();
        });

        assert!(dropped.load(Ordering::Acquire));
        assert!(!work_done.load(Ordering::Acquire));
        drop(task);
    }

    #[test]
    fn runtime_shutdown_cancels_queued_work_before_worker_shutdown() {
        let runtime_shutdown = Shutdown::default();
        let worker = Worker::new(Pool::new(Some(limit(1))), runtime_shutdown.clone(), PanicPolicy::Ignore);
        let (started_tx, started_rx) = channel();
        let (release_tx, release_rx) = Event::<()>::boxed();

        let running = worker.spawn_blocking(move || {
            started_tx.send(()).unwrap();
            futures::executor::block_on(release_rx).unwrap();
        });
        started_rx.recv().unwrap();

        let invoked = Arc::new(AtomicBool::new(false));
        let queued = worker.spawn_blocking({
            let invoked = Arc::clone(&invoked);
            move || invoked.store(true, Ordering::Release)
        });
        runtime_shutdown.begin(ShutdownCause::default());
        release_tx.send(());

        running.join().unwrap();
        assert!(queued.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Acquire));
        worker.remote_results.shutdown().begin(ShutdownCause::default());
        worker.join();
    }

    #[test]
    fn runtime_shutdown_rejects_new_work_before_worker_shutdown() {
        let runtime_shutdown = Shutdown::default();
        runtime_shutdown.begin(ShutdownCause::default());
        let worker = Worker::new(Pool::new(Some(limit(1))), runtime_shutdown, PanicPolicy::Ignore);
        let invoked = Arc::new(AtomicBool::new(false));

        let rejected = worker.spawn_blocking({
            let invoked = Arc::clone(&invoked);
            move || invoked.store(true, Ordering::Release)
        });

        assert!(rejected.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Acquire));
        worker.remote_results.shutdown().begin(ShutdownCause::default());
        worker.join();
    }

    #[test]
    fn worker_shutdown_rejects_same_pool_join_without_deadlock_guard() {
        let worker = Worker::new(Pool::new(Some(limit(1))), Shutdown::default(), PanicPolicy::Ignore);
        attach_thread(Arc::clone(&worker.pool.identity));
        worker.remote_results.shutdown().begin(ShutdownCause::default());

        let rejected = worker.spawn_blocking(|| 7).join().unwrap_err();

        assert!(rejected.is_shutdown());
        worker.join();
    }
}
