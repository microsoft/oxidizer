// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::TaskInfo;
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
    fn info(&self) -> &TaskInfo;
    fn run(&self, finish: &mut dyn FnMut(BlockingTaskOutcome));
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockingPoolMonitoring {
    events: std::sync::Arc<std::sync::OnceLock<seismograph_runtime::task::EventContext>>,
}

impl BlockingPoolMonitoring {
    pub(crate) fn task<F: 'static>(&self, shutdown: &Shutdown) -> BlockingTaskMonitoring {
        BlockingTaskMonitoring {
            info: matches!(shutdown.state(), ShutdownState::Running)
                .then(|| self.events().map(TaskInfo::new::<F>))
                .flatten(),
        }
    }

    pub(crate) fn enable(&self, events: seismograph_runtime::task::EventContext) {
        self.events.set(events).expect("monitoring is enabled once under the queue lock");
    }

    pub(crate) fn events(&self) -> Option<seismograph_runtime::task::EventContext> {
        self.events.get().copied()
    }
}

#[derive(Default)]
pub(crate) struct BlockingTaskMonitoring {
    info: Option<TaskInfo>,
}

impl BlockingTaskMonitoring {
    pub(crate) fn prepare<F, R>(self, task: BlockingTask<F, R>) -> BlockingJob
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        if let Some(info) = self.info {
            return BlockingJob::Monitored(std::sync::Arc::new(MonitoredTask {
                task: std::sync::Mutex::new(task),
                info,
            }));
        }
        BlockingJob::Plain(Box::new(move || {
            task.run(|_| {});
        }))
    }
}

// Inspection shares this allocation with execution. Only taking the callback
// needs the mutex; neither the callback nor its destructor runs under the lock.
struct MonitoredTask<F, R: Send + 'static> {
    task: std::sync::Mutex<BlockingTask<F, R>>,
    info: TaskInfo,
}

impl<F, R> MonitoredBlockingJob for MonitoredTask<F, R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    fn info(&self) -> &TaskInfo {
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

impl<F, R: Send + 'static> Drop for MonitoredTask<F, R> {
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
        if let Some(worker) = self.handle.as_ref().filter(|_| !canceled)
            && let Some(body) = &monitored
        {
            let info = body.info();
            let events = worker.task_events();
            let observed = worker.is_observed();
            let token = observed.then(|| worker.task_poll_started(info.id()));
            info.started_on(events, token.as_ref().and_then(seismograph_runtime::task::TaskPoll::started_at));
            if let Some(token) = token {
                current = Some(seismograph_runtime::task::enter_task(info.id(), events));
                poll = Some((worker, token));
            }
        }
        let mut finish = |terminal| {
            drop(current.take());
            if let Some((worker, poll)) = poll.take() {
                worker.task_poll_finished(poll);
            }
            if let Some(body) = &monitored {
                match terminal {
                    BlockingTaskOutcome::Completed => body.info().completed(),
                    BlockingTaskOutcome::Panicked => body.info().panicked(),
                }
            }
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if canceled {
                drop(body);
            } else {
                body.run_with(&mut finish);
            }
        }));
        drop(finish);
        drop(current.take());
        if let Some((worker, poll)) = poll.take() {
            worker.task_poll_finished(poll);
        }
        outcome.map(|_| ())
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
        let monitoring = BlockingPoolMonitoring::default();
        monitoring.enable(worker.handle().task_events());
        let shutdown = Shutdown::default();
        let pool = crate::task::handle::JoinHandlePool::new(shutdown.clone());
        let (body, handle) = crate::task::preparation::prepare_blocking(&pool, || 42, monitoring.task::<()>(&shutdown));

        body.run();

        assert_eq!(handle.join().unwrap(), 42);
    }
}
