// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::{TaskCompletion, TaskInstrumentation, TaskSet};
use seismograph_runtime::TaskIdentity;

use crate::core::Thread;
use crate::runtime::hook::TaskObservers;
/// Captures submission metadata before a task crosses threads.
pub(crate) struct SpawnOptions {
    identity: TaskIdentity,
    origin: Option<seismograph_runtime::WorkerOrigin>,
    parent: Option<seismograph::recorder::runtime::TaskId>,
    future_size: usize,
    observers: Option<TaskObservers>,
}

impl SpawnOptions {
    pub(crate) fn new(identity: TaskIdentity, future_size: usize, parent: Option<TaskIdentity>) -> Self {
        Self {
            identity,
            origin: crate::runtime::monitoring::current_origin(),
            parent: parent.map(TaskIdentity::id),
            future_size,
            observers: None,
        }
    }

    pub(crate) const fn identity(&self) -> TaskIdentity {
        self.identity
    }

    pub(crate) fn observed_by(mut self, observers: Option<TaskObservers>) -> Self {
        self.observers = observers;
        self
    }

    pub(crate) fn register<F, T>(mut self, destination: &Thread, tasks: &TaskSet, future: F, completion: T)
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
    {
        let Some(observers) = self.observers.take() else {
            self.register_with(tasks, future, completion, ());
            return;
        };
        let observers = observers.relocate_to(destination);
        match observers.into_single() {
            Ok(observer) => self.register_with(tasks, future, completion, observer),
            Err(observers) => self.register_with(tasks, future, completion, observers),
        }
    }

    fn register_with<F, T, I>(self, tasks: &TaskSet, future: F, completion: T, instrumentation: I)
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: TaskInstrumentation,
    {
        crate::runtime::monitoring::register_monitored(
            tasks,
            future,
            completion,
            instrumentation,
            crate::runtime::monitoring::TaskSubmission {
                identity: self.identity,
                origin: self.origin,
                parent: self.parent,
                body_size: self.future_size,
            },
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;

    use arty_executor::{Executor, InspectionStatus};
    use seismograph_runtime::{Runtime, WorkerMetadata, WorkerRole};
    use thread_aware::ThreadBuilder;

    use super::*;
    use crate::runtime::Shutdown;
    use crate::runtime::worker::asynchronous::LocalWorker;
    use crate::task::handle::JoinHandlePool;

    #[test]
    fn erased_submission_preserves_its_original_size_and_submission_parent() {
        type ErasedFuture = Pin<Box<dyn Future<Output = ()>>>;

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = Runtime::builder("submission-metadata").configured_workers(1).build();
        let worker = runtime.async_worker(WorkerMetadata::new(WorkerRole::Core));
        let parent = TaskIdentity::from_location(std::panic::Location::caller());
        let identity = TaskIdentity::from_location(std::panic::Location::caller());
        LocalWorker::attach(Some(worker));
        let mut executor = Executor::builder().build();
        let results = JoinHandlePool::new(Shutdown::default());
        let options = std::rc::Rc::new(std::cell::RefCell::new(None));
        let submitted = std::rc::Rc::clone(&options);
        let (_handle, sender) = results.handle();
        let parent_tasks = executor.tasks();
        executor
            .tasks()
            .task(parent, async move {
                *submitted.borrow_mut() = Some(SpawnOptions::new(identity, 37, parent_tasks.current()));
            })
            .register(sender);
        let _ = executor.execute_cycle();
        let options = options.borrow_mut().take().unwrap();
        let (_handle, sender) = results.handle();
        let future: ErasedFuture = Box::pin(std::future::pending());
        let destination = ThreadBuilder::default().build(std::thread::current().id());
        let tasks = executor.tasks();
        let (_other_handle, other_sender) = results.handle();
        executor
            .tasks()
            .task(TaskIdentity::from_location(std::panic::Location::caller()), async move {
                options.register(&destination, &tasks, future, sender);
            })
            .register(other_sender);
        let _ = executor.execute_cycle();
        let mut snapshots = Vec::new();

        let status = executor
            .inspect(1, |task: &seismograph_runtime::snapshot::Task| snapshots.push(task.clone()))
            .unwrap();

        assert_eq!(status, InspectionStatus::Complete);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            (snapshots[0].id, snapshots[0].type_descriptor),
            (identity.id(), identity.descriptor())
        );
        assert_eq!(snapshots[0].parent, Some(parent.id()));
        assert_eq!(snapshots[0].future_size_bytes, Some(37));
        let mut identities = Vec::new();
        executor.inspect(1, |identity: &TaskIdentity| identities.push(*identity)).unwrap();
        assert_eq!(identities, [identity]);
    }
}
