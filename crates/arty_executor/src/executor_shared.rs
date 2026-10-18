// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem;
use std::rc::Rc;
use std::sync::Arc;
use std::task::Waker;

use observed::Sink;
use plurality::MultiPool;

use crate::remote::Sender as RemoteSender;
use crate::{JoinEventLake, JoinHandle, Task, TaskOptions, TaskRef, TypeErasedTask, WakeShared, WakeSignal};

/// Cloneable handle to state used by reentrant task submissions.
#[derive(Clone, Debug)]
pub(crate) struct ExecutorShared {
    inner: Rc<ExecutorSharedInner>,
}

#[derive(Debug)]
struct ExecutorSharedInner {
    new_tasks: RefCell<VecDeque<TaskRef>>,
    task_storage: MultiPool,
    task_count: Cell<usize>,
    join_events: JoinEventLake,
    shutting_down: Cell<bool>,
    wake: Arc<WakeShared>,
    sink: Sink,
}

impl ExecutorShared {
    pub(crate) fn new(owner_waker: Waker, sink: Sink) -> Self {
        Self {
            inner: Rc::new(ExecutorSharedInner {
                new_tasks: RefCell::new(VecDeque::new()),
                task_storage: MultiPool::new(),
                task_count: Cell::new(0),
                join_events: JoinEventLake::new(),
                shutting_down: Cell::new(false),
                wake: Arc::new(WakeShared::new(owner_waker)),
                sink,
            }),
        }
    }

    pub(crate) fn register_current(&self) {
        self.inner.wake.register_current();
    }

    pub(crate) fn wake(&self) -> &WakeShared {
        &self.inner.wake
    }

    pub(crate) fn wake_handle(&self) -> Arc<WakeShared> {
        Arc::clone(&self.inner.wake)
    }

    pub(crate) fn is_shutting_down(&self) -> bool {
        self.inner.shutting_down.get()
    }

    pub(crate) fn accept_new_tasks(&self, active: &mut VecDeque<TaskRef>) -> usize {
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

    pub(crate) fn drain_new_tasks(&self, target: &mut VecDeque<TaskRef>) {
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
        self.register_task(
            move |join_events, sink| {
                let (result_tx, join_handle) = join_events.rent();
                (Task::new_with_options(future, result_tx, options, sink), join_handle)
            },
            JoinEventLake::disconnected,
        )
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
        self.register_task(
            move |_join_events, sink| (Task::new_remote(future, options, sink, remote_result_tx), ()),
            |_join_events| (),
        );
    }

    fn rejected_submission<R: 'static>(&self) -> Option<JoinHandle<R>> {
        self.inner.shutting_down.get().then(|| self.inner.join_events.disconnected())
    }

    fn register_task<O, T>(&self, create: impl FnOnce(&JoinEventLake, Sink) -> (T, O), reject: impl FnOnce(&JoinEventLake) -> O) -> O
    where
        T: TypeErasedTask + 'static,
    {
        if self.inner.shutting_down.get() {
            return reject(&self.inner.join_events);
        }
        let (task, output) = create(&self.inner.join_events, self.inner.sink.clone());
        let task_ref = TaskRef::new(self.inner.task_storage.alloc_arc_pin(task));
        let wake_signal = WakeSignal::new(Arc::clone(&self.inner.wake), task_ref.pointer());

        // SAFETY: The new pinned task has one live owner and no aliases have been exposed.
        let task = unsafe { task_ref.as_task() };
        task.initialize(wake_signal);
        let reporter = task.created_reporter();
        self.inner.new_tasks.borrow_mut().push_back(task_ref);
        self.inner
            .task_count
            .set(self.inner.task_count.get().checked_add(1).expect("live task count cannot overflow"));

        if let Some(reporter) = reporter {
            reporter.report();
        }
        output
    }

    pub(crate) fn has_tasks(&self) -> bool {
        self.inner.task_count.get() != 0
    }
}
