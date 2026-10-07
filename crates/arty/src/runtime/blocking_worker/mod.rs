// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Blocking-task admission and blocking-pool execution.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use observed::{Sink, emit};
use performables::arc::Arc;
use performables::sync::mutex::Mutex;
use threadpool::ThreadPool;

use crate::runtime::telemetry::events::{BlockingWorkerPoolMode, BlockingWorkerPoolSaturated, SystemMetricCount};
use crate::task::execution::prepare_blocking;
use crate::task::join::JoinHandle;

const ERR_POISONED_LOCK: &str = "poisoned lock - cannot continue execution because security and privacy guarantees can no longer be upheld";

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

/// Worker for blocking tasks. Meant to be created for each async worker thread to allow for scheduling of blocking tasks.
#[derive(Debug)]
pub(crate) struct BlockingWorker {
    pool: BlockingPool,
    is_shutting_down: Arc<AtomicBool>,
    runtime_shutdown: Arc<AtomicBool>,
    sink: Sink,
}

impl BlockingWorker {
    #[cfg(test)]
    pub(in crate::runtime) fn new(pool: BlockingPool, sink: Sink) -> Arc<Self> {
        Self::new_with_shutdown(pool, sink, Arc::new(AtomicBool::new(false)))
    }

    pub(in crate::runtime) fn new_with_shutdown(pool: BlockingPool, sink: Sink, runtime_shutdown: Arc<AtomicBool>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            is_shutting_down: Arc::new(AtomicBool::new(false)),
            runtime_shutdown,
            sink,
        })
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
        let shutdown = Arc::clone(&self.is_shutting_down);
        let runtime_shutdown = Arc::clone(&self.runtime_shutdown);
        let (task, join_handle) = prepare_blocking(body);
        let join_handle = join_handle.blocking_pool(Arc::clone(&self.pool.identity));
        let task = move || {
            let _scope = BlockingTaskScope::enter(identity);
            if shutdown.load(Ordering::Acquire) || runtime_shutdown.load(Ordering::Acquire) {
                drop(task);
            } else {
                task();
            }
            pool.clear_saturation_if_recovered();
            completion();
        };

        if !self.is_shutting_down.load(Ordering::Acquire)
            && !self.runtime_shutdown.load(Ordering::Acquire)
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

    /// Rejects new work and cancels queued work; already-running blocking calls finish.
    pub(in crate::runtime) fn shutdown(&self) {
        self.is_shutting_down.store(true, Ordering::Release);
    }

    pub(in crate::runtime) fn is_current_task(&self) -> bool {
        CURRENT_POOL.with_borrow(|current| current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &self.pool.identity)))
    }

    /// Waits for the currently running blocking tasks to complete.
    #[cfg_attr(test, mutants::skip)] // Impractical to test without overly-expensive timeout logic.
    pub(in crate::runtime) fn join(&self) {
        self.pool.join();
    }
}

pub(crate) fn is_current_blocking_pool(pool: &Arc<()>) -> bool {
    CURRENT_POOL.with_borrow(|current| current.as_ref().is_some_and(|current| Arc::ptr_eq(current, pool)))
}

#[derive(Debug, Clone)]
pub(in crate::runtime) struct BlockingPool {
    pool: Arc<Mutex<Option<ThreadPool>>>,
    identity: Arc<()>,
    saturation_reported: Arc<AtomicBool>,
    max_thread_count: NonZeroUsize,
    mode: &'static str,
}

