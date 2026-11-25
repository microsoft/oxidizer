// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::Executor;
use seismograph_runtime::RuntimeHandle;
use seismograph_runtime::snapshot::TaskCollection;
use seismograph_runtime::task::TaskRequest;
use seismograph_runtime::worker::{WorkerHandle, WorkerMetadata, WorkerRegistration, WorkerRole};

use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::monitoring::task::set_events;

#[derive(Debug)]
pub(crate) struct WorkerMonitoring {
    _registration: WorkerRegistration,
    handle: WorkerHandle,
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
        }
    }

    pub(crate) fn handle(&self) -> &WorkerHandle {
        &self.handle
    }

    pub(crate) fn collect(&self, executor: &Executor, request: TaskRequest) {
        if request.is_canceled() {
            return;
        }
        let mut tasks = Vec::new();
        let truncated = executor.inspect_tasks(request.limit(), |task| tasks.push(task));
        request.respond(
            tasks,
            if truncated {
                TaskCollection::Truncated
            } else {
                TaskCollection::Complete
            },
        );
    }

    pub(crate) fn parked(&self) {
        self.handle.parked();
    }

    pub(crate) fn unparked(&self) {
        self.handle.unparked();
    }
}

impl Drop for WorkerMonitoring {
    fn drop(&mut self) {
        set_events(None);
    }
}
