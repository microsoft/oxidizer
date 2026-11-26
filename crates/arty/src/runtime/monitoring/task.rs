// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use arty_executor::{TaskCompletion, TaskInstrumentation, TaskSet, TaskState};
use seismograph::recorder::event::EventTimestamp;
use seismograph::recorder::runtime::TaskId;
use seismograph_runtime::asynchronous::{Poll as TaskPoll, QueuedTask, Task, Worker};
use seismograph_runtime::snapshot::{Task as TaskSnapshot, TaskActivityState};
use seismograph_runtime::{TaskIdentity, TaskOutcome, WorkerOrigin};

pub(crate) fn current_worker() -> Option<Worker> {
    crate::runtime::worker::asynchronous::LocalWorker::monitoring()
}

pub(crate) fn current_origin() -> Option<WorkerOrigin> {
    current_worker()
        .map(|worker| worker.origin())
        .or_else(|| crate::runtime::worker::blocking::LocalWorker::monitoring().map(|worker| worker.origin()))
}

#[derive(Debug)]
struct AsyncTaskInfo {
    queued: Option<QueuedTask>,
    task: Option<Task>,
    poll: Option<TaskPoll>,
    parent: Option<TaskId>,
    enqueued_at: Option<EventTimestamp>,
}

impl AsyncTaskInfo {
    fn new(tasks: &TaskSet, identity: TaskIdentity, origin: Option<WorkerOrigin>, parent: Option<TaskId>, body_size: usize) -> Self {
        let queued = crate::runtime::worker::asynchronous::LocalWorker::monitoring().and_then(|worker| {
            worker.runtime().enqueue_async(
                origin,
                identity,
                parent,
                body_size,
                u64::try_from(tasks.len()).expect("live task count fits the platform address space"),
            )
        });
        let enqueued_at = queued.as_ref().map(QueuedTask::enqueued_at);
        Self {
            queued,
            task: None,
            poll: None,
            parent,
            enqueued_at,
        }
    }

    fn describe(&self, identity: &TaskIdentity, state: TaskActivityState, body_size: usize) -> TaskSnapshot {
        let mut task = TaskSnapshot::new(identity.id(), identity.descriptor(), state);
        task.parent = self.parent;
        task.spawned_at = self.enqueued_at;
        task.future_size_bytes = Some(u64::try_from(body_size).expect("a task body fits the platform address space"));
        task
    }

    fn poll_started(&mut self) {
        if let Some(worker) = crate::runtime::worker::asynchronous::LocalWorker::monitoring() {
            if let Some(queued) = self.queued.take() {
                self.task = Some(queued.activated(&worker));
            }
            if let Some(task) = self.task.take() {
                self.poll = Some(task.poll_started(&worker));
            }
        }
    }

    fn poll_finished(&mut self, outcome: Option<TaskOutcome>) {
        if let Some(worker) = crate::runtime::worker::asynchronous::LocalWorker::monitoring() {
            if let Some(poll) = self.poll.take() {
                self.task = poll.finished(&worker, outcome);
            }
        }
    }
}

#[derive(Debug)]
struct TaskMonitor<I> {
    instrumentation: I,
    info: AsyncTaskInfo,
    body_size: usize,
}

impl<I> TaskInstrumentation for TaskMonitor<I>
where
    I: TaskInstrumentation,
{
    fn inspect(&self, identity: &TaskIdentity, state: TaskState, visitor: &mut dyn FnMut(&dyn std::any::Any)) {
        visitor(identity);
        match state {
            TaskState::Ready | TaskState::Waiting => {
                let activity = if state == TaskState::Ready {
                    TaskActivityState::Ready
                } else {
                    TaskActivityState::Waiting
                };
                let task = seismograph::recorder::without_recording(|| self.info.describe(identity, activity, self.body_size));
                visitor(&task);
            }
            TaskState::Running => {}
        }
    }

    fn poll_start(&mut self) {
        self.info.poll_started();
        self.instrumentation.poll_start();
    }

    fn poll_end(&mut self, outcome: std::task::Poll<()>) {
        if outcome.is_pending() {
            self.info.poll_finished(None);
        }
        self.instrumentation.poll_end(outcome);
    }

    fn completed(&mut self, identity: &TaskIdentity) {
        self.info.poll_finished(Some(TaskOutcome::Completed));
        self.instrumentation.completed(identity);
    }

    fn panicked(&mut self, identity: &TaskIdentity) {
        self.info.poll_finished(Some(TaskOutcome::Panicked));
        self.instrumentation.panicked(identity);
    }

    fn cancelled(&mut self, identity: &TaskIdentity) {
        self.info.poll_finished(Some(TaskOutcome::Cancelled));
        self.info.task.take();
        self.info.queued.take();
        self.instrumentation.cancelled(identity);
    }
}

pub(crate) fn register_monitored<F, T, I>(tasks: &TaskSet, future: F, completion: T, instrumentation: I, submission: TaskSubmission)
where
    F: IntoFuture + 'static,
    F::Output: 'static,
    T: TaskCompletion<F::Output>,
    I: TaskInstrumentation,
{
    let TaskSubmission {
        identity,
        origin,
        parent,
        body_size,
    } = submission;
    let monitor = TaskMonitor {
        instrumentation,
        info: AsyncTaskInfo::new(tasks, identity, origin, parent, body_size),
        body_size,
    };
    tasks.task(identity, future).instrument(monitor).register(completion);
}

pub(crate) struct TaskSubmission {
    pub(crate) identity: TaskIdentity,
    pub(crate) origin: Option<WorkerOrigin>,
    pub(crate) parent: Option<TaskId>,
    pub(crate) body_size: usize,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use arty_executor::TaskInstrumentation;
    use seismograph_runtime::{Runtime, TaskIdentity};

    use super::{AsyncTaskInfo, TaskMonitor};

    struct Recorder(Rc<RefCell<Vec<&'static str>>>);

    impl TaskInstrumentation for Recorder {
        fn poll_start(&mut self) {
            self.0.borrow_mut().push("start");
        }

        fn poll_end(&mut self, _outcome: std::task::Poll<()>) {
            self.0.borrow_mut().push("end");
        }

        fn completed(&mut self, _identity: &TaskIdentity) {
            self.0.borrow_mut().push("completed");
        }

        fn panicked(&mut self, _identity: &TaskIdentity) {
            self.0.borrow_mut().push("panicked");
        }

        fn cancelled(&mut self, _identity: &TaskIdentity) {
            self.0.borrow_mut().push("cancelled");
        }
    }

    #[test]
    fn task_monitor_forwards_instrumentation_callbacks() {
        let runtime = Runtime::builder("instrumentation-forwarding").configured_workers(1).build();
        let worker = runtime.async_worker(seismograph_runtime::WorkerMetadata::new(seismograph_runtime::WorkerRole::Core));
        crate::runtime::worker::asynchronous::LocalWorker::attach(Some(worker));
        let calls = Rc::new(RefCell::new(Vec::new()));
        let identity = TaskIdentity::from_location(std::panic::Location::caller());
        let executor = arty_executor::Executor::builder().build();
        let mut monitor = TaskMonitor {
            instrumentation: Recorder(Rc::clone(&calls)),
            info: AsyncTaskInfo::new(&executor.tasks(), identity, None, None, 0),
            body_size: 0,
        };

        monitor.poll_start();
        monitor.poll_end(std::task::Poll::Pending);
        monitor.poll_start();
        monitor.poll_end(std::task::Poll::Ready(()));
        monitor.completed(&identity);

        assert_eq!(*calls.borrow(), ["start", "end", "start", "end", "completed"]);
    }
}
