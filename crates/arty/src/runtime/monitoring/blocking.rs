// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::TaskInfo;
use crate::runtime::shutdown::{Shutdown, ShutdownState};
use crate::task::TaskIdentity;
use crate::task::preparation::BlockingTask;

pub(crate) enum BlockingJob {
    Plain(Box<dyn FnOnce() + Send + 'static>),
    Monitored(std::sync::Arc<dyn MonitoredBlockingJob>),
}

impl BlockingJob {
    pub(crate) fn run(self) {
        match self {
            Self::Plain(body) => body(),
            Self::Monitored(body) => body.run(),
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
    fn run(&self);
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockingPoolMonitoring {
    events: std::sync::Arc<std::sync::OnceLock<seismograph_runtime::task::EventContext>>,
}

impl BlockingPoolMonitoring {
    pub(crate) fn task<F: 'static>(&self, shutdown: &Shutdown, identity: TaskIdentity) -> BlockingTaskMonitoring {
        BlockingTaskMonitoring {
            info: matches!(shutdown.state(), ShutdownState::Running)
                .then(|| self.events().map(|events| TaskInfo::new::<F>(events, identity)))
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
        BlockingJob::Plain(Box::new(move || task.run(BlockingTaskEvents::default())))
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct BlockingTaskEvents<'a> {
    info: Option<&'a TaskInfo>,
}

impl BlockingTaskEvents<'_> {
    pub(crate) fn completed(self) {
        if let Some(info) = self.info {
            info.completed();
        }
    }

    pub(crate) fn panicked(self) {
        if let Some(info) = self.info {
            info.panicked();
        }
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

    fn run(&self) {
        let task = std::mem::replace(
            &mut *self.task.lock().expect("taking a blocking callback does not execute user code"),
            BlockingTask {
                pending: None,
                identity: self.info.identity(),
            },
        );
        debug_assert_eq!(
            task.identity,
            self.info.identity(),
            "blocking metadata must retain the submitted task identity"
        );
        task.run(BlockingTaskEvents { info: Some(&self.info) });
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
        let observation = self.handle.as_ref().filter(|_| !canceled).and_then(|worker| {
            body.monitoring().map(|body| {
                let info = body.info();
                info.started_on(worker.task_events());
                let poll = worker.is_observed().then(|| worker.task_poll_started(info.id()));
                (worker, poll)
            })
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _current = observation
                .as_ref()
                .and_then(|(_, poll)| poll.as_ref())
                .and_then(|_| body.monitoring())
                .map(|body| super::task::enter_current(body.info().id()));
            if canceled {
                drop(body);
            } else {
                body.run();
            }
        }));
        if let Some((worker, Some(poll))) = observation {
            worker.task_poll_finished(poll);
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
        let monitoring = BlockingPoolMonitoring::default();
        monitoring.enable(worker.handle().task_events());
        let shutdown = Shutdown::default();
        let pool = crate::task::handle::JoinHandlePool::new(shutdown.clone());
        let identity = TaskIdentity::from_location(std::panic::Location::caller());
        let (body, handle) = crate::task::preparation::prepare_blocking(&pool, || 42, monitoring.task::<()>(&shutdown, identity), identity);

        body.run();

        assert_eq!(handle.join().unwrap(), 42);
    }
}
