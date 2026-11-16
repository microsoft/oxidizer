// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native storage for queued blocking functions and active worker slots.
//! Worker handles are retained on their execution thread. The pool shares one
//! immutable task-event context so submissions do not lock this queue for it.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

#[cfg(feature = "monitor")]
use crate::runtime::monitoring::MonitoredBlockingJob;
use crate::runtime::monitoring::{BlockingJob, BlockingPoolMonitoring, BlockingWorkerMonitoring};

pub(super) struct QueuedJob {
    pub(super) body: BlockingJob,
    pub(super) shutdown: Option<crate::runtime::shutdown::Shutdown>,
}

#[derive(Default)]
struct WorkerSlot {
    #[cfg(feature = "monitor")]
    running: bool,
    #[cfg(feature = "monitor")]
    registration: Option<seismograph_runtime::worker::WorkerRegistration>,
    #[cfg(feature = "monitor")]
    current: Option<Arc<dyn MonitoredBlockingJob>>,
}

#[derive(Default)]
struct State {
    jobs: VecDeque<QueuedJob>,
    workers: Vec<WorkerSlot>,
    stopped: bool,
    #[cfg(feature = "monitor")]
    runtime: Option<seismograph_runtime::RuntimeHandle>,
}

#[derive(Default)]
pub(super) struct Queue {
    state: Mutex<State>,
    ready: Condvar,
    pub(super) monitoring: BlockingPoolMonitoring,
}

impl Queue {
    pub(super) fn push(&self, job: QueuedJob) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        if state.stopped {
            drop(state);
            drop(job);
            return false;
        }
        state.jobs.push_back(job);
        drop(state);
        self.ready.notify_one();
        true
    }

    pub(super) fn add_worker(self: &Arc<Self>) -> usize {
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        let index = state.workers.len();
        #[cfg(feature = "monitor")]
        let registration = state.runtime.as_ref().map(|runtime| self.register_worker(runtime, index));
        let slot = WorkerSlot::default();
        #[cfg(feature = "monitor")]
        let slot = WorkerSlot { registration, ..slot };
        state.workers.push(slot);
        index
    }

    #[cfg_attr(
        not(feature = "monitor"),
        expect(
            clippy::needless_pass_by_ref_mut,
            reason = "The worker handle is assigned only when monitoring is compiled in."
        )
    )]
    pub(super) fn pop(&self, index: usize, worker: &mut BlockingWorkerMonitoring) -> Option<QueuedJob> {
        #[cfg(not(feature = "monitor"))]
        let _ = index;
        // Drop the previous job after the queue guard, including on early return.
        #[cfg(feature = "monitor")]
        let mut previous;
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        #[cfg(feature = "monitor")]
        {
            let slot = &mut state.workers[index];
            previous = slot.current.take();
            slot.running = false;
        }
        loop {
            #[cfg(feature = "monitor")]
            if worker.handle.is_none()
                && let Some(registration) = &state.workers[index].registration
            {
                let handle = registration.handle();
                handle.attach_current_thread();
                worker.handle = Some(handle);
            }
            if let Some(job) = state.jobs.pop_front() {
                #[cfg(feature = "monitor")]
                if worker.handle.is_some() {
                    state.workers[index].running = true;
                    state.workers[index].current = job.body.monitoring().cloned();
                }
                return Some(job);
            }
            if state.stopped {
                return None;
            }
            #[cfg(feature = "monitor")]
            if previous.is_some() {
                drop(state);
                drop(previous.take());
                state = self
                    .state
                    .lock()
                    .expect("a blocking queue operation must not panic while holding its lock");
                continue;
            }
            worker.parked();
            state = self
                .ready
                .wait(state)
                .expect("a blocking queue operation must not panic while holding its lock");
            worker.unparked();
        }
    }

    pub(super) fn stop(&self) {
        self.state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock")
            .stopped = true;
        self.ready.notify_all();
    }

    #[cfg_attr(
        not(feature = "monitor"),
        expect(
            clippy::unused_self,
            reason = "There is no worker registration to retire when monitoring is compiled out."
        )
    )]
    pub(super) fn retire(&self, index: usize) {
        #[cfg(not(feature = "monitor"))]
        let _ = index;
        #[cfg(feature = "monitor")]
        {
            let mut state = self
                .state
                .lock()
                .expect("a blocking queue operation must not panic while holding its lock");
            let registration = state.workers[index].registration.take();
            drop(state);
            drop(registration);
        }
    }

    #[cfg(feature = "monitor")]
    pub(super) fn enable_monitoring(self: &Arc<Self>, runtime: &seismograph_runtime::RuntimeHandle) {
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        if state.runtime.is_some() {
            return;
        }
        for (index, slot) in state.workers.iter_mut().enumerate() {
            slot.registration = Some(self.register_worker(runtime, index));
        }
        let events = state
            .workers
            .first()
            .and_then(|slot| slot.registration.as_ref())
            .expect("a blocking pool starts with at least one worker")
            .handle()
            .task_events();
        self.monitoring.enable(events);
        state.runtime = Some(runtime.clone());
        drop(state);
        self.ready.notify_all();
    }

    #[cfg(feature = "monitor")]
    fn register_worker(
        self: &Arc<Self>,
        runtime: &seismograph_runtime::RuntimeHandle,
        index: usize,
    ) -> seismograph_runtime::worker::WorkerRegistration {
        use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

        let registration = runtime.register_worker(WorkerMetadata::new(WorkerRole::Blocking));
        let queue = Arc::downgrade(self);
        registration
            .handle()
            .register_task_provider(move |request| {
                if let Some(queue) = queue.upgrade() {
                    queue.collect(index, request);
                }
            })
            .expect("a newly registered blocking worker has not stopped");
        registration
    }

    #[cfg(feature = "monitor")]
    fn collect(&self, index: usize, request: seismograph_runtime::task::TaskRequest) {
        use seismograph_runtime::snapshot::{TaskActivityState, TaskCollection};

        if request.is_canceled() {
            return;
        }
        let state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        let current = state.workers[index]
            .running
            .then_some((
                state.workers[index].current.as_ref().map(|body| body.info()),
                TaskActivityState::Running,
            ))
            .into_iter();
        // The first worker owns inspection of the shared submission queue.
        let queued = state
            .jobs
            .iter()
            .take(if index == 0 { usize::MAX } else { 0 })
            .map(|job| (job.body.monitoring().map(|body| body.info()), TaskActivityState::Ready));
        let mut descriptions = current.chain(queued);
        let tasks = descriptions
            .by_ref()
            .take(request.limit())
            .filter_map(|(info, activity)| info.map(|info| info.describe(activity)))
            .collect();
        let status = if descriptions.next().is_some() {
            TaskCollection::Truncated
        } else {
            TaskCollection::Complete
        };
        drop(state);
        request.respond(tasks, status);
    }
}

