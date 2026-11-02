// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;

use arty_executor::{Shutdown as ExecutorShutdown, ShutdownCause, TaskSet};
use thread_aware::Thread;

use crate::io::RuntimeDriverHost;
use crate::runtime::dispatch::DispatcherClient;
use crate::runtime::worker::{self, WorkerMarker};
use crate::task::Context;
use crate::task::handle::JoinHandlePool;

thread_local! {
    static CURRENT: RefCell<Option<LocalWorker>> = const { RefCell::new(None) };
}

/// Asynchronous worker state that remains attached throughout thread teardown.
#[derive(Debug)]
pub(crate) struct LocalWorker {
    scheduler: Option<SchedulerContext>,
    driver_host: Option<RuntimeDriverHost>,
}

impl LocalWorker {
    fn new() -> Self {
        Self {
            scheduler: None,
            driver_host: None,
        }
    }

    pub(crate) fn attach() {
        worker::WorkerMarker::attach();
        CURRENT.with_borrow_mut(|current| {
            assert!(current.is_none(), "a task local worker is already attached to this thread");
            *current = Some(Self::new());
        });
    }

    pub(super) fn register_scheduler(context: Context, tasks: TaskSet, executor_shutdown: ExecutorShutdown) {
        let results = JoinHandlePool::new(context.dispatcher().shutdown().clone());
        let scheduler = SchedulerContext {
            thread: context.thread().clone(),
            context,
            tasks,
            executor_shutdown,
            results,
        };
        Self::with_current_mut(|current| {
            let current = current.expect("task worker startup attaches its local worker before registering the scheduler");
            assert!(current.scheduler.is_none(), "a scheduler is already registered on this task worker");
            current.scheduler = Some(scheduler);
        });
    }

    pub(crate) fn is_current() -> bool {
        CURRENT.try_with(|current| current.borrow().is_some()).unwrap_or(false)
    }

    pub(crate) fn current_thread() -> Option<Thread> {
        Self::with_scheduler(|current| current.map(|current| current.thread().clone()))
    }

    pub(crate) fn with_scheduler<R>(f: impl FnOnce(Option<&SchedulerContext>) -> R) -> R {
        Self::with_current(|current| f(current.and_then(|current| current.scheduler.as_ref())))
    }

    pub(crate) fn begin_executor_shutdown(dispatcher: &DispatcherClient, cause: ShutdownCause) {
        _ = CURRENT.try_with(|current| {
            let current = current.borrow();
            let Some(current) = current.as_ref().and_then(|current| current.scheduler.as_ref()) else {
                return;
            };
            if dispatcher.owns(current.thread()) {
                current.executor_shutdown.trigger(cause);
            }
        });
    }

    #[cfg_attr(test, mutants::skip)] // Retaining direct registration during executor teardown can strand shutdown.
    pub(super) fn clear_scheduler() {
        let scheduler = Self::with_current_mut(|current| current.and_then(|current| current.scheduler.take()));
        drop(scheduler);
    }

    pub(crate) fn install_driver_host(driver_host: RuntimeDriverHost) {
        Self::with_current_mut(|current| {
            let current = current.expect("task worker startup attaches its local worker before installing the I/O driver host");
            assert!(
                current.driver_host.is_none(),
                "an I/O driver host is already installed on this task worker"
            );
            current.driver_host = Some(driver_host);
        });
    }

    pub(crate) fn uninstall_driver_host() -> RuntimeDriverHost {
        Self::with_current_mut(|current| {
            let current = current.expect("task worker teardown retains its local worker while uninstalling the I/O driver host");
            current
                .driver_host
                .take()
                .expect("the task worker must uninstall the I/O driver host that it installed")
        })
    }

    pub(crate) fn with_driver_host<R>(f: impl FnOnce(&RuntimeDriverHost) -> R) -> R {
        Self::with_current(|current| current.and_then(|current| current.driver_host.as_ref()).map(f))
            .expect("the task worker must install its I/O driver host before running")
    }

    pub(crate) fn try_with_driver_host<R>(f: impl FnOnce(&RuntimeDriverHost) -> R) -> Option<R> {
        if !WorkerMarker::is_current() {
            return None;
        }
        Self::with_current(|current| current.and_then(|current| current.driver_host.as_ref().map(f)))
    }

    #[cfg(test)]
    pub(crate) fn has_scheduler_context() -> bool {
        Self::with_scheduler(|current| current.is_some())
    }

    fn with_current<R>(f: impl FnOnce(Option<&Self>) -> R) -> R {
        let mut f = Some(f);
        CURRENT
            .try_with(|current| {
                let current = current.borrow();
                f.take().expect("the local-worker callback is invoked exactly once")(current.as_ref())
            })
            .unwrap_or_else(|_| f.take().expect("the local-worker callback is invoked exactly once")(None))
    }

    fn with_current_mut<R>(f: impl FnOnce(Option<&mut Self>) -> R) -> R {
        CURRENT.with_borrow_mut(|current| f(current.as_mut()))
    }
}

/// Task worker state available while direct task registration is safe.
#[derive(Debug)]
pub(crate) struct SchedulerContext {
    thread: Thread,
    context: Context,
    tasks: TaskSet,
    executor_shutdown: ExecutorShutdown,
    results: JoinHandlePool,
}

impl SchedulerContext {
    pub(crate) fn thread(&self) -> &Thread {
        &self.thread
    }

    pub(crate) fn context(&self) -> &Context {
        &self.context
    }

    pub(crate) fn tasks(&self) -> &TaskSet {
        &self.tasks
    }

    pub(crate) fn results(&self) -> &JoinHandlePool {
        &self.results
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    thread_local! {
        static EXTERNAL_OWNER: ExternalOwner = const { ExternalOwner };
    }

    struct ExternalOwner;

    impl Drop for ExternalOwner {
        fn drop(&mut self) {
            assert!(LocalWorker::try_with_driver_host(|_| ()).is_none());
        }
    }

    #[test]
    fn external_lookup_does_not_access_destroyed_worker_slot() {
        std::thread::spawn(|| {
            EXTERNAL_OWNER.with(|_| {});
            LocalWorker::with_scheduler(|_| ());
        })
        .join()
        .unwrap();
    }
}
