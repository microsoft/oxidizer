// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem;
use std::rc::Rc;
use std::task::Waker;
use std::thread::ThreadId;

use observed::Sink;
use performables::sync::channel;
use plurality::MultiPool;

use crate::wake::RemoteWake;
use crate::{PanicPolicy, Shutdown, TaskCompletion, TaskHandle, TaskOptions, WakeShared, WakeSignal, allocate_task};

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
    shutdown: Shutdown,
    wake: WakeShared,
    // Owner-only reception breaks the wake queue -> task -> shared wake state ownership cycle.
    remote_wakes: channel::Receiver<RemoteWake>,
    sink: Sink,
    panic_policy: PanicPolicy,
}

impl ExecutorShared {
    #[cfg(feature = "inspection")]
    pub(crate) fn add_inspected_task<F, R, T, I>(&self, future: F, options: TaskOptions, completion: T, inspector: I)
    where
        F: Future<Output = R> + 'static,
        R: 'static,
        T: TaskCompletion<R>,
        I: crate::inspection::Inspector<F>,
    {
        if self.shutdown().is_triggered() {
            completion.cancelled(self.shutdown().cause());
            return;
        }
        self.register_task(crate::task::allocate_inspected_task(
            &self.inner.task_storage,
            future,
            options,
            self.inner.sink.clone(),
            completion,
            inspector,
        ));
    }

    #[cfg(feature = "inspection")]
    pub(crate) fn inspect_new_tasks(&self, mut visit: impl FnMut(&TaskHandle) -> bool) -> Result<(), crate::InspectionError> {
        let tasks = self.inner.new_tasks.try_borrow().map_err(|_borrow| crate::InspectionError::Busy)?;
        for task in tasks.iter() {
            if !visit(task) {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn new(owner_waker: Waker, sink: Sink, thread_id: ThreadId, panic_policy: PanicPolicy) -> Self {
        let shutdown = Shutdown::new(owner_waker.clone());
        let (wake, remote_wakes) = WakeShared::new(owner_waker, thread_id);
        Self {
            inner: Rc::new(ExecutorSharedInner {
                new_tasks: RefCell::new(VecDeque::new()),
                task_storage: MultiPool::new(),
                task_count: Cell::new(0),
                shutdown,
                wake,
                remote_wakes,
                sink,
                panic_policy,
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

    pub(crate) fn shutdown(&self) -> &Shutdown {
        &self.inner.shutdown
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
        self.shutdown().is_triggered() && self.inner.task_count.get() == 0
    }

    pub(crate) fn add_task<F, R, T>(&self, future: F, options: TaskOptions, completion: T)
    where
        F: IntoFuture<Output = R> + 'static,
        R: 'static,
        T: TaskCompletion<R>,
    {
        if self.shutdown().is_triggered() {
            completion.cancelled(self.shutdown().cause());
            return;
        }
        let future = future.into_future();
        if self.shutdown().is_triggered() {
            completion.cancelled(self.shutdown().cause());
            return;
        }
        self.register_task(allocate_task(
            &self.inner.task_storage,
            future,
            options,
            self.inner.sink.clone(),
            completion,
        ));
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

    pub(crate) fn panic_policy(&self) -> &PanicPolicy {
        &self.inner.panic_policy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShutdownCause;

    #[test]
    fn shutdown_completion_requires_a_shutdown_request() {
        let shared = ExecutorShared::new(
            Waker::noop().clone(),
            Sink::noop(),
            std::thread::current().id(),
            PanicPolicy::default(),
        );

        assert!(!shared.shutdown_complete());
        shared.shutdown().trigger(ShutdownCause::default());
        assert!(shared.shutdown_complete());
    }
}
