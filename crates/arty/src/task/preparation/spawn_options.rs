// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::{TaskCompletion, TaskObserver, TaskSet};
use seismograph_runtime::TaskIdentity;

use crate::core::Thread;
use crate::runtime::hook::TaskObservers;
/// Captures submission metadata before a task crosses threads.
pub(crate) struct SpawnOptions {
    identity: TaskIdentity,
    origin: Option<seismograph_runtime::WorkerOrigin>,
    task: Option<seismograph_runtime::asynchronous::TaskHandle>,
    parent: Option<TaskIdentity>,
    future_size: usize,
    observers: Option<TaskObservers>,
}

impl SpawnOptions {
    pub(crate) fn new(identity: TaskIdentity, future_size: usize, parent: Option<TaskIdentity>) -> Self {
        Self {
            identity,
            origin: crate::runtime::monitoring::current_origin(),
            task: None,
            parent,
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

    pub(crate) fn monitor(mut self, runtime: &seismograph_runtime::Runtime) -> Self {
        if self.task.is_none() {
            self.task = runtime.async_task(
                self.origin.take(),
                self.identity,
                self.parent.map(TaskIdentity::id),
                self.future_size,
            );
        }
        self
    }

    pub(crate) fn register<F, T>(self, destination: &Thread, tasks: &TaskSet, future: F, completion: T)
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
    {
        let mut options = crate::runtime::worker::asynchronous::LocalWorker::with_monitoring(|worker| match worker {
            Some(worker) => self.monitor(worker.runtime()),
            None => self,
        });
        let Some(observers) = options.observers.take() else {
            options.register_with(tasks, future, completion, ());
            return;
        };
        let observers = observers.relocate_to(destination);
        match observers.into_single() {
            Ok(observer) => options.register_with(tasks, future, completion, observer),
            Err(observers) => options.register_with(tasks, future, completion, observers),
        }
    }

    fn register_with<F, T, I>(self, tasks: &TaskSet, future: F, completion: T, instrumentation: I)
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: TaskObserver,
    {
        let task = tasks.task(self.identity, future);
        let queued = self
            .task
            .map(|task| task.enqueued(u64::try_from(tasks.len()).expect("live task count fits the platform address space")));
        task.monitor(queued).observe(instrumentation).register(completion);
    }
}
