// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;

use arty_executor::{TaskCompletion, TaskPlacement, TaskSet};

/// Captures submission metadata before a task crosses threads.
pub(crate) struct TaskMonitoring<Body, S = ()> {
    #[cfg(any(test, feature = "monitor"))]
    parent: Option<seismograph::recorder::runtime::TaskId>,
    #[cfg(any(test, feature = "monitor"))]
    body_size: S,
    marker: PhantomData<fn() -> (Body, S)>,
}

impl<Body: 'static> TaskMonitoring<Body> {
    pub(crate) fn capture() -> TaskMonitoring<Body, impl Fn() -> usize + Send> {
        // A function item stores no bytes; only erased scoped futures carry a size.
        TaskMonitoring::new(size_of::<Body>)
    }

    pub(crate) fn with_body_size(body_size: usize) -> TaskMonitoring<Body, impl Fn() -> usize + Send> {
        TaskMonitoring::new(move || body_size)
    }
}

impl<Body, S> TaskMonitoring<Body, S> {
    fn new(body_size: S) -> Self {
        #[cfg(not(any(test, feature = "monitor")))]
        drop(body_size);
        Self {
            #[cfg(any(test, feature = "monitor"))]
            parent: super::task::current_task(),
            #[cfg(any(test, feature = "monitor"))]
            body_size,
            marker: PhantomData,
        }
    }
}

impl<Body, S> TaskMonitoring<Body, S>
where
    Body: 'static,
    S: Fn() -> usize + 'static,
{
    #[cfg_attr(
        not(any(test, feature = "monitor")),
        expect(clippy::unused_self, reason = "Submission metadata is empty when monitoring is compiled out.")
    )]
    pub(crate) fn register<F, R, T>(self, tasks: &TaskSet, future: F, placement: TaskPlacement, completion: T)
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
        T: TaskCompletion<R>,
    {
        #[cfg(any(test, feature = "monitor"))]
        super::task::add_task_with_metadata::<Body, _, _, _, _>(tasks, future, placement, completion, self.parent, self.body_size);
        // #[gamma::skip(stmt.delete_call, reason = "This branch is absent from Gamma's all-features builds; runtime-only integration tests exercise it.")]
        #[cfg(not(any(test, feature = "monitor")))]
        tasks.task(future).placement(placement).register(completion);
    }
}

pub(crate) fn add_task<Body, F, R, T>(tasks: &TaskSet, future: F, placement: TaskPlacement, completion: T)
where
    Body: 'static,
    F: IntoFuture<Output = R> + 'static,
    R: 'static,
    T: TaskCompletion<R>,
{
    TaskMonitoring::<Body>::capture().register(tasks, future, placement, completion);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;

    use arty_executor::{Executor, InspectionStatus};
    use seismograph_runtime::RuntimeMetadata;
    use seismograph_runtime::worker::{WorkerMetadata, WorkerRole};

    use super::*;
    use crate::runtime::monitoring::task::{Description, enter_current, set_events};
    use crate::runtime::shutdown::Shutdown;
    use crate::task::handle::JoinHandlePool;

    #[test]
    fn erased_submission_preserves_its_original_size_and_submission_parent() {
        type ErasedFuture = Pin<Box<dyn Future<Output = ()>>>;

        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(RuntimeMetadata::new("submission-metadata", 1));
        let worker = runtime.register_worker(WorkerMetadata::new(WorkerRole::Core));
        let parent = seismograph_runtime::allocate_task_id();
        let monitoring = {
            let _parent = enter_current(parent);
            TaskMonitoring::<ErasedFuture>::with_body_size(37)
        };
        let _other_parent = enter_current(seismograph_runtime::allocate_task_id());
        set_events(Some(worker.handle().task_events()));
        let _events = scopeguard::guard((), |()| set_events(None));
        let mut executor = Executor::builder().build();
        let results = JoinHandlePool::new(Shutdown::default());
        let (_handle, sender) = results.handle();
        let future: ErasedFuture = Box::pin(std::future::pending());
        monitoring.register(&executor.tasks(), future, TaskPlacement::Any, sender);
        let mut snapshots = Vec::new();

        let status = executor
            .inspect(1, |description: &Description| {
                if let Description::Snapshot(task) = description {
                    snapshots.push(task.clone());
                }
            })
            .unwrap();

        assert_eq!(status, InspectionStatus::Complete);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].parent, Some(parent));
        assert_eq!(snapshots[0].future_size_bytes, Some(37));
    }
}
