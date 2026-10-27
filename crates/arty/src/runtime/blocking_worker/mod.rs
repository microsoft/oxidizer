// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Blocking-task admission and blocking-pool execution.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
use std::{fmt, thread};

use arty_executor::discard_task_panic_payload;
use arty_executor::remote::Pool as RemotePool;
use observed::{Sink, emit};
use performables::arc::Arc;
use performables::sync::channel;
use performables::sync::mutex::Mutex;

use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::runtime::telemetry::events::{BlockingWorkerPoolMode, BlockingWorkerPoolSaturated, SystemMetricCount};
use crate::task::execution::prepare_blocking;
use crate::task::join::JoinHandle;

const ERR_POISONED_LOCK: &str = "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";

type BlockingJob = Box<dyn FnOnce() + Send + 'static>;

thread_local! {
    static CURRENT_POOL: RefCell<Option<Arc<()>>> = const { RefCell::new(None) };
}

struct BlockingTaskScope {
    previous: Option<Arc<()>>,
    _not_send: PhantomData<Rc<()>>,
}

impl BlockingTaskScope {
    fn enter(identity: Arc<()>) -> Self {
        Self {
            previous: CURRENT_POOL.replace(Some(identity)),
            _not_send: PhantomData,
        }
    }
}

impl Drop for BlockingTaskScope {
    fn drop(&mut self) {
        CURRENT_POOL.set(self.previous.take());
    }
}

/// Grow-only blocking pool that retains worker handles for deterministic shutdown.
struct JoinableThreadPool {
    jobs: Option<channel::Sender<BlockingJob>>,
    workers: Vec<thread::JoinHandle<()>>,
    worker_threads: Arc<Mutex<Vec<ThreadId>>>,
    receiver: channel::Receiver<BlockingJob>,
    queued_count: Arc<std::sync::atomic::AtomicUsize>,
    active_count: Arc<std::sync::atomic::AtomicUsize>,
}

