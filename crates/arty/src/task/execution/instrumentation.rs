// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

pub(crate) trait PendingInstrumentation: Send + 'static {
    type Task: TaskInstrumentation;
    fn enqueued(&self);
    fn materialized(self) -> Self::Task;
    fn panicked(self);
}

pub(crate) trait TaskInstrumentation: 'static {
    fn completed(&mut self);
    fn canceled(&mut self);
    fn panicked(&mut self);

    #[cfg(any(test, feature = "seismograph"))]
    fn identity(&self) -> Option<seismograph::recorder::runtime::TaskId>;

    #[cfg(any(test, feature = "seismograph"))]
    fn snapshot(&self, state: arty_executor::TaskState) -> Option<seismograph_runtime::snapshot::Task>;
}

impl PendingInstrumentation for () {
    type Task = ();
    fn enqueued(&self) {}
    fn materialized(self) {}
    fn panicked(self) {}
}

impl TaskInstrumentation for () {
    fn completed(&mut self) {}
    fn canceled(&mut self) {}
    fn panicked(&mut self) {}

    #[cfg(any(test, feature = "seismograph"))]
    fn identity(&self) -> Option<seismograph::recorder::runtime::TaskId> {
        None
    }

    #[cfg(any(test, feature = "seismograph"))]
    fn snapshot(&self, _state: arty_executor::TaskState) -> Option<seismograph_runtime::snapshot::Task> {
        None
    }
}
