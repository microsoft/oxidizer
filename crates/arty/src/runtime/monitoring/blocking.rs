// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::blocking::{Execution, QueuedTask, TaskHandle, Worker};

use crate::task::preparation::BlockingTask;

pub(crate) enum BlockingJob {
    Plain(Box<dyn FnOnce() + Send + 'static>),
    Monitored(Box<dyn MonitoredBlockingJob>),
}

impl BlockingJob {
    pub(crate) fn prepare<F, R>(task: BlockingTask<F, R>, monitoring: Option<TaskHandle>) -> Self
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        match monitoring {
            Some(monitoring) => Self::Monitored(Box::new(MonitoredTask {
                task: Some(task),
                monitoring: Some(monitoring),
                queued: None,
                execution: None,
            })),
            None => Self::Plain(Box::new(move || task.run(|_| {}))),
        }
    }

    pub(crate) fn enqueue(&mut self, queue_len: u64) {
        if let Self::Monitored(body) = self {
            body.enqueue(queue_len);
        }
    }

    pub(crate) fn started_on(&mut self, worker: &Worker) {
        if let Self::Monitored(body) = self {
            body.started_on(worker);
        }
    }

    pub(crate) fn run(self, canceled: bool) -> std::thread::Result<()> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if canceled {
                drop(self);
            } else {
                match self {
                    Self::Plain(body) => body(),
                    Self::Monitored(body) => body.run(),
                }
            }
        }))
    }
}

pub(crate) trait MonitoredBlockingJob: Send {
    fn enqueue(&mut self, queue_len: u64);
    fn started_on(&mut self, worker: &Worker);
    fn run(self: Box<Self>);
}

struct MonitoredTask<F, R: Send + 'static> {
    task: Option<BlockingTask<F, R>>,
    monitoring: Option<TaskHandle>,
    queued: Option<QueuedTask>,
    execution: Option<Execution>,
}

impl<F, R> MonitoredBlockingJob for MonitoredTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn enqueue(&mut self, queue_len: u64) {
        let monitoring = self.monitoring.take().expect("a blocking task enters its queue once");
        self.queued = Some(monitoring.enqueued(queue_len));
    }

    fn started_on(&mut self, worker: &Worker) {
        if let Some(queued) = self.queued.take() {
            self.execution = Some(queued.started(worker));
        }
    }

    fn run(mut self: Box<Self>) {
        let task = self.task.take().expect("a blocking callback runs exactly once");
        let execution = self.execution.take();
        task.run(move |outcome| {
            crate::runtime::worker::blocking::LocalWorker::with_monitoring(|worker| {
                if let (Some(worker), Some(execution)) = (worker, execution) {
                    execution.finished(worker, outcome);
                }
            });
        });
    }
}

impl<F, R: Send + 'static> Drop for MonitoredTask<F, R> {
    fn drop(&mut self) {
        if self.task.is_some() {
            drop(self.execution.take());
            drop(self.queued.take());
        }
    }
}
