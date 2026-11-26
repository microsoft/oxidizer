// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use seismograph_runtime::blocking::{Execution, QueuedTask, Worker};
use seismograph_runtime::{Runtime, TaskIdentity, WorkerOrigin};

use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::task::preparation::BlockingTask;

fn current_worker() -> Option<Worker> {
    crate::runtime::worker::blocking::LocalWorker::monitoring()
}

fn current_origin() -> Option<WorkerOrigin> {
    crate::runtime::worker::asynchronous::LocalWorker::monitoring()
        .map(|worker| worker.origin())
        .or_else(|| current_worker().map(|worker| worker.origin()))
}

pub(crate) enum BlockingJob {
    Plain(Box<dyn FnOnce() + Send + 'static>),
    Monitored(Box<dyn MonitoredBlockingJob>),
}

impl BlockingJob {
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

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockingPoolMonitoring {
    runtime: Option<Runtime>,
}

impl BlockingPoolMonitoring {
    pub(crate) fn new(runtime: Option<Runtime>) -> Self {
        Self { runtime }
    }

    pub(crate) fn task<F: 'static>(&self, shutdown: &Shutdown, identity: TaskIdentity) -> BlockingTaskMonitoring {
        BlockingTaskMonitoring {
            metadata: matches!(shutdown.state(), ShutdownState::Running)
                .then(|| {
                    self.runtime().map(|runtime| BlockingTaskMetadata {
                        runtime,
                        origin: current_origin(),
                        identity,
                        parent: crate::runtime::worker::asynchronous::LocalWorker::current_task().map(TaskIdentity::id),
                        body_size: size_of::<F>(),
                    })
                })
                .flatten(),
        }
    }

    pub(crate) fn runtime(&self) -> Option<Runtime> {
        self.runtime.clone()
    }
}

struct BlockingTaskMetadata {
    runtime: Runtime,
    origin: Option<WorkerOrigin>,
    identity: TaskIdentity,
    parent: Option<seismograph::recorder::runtime::TaskId>,
    body_size: usize,
}

#[derive(Default)]
pub(crate) struct BlockingTaskMonitoring {
    metadata: Option<BlockingTaskMetadata>,
}

impl BlockingTaskMonitoring {
    pub(crate) fn prepare<F, R>(self, task: BlockingTask<F, R>) -> BlockingJob
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        if let Some(metadata) = self.metadata {
            debug_assert_eq!(task.identity, metadata.identity);
            return BlockingJob::Monitored(Box::new(MonitoredTask {
                task: Some(task),
                metadata: Some(metadata),
                queued: None,
                execution: None,
            }));
        }
        BlockingJob::Plain(Box::new(move || task.run(|_| {})))
    }
}

struct MonitoredTask<F, R: Send + 'static> {
    task: Option<BlockingTask<F, R>>,
    metadata: Option<BlockingTaskMetadata>,
    queued: Option<QueuedTask>,
    execution: Option<Execution>,
}

impl<F, R> MonitoredBlockingJob for MonitoredTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn enqueue(&mut self, queue_len: u64) {
        let metadata = self.metadata.take().expect("a blocking task enters its queue once");
        self.queued = metadata
            .runtime
            .enqueue_blocking(metadata.origin, metadata.identity, metadata.parent, metadata.body_size, queue_len);
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
            if let (Some(worker), Some(execution)) = (current_worker(), execution) {
                execution.finished(&worker, outcome);
            }
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
