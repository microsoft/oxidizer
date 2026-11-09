// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native storage for queued blocking functions and active worker slots.
//! Worker handles are retained on their execution thread. The pool shares one
//! immutable task-event context so submissions do not lock this queue for it.

use std::collections::VecDeque;
#[cfg(feature = "monitor")]
use std::sync::OnceLock;
use std::sync::{Arc, Condvar, Mutex};

#[cfg(feature = "monitor")]
use crate::runtime::monitoring::TaskInfo;

pub(super) type Job = Box<dyn FnOnce() + Send + 'static>;

pub(super) struct QueuedJob {
    pub(super) body: Job,
    pub(super) shutdown: Option<crate::runtime::shutdown::Shutdown>,
    #[cfg(feature = "monitor")]
    pub(super) info: Option<Arc<TaskInfo>>,
}

#[cfg(all(test, feature = "monitor", feature = "test-util"))]
mod tests {
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::snapshot::{CaptureOptions, TaskActivityState, TaskCollection, WorkerState};
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

    use super::*;

    #[test]
    fn blocking_slots_finish_and_retire_without_retaining_a_running_task() {
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("blocking-slots", 1));
        let queue = Arc::new(Queue::default());
        let index = queue.add_worker();
        queue.enable_monitoring(&runtime.handle());
        queue.enable_monitoring(&runtime.handle());
        let info = Arc::new(TaskInfo::new::<()>(*queue.events.get().unwrap()));
        assert!(queue.push(QueuedJob {
            body: Box::new(|| {}),
            shutdown: None,
            info: Some(Arc::clone(&info)),
        }));
        let mut worker = None;
        let job = queue.pop(index, &mut worker).unwrap();
        let active = runtime.handle().capture(CaptureOptions::default()).unwrap();

        queue.finish(index);
        let finished = runtime.handle().capture(CaptureOptions::default()).unwrap();
        queue.retire(index);
        let retired = runtime.handle().capture(CaptureOptions::default()).unwrap();
        drop(job);

        assert_eq!(active.tasks.len(), 1);
        assert_eq!(active.tasks[0].id, info.id());
        assert_eq!(active.tasks[0].activity.unwrap().state, TaskActivityState::Running);
        assert!(finished.tasks.is_empty());
        assert!(retired.tasks.is_empty());
        assert_eq!(retired.workers[0].state, WorkerState::Stopped);
    }

    #[test]
    fn blocking_collection_handles_missing_metadata_limits_and_canceled_requests() {
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("blocking-limits", 1));
        let queue = Arc::new(Queue::default());
        queue.add_worker();
        queue.enable_monitoring(&runtime.handle());
        for info in [None, Some(Arc::new(TaskInfo::new::<()>(*queue.events.get().unwrap())))] {
            assert!(queue.push(QueuedJob {
                body: Box::new(|| {}),
                shutdown: None,
                info,
            }));
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
}

#[derive(Default)]
struct WorkerSlot {
    #[cfg(feature = "monitor")]
    running: bool,
    #[cfg(feature = "monitor")]
    registration: Option<seismograph_runtime::worker::WorkerRegistration>,
    #[cfg(feature = "monitor")]
    current: Option<Arc<TaskInfo>>,
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
    #[cfg(feature = "monitor")]
    pub(super) events: Arc<OnceLock<seismograph_runtime::task::EventContext>>,
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

    pub(super) fn pop(
        &self,
        index: usize,
        #[cfg(feature = "monitor")] worker: &mut Option<seismograph_runtime::worker::WorkerHandle>,
    ) -> Option<QueuedJob> {
        #[cfg(not(feature = "monitor"))]
        let _ = index;
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        loop {
            #[cfg(feature = "monitor")]
            if worker.is_none()
                && let Some(registration) = &state.workers[index].registration
            {
                let handle = registration.handle();
                handle.attach_current_thread();
                *worker = Some(handle);
            }
            if let Some(job) = state.jobs.pop_front() {
                #[cfg(feature = "monitor")]
                {
                    state.workers[index].running = true;
                    state.workers[index].current.clone_from(&job.info);
                }
                return Some(job);
            }
            if state.stopped {
                return None;
            }
            #[cfg(feature = "monitor")]
            if let Some(worker) = worker.as_ref() {
                worker.parked();
            }
            state = self
                .ready
                .wait(state)
                .expect("a blocking queue operation must not panic while holding its lock");
            #[cfg(feature = "monitor")]
            if let Some(worker) = worker.as_ref() {
                worker.unparked();
            }
        }
    }

    #[cfg(feature = "monitor")]
    pub(super) fn finish(&self, index: usize) {
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        state.workers[index].current = None;
        state.workers[index].running = false;
    }

    pub(super) fn stop(&self) {
        self.state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock")
            .stopped = true;
        self.ready.notify_all();
    }

    #[cfg(feature = "monitor")]
    pub(super) fn retire(&self, index: usize) {
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        let registration = state.workers[index].registration.take();
        drop(state);
        drop(registration);
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
        self.events.set(events).expect("monitoring is enabled once under the queue lock");
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
            .then_some((state.workers[index].current.as_ref(), TaskActivityState::Running))
            .into_iter();
        // The first worker owns inspection of the shared submission queue.
        let queued = state
            .jobs
            .iter()
            .take(if index == 0 { usize::MAX } else { 0 })
            .map(|job| (job.info.as_ref(), TaskActivityState::Ready));
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
