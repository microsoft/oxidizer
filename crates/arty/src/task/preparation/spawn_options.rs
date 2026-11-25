// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::{TaskCompletion, TaskMetadata, TaskObserver as ExecutorTaskObserver, TaskSet};

use crate::core::Thread;
use crate::runtime::hook::TaskObservers;
/// Captures submission metadata before a task crosses threads.
pub(crate) struct TaskSpawnOptions {
    parent: Option<seismograph::recorder::runtime::TaskId>,
    capture: fn(seismograph_runtime::task::EventContext, Option<seismograph::recorder::runtime::TaskId>, usize) -> TaskMetadata,
    future_size: usize,
    observers: Option<TaskObservers>,
}

impl TaskSpawnOptions {
    pub(crate) fn new<Body: 'static>() -> Self {
        Self::with_future_size::<Body>(size_of::<Body>())
    }

    pub(crate) fn with_future_size<Body: 'static>(future_size: usize) -> Self {
        Self {
            parent: crate::runtime::monitoring::current_task(),
            capture: TaskMetadata::capture::<Body>,
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

    fn register_with<F, T, I>(self, tasks: &TaskSet, future: F, completion: T, observer: I)
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: ExecutorTaskObserver,
    {
        let monitored = crate::runtime::monitoring::current_events().map(|events| (self.capture)(events, self.parent, self.future_size));
        tasks.task(future).observe(observer).monitor(monitored).register(completion);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;

    use arty_executor::Executor;
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
        let executor = Executor::builder().monitor(worker.handle()).build();
        let results = JoinHandlePool::new(Shutdown::default());
        let (_handle, sender) = results.handle();
        let future: ErasedFuture = Box::pin(std::future::pending());
        let destination = ThreadBuilder::default().build(std::thread::current().id());
        options.register(&destination, &executor.tasks(), future, sender);
        let mut snapshots = Vec::new();

        let truncated = executor.inspect_tasks(1, |task| snapshots.push(task));

        assert!(!truncated);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].parent, Some(parent));
        assert_eq!(snapshots[0].future_size_bytes, Some(37));
    }
}