#[cfg(all(test, feature = "monitor", feature = "test-util"))]
mod tests {
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::snapshot::{CaptureOptions, TaskActivityState, TaskCollection, WorkerState};
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

    use super::*;
    use crate::runtime::monitoring::TaskInfo;

    struct TestJob(TaskInfo);

    impl MonitoredBlockingJob for TestJob {
        fn info(&self) -> &TaskInfo {
            &self.0
        }

        fn run(&self) {}
    }

    #[test]
    fn blocking_slots_advance_and_retire_without_retaining_a_running_task() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("blocking-slots", 1));
        let queue = Arc::new(Queue::default());
        let index = queue.add_worker();
        queue.enable_monitoring(&runtime.handle());
        queue.enable_monitoring(&runtime.handle());
        let body: Arc<dyn MonitoredBlockingJob> = Arc::new(TestJob(TaskInfo::new::<()>(queue.monitoring.events().unwrap())));
        assert!(queue.push(QueuedJob {
            body: BlockingJob::Monitored(Arc::clone(&body)),
            shutdown: None,
        }));
        let mut worker = BlockingWorkerMonitoring::default();
        let job = queue.pop(index, &mut worker).unwrap();
        let active = runtime.handle().capture(CaptureOptions::default()).unwrap();

        assert!(queue.push(QueuedJob {
            body: BlockingJob::Plain(Box::new(|| {})),
            shutdown: None,
        }));
        drop(job);
        let next = queue.pop(index, &mut worker).unwrap();
        let finished = runtime.handle().capture(CaptureOptions::default()).unwrap();
        drop(next);
        queue.stop();
        assert!(queue.pop(index, &mut worker).is_none());
        let stopped = runtime.handle().capture(CaptureOptions::default()).unwrap();
        queue.retire(index);
        let retired = runtime.handle().capture(CaptureOptions::default()).unwrap();

        assert_eq!(active.tasks.len(), 1);
        assert_eq!(active.tasks[0].id, body.info().id());
        assert_eq!(active.tasks[0].activity.unwrap().state, TaskActivityState::Running);
        assert!(finished.tasks.is_empty());
        assert!(stopped.tasks.is_empty());
        assert!(retired.tasks.is_empty());
        assert_eq!(retired.workers[0].state, WorkerState::Stopped);
    }

    #[test]
    fn dequeuing_or_stopping_releases_a_canceled_capture_outside_the_queue_lock() {
        struct Capture(Arc<Queue>, std::sync::mpsc::Sender<bool>);

        impl Drop for Capture {
            fn drop(&mut self) {
                self.1.send(self.0.state.try_lock().is_ok()).unwrap();
            }
        }

        for stopped in [false, true] {
            let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
            let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("blocking-capture-drop", 1));
            let queue = Arc::new(Queue::default());
            let index = queue.add_worker();
            queue.enable_monitoring(&runtime.handle());
            let (dropped, received) = std::sync::mpsc::channel();
            let capture = Capture(Arc::clone(&queue), dropped);
            let pool = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
            let (body, handle) = crate::task::preparation::prepare_blocking(
                &pool,
                move || drop(capture),
                queue.monitoring.task::<()>(&crate::runtime::shutdown::Shutdown::default()),
            );
            assert!(queue.push(QueuedJob { body, shutdown: None }));
            let mut worker = BlockingWorkerMonitoring::default();
            drop(queue.pop(index, &mut worker).unwrap());

            if stopped {
                queue.stop();
            } else {
                assert!(queue.push(QueuedJob {
                    body: BlockingJob::Plain(Box::new(|| {})),
                    shutdown: None,
                }));
            }
            let next = queue.pop(index, &mut worker);

            assert_eq!(next.is_none(), stopped);
            assert!(received.try_recv().unwrap());
            assert!(handle.join().unwrap_err().is_shutdown());
        }
    }

    #[test]
    fn an_idle_blocking_worker_releases_its_canceled_capture_before_waiting() {
        struct Capture(Arc<Queue>, std::sync::mpsc::Sender<bool>);

        impl Drop for Capture {
            fn drop(&mut self) {
                let mut state = self.0.state.try_lock().unwrap();
                let released = state.workers[0].current.is_none() && !state.workers[0].running;
                state.stopped = true;
                self.1.send(released).unwrap();
            }
        }

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("idle-blocking-drop", 1));
        let queue = Arc::new(Queue::default());
        let index = queue.add_worker();
        queue.enable_monitoring(&runtime.handle());
        let (dropped, received) = std::sync::mpsc::channel();
        let capture = Capture(Arc::clone(&queue), dropped);
        let pool = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (body, handle) = crate::task::preparation::prepare_blocking(
            &pool,
            move || drop(capture),
            queue.monitoring.task::<()>(&crate::runtime::shutdown::Shutdown::default()),
        );
        assert!(queue.push(QueuedJob { body, shutdown: None }));
        let mut worker = BlockingWorkerMonitoring::default();
        drop(queue.pop(index, &mut worker).unwrap());

        assert!(queue.pop(index, &mut worker).is_none());

        assert!(received.try_recv().unwrap());
        assert!(handle.join().unwrap_err().is_shutdown());
    }

    #[test]
    fn blocking_collection_handles_missing_metadata_limits_and_canceled_requests() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("blocking-limits", 1));
        let queue = Arc::new(Queue::default());
        queue.add_worker();
        queue.enable_monitoring(&runtime.handle());
        for body in [
            BlockingJob::Plain(Box::new(|| {})),
            BlockingJob::Monitored(Arc::new(TestJob(TaskInfo::new::<()>(queue.monitoring.events().unwrap())))),
        ] {
            assert!(queue.push(QueuedJob { body, shutdown: None }));
        }
        let truncated = runtime
            .handle()
            .capture(CaptureOptions {
                limit_per_worker: 1,
                ..CaptureOptions::default()
            })
            .unwrap();
        let complete = runtime.handle().capture(CaptureOptions::default()).unwrap();

        let source = seismograph_runtime::register_runtime(RuntimeMetadata::new("canceled-blocking-request", 1));
        let worker = source.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let (sender, receiver) = std::sync::mpsc::channel();
        worker
            .handle()
            .register_task_provider(move |request| sender.send(request).unwrap())
            .unwrap();
        source.handle().capture_with_timeout(CaptureOptions::default()).unwrap();
        queue.collect(0, receiver.try_recv().unwrap());

        assert!(truncated.tasks.is_empty());
        assert_eq!(truncated.workers[0].task_collection, TaskCollection::Truncated);
        assert_eq!(complete.tasks.len(), 1);
        assert_eq!(complete.tasks[0].activity.unwrap().state, TaskActivityState::Ready);
        assert_eq!(complete.workers[0].task_collection, TaskCollection::Complete);
    }

    #[test]
    fn a_dropped_blocking_queue_reports_collection_failure() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("dropped-blocking-queue", 1));
        let queue = Arc::new(Queue::default());
        let worker = queue.register_worker(&runtime.handle(), 0);
        drop(queue);

        let snapshot = runtime.handle().capture(CaptureOptions::default()).unwrap();
        assert!(snapshot.tasks.is_empty());
        assert_eq!(snapshot.workers[0].task_collection, TaskCollection::Failed);
        drop(worker);
    }
}