impl JoinableThreadPool {
    fn new(thread_count: usize, worker_threads: Arc<Mutex<Vec<ThreadId>>>) -> Self {
        let (jobs, receiver) = channel::unbounded();
        let mut pool = Self {
            jobs: Some(jobs),
            workers: Vec::with_capacity(thread_count),
            worker_threads,
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
            let worker_threads = Arc::clone(&self.worker_threads);
            let worker = thread::Builder::new()
                .name("arty-blocking".to_string())
                .spawn(move || {
                    worker_threads.lock().push(thread::current().id());
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
        self.worker_threads.lock().clear();
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

/// Worker for blocking tasks. Meant to be created for each async worker thread to allow for scheduling of blocking tasks.
#[derive(Debug)]
pub(crate) struct BlockingWorker {
    pool: BlockingPool,
    runtime_shutdown: Shutdown,
    sink: Sink,
    remote_results: RemotePool,
}

impl BlockingWorker {
    pub(crate) fn new(pool: BlockingPool, sink: Sink, runtime_shutdown: Shutdown) -> Arc<Self> {
        Arc::new(Self {
            pool,
            runtime_shutdown,
            sink,
            remote_results: RemotePool::new(),
        })
    }

    pub(crate) fn sink(&self) -> &Sink {
        &self.sink
    }

    /// Submits a blocking task to the worker. If the worker is shutting down, the task will be ignored.
    pub(crate) fn spawn_blocking<F, R>(&self, body: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_blocking_with_completion(body, || {})
    }

    fn spawn_blocking_with_completion<F, R, C>(&self, body: F, completion: C) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
        C: FnOnce() + Send + 'static,
    {
        let identity = Arc::clone(&self.pool.identity);
        let pool = self.pool.clone();
        let runtime_shutdown = self.runtime_shutdown.clone();
        let (task, join_handle) = prepare_blocking(&self.remote_results, runtime_shutdown.clone(), body);
        if matches!(runtime_shutdown.state(), ShutdownState::Started) {
            drop(task);
            return join_handle;
        }
        let join_handle = join_handle.blocking_pool(Arc::clone(&self.pool.identity));
        let task = move || {
            let _scope = BlockingTaskScope::enter(identity);
            if matches!(runtime_shutdown.state(), ShutdownState::Started) {
                drop(task);
            } else {
                task();
            }
            pool.clear_saturation_if_recovered();
            completion();
        };

        if matches!(self.runtime_shutdown.state(), ShutdownState::Running)
            && self.pool.execute(task)
            && self.pool.record_saturation_transition()
        {
            emit!(
                &self.sink,
                BlockingWorkerPoolSaturated {
                    blocking_worker_pool_mode: BlockingWorkerPoolMode(self.pool.mode()),
                    max_threads: SystemMetricCount::from(self.pool.max_thread_count()),
                }
            );
        }

        join_handle
    }

    #[cfg_attr(test, mutants::skip)] // Removing this wait guard deadlocks runtime-owner teardown.
    pub(crate) fn owns_current_thread(&self) -> bool {
        self.pool.owns_current_thread()
            || CURRENT_POOL.with_borrow(|current| current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &self.pool.identity)))
    }

    /// Waits for the currently running blocking tasks to complete.
    #[cfg_attr(test, mutants::skip)] // Impractical to test without overly-expensive timeout logic.
    pub(crate) fn join(&self) {
        self.pool.join();
    }
}

pub(crate) fn is_current_blocking_pool(pool: &Arc<()>) -> bool {
    CURRENT_POOL.with_borrow(|current| current.as_ref().is_some_and(|current| Arc::ptr_eq(current, pool)))
}

#[derive(Debug, Clone)]
pub(crate) struct BlockingPool {
    pool: Arc<Mutex<Option<JoinableThreadPool>>>,
    identity: Arc<()>,
    worker_threads: Arc<Mutex<Vec<ThreadId>>>,
    saturation_reported: Arc<AtomicBool>,
    max_thread_count: NonZeroUsize,
    mode: &'static str,
}

impl BlockingPool {
    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn shares_pool_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.pool, &other.pool)
    }

    /// Initial number of threads in the pool.
    /// It should be reasonable to start at one thread and ramp up the number if necessary.
    const INITIAL_THREAD_COUNT: usize = 1;

    /// Queue-growth heuristic following the [`blocking`](https://github.com/smol-rs/blocking/blob/master/src/lib.rs) pool.
    const MAX_TASKS_PER_THREAD: usize = 5;

    /// Default per-pool thread limit for blocking tasks.
    const MAX_THREAD_COUNT_DEFAULT: NonZeroUsize = NonZeroUsize::new(64).expect("the default blocking thread limit is nonzero");

    #[cfg(test)]
    pub(crate) fn new(max_thread_count: Option<NonZeroUsize>) -> Self {
        Self::new_with_mode(max_thread_count, "shared")
    }

    pub(crate) fn new_with_mode(max_thread_count: Option<NonZeroUsize>, mode: &'static str) -> Self {
        // Start with one thread and let the pool grow as needed.
        let worker_threads = Arc::new(Mutex::new(Vec::new()));
        let thread_pool = JoinableThreadPool::new(Self::INITIAL_THREAD_COUNT, Arc::clone(&worker_threads));

        Self {
            pool: Arc::new(Mutex::new(Some(thread_pool))),
            identity: Arc::new(()),
            worker_threads,
            saturation_reported: Arc::new(AtomicBool::new(false)),
            max_thread_count: max_thread_count.unwrap_or(Self::MAX_THREAD_COUNT_DEFAULT),
            mode,
        }
    }

    pub(crate) fn execute<F>(&self, f: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        let pool = self.pool.lock_result().expect(ERR_POISONED_LOCK);
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
        let pool = self.pool.lock_result().expect(ERR_POISONED_LOCK).take();
        if let Some(pool) = pool {
            pool.join();
        }
    }

    #[cfg_attr(test, mutants::skip)] // Removing this wait guard deadlocks runtime-owner teardown.
    fn owns_current_thread(&self) -> bool {
        let current = thread::current().id();
        self.worker_threads.lock().contains(&current)
    }

    /// Grows the pool by one thread if possible, returning whether it grew.
    ///
    /// Returns `false` when the pool is already at its maximum permitted size.
    #[cfg(test)]
    fn grow(&self) -> bool {
        let mut pool = self.pool.lock_result().expect(ERR_POISONED_LOCK);
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

    pub(crate) fn max_thread_count(&self) -> usize {
        self.max_thread_count.get()
    }

    fn mode(&self) -> &'static str {
        self.mode
    }

    #[cfg(test)]
    fn is_overloaded(&self) -> bool {
        let pool = self.pool.lock_result().expect(ERR_POISONED_LOCK);
        pool.as_ref().is_some_and(Self::thread_pool_is_overloaded)
    }

    fn record_saturation_transition(&self) -> bool {
        let mut pool = self.pool.lock_result().expect(ERR_POISONED_LOCK);
        let Some(pool) = pool.as_mut() else {
            return false;
        };
        if !Self::thread_pool_is_overloaded(pool) {
            self.saturation_reported.store(false, Ordering::Release);
            return false;
        }

        let new_thread_count = pool.max_count().saturating_add(1);
        if new_thread_count <= self.max_thread_count.get() {
            pool.set_num_threads(new_thread_count);
            return false;
        }

        !self.saturation_reported.swap(true, Ordering::AcqRel)
    }

    fn clear_saturation_if_recovered(&self) {
        let pool = self.pool.lock_result().expect(ERR_POISONED_LOCK);
        if pool.as_ref().is_none_or(|pool| !Self::thread_pool_is_overloaded(pool)) {
            self.saturation_reported.store(false, Ordering::Release);
        }
    }

    fn thread_pool_is_overloaded(pool: &JoinableThreadPool) -> bool {
        pool.active_count().saturating_add(pool.queued_count()) > pool.max_count().saturating_mul(Self::MAX_TASKS_PER_THREAD)
    }
}

#[cfg(test)]
#[test]
#[cfg_attr(coverage_nightly, coverage(off))] // The rejected callback is deliberately asserted not to execute.
fn closed_pool_rejects_worker_submission_after_admission() {
    let pool = BlockingPool::new(NonZeroUsize::new(1));
    let worker = BlockingWorker::new(pool.clone(), Sink::noop(), Shutdown::default());
    pool.join();

    let invoked = Arc::new(AtomicBool::new(false));
    let join = worker.spawn_blocking({
        let invoked = Arc::clone(&invoked);
        move || invoked.store(true, Ordering::Relaxed)
    });

    assert!(join.join().unwrap_err().is_shutdown());
    assert!(!invoked.load(Ordering::Relaxed));
    assert!(!pool.record_saturation_transition());
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
pub(super) mod blocking_worker_tests {
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::channel;
    use std::thread;
    use std::thread::ThreadId;

    use arty_executor::ShutdownCause;
    use events_once::Event;
    use observed::{Sink, Value};
    use observed_testing::{CapturedEvent, TEST_ID, test_emitter};
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;
    use testing_aids::{TEST_TIMEOUT, execute_or_abandon};

    use crate::runtime::blocking_worker::{
        BlockingJob, BlockingPool, BlockingTaskScope, BlockingWorker, CURRENT_POOL, JoinableThreadPool, is_current_blocking_pool,
    };
    use crate::runtime::shutdown::{Shutdown, ShutdownState};

    fn limit(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    fn joinable_pool(thread_count: usize) -> (JoinableThreadPool, Arc<Mutex<Vec<ThreadId>>>) {
        let workers = Arc::new(Mutex::new(Vec::new()));
        (JoinableThreadPool::new(thread_count, Arc::clone(&workers)), workers)
    }

    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn is_blocking_worker_shutting_down(worker: &BlockingWorker) -> bool {
        matches!(worker.runtime_shutdown.state(), ShutdownState::Started)
    }

    #[test]
    fn blocking_task_scopes_restore_the_previous_pool_identity() {
        assert!(CURRENT_POOL.with_borrow(Option::is_none));
        let outer_identity = Arc::new(());
        let inner_identity = Arc::new(());
        {
            let _outer = BlockingTaskScope::enter(Arc::clone(&outer_identity));
            CURRENT_POOL.with_borrow(|current| {
                assert!(Arc::ptr_eq(current.as_ref().unwrap(), &outer_identity));
            });
            {
                let _inner = BlockingTaskScope::enter(Arc::clone(&inner_identity));
                CURRENT_POOL.with_borrow(|current| {
                    assert!(Arc::ptr_eq(current.as_ref().unwrap(), &inner_identity));
                });
            }
            CURRENT_POOL.with_borrow(|current| {
                assert!(Arc::ptr_eq(current.as_ref().unwrap(), &outer_identity));
            });
        }
        assert!(CURRENT_POOL.with_borrow(Option::is_none));
    }

    #[test]
    fn current_blocking_pool_detects_direct_task_scope() {
        let current = Arc::new(());
        let other = Arc::new(());
        assert!(!is_current_blocking_pool(&current));
        let _scope = BlockingTaskScope::enter(Arc::clone(&current));
        assert!(is_current_blocking_pool(&current));
        assert!(!is_current_blocking_pool(&other));
    }

    #[test]
    fn joinable_pool_reports_active_work() {
        let (pool, _) = joinable_pool(1);
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
        let (mut pool, workers) = joinable_pool(1);
        pool.shutdown_and_join();

        assert!(!pool.execute(|| panic!("a shut down pool must reject work")));
        assert!(workers.lock().is_empty());
    }

    #[test]
    fn joinable_pool_reports_disconnected_submission() {
        let (jobs, disconnected) = performables::sync::channel::unbounded::<BlockingJob>();
        drop(disconnected);
        let (_, receiver) = performables::sync::channel::unbounded();
        let mut pool = JoinableThreadPool {
            jobs: Some(jobs),
            workers: Vec::new(),
            worker_threads: Arc::new(Mutex::new(Vec::new())),
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
        let (pool, _) = joinable_pool(1);
        assert!(pool.execute(|| panic!("direct pool job panic")));
        let (completed, received) = channel();
        assert!(pool.execute(move || completed.send(()).unwrap()));

        received.recv_timeout(TEST_TIMEOUT).unwrap();
        pool.join();
    }

    #[test]
    fn joinable_pool_discards_worker_panic_and_clears_membership() {
        let workers = Arc::new(Mutex::new(Vec::new()));
        let worker = thread::spawn(|| panic!("worker failure"));
        workers.lock().push(worker.thread().id());
        let (_, receiver) = performables::sync::channel::unbounded();
        let mut pool = JoinableThreadPool {
            jobs: None,
            workers: vec![worker],
            worker_threads: Arc::clone(&workers),
            receiver,
            queued_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            active_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        pool.shutdown_and_join();
        assert!(workers.lock().is_empty());
    }

    #[test]
    fn dropping_joinable_pool_clears_worker_membership() {
        let (pool, workers) = joinable_pool(1);
        let (completed, received) = channel();
        assert!(pool.execute(move || completed.send(()).unwrap()));
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(workers.lock().len(), 1);

        drop(pool);
        assert!(workers.lock().is_empty());
    }

    #[test]
    fn joinable_pool_debug_reports_counts() {
        let (pool, _) = joinable_pool(1);

        let debug = format!("{pool:?}");
        assert!(debug.contains("queued_count: 0"));
        assert!(debug.contains("active_count: 0"));
        assert!(debug.contains("max_count: 1"));
        pool.join();
    }

    #[test]
    fn blocking_worker_join_waits_for_tasks_to_complete() {
        let events = Arc::new(Mutex::<Vec<&str>>::new(Vec::new()));
        let (task_start_tx, task_start_rx) = channel();

        let thread_join_handle = thread::spawn({
            let events = Arc::clone(&events);
            move || {
                let worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop(), Shutdown::default());

                drop(worker.spawn_blocking({
                    let events = Arc::clone(&events);
                    move || {
                        events.lock_result().unwrap().push("task started");
                        task_start_tx.send(()).unwrap();
                        events.lock_result().unwrap().push("task finished");
                    }
                }));

                // The sender lives only inside the blocking task. A worker that never runs
                // the task drops it (and its sender) instead, so `recv` observes an
                // immediate disconnect and this test fails deterministically rather than
                // hanging.
                task_start_rx.recv().expect("the worker must run the blocking task");

                worker.runtime_shutdown.begin(ShutdownCause::default());
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

        let worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop(), Shutdown::default());

        worker.runtime_shutdown.begin(ShutdownCause::default());

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
        let worker = BlockingWorker::new(BlockingPool::new(Some(limit(1))), Sink::noop(), runtime_shutdown.clone());
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
        worker.runtime_shutdown.begin(ShutdownCause::default());
        worker.join();
    }

    #[test]
    fn runtime_shutdown_rejects_new_work_before_worker_shutdown() {
        let runtime_shutdown = Shutdown::default();
        runtime_shutdown.begin(ShutdownCause::default());
        let worker = BlockingWorker::new(BlockingPool::new(Some(limit(1))), Sink::noop(), runtime_shutdown);
        let invoked = Arc::new(AtomicBool::new(false));

        let rejected = worker.spawn_blocking({
            let invoked = Arc::clone(&invoked);
            move || invoked.store(true, Ordering::Release)
        });

        assert!(rejected.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Acquire));
        worker.runtime_shutdown.begin(ShutdownCause::default());
        worker.join();
    }

    #[test]
    fn worker_shutdown_rejects_same_pool_join_without_deadlock_guard() {
        let worker = BlockingWorker::new(BlockingPool::new(Some(limit(1))), Sink::noop(), Shutdown::default());
        let _scope = BlockingTaskScope::enter(Arc::clone(&worker.pool.identity));
        worker.runtime_shutdown.begin(ShutdownCause::default());

        let rejected = worker.spawn_blocking(|| 7).join().unwrap_err();

        assert!(rejected.is_shutdown());
        worker.join();
    }

    #[test]
    fn blocking_pool_grow_increases_thread_count() {
        let blocking_pool = BlockingPool::new(None);
        assert!(blocking_pool.grow());
        assert_eq!(
            blocking_pool.pool.lock_result().unwrap().as_ref().unwrap().max_count(),
            BlockingPool::INITIAL_THREAD_COUNT + 1,
            "Number of worker threads should be increased by 1"
        );
    }

    #[test]
    fn blocking_pool_grow_to_maximum() {
        let blocking_pool = BlockingPool::new(Some(limit(5)));

        // It says "max" but it is effectively the "current" count because growth is async.
        assert_eq!(blocking_pool.pool.lock_result().unwrap().as_ref().unwrap().max_count(), 1);
        assert!(blocking_pool.grow()); // 2
        assert!(blocking_pool.grow()); // 3
        assert!(blocking_pool.grow()); // 4
        assert!(blocking_pool.grow()); // 5
        assert!(!blocking_pool.grow()); // 5 - should not grow further

        // It says "max" but it is effectively the "current" count because growth is async.
        assert_eq!(blocking_pool.pool.lock_result().unwrap().as_ref().unwrap().max_count(), 5);
    }

    #[test]
    fn blocking_pool_reports_configured_max_thread_count() {
        // A configured value distinct from both 1 and the initial thread count keeps
        // this assertion honest: the getter must return the exact maximum it was
        // built with, and the default path must fall back to the crate default.
        assert_eq!(BlockingPool::new(Some(limit(7))).max_thread_count(), 7);
        assert_eq!(
            BlockingPool::new(None).max_thread_count(),
            BlockingPool::MAX_THREAD_COUNT_DEFAULT.get()
        );
    }

    #[test]
    fn closed_pool_rejects_work_and_cannot_grow() {
        let pool = BlockingPool::new(Some(limit(2)));
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
            assert!(worker.pool.pool.lock_result().unwrap().is_none());
        })
        .unwrap();
    }

    #[test]
    fn spawn_on_worker() {
        let blocking_pool = BlockingPool::new(None);
        let (sender, receiver) = channel();
        blocking_pool.execute(move || {
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
            let blocking_pool = BlockingPool::new(None);
            let (worker_thread_started_tx, worker_thread_started_rx) = channel();
            let (worker_thread_finished_tx, worker_thread_finished_rx) = Event::<()>::boxed();

            // These are only used by the first thread but the compiler does not know that, so we
            // wrap them in Arc + Mutex + Option to make it safe.
            let worker_thread_finished_rx = Arc::new(Mutex::<Option<_>>::new(Some(worker_thread_finished_rx)));

            for i in 0..BlockingPool::MAX_TASKS_PER_THREAD {
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
                "Worker pool should be at the task limit but not overloaded"
            );

            // This task can be empty, the blocked thread pool won't have a chance to start it anyway
            blocking_pool.execute(move || {});

            assert!(blocking_pool.is_overloaded(), "Worker pool should be overloaded");

            // Now we can release the second barrier and clean up
            worker_thread_finished_tx.send(());
            blocking_pool.join();
        })
        .unwrap();
    }

    #[cfg_attr(test, mutants::skip)] // Test-only helper.
    fn dimension(event: &CapturedEvent, key: &str) -> Option<Value> {
        event.dimensions().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Runs `spawn_blocking` against a pool of the given size, holding one worker
    /// thread busy while `queued_task_count` further tasks are spawned, and returns
    /// the telemetry captured while the saturation guard was evaluated on each spawn.
    #[cfg_attr(test, mutants::skip)] // Test-only helper.
    fn spawn_blocking_telemetry(max_thread_count: Option<usize>, queued_task_count: usize) -> (Vec<CapturedEvent>, Vec<usize>) {
        let (sink, processor) = test_emitter(TEST_ID);
        let worker = BlockingWorker::new(
            BlockingPool::new(max_thread_count.and_then(NonZeroUsize::new)),
            sink,
            Shutdown::default(),
        );

        // Occupy the pool's single initial thread with a task that blocks until released,
        // so every task spawned afterwards stays queued and counts towards the overload
        // threshold instead of completing and disappearing.
        let (blocker_started_tx, blocker_started_rx) = channel();
        let (release_tx, release_rx) = Event::<()>::boxed();
        let release_rx = Arc::new(Mutex::<Option<_>>::new(Some(release_rx)));

        drop(worker.spawn_blocking({
            let release_rx = Arc::clone(&release_rx);
            move || {
                blocker_started_tx.send(()).unwrap();
                futures::executor::block_on(async {
                    let release_rx = release_rx.lock_result().unwrap().take().unwrap();
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
        let mut saturation_counts = Vec::with_capacity(queued_task_count);
        for _ in 0..queued_task_count {
            drop(worker.spawn_blocking(|| {}));
            saturation_counts.push(
                processor
                    .events()
                    .iter()
                    .filter(|event| event.name() == "arty.rt.blocking_worker.pool_saturated")
                    .count(),
            );
        }

        let events = processor.events();

        // Release the blocker so the queued tasks drain, then wait for the pool to settle.
        release_tx.send(());
        worker.runtime_shutdown.begin(ShutdownCause::default());
        worker.join();

        (events, saturation_counts)
    }

    #[test]
    fn spawn_blocking_reports_saturation_when_overloaded_pool_cannot_grow() {
        // A pool capped at one thread cannot grow, so overloading it must surface saturation.
        let (events, saturation_counts) = spawn_blocking_telemetry(Some(1), BlockingPool::MAX_TASKS_PER_THREAD + 1);

        let saturated: Vec<_> = events
            .iter()
            .filter(|event| event.name() == "arty.rt.blocking_worker.pool_saturated")
            .collect();
        assert!(!saturated.is_empty(), "an overloaded pool that cannot grow must report saturation");
        assert_eq!(saturated.len(), 1, "one saturation episode emits one warning");
        assert_eq!(
            saturation_counts[BlockingPool::MAX_TASKS_PER_THREAD - 1],
            1,
            "the first overloaded submission emits the warning, not a later submission"
        );
        assert_eq!(
            dimension(saturated[0], "blocking_worker_pool.max_threads"),
            Some("1".into()),
            "the saturation event reports the pool's maximum thread count"
        );
        assert_eq!(
            dimension(saturated[0], "blocking_worker_pool.mode"),
            Some("shared".into()),
            "the saturation event reports the configured pool mode"
        );
    }

    #[test]
    fn spawn_blocking_reports_each_distinct_saturation_episode() {
        let (sink, processor) = test_emitter(TEST_ID);
        let worker = BlockingWorker::new(BlockingPool::new(Some(limit(1))), sink, Shutdown::default());

        for expected_events in 1..=2 {
            let (blocker_started_tx, blocker_started_rx) = channel();
            let (release_tx, release_rx) = Event::<()>::boxed();
            drop(worker.spawn_blocking(move || {
                blocker_started_tx.send(()).unwrap();
                futures::executor::block_on(release_rx).unwrap();
            }));
            blocker_started_rx.recv_timeout(TEST_TIMEOUT).unwrap();

            for _ in 0..=BlockingPool::MAX_TASKS_PER_THREAD {
                drop(worker.spawn_blocking(|| {}));
            }
            let (completed_tx, completed_rx) = channel();
            drop(worker.spawn_blocking_with_completion(|| {}, move || completed_tx.send(()).unwrap()));
            release_tx.send(());
            completed_rx.recv_timeout(TEST_TIMEOUT).unwrap();
            assert!(
                !worker.pool.saturation_reported.load(Ordering::Acquire),
                "the task wrapper must reset saturation before reporting completion"
            );
            assert_eq!(
                processor
                    .events()
                    .iter()
                    .filter(|event| event.name() == "arty.rt.blocking_worker.pool_saturated")
                    .count(),
                expected_events,
            );
        }

        worker.runtime_shutdown.begin(ShutdownCause::default());
        worker.join();
    }

    #[test]
    fn task_completion_during_overload_does_not_split_the_episode() {
        let (sink, processor) = test_emitter(TEST_ID);
        let worker = BlockingWorker::new(BlockingPool::new(Some(limit(1))), sink, Shutdown::default());
        let (first_started_tx, first_started_rx) = channel();
        let (release_first_tx, release_first_rx) = Event::<()>::boxed();
        drop(worker.spawn_blocking(move || {
            first_started_tx.send(()).unwrap();
            futures::executor::block_on(release_first_rx).unwrap();
        }));
        first_started_rx.recv_timeout(TEST_TIMEOUT).unwrap();

        let (second_started_tx, second_started_rx) = channel();
        let (release_second_tx, release_second_rx) = Event::<()>::boxed();
        drop(worker.spawn_blocking(move || {
            second_started_tx.send(()).unwrap();
            futures::executor::block_on(release_second_rx).unwrap();
        }));
        for _ in 0..BlockingPool::MAX_TASKS_PER_THREAD {
            drop(worker.spawn_blocking(|| {}));
        }
        assert_eq!(
            processor
                .events()
                .iter()
                .filter(|event| event.name() == "arty.rt.blocking_worker.pool_saturated")
                .count(),
            1,
        );

        release_first_tx.send(());
        second_started_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        drop(worker.spawn_blocking(|| {}));
        assert_eq!(
            processor
                .events()
                .iter()
                .filter(|event| event.name() == "arty.rt.blocking_worker.pool_saturated")
                .count(),
            1,
            "a task completion while the queue remains overloaded must not start a new episode"
        );

        release_second_tx.send(());
        worker.runtime_shutdown.begin(ShutdownCause::default());
        worker.join();
    }

    #[test]
    fn spawn_blocking_grows_pool_without_reporting_saturation() {
        let (events, _) = spawn_blocking_telemetry(Some(2), BlockingPool::MAX_TASKS_PER_THREAD + 1);

        assert!(
            events.iter().all(|event| event.name() != "arty.rt.blocking_worker.pool_saturated"),
            "an overloaded pool that can grow must not report saturation"
        );
    }

    #[test]
    fn spawn_blocking_stays_quiet_when_maxed_out_pool_is_not_overloaded() {
        // A pool at its maximum size that is not overloaded must stay quiet: saturation is
        // only reported when an overload coincides with an inability to grow, never on the
        // inability to grow alone.
        let (events, _) = spawn_blocking_telemetry(Some(1), 1);

        assert!(
            events.iter().all(|event| event.name() != "arty.rt.blocking_worker.pool_saturated"),
            "a pool that is merely at its size limit, without being overloaded, must stay quiet"
        );
    }
}
