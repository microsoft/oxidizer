// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::task::Waker;
use std::thread::ThreadId;

use performables::sync::channel;
use plurality::{Arc as PoolArc, MultiPool};

use crate::monitoring::{Monitored, Unmonitored};
use crate::wake::RemoteWake;
use crate::{
    PanicPolicy, Shutdown, TaskCompletion, TaskHandle, TaskIdentity, TaskObserver, TypeErasedTask, WakeShared, WakeSignal, allocate_task,
};

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
    current: Cell<Option<TaskIdentity>>,
    shutdown: Shutdown,
    wake: WakeShared,
    // Owner-only reception breaks the wake queue -> task -> shared wake state ownership cycle.
    remote_wakes: channel::Receiver<RemoteWake>,
    panic_policy: PanicPolicy,
    monitoring: Option<seismograph_runtime::asynchronous::Worker>,
}

impl ExecutorShared {
    pub(crate) fn new_with_monitoring(
        owner_waker: Waker,
        thread_id: ThreadId,
        panic_policy: PanicPolicy,
        monitoring: Option<seismograph_runtime::asynchronous::Worker>,
    ) -> Self {
        let shutdown = Shutdown::new(owner_waker.clone());
        let (wake, remote_wakes) = WakeShared::new(owner_waker, thread_id);
        Self {
            inner: Rc::new(ExecutorSharedInner {
                new_tasks: RefCell::new(VecDeque::new()),
                task_storage: MultiPool::new(),
                task_count: Cell::new(0),
                current: Cell::new(None),
                shutdown,
                wake,
                remote_wakes,
                panic_policy,
                monitoring,
            }),
        }
    }

    pub(crate) fn wake(&self) -> &WakeShared {
        &self.inner.wake
    }

    pub(crate) fn current(&self) -> Option<TaskIdentity> {
        self.inner.current.get()
    }

    pub(crate) fn with_current<R>(&self, identity: TaskIdentity, body: impl FnOnce() -> R) -> R {
        let _current = CurrentTask {
            slot: &self.inner.current,
            previous: self.inner.current.replace(Some(identity)),
        };
        body()
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

    pub(crate) fn monitoring(&self) -> Option<&seismograph_runtime::asynchronous::Worker> {
        self.inner.monitoring.as_ref()
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

    pub(crate) fn task_count(&self) -> usize {
        self.inner.task_count.get()
    }

    pub(crate) fn queued_task_count(&self) -> usize {
        self.inner.new_tasks.borrow().len()
    }

    pub(crate) fn shutdown_complete(&self) -> bool {
        debug_assert!(self.inner.new_tasks.borrow().is_empty());
        self.shutdown().is_triggered() && self.inner.task_count.get() == 0
    }

    pub(crate) fn add_task<F, T, I>(
        &self,
        identity: TaskIdentity,
        future: F,
        observer: I,
        monitoring: Option<seismograph_runtime::asynchronous::QueuedTask>,
        completion: T,
    ) where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: TaskObserver,
    {
        if self.shutdown().is_triggered() {
            Self::reject_task::<F::IntoFuture, _, _>(&identity, observer, completion, self.shutdown().cause());
            return;
        }
        let future = future.into_future();
        if self.shutdown().is_triggered() {
            Self::reject_task::<F::IntoFuture, _, _>(&identity, observer, completion, self.shutdown().cause());
            return;
        }
        if let Some(monitoring) = monitoring {
            self.register_task(allocate_task(
                &self.inner.task_storage,
                identity,
                future,
                observer,
                Monitored::new(monitoring),
                completion,
            ));
        } else {
            self.register_task(allocate_task(
                &self.inner.task_storage,
                identity,
                future,
                observer,
                Unmonitored,
                completion,
            ));
        }
    }

    fn reject_task<F, T, I>(_identity: &TaskIdentity, observer: I, completion: T, cause: crate::ShutdownCause)
    where
        F: Future + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: TaskObserver,
    {
        let observer_panic = catch_unwind(AssertUnwindSafe(|| drop(observer))).err();
        completion.cancelled(cause);
        if let Some(panic) = observer_panic {
            resume_unwind(panic);
        }
    }

    #[inline]
    fn register_task<T: TypeErasedTask + 'static>(&self, task: Pin<PoolArc<T>>) {
        // Keep the concrete type at this boundary so task setup can use direct calls.
        let task_handle = TaskHandle::new(task);
        let wake_signal = WakeSignal::new(self.inner.wake.clone(), task_handle.as_ptr());

        // SAFETY: The new pinned task has one live owner and no aliases have been exposed.
        let task = unsafe { task_handle.as_task() };
        task.initialize(wake_signal);
        self.inner.new_tasks.borrow_mut().push_back(task_handle);
        self.inner
            .task_count
            .set(self.inner.task_count.get().checked_add(1).expect("live task count cannot overflow"));
    }

    pub(crate) fn has_tasks(&self) -> bool {
        self.inner.task_count.get() != 0
    }

    pub(crate) fn panic_policy(&self) -> &PanicPolicy {
        &self.inner.panic_policy
    }
}

struct CurrentTask<'a> {
    slot: &'a Cell<Option<TaskIdentity>>,
    previous: Option<TaskIdentity>,
}

impl Drop for CurrentTask<'_> {
    fn drop(&mut self) {
        self.slot.set(self.previous);
    }
}
