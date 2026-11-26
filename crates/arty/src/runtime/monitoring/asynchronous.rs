// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::cell::{Cell, RefCell};

use arty_executor::{Executor, InspectionError, InspectionStatus, PollObserver, TaskIdentity};
use seismograph_runtime::RuntimeHandle;
use seismograph_runtime::snapshot::TaskCollection;
use seismograph_runtime::task::{TaskPoll, TaskRequest};
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRegistration, WorkerRole};

use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::monitoring::task::set_events;

#[derive(Debug)]
pub(crate) struct WorkerMonitoring {
    _registration: WorkerRegistration,
    handle: WorkerHandle,
    poll: RefCell<Option<TaskPoll>>,
    failed: Cell<bool>,
}

impl WorkerMonitoring {
    pub(crate) fn new(runtime: &RuntimeHandle, dispatcher: &DispatcherClient, thread: &thread_aware::Thread) -> Self {
        let registration = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let handle = registration.handle();
        dispatcher.register_task_provider(thread.id(), &handle);
        registration.attach_current_thread();
        set_events(Some(handle.task_events()));
        Self {
            _registration: registration,
            handle,
            poll: RefCell::new(None),
            failed: Cell::new(false),
        }
    }

    pub(crate) fn is_observed(&self) -> bool {
        self.handle.is_observed()
    }

    pub(crate) fn collect(&self, executor: &mut Executor, request: TaskRequest) {
        if request.is_canceled() {
            return;
        }
        let mut tasks = Vec::new();
        let status = collection_status(executor.inspect(request.limit(), |task: &seismograph_runtime::snapshot::Task| {
            tasks.push(task.clone());
        }));
        let status = if self.failed.replace(false) {
            TaskCollection::Failed
        } else {
            status
        };
        request.respond(tasks, status);
    }

    pub(crate) fn parked(&self) {
        self.handle.parked();
    }

    pub(crate) fn unparked(&self) {
        self.handle.unparked();
    }
}

impl PollObserver for WorkerMonitoring {
    fn poll_started(&self, description: &dyn Any) {
        if let Some(identity) = description.downcast_ref::<TaskIdentity>() {
            *self.poll.borrow_mut() = Some(self.handle.task_poll_started(identity.id()));
        } else {
            self.failed.set(true);
        }
    }

    // #[gamma::skip(parameter.default_shadow, reason = "The observer deliberately ignores completion; the completion target records the outcome.")]
    fn poll_finished(&self) {
        if let Some(poll) = self.poll.borrow_mut().take() {
            self.handle.task_poll_finished(poll);
        }
    }

    fn inspection_failed(&self) {
        self.failed.set(true);
    }
}

impl Drop for WorkerMonitoring {
    fn drop(&mut self) {
        set_events(None);
    }
}

