// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::mem;
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::rc::Rc;
use std::task::Waker;
use std::thread::ThreadId;

use performables::sync::channel;
use plurality::{Arc as PoolArc, MultiPool};

use crate::wake::RemoteWake;
use crate::{
    PanicPolicy, Shutdown, TaskCompletion, TaskHandle, TaskIdentity, TaskInstrumentation, TypeErasedTask, WakeShared, WakeSignal,
    allocate_task, discard_task_panic_payload,
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
    shutdown: Shutdown,
    wake: WakeShared,
    // Owner-only reception breaks the wake queue -> task -> shared wake state ownership cycle.
    remote_wakes: channel::Receiver<RemoteWake>,
    panic_policy: PanicPolicy,
}

impl ExecutorShared {
    pub(crate) fn inspect_new_tasks(&self, mut visit: impl FnMut(&TaskHandle) -> ControlFlow<()>) -> Result<(), crate::InspectionError> {
        let tasks = self.inner.new_tasks.try_borrow().map_err(|_borrow| crate::InspectionError::Busy)?;
        for task in tasks.iter() {
            if visit(task).is_break() {
                break;
            }
        }
        Ok(())
    }

    pub(crate) fn new(owner_waker: Waker, thread_id: ThreadId, panic_policy: PanicPolicy) -> Self {
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

    pub(crate) fn add_task<F, T, I>(&self, identity: TaskIdentity, future: F, instrumentation: I, completion: T)
    where
        F: IntoFuture + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: TaskInstrumentation,
    {
        if self.shutdown().is_triggered() {
            Self::reject_task::<F::IntoFuture, _, _>(&identity, instrumentation, completion, self.shutdown().cause());
            return;
        }
        let future = future.into_future();
        if self.shutdown().is_triggered() {
            Self::reject_task::<F::IntoFuture, _, _>(&identity, instrumentation, completion, self.shutdown().cause());
            return;
        }
        self.register_task(allocate_task(
            &self.inner.task_storage,
            identity,
            future,
            instrumentation,
            completion,
        ));
    }

    fn reject_task<F, T, I>(identity: &TaskIdentity, mut instrumentation: I, completion: T, cause: crate::ShutdownCause)
    where
        F: Future + 'static,
        F::Output: 'static,
        T: TaskCompletion<F::Output>,
        I: TaskInstrumentation,
    {
        let callback_panic = catch_unwind(AssertUnwindSafe(|| instrumentation.cancelled(identity))).err();
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| drop(instrumentation))) {
            discard_task_panic_payload(panic);
        }
        completion.cancelled(cause);
        if let Some(panic) = callback_panic {
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

#[cfg(test)]
// Public submission does not expose a mutable borrow of the pending-task queue.
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Cancelled(Rc<Cell<bool>>);

    impl crate::TaskCompletion<()> for Cancelled {
        fn completed(self, (): ()) {}

        fn panicked(self, panic: crate::TaskPanicPayload, _cause: Option<crate::ShutdownCause>) {
            crate::discard_task_panic_payload(panic);
        }

        fn cancelled(self, _cause: crate::ShutdownCause) {
            self.0.set(true);
        }
    }

    struct CountCancellation(Rc<Cell<usize>>);

    impl crate::TaskInstrumentation for CountCancellation {
        fn cancelled(&mut self, _identity: &TaskIdentity) {
            self.0.set(self.0.get() + 1);
        }
    }

    struct PanickingCancellation;

    impl crate::TaskInstrumentation for PanickingCancellation {
        fn cancelled(&mut self, _identity: &TaskIdentity) {
            panic!("rejected instrumentation cancellation");
        }
    }

    struct StopOnConvert(Shutdown);

    impl IntoFuture for StopOnConvert {
        type Output = ();
        type IntoFuture = std::future::Pending<()>;

        fn into_future(self) -> Self::IntoFuture {
            self.0.trigger(crate::ShutdownCause::default());
            std::future::pending()
        }
    }

    #[test]
    fn borrowed_submission_storage_reports_busy_and_recovers_after_release() {
        let shared = ExecutorShared::new(Waker::noop().clone(), std::thread::current().id(), PanicPolicy::default());
        let borrowed = shared.inner.new_tasks.borrow_mut();

        let busy = shared.inspect_new_tasks(|_| ControlFlow::Continue(()));
        drop(borrowed);
        let released = shared.inspect_new_tasks(|_| ControlFlow::Continue(()));

        assert_eq!((busy, released), (Err(crate::InspectionError::Busy), Ok(())));
    }

    #[test]
    fn rejected_tasks_notify_instrumentation_before_and_during_conversion() {
        for stop_before_conversion in [true, false] {
            let shared = ExecutorShared::new(Waker::noop().clone(), std::thread::current().id(), PanicPolicy::default());
            if stop_before_conversion {
                shared.shutdown().trigger(crate::ShutdownCause::default());
            }
            let cancellations = Rc::new(Cell::new(0));
            let cancelled = Rc::new(Cell::new(false));
            shared.add_task(
                TaskIdentity::from_location(std::panic::Location::caller()),
                StopOnConvert(shared.shutdown().clone()),
                CountCancellation(Rc::clone(&cancellations)),
                Cancelled(Rc::clone(&cancelled)),
            );

            assert_eq!(cancellations.get(), 1);
            assert!(cancelled.get());
        }
    }

    #[test]
    fn rejected_instrumentation_panics_after_cancellation_delivery() {
        for stop_before_conversion in [true, false] {
            let shared = ExecutorShared::new(Waker::noop().clone(), std::thread::current().id(), PanicPolicy::default());
            if stop_before_conversion {
                shared.shutdown().trigger(crate::ShutdownCause::default());
            }
            let cancelled = Rc::new(Cell::new(false));

            let panic = catch_unwind(AssertUnwindSafe(|| {
                shared.add_task(
                    TaskIdentity::from_location(std::panic::Location::caller()),
                    StopOnConvert(shared.shutdown().clone()),
                    PanickingCancellation,
                    Cancelled(Rc::clone(&cancelled)),
                );
            }))
            .unwrap_err();

            assert_eq!(*panic.downcast::<&str>().unwrap(), "rejected instrumentation cancellation");
            assert!(cancelled.get());
        }
    }
}