impl BlockingPool {
    #[cfg(test)]
    #[cfg_attr(test, mutants::skip)]
    pub(in crate::runtime) fn shares_pool_with(&self, other: &Self) -> bool {
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
    pub(in crate::runtime) fn new(max_thread_count: Option<NonZeroUsize>) -> Self {
        Self::new_with_mode(max_thread_count, "shared")
    }

    pub(in crate::runtime) fn new_with_mode(max_thread_count: Option<NonZeroUsize>, mode: &'static str) -> Self {
        // Start with one thread and let the pool grow as needed.
        let thread_pool = ThreadPool::with_name("arty-blocking".to_string(), Self::INITIAL_THREAD_COUNT);

        Self {
            pool: Arc::new(Mutex::new(Some(thread_pool))),
            identity: Arc::new(()),
            saturation_reported: Arc::new(AtomicBool::new(false)),
            max_thread_count: max_thread_count.unwrap_or(Self::MAX_THREAD_COUNT_DEFAULT),
            mode,
        }
    }

    pub(in crate::runtime) fn execute<F>(&self, f: F) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        let pool = self.pool.lock_result().expect(ERR_POISONED_LOCK);
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
        let pool = self.pool.lock_result().expect(ERR_POISONED_LOCK).take();
        if let Some(pool) = pool {
            pool.join();
        }
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

    fn max_thread_count(&self) -> usize {
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

    fn thread_pool_is_overloaded(pool: &ThreadPool) -> bool {
        pool.active_count().saturating_add(pool.queued_count()) > pool.max_count().saturating_mul(Self::MAX_TASKS_PER_THREAD)
    }
}

// Keep the private closed-pool race outside the coverage-excluded test scaffolding.
#[cfg(test)]
#[test]
fn closed_pool_rejects_worker_submission_after_admission() {
    let pool = BlockingPool::new(NonZeroUsize::new(1));
    let worker = BlockingWorker::new(pool.clone(), Sink::noop());
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

    use events_once::Event;
    use observed::{Sink, Value};
    use observed_testing::{CapturedEvent, TEST_ID, test_emitter};
    use performables::arc::Arc;
    use performables::sync::mutex::Mutex;
    use testing_aids::{TEST_TIMEOUT, execute_or_abandon};

    use crate::runtime::blocking_worker::{BlockingPool, BlockingTaskScope, BlockingWorker, CURRENT_POOL, is_current_blocking_pool};

    fn limit(value: usize) -> NonZeroUsize {
        NonZeroUsize::new(value).unwrap()
    }

    #[cfg_attr(test, mutants::skip)]
    pub(in crate::runtime) fn is_blocking_worker_shutting_down(worker: &BlockingWorker) -> bool {
        worker.is_shutting_down.load(Ordering::Acquire)
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
    fn blocking_worker_join_waits_for_tasks_to_complete() {
        let events = Arc::new(Mutex::<Vec<&str>>::new(Vec::new()));
        let (task_start_tx, task_start_rx) = channel();

        let thread_join_handle = thread::spawn({
            let events = Arc::clone(&events);
            move || {
                let worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());

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

                worker.shutdown();
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

        let worker = BlockingWorker::new(BlockingPool::new(None), Sink::noop());

        worker.shutdown();

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
        let runtime_shutdown = Arc::new(AtomicBool::new(false));
        let worker = BlockingWorker::new_with_shutdown(BlockingPool::new(Some(limit(1))), Sink::noop(), Arc::clone(&runtime_shutdown));
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
        runtime_shutdown.store(true, Ordering::Release);
        release_tx.send(());

        running.join().unwrap();
        assert!(queued.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Acquire));
        worker.shutdown();
        worker.join();
    }

    #[test]
    fn runtime_shutdown_rejects_new_work_before_worker_shutdown() {
        let runtime_shutdown = Arc::new(AtomicBool::new(true));
        let worker = BlockingWorker::new_with_shutdown(BlockingPool::new(Some(limit(1))), Sink::noop(), runtime_shutdown);
        let invoked = Arc::new(AtomicBool::new(false));

        let rejected = worker.spawn_blocking({
            let invoked = Arc::clone(&invoked);
            move || invoked.store(true, Ordering::Release)
        });

        assert!(rejected.join().unwrap_err().is_shutdown());
        assert!(!invoked.load(Ordering::Acquire));
        worker.shutdown();
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
        let worker = BlockingWorker::new(BlockingPool::new(max_thread_count.and_then(NonZeroUsize::new)), sink);

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
        worker.shutdown();
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
        let worker = BlockingWorker::new(BlockingPool::new(Some(limit(1))), sink);

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

        worker.shutdown();
        worker.join();
    }

    #[test]
    fn task_completion_during_overload_does_not_split_the_episode() {
        let (sink, processor) = test_emitter(TEST_ID);
        let worker = BlockingWorker::new(BlockingPool::new(Some(limit(1))), sink);
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
        worker.shutdown();
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
