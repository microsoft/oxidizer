// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::any::Any;
use std::cell::{Cell, RefCell};

use arty_executor::{Executor, InspectionError, InspectionStatus, PollObserver};
use seismograph_runtime::RuntimeHandle;
use seismograph_runtime::snapshot::TaskCollection;
use seismograph_runtime::task::{TaskPoll, TaskRequest};
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRegistration, WorkerRole};

use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::monitoring::task::{Description, set_current, set_events};

#[derive(Debug)]
pub(crate) struct WorkerDiagnostics {
    _registration: WorkerRegistration,
    handle: WorkerHandle,
    poll: RefCell<Option<TaskPoll>>,
    failed: Cell<bool>,
}

impl WorkerDiagnostics {
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
        let status = collection_status(executor.inspect(request.limit(), |description: &Description| {
            if let Description::Snapshot(task) = description {
                tasks.push(task.clone());
            }
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

impl PollObserver for WorkerDiagnostics {
    fn poll_started(&self, description: &dyn Any) {
        if let Some(Description::Poll(id)) = description.downcast_ref::<Description>() {
            set_current(Some(*id));
            *self.poll.borrow_mut() = Some(self.handle.task_poll_started(*id));
        } else {
            self.failed.set(true);
        }
    }

    fn poll_finished(&self, _completed: bool) {
        set_current(None);
        if let Some(poll) = self.poll.borrow_mut().take() {
            self.handle.task_poll_finished(poll);
        }
    }

    fn inspection_failed(&self, _error: InspectionError) {
        self.failed.set(true);
    }
}

impl Drop for WorkerDiagnostics {
    fn drop(&mut self) {
        set_current(None);
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

        use crate::runtime::monitoring::task::{current_events, current_task};

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("worker-bridge", 1));
        let thread = thread_aware::ThreadBuilder::default().build(std::thread::current().id());
        let dispatcher = DispatcherClient::for_test(&thread);
        let diagnostics = WorkerDiagnostics::new(&runtime.handle(), &dispatcher, &thread);
        let (sender, receiver) = mpsc::channel();
        diagnostics
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
                let request = receiver.recv_timeout(TEST_TIMEOUT).unwrap();
                assert!(diagnostics.is_observed());
                diagnostics.collect(executor, request);
                captured.join().unwrap()
            })
        };

        assert!(current_events().is_some());
        assert!(!seismograph::recorder::without_recording(|| diagnostics.is_observed()));
        let completion = crate::task::handle::JoinHandlePool::new(crate::runtime::shutdown::Shutdown::default());
        let (_handle, sender) = completion.handle();
        let parent = seismograph_runtime::allocate_task_id();
        {
            let _parent = crate::runtime::monitoring::task::enter_current(parent);
            crate::runtime::monitoring::add_task::<(), _, _, _>(
                &executor.tasks(),
                std::future::pending::<()>(),
                arty_executor::TaskPlacement::Any,
                sender,
            );
        }
        let snapshot = capture(&mut executor);
        assert_eq!(snapshot.tasks[0].parent, Some(parent));
        assert_eq!(
            snapshot.tasks[0].activity.unwrap().state,
            seismograph_runtime::snapshot::TaskActivityState::Ready
        );
        diagnostics.parked();
        assert_eq!(capture(&mut executor).workers[0].state, WorkerState::Parked);
        diagnostics.unparked();
        assert_eq!(capture(&mut executor).workers[0].state, WorkerState::Running);

        diagnostics.inspection_failed(InspectionError::Panicked);
        assert_eq!(capture(&mut executor).workers[0].task_collection, TaskCollection::Failed);
        assert_eq!(capture(&mut executor).workers[0].task_collection, TaskCollection::Complete);
        diagnostics.poll_started(&());
        diagnostics.poll_finished(false);
        assert_eq!(capture(&mut executor).workers[0].task_collection, TaskCollection::Failed);

        let (_handle, sender) = completion.handle();
        executor
            .tasks()
            .task(std::future::pending::<()>())
            .inspect(|_, _| Some(Description::Poll(seismograph_runtime::allocate_task_id())))
            .register(sender);
        assert_eq!(capture(&mut executor).tasks.len(), 1);

        let task = seismograph_runtime::allocate_task_id();
        diagnostics.poll_started(&Description::Poll(task));
        assert_eq!(current_task(), Some(task));
        diagnostics.poll_finished(false);
        assert_eq!(current_task(), None);
        diagnostics.poll_started(&Description::Poll(task));
        drop(diagnostics);
        assert_eq!(current_task(), None);
        assert!(current_events().is_none());
        assert_eq!(
            runtime.handle().capture(CaptureOptions::default()).unwrap().workers[0].state,
            WorkerState::Stopped
        );
    }
}
