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
        let status = match executor.inspect(request.limit(), |description: &Description| {
            if let Description::Snapshot(task) = description {
                tasks.push(task.clone());
            }
        }) {
            Ok(InspectionStatus::Complete) => TaskCollection::Complete,
            Ok(InspectionStatus::Truncated) => TaskCollection::Truncated,
            Err(InspectionError::Busy) => TaskCollection::Busy,
            Err(InspectionError::InvalidLimit | InspectionError::Panicked) => TaskCollection::Failed,
        };
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
