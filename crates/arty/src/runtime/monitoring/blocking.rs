// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::marker::PhantomData;

#[cfg(any(test, feature = "monitor"))]
use super::TaskInfo;
use crate::runtime::shutdown::Shutdown;
#[cfg(any(test, feature = "monitor"))]
use crate::runtime::shutdown::ShutdownState;
use crate::task::preparation::BlockingTask;

pub(crate) enum BlockingJob {
    Plain(Box<dyn FnOnce() + Send + 'static>),
    #[cfg(any(test, feature = "monitor"))]
    Monitored(std::sync::Arc<dyn MonitoredBlockingJob>),
}

impl BlockingJob {
    pub(crate) fn run(self) {
        match self {
            Self::Plain(body) => body(),
            #[cfg(any(test, feature = "monitor"))]
            Self::Monitored(body) => body.run(),
        }
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn monitoring(&self) -> Option<&std::sync::Arc<dyn MonitoredBlockingJob>> {
        match self {
            Self::Plain(_) => None,
            Self::Monitored(body) => Some(body),
        }
    }
}

#[cfg(any(test, feature = "monitor"))]
pub(crate) trait MonitoredBlockingJob: Send + Sync {
    fn info(&self) -> &TaskInfo;
    fn run(&self);
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockingPoolMonitoring {
    #[cfg(any(test, feature = "monitor"))]
    events: std::sync::Arc<std::sync::OnceLock<seismograph_runtime::task::EventContext>>,
}

impl BlockingPoolMonitoring {
    #[cfg_attr(
        not(any(test, feature = "monitor")),
        expect(
            clippy::unused_self,
            clippy::extra_unused_type_parameters,
            reason = "The monitoring interface is unchanged when monitoring is compiled out."
        )
    )]
    pub(crate) fn task<F: 'static>(&self, shutdown: &Shutdown) -> BlockingTaskMonitoring {
        #[cfg(not(any(test, feature = "monitor")))]
        let _ = shutdown;
        BlockingTaskMonitoring {
            #[cfg(any(test, feature = "monitor"))]
            info: matches!(shutdown.state(), ShutdownState::Running)
                .then(|| self.events().map(TaskInfo::new::<F>))
                .flatten(),
        }
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn enable(&self, events: seismograph_runtime::task::EventContext) {
        self.events.set(events).expect("monitoring is enabled once under the queue lock");
    }

    #[cfg(any(test, feature = "monitor"))]
    pub(crate) fn events(&self) -> Option<seismograph_runtime::task::EventContext> {
        self.events.get().copied()
    }
}

#[derive(Default)]
pub(crate) struct BlockingTaskMonitoring {
    #[cfg(any(test, feature = "monitor"))]
    info: Option<TaskInfo>,
}

impl BlockingTaskMonitoring {
    #[cfg_attr(
        not(any(test, feature = "monitor")),
        expect(clippy::unused_self, reason = "Task monitoring is empty when monitoring is compiled out.")
    )]
    pub(crate) fn prepare<F, R>(self, task: BlockingTask<F, R>) -> BlockingJob
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        #[cfg(any(test, feature = "monitor"))]
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
    #[cfg(any(test, feature = "monitor"))]
    info: Option<&'a TaskInfo>,
    marker: PhantomData<&'a ()>,
}

#[cfg_attr(
    not(any(test, feature = "monitor")),
    expect(clippy::unused_self, reason = "Task events do nothing when monitoring is compiled out.")
)]
impl BlockingTaskEvents<'_> {
    pub(crate) fn completed(self) {
        #[cfg(any(test, feature = "monitor"))]
        if let Some(info) = self.info {
            info.completed();
        }
    }

    pub(crate) fn panicked(self) {
        #[cfg(any(test, feature = "monitor"))]
        if let Some(info) = self.info {
            info.panicked();
        }
    }
}

// Inspection shares this allocation with execution. Only taking the callback
// needs the mutex; neither the callback nor its destructor runs under the lock.
#[cfg(any(test, feature = "monitor"))]
struct MonitoredTask<F, R: Send + 'static> {
    task: std::sync::Mutex<BlockingTask<F, R>>,
    info: TaskInfo,
}

#[cfg(any(test, feature = "monitor"))]
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
            BlockingTask { pending: None },
        );
        task.run(BlockingTaskEvents {
            info: Some(&self.info),
            marker: PhantomData,
        });
    }
}

#[cfg(any(test, feature = "monitor"))]
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
    #[cfg(any(test, feature = "monitor"))]
    pub(crate) handle: Option<seismograph_runtime::worker::WorkerHandle>,
}

#[cfg_attr(
    not(any(test, feature = "monitor")),
    expect(clippy::unused_self, reason = "Worker monitoring is empty when monitoring is compiled out.")
)]
impl BlockingWorkerMonitoring {
    pub(crate) fn parked(&self) {
        #[cfg(any(test, feature = "monitor"))]
        if let Some(worker) = &self.handle {
            worker.parked();
        }
    }

    pub(crate) fn unparked(&self) {
        #[cfg(any(test, feature = "monitor"))]
        if let Some(worker) = &self.handle {
            worker.unparked();
        }
    }

    pub(crate) fn run(&self, body: BlockingJob, canceled: bool) -> std::thread::Result<()> {
        #[cfg(any(test, feature = "monitor"))]
        let observation = self.handle.as_ref().filter(|_| !canceled).and_then(|worker| {
            body.monitoring().map(|body| {
                let info = body.info();
                info.started_on(worker.task_events());
                let poll = worker.is_observed().then(|| worker.task_poll_started(info.id()));
                (worker, poll)
            })
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(any(test, feature = "monitor"))]
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
        #[cfg(any(test, feature = "monitor"))]
        if let Some((worker, Some(poll))) = observation {
            worker.task_poll_finished(poll);
        }
        outcome
    }
}
