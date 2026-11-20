// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::{TaskCompletion, TaskInstrumentation, TaskSet};

use crate::core::Thread;
use crate::runtime::hook::TaskObservers;
/// Captures submission metadata before a task crosses threads.
pub(crate) struct TaskSpawnOptions {
    #[cfg(any(test, feature = "monitor"))]
    parent: Option<seismograph::recorder::runtime::TaskId>,
    #[cfg(any(test, feature = "monitor"))]
    describe_type: fn() -> seismograph::recorder::runtime::TypeDescriptorId,
    #[cfg(any(test, feature = "monitor"))]
    future_size: usize,
    observers: Option<TaskObservers>,
}

impl TaskSpawnOptions {
    pub(crate) fn new<Body: 'static>() -> Self {
        Self::with_future_size::<Body>(size_of::<Body>())
    }

    pub(crate) fn with_future_size<Body: 'static>(future_size: usize) -> Self {
        #[cfg(not(any(test, feature = "monitor")))]
        let _ = future_size;
        Self {
            #[cfg(any(test, feature = "monitor"))]
            parent: crate::runtime::monitoring::current_task(),
            #[cfg(any(test, feature = "monitor"))]
            describe_type: crate::runtime::monitoring::descriptor::<Body>,
            #[cfg(any(test, feature = "monitor"))]
            future_size,
            observers: None,
        }
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

    #[cfg_attr(
        not(any(test, feature = "monitor")),
        expect(clippy::unused_self, reason = "Submission metadata is empty when monitoring is compiled out.")
    )]
    fn register_with<F, T, I>(self, tasks: &TaskSet, future: F, completion: T, instrumentation: I)
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: TaskInstrumentation,
    {
        #[cfg(any(test, feature = "monitor"))]
        crate::runtime::monitoring::register_monitored(
            tasks,
            future,
            completion,
            instrumentation,
            self.parent,
            self.describe_type,
            self.future_size,
        );
        // #[gamma::skip(stmt.delete_call, reason = "This branch is absent from Gamma's all-features builds; runtime-only integration tests exercise it.")]
        #[cfg(not(any(test, feature = "monitor")))]
        tasks.task(future).instrument(instrumentation).register(completion);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;

    use arty_executor::{Executor, InspectionStatus};
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};
    use thread_aware::ThreadBuilder;

    use super::*;
    use crate::runtime::Shutdown;
    use crate::runtime::monitoring::{enter_current, set_events};
    use crate::task::handle::JoinHandlePool;

    #[test]
    fn erased_submission_preserves_its_original_size_and_submission_parent() {
        type ErasedFuture = Pin<Box<dyn Future<Output = ()>>>;

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("submission-metadata", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let parent = seismograph_runtime::allocate_task_id();
        let options = {
            let _parent = enter_current(parent);
            TaskSpawnOptions::with_future_size::<ErasedFuture>(37)
        };
        let _other_parent = enter_current(seismograph_runtime::allocate_task_id());
        set_events(Some(worker.handle().task_events()));
        let _events = scopeguard::guard((), |()| set_events(None));
        let mut executor = Executor::builder().build();
        let results = JoinHandlePool::new(Shutdown::default());
        let (_handle, sender) = results.handle();
        let future: ErasedFuture = Box::pin(std::future::pending());
        let destination = ThreadBuilder::default().build(std::thread::current().id());
        options.register(&destination, &executor.tasks(), future, sender);
        let mut snapshots = Vec::new();

        let status = executor
            .inspect(1, |task: &seismograph_runtime::snapshot::Task| snapshots.push(task.clone()))
            .unwrap();

        assert_eq!(status, InspectionStatus::Complete);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].parent, Some(parent));
        assert_eq!(snapshots[0].future_size_bytes, Some(37));
    }
}