fn collection_status(result: Result<InspectionStatus, InspectionError>) -> TaskCollection {
    match result {
        Ok(InspectionStatus::Complete) => TaskCollection::Complete,
        Ok(InspectionStatus::Truncated) => TaskCollection::Truncated,
        Err(InspectionError::Busy) => TaskCollection::Busy,
        Err(InspectionError::InvalidLimit | InspectionError::Panicked) => TaskCollection::Failed,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(all(feature = "test-util", not(miri)))]
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn inspection_outcomes_preserve_completeness_and_failure() {
        assert_eq!(
            [
                Ok(InspectionStatus::Complete),
                Ok(InspectionStatus::Truncated),
                Err(InspectionError::Busy),
                Err(InspectionError::InvalidLimit),
                Err(InspectionError::Panicked),
            ]
            .map(collection_status),
            [
                TaskCollection::Complete,
                TaskCollection::Truncated,
                TaskCollection::Busy,
                TaskCollection::Failed,
                TaskCollection::Failed,
            ]
        );
    }

    #[cfg(all(feature = "test-util", not(miri)))]
    #[test]
    fn worker_bridge_preserves_demand_failures_and_thread_local_lifecycle() {
        use seismograph_runtime::RuntimeMetadata;
        use seismograph_runtime::snapshot::{CaptureOptions, WorkerState};
        use testing_aids::TEST_TIMEOUT;

        use crate::runtime::monitoring::task::current_events;

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("worker-bridge", 1));
        let thread = thread_aware::ThreadBuilder::default().build(std::thread::current().id());
        let dispatcher = DispatcherClient::for_test(&thread);
        let monitoring = WorkerMonitoring::new(&runtime.handle(), &dispatcher, &thread);
        let (sender, receiver) = mpsc::channel();
        monitoring
            .handle
            .register_task_provider(move |request| sender.send(request).unwrap())
            .unwrap();
        let mut executor = Executor::builder().build();
        let capture = |executor: &mut Executor| {
            std::thread::scope(|scope| {
                let handle = runtime.handle();
                let captured = scope.spawn(move || {
                    handle
                        .capture(CaptureOptions {
                            timeout: TEST_TIMEOUT.min(std::time::Duration::from_secs(5)),
                            ..CaptureOptions::default()
                        })
                        .unwrap()
                });
                let request = if testing_aids::is_mutation_testing() {
                    receiver.recv().unwrap()
                } else {
                    receiver.recv_timeout(TEST_TIMEOUT).unwrap()
                };
                assert!(monitoring.is_observed());
                monitoring.collect(executor, request);
                captured.join().unwrap()
            })
        };

        assert!(current_events().is_some());
        assert_eq!(
            capture(&mut executor).workers[0].thread_id,
            Some(seismograph::recorder::thread_id())
        );
        assert!(!seismograph::recorder::without_recording(|| monitoring.is_observed()));
        let completion = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (_handle, sender) = completion.handle();
        let parent = TaskIdentity::from_location(std::panic::Location::caller());
        let tasks = executor.tasks();
        let (_parent_handle, parent_sender) = completion.handle();
        executor
            .tasks()
            .task(parent, async move {
                crate::task::preparation::SpawnOptions::new(
                    TaskIdentity::from_location(std::panic::Location::caller()),
                    0,
                    tasks.current(),
                )
                .register(&thread, &tasks, std::future::pending::<()>(), sender);
            })
            .register(parent_sender);
        let _ = executor.execute_cycle();
        let snapshot = capture(&mut executor);
        assert_eq!(snapshot.tasks[0].parent, Some(parent.id()));
        assert_eq!(
            snapshot.tasks[0].activity.unwrap().state,
            seismograph_runtime::snapshot::TaskActivityState::Ready
        );
        monitoring.parked();
        assert_eq!(capture(&mut executor).workers[0].state, WorkerState::Parked);
        monitoring.unparked();
        assert_eq!(capture(&mut executor).workers[0].state, WorkerState::Running);

        monitoring.inspection_failed();
        runtime.handle().capture_with_timeout(CaptureOptions::default()).unwrap();
        monitoring.collect(&mut executor, receiver.try_recv().unwrap());
        assert_eq!(capture(&mut executor).workers[0].task_collection, TaskCollection::Failed);
        assert_eq!(capture(&mut executor).workers[0].task_collection, TaskCollection::Complete);
        monitoring.poll_started(&());
        monitoring.poll_finished();
        assert_eq!(capture(&mut executor).workers[0].task_collection, TaskCollection::Failed);

        let (_handle, sender) = completion.handle();
        executor
            .tasks()
            .task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                std::future::pending::<()>(),
            )
            .inspect(|_| Some(TaskIdentity::from_location(std::panic::Location::caller())))
            .register(sender);
        assert_eq!(capture(&mut executor).tasks.len(), 1);

        let task = TaskIdentity::from_location(std::panic::Location::caller());
        monitoring.poll_started(&task);
        assert_eq!(executor.tasks().current(), None);
        monitoring.poll_finished();
        assert_eq!(executor.tasks().current(), None);
        monitoring.poll_started(&task);
        drop(monitoring);
        assert_eq!(executor.tasks().current(), None);
        assert!(current_events().is_none());
        assert_eq!(
            runtime.handle().capture(CaptureOptions::default()).unwrap().workers[0].state,
            WorkerState::Stopped
        );
    }
}
