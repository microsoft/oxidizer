// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::BlockingTaskInfo;
use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::task::preparation::{BlockingTask, BlockingTaskOutcome};

pub(crate) enum BlockingJob {
    Plain(Box<dyn FnOnce() + Send + 'static>),
    Monitored(std::sync::Arc<dyn MonitoredBlockingJob>),
}

impl BlockingJob {
    #[cfg(test)]
    pub(crate) fn run(self) -> Option<BlockingTaskOutcome> {
        let mut outcome = None;
        self.run_with(&mut |terminal| outcome = Some(terminal));
        outcome
    }

    fn run_with(self, finish: &mut dyn FnMut(BlockingTaskOutcome)) {
        match self {
            Self::Plain(body) => {
                body();
            }
            Self::Monitored(body) => body.run(finish),
        }
    }

    pub(crate) fn monitoring(&self) -> Option<&std::sync::Arc<dyn MonitoredBlockingJob>> {
        match self {
            Self::Plain(_) => None,
            Self::Monitored(body) => Some(body),
        }
    }
}

pub(crate) trait MonitoredBlockingJob: Send + Sync {
    fn info(&self) -> &BlockingTaskInfo;
    fn run(&self, finish: &mut dyn FnMut(BlockingTaskOutcome));
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockingPoolMonitoring {
    runtime: Option<seismograph_runtime::RuntimeHandle>,
}

impl BlockingPoolMonitoring {
    pub(crate) fn new(runtime: Option<seismograph_runtime::RuntimeHandle>) -> Self {
        Self { runtime }
    }

    pub(crate) fn task<F: 'static>(&self, shutdown: &Shutdown) -> BlockingTaskMonitoring {
        let runtime = matches!(shutdown.state(), ShutdownState::Running)
            .then_some(self.runtime.as_ref())
            .flatten();
        BlockingTaskMonitoring {
            task: runtime.map(BlockingTaskInfo::capture::<F>),
        }
    }

    #[cfg(all(test, feature = "test-util"))]
    pub(crate) fn runtime(&self) -> Option<&seismograph_runtime::RuntimeHandle> {
        self.runtime.as_ref()
    }
}

#[derive(Default)]
pub(crate) struct BlockingTaskMonitoring {
    task: Option<BlockingTaskInfo>,
}

impl BlockingTaskMonitoring {
    pub(crate) fn prepare<F, R>(self, task: BlockingTask<F, R>) -> BlockingJob
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        match self.task {
            None => BlockingJob::Plain(Box::new(move || {
                task.run(|_| {});
            })),
            Some(info) => BlockingJob::Monitored(std::sync::Arc::new(MonitoredBlockingTask {
                task: std::sync::Mutex::new(task),
                info,
            })),
        }
    }
}

// Inspection shares this allocation with execution. Only taking the callback
// needs the mutex; neither the callback nor its destructor runs under the lock.
struct MonitoredBlockingTask<F, R: Send + 'static> {
    task: std::sync::Mutex<BlockingTask<F, R>>,
    info: BlockingTaskInfo,
}

impl<F, R> MonitoredBlockingJob for MonitoredBlockingTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn info(&self) -> &BlockingTaskInfo {
        &self.info
    }

    fn run(&self, finish: &mut dyn FnMut(BlockingTaskOutcome)) {
        let task = std::mem::replace(
            &mut *self.task.lock().expect("taking a blocking callback does not execute user code"),
            BlockingTask { pending: None },
        );
        task.run(finish);
    }
}

impl<F, R: Send + 'static> Drop for MonitoredBlockingTask<F, R> {
    fn drop(&mut self) {
        if self
            .task
            .get_mut()
            .expect("taking a blocking callback does not execute user code")
            .pending
            .is_some()
        {
            self.info.canceled();
        }
    }
}

#[derive(Default)]
pub(crate) struct BlockingWorkerMonitoring {
    pub(crate) handle: Option<seismograph_runtime::worker::WorkerHandle>,
}

impl BlockingWorkerMonitoring {
    pub(crate) fn parked(&self) {
        if let Some(worker) = &self.handle {
            worker.parked();
        }
    }

    pub(crate) fn unparked(&self) {
        if let Some(worker) = &self.handle {
            worker.unparked();
        }
    }

    pub(crate) fn run(&self, body: BlockingJob, canceled: bool) -> std::thread::Result<()> {
        let monitored = body.monitoring().cloned();
        let mut poll = None;
        let mut current = None;
        let worker = self.handle.as_ref().filter(|_| !canceled);
        if let Some(worker) = worker
            && let Some(body) = &monitored
        {
            let info = body.info();
            let observed = worker.is_observed();
            poll = info.started_on(worker, observed);
            if poll.is_some() {
                current = Some(arty_executor::enter_task(info.id()));
            }
        }
        let outcome = {
            let mut finish = |terminal| {
                drop(current.take());
                if let Some(poll) = poll.take()
                    && let Some(worker) = worker
                    && let Some(body) = &monitored
                {
                    body.info().poll_finished(worker, poll);
                }
                if let Some(body) = &monitored {
                    match terminal {
                        BlockingTaskOutcome::Completed => body.info().completed(),
                        BlockingTaskOutcome::Panicked => body.info().panicked(),
                    }
                }
            };
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if canceled {
                    drop(body);
                } else {
                    body.run_with(&mut finish);
                }
            }))
        };
        drop(current.take());
        if let Some(poll) = poll.take()
            && let Some(worker) = worker
            && let Some(body) = &monitored
        {
            body.info().poll_finished(worker, poll);
        }
        outcome
    }
}

#[cfg(all(test, feature = "test-util"))]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn a_monitored_callback_runs_and_delivers_its_result() {
        let _monitoring = crate::runtime::monitoring::TEST_LOCK.lock().unwrap();
        let runtime = seismograph_runtime::register_runtime(seismograph_runtime::RuntimeMetadata::new("blocking-callback-result", 1));
        let worker = runtime.register_worker(seismograph_runtime::worker::WorkerMetadata::new(
            seismograph_runtime::worker::WorkerRole::Blocking,
        ));
        let monitoring = BlockingPoolMonitoring::new(Some(runtime.handle()));
        let shutdown = Shutdown::default();
        let pool = crate::task::handle::JoinHandlePool::new(shutdown.clone());
        let (body, handle) = crate::task::preparation::prepare_blocking(&pool, || 42, monitoring.task::<()>(&shutdown));

        body.run();

        assert_eq!(handle.join().unwrap(), 42);
    }
}
