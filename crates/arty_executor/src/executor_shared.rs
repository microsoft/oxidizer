// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem;
use std::rc::Rc;
use std::task::Waker;
use std::thread::ThreadId;

use events_once::LocalEventLake;
use observed::Sink;
use performables::sync::channel;
use plurality::MultiPool;

use crate::remote::Sender as RemoteSender;
use crate::wake::RemoteWake;
use crate::{JoinHandle, TaskHandle, TaskOptions, WakeShared, WakeSignal, allocate_remote_task, allocate_task};

/// Cloneable handle to state used by reentrant task submissions.
#[derive(Clone, Debug)]
pub(crate) struct ExecutorShared {
    inner: Rc<ExecutorSharedInner>,
}

#[derive(Debug)]
struct ExecutorSharedInner {
    new_tasks: RefCell<VecDeque<TaskHandle>>,
    task_storage: MultiPool,
    task_count: Cell<usize>,
    join_events: LocalEventLake,
    shutting_down: Cell<bool>,
    wake: WakeShared,
    // Owner-only reception breaks the wake queue -> task -> shared wake state ownership cycle.
    remote_wakes: channel::Receiver<RemoteWake>,
    sink: Sink,
}

impl ExecutorShared {
    pub(crate) fn new(owner_waker: Waker, sink: Sink, thread_id: ThreadId) -> Self {
        let (wake, remote_wakes) = WakeShared::new(owner_waker, thread_id);
        Self {
            inner: Rc::new(ExecutorSharedInner {
                new_tasks: RefCell::new(VecDeque::new()),
                task_storage: MultiPool::new(),
                task_count: Cell::new(0),
                join_events: LocalEventLake::new(),
                shutting_down: Cell::new(false),
                wake,
                remote_wakes,
                sink,
            }),
        }
    }

    pub(crate) fn wake(&self) -> &WakeShared {
        &self.inner.wake
    }

    pub(crate) fn drain_remote_wakes(&self) {
        self.inner.wake.drain_remote_wakes(&self.inner.remote_wakes);
    }

    pub(crate) fn close_remote_wakes(&self) {
        self.inner.wake.close_remote_wakes(&self.inner.remote_wakes);
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.inner.shutting_down.get()
    }

    pub(crate) fn accept_new_tasks(&self, active: &mut VecDeque<TaskHandle>) -> usize {
        let mut new_tasks = self.inner.new_tasks.borrow_mut();
        let accepted = new_tasks.len();
        if accepted == 0 {
            return 0;
        }
        debug_assert!(active.is_empty(), "active tasks are drained at the end of each cycle");
        mem::swap(active, &mut new_tasks);
        accepted
    }

    pub(crate) fn has_new_tasks(&self) -> bool {
        !self.inner.new_tasks.borrow().is_empty()
    }

    pub(crate) fn drain_new_tasks(&self, target: &mut VecDeque<TaskHandle>) {
        target.extend(self.inner.new_tasks.borrow_mut().drain(..));
    }

    pub(crate) fn decrement_task_count(&self, count: usize) {
        self.inner.task_count.set(
            self.inner
                .task_count
                .get()
                .checked_sub(count)
                .expect("completed task count cannot exceed live task count"),
        );
    }

    pub(crate) fn shutdown_complete(&self) -> bool {
        debug_assert!(self.inner.new_tasks.borrow().is_empty());
        self.inner.shutting_down.get() && self.inner.task_count.get() == 0
    }

    pub(crate) fn begin_shutdown(&self) {
        assert!(
            !self.inner.shutting_down.replace(true),
            "Cannot start shutdown process when it has already been started"
        );
    }

    pub(crate) fn request_shutdown(&self) {
        self.inner.shutting_down.set(true);
    }

    pub(crate) fn add_task<F, R>(&self, future: F, options: TaskOptions) -> JoinHandle<R>
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
    {
        if let Some(join_handle) = self.rejected_submission() {
            return join_handle;
        }
        let future = future.into_future();
        if let Some(join_handle) = self.rejected_submission() {
            return join_handle;
        }
        let (result_tx, result_rx) = self.inner.join_events.rent();
        self.register_task(allocate_task(
            &self.inner.task_storage,
            future,
            result_tx,
            options,
            self.inner.sink.clone(),
        ));
        JoinHandle::new(result_rx)
    }

    pub(crate) fn add_remote_task<F, R>(&self, future: F, options: TaskOptions, remote_result_tx: RemoteSender<R>)
    where
        F: IntoFuture<Output = R> + 'static,
        R: Send + 'static,
    {
        if self.is_shutting_down() {
            return;
        }
        let future = future.into_future();
        if self.is_shutting_down() {
            return;
        }
        self.register_task(allocate_remote_task(
            &self.inner.task_storage,
            future,
            options,
            self.inner.sink.clone(),
            remote_result_tx,
        ));
    }

    fn rejected_submission<R: 'static>(&self) -> Option<JoinHandle<R>> {
        self.is_shutting_down().then(|| JoinHandle::disconnected(&self.inner.join_events))
    }

    fn register_task(&self, task_handle: TaskHandle) {
        let wake_signal = WakeSignal::new(self.inner.wake.clone(), task_handle.as_ptr());

        // SAFETY: The new pinned task has one live owner and no aliases have been exposed.
        let task = unsafe { task_handle.as_task() };
        task.initialize(wake_signal);
        let reporter = task.created_reporter();
        self.inner.new_tasks.borrow_mut().push_back(task_handle);
        self.inner
            .task_count
            .set(self.inner.task_count.get().checked_add(1).expect("live task count cannot overflow"));

        if let Some(reporter) = reporter {
            reporter.report();
        }
    }

    pub(crate) fn has_tasks(&self) -> bool {
        self.inner.task_count.get() != 0
    }

    #[cfg(test)]
    pub(crate) fn task_storage_is_empty(&self) -> bool {
        self.inner.task_storage.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_completion_requires_a_shutdown_request() {
        let shared = ExecutorShared::new(Waker::noop().clone(), Sink::noop(), std::thread::current().id());

        assert!(!shared.shutdown_complete());
        shared.request_shutdown();
        assert!(shared.shutdown_complete());
    }
}
