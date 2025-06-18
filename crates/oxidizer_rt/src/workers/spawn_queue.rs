// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::collections::vec_deque::Drain;
use std::pin::Pin;
use std::rc::Rc;
use std::{fmt, thread};

use crate::{AsyncTask, LocalJoinHandle, Task};

/// Maintains the queue of new local tasks to be spawned. The worker uses this to enqueue tasks
/// received from other threads and the tasks on the same thread use this (via the context) to spawn
/// more tasks on the same thread.
///
/// # Ownership
///
/// Shared ownership between worker and its tasks - we use interior mutability. There cannot be any
/// concurrent borrowing because the worker is single-threaded, and we are careful about reentrancy
/// internally.
///
/// The type is only owned via Rc and shares references to itself with the tasks it enqueues.
///
/// # Safety
///
/// For correct operation, the queue must be drained by the worker that created it before dropping
/// that worker's reference. Failure to do so will result in a panic when the queue is dropped.
pub struct SpawnQueue {
    new_tasks: RefCell<VecDeque<Pin<Box<dyn AsyncTask>>>>,
}

impl SpawnQueue {
    /// # Safety
    ///
    /// For correct operation, the queue must be drained by the worker that
    /// created it before dropping that worker's reference.
    pub unsafe fn new() -> Rc<Self> {
        Rc::new(Self {
            new_tasks: RefCell::new(VecDeque::new()),
        })
    }

    /// Spawns a new local task whose body will be the future constructed via the provided factory.
    /// This method allows tasks with single-threaded state and single-threaded return values.
    ///
    /// This is ONLY intended to be used from within the same thread, as part of reentrant logic
    /// initiated from a currently executing task.
    ///
    /// # Remote spawning
    ///
    /// There is no concept in the worker itself of spawning remote tasks - a remote task that is
    /// scheduled from another thread is simply a local task that happens to forward its result to
    /// a different thread via some cross-thread communication mechanism. The worker is ignorant.
    pub(crate) fn spawn_local<FF, F, R>(self: &Rc<Self>, future_factory: FF) -> LocalJoinHandle<R>
    where
        FF: FnOnce() -> F + 'static,
        F: Future<Output = R> + 'static,
        R: 'static,
    {
        // SAFETY: We are not allowed to drop the task until it signals that it is inert. We fulfil
        // this responsibility by requiring the caller to drain the queue before it can be dropped.
        let mut task = unsafe { Task::new(future_factory()) };

        let join_handle = task.join_handle();

        self.enqueue(task);

        join_handle
    }

    /// Enqueue an already pinned and prepared task.
    pub(crate) fn enqueue(self: &Rc<Self>, task: Pin<Box<dyn AsyncTask>>) {
        self.new_tasks.borrow_mut().push_back(task);
    }

    /// Drains all the tasks from the queue, passing them to the provided callback.
    ///
    /// # Panics
    ///
    /// Panics if an attempt is made to enqueue new tasks during the drain.
    ///
    /// # Safety
    ///
    /// Refer to safety comments of `AsyncTask` - any tasks provided to the callback must not be
    /// dropped until they signal that they are inert.
    pub unsafe fn drain<CB, R>(self: &Rc<Self>, mut cb: CB) -> R
    where
        CB: FnMut(&mut Drain<Pin<Box<dyn AsyncTask>>>) -> R,
    {
        let mut tasks = self.new_tasks.borrow_mut();

        let mut drain = tasks.drain(..);
        cb(&mut drain)
    }

    pub(crate) fn has_new_tasks(&self) -> bool {
        !self.new_tasks.borrow().is_empty()
    }
}

impl Drop for SpawnQueue {
    // The only way to get here in the current implementation is to first call `shutdown()`
    // which makes this function a no-op, so mutating it does nothing useful.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        if thread::panicking() {
            // We skip the assertion if we are already panicking because a double panic more often
            // does not help anything and may even obscure the initial panic in test runs.
            return;
        }

        assert!(
            self.new_tasks.borrow().is_empty(),
            "the worker must drain the spawn queue before dropping it"
        );
    }
}

impl fmt::Debug for SpawnQueue {
    #[cfg_attr(test, mutants::skip)] // We have no contract to test here - can return anything.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnQueue")
            .field("new_tasks", &self.new_tasks.borrow().len())
            .finish()
    }
}

#[cfg(test)]
pub mod tests {
    use std::cell::Cell;
    use std::task;

    use futures::task::noop_waker_ref;
    use scopeguard::{Always, ScopeGuard};

    use super::*;

    #[test]
    fn queue_smoke_test() {
        // We enqueue two tasks and drain them, verifying that the tasks we get back are correctly
        // wired up and that the queue is empty afterward.

        let queue = new_guarded_queue();

        let executed_signal_1 = Rc::new(Cell::new(false));
        let executed_signal_2 = Rc::new(Cell::new(false));

        _ = queue.spawn_local({
            let executed_signal_1 = Rc::clone(&executed_signal_1);
            async move || {
                executed_signal_1.set(true);
            }
        });
        _ = queue.spawn_local({
            let executed_signal_2 = Rc::clone(&executed_signal_2);
            async move || {
                executed_signal_2.set(true);
            }
        });

        drain_and_process_simple_tasks_from_queue(&queue, 2);
        drain_and_process_simple_tasks_from_queue(&queue, 0);

        assert!(executed_signal_1.get());
        assert!(executed_signal_2.get());
    }

    #[test]
    fn queue_reentrancy() {
        // We enqueue a task that, when executed, enqueues another task. We verify that the second
        // task can correctly be dequeued and executed.

        let queue = new_guarded_queue();

        let inner_executed = Rc::new(Cell::new(false));

        let queue_clone = Rc::clone(&queue);
        _ = queue.spawn_local({
            let inner_executed = Rc::clone(&inner_executed);
            async move || {
                _ = queue_clone.spawn_local(async move || {
                    inner_executed.set(true);
                });
            }
        });

        // We expect one iteration for first level task, one for the second level task.
        drain_and_process_simple_tasks_from_queue(&queue, 1);
        drain_and_process_simple_tasks_from_queue(&queue, 1);
        drain_and_process_simple_tasks_from_queue(&queue, 0);

        assert!(inner_executed.get());
    }

    #[test]
    #[should_panic]
    #[expect(
        clippy::should_panic_without_expect,
        reason = "TODO: Add an expected string to [should_panic]"
    )]
    fn queue_with_tasks_after_shutdown_panics() {
        // SAFETY: We intentionally violate the safety requirement here as that should cause the drop
        // to panic.
        let queue = unsafe { SpawnQueue::new() };

        queue.spawn_local(async move || {});
    }

    #[test]
    fn has_new_tasks_works_correctly() {
        // SAFETY: We take responsibility to not drop before we drain.
        let queue = unsafe { SpawnQueue::new() };

        assert!(!queue.has_new_tasks());
        queue.spawn_local(async move || {});
        assert!(queue.has_new_tasks());

        drain_and_process_simple_tasks_from_queue(&queue, 1);
    }

    // Drains all tasks from the queue and processes them, assuming that each only requires a single
    // poll to complete. Verifies that the expected number of tasks were processed.
    pub fn drain_and_process_simple_tasks_from_queue(
        queue: &Rc<SpawnQueue>,
        expected_count: usize,
    ) {
        // SAFETY: We take the responsibility here to not drop the tasks between the first poll and
        // when they signal that they have become inert via `.is_inert()`. We fulfil this obligation
        // in the processing loop below.
        let tasks = unsafe { queue.drain(|tasks| tasks.collect::<Vec<_>>()) };

        assert_eq!(tasks.len(), expected_count);

        for mut task in tasks {
            let mut cx = task::Context::from_waker(noop_waker_ref());

            let poll_result = task.as_mut().poll(&mut cx);

            // The function requires the tasks to complete immediately.
            assert!(matches!(poll_result, task::Poll::Ready(())));

            // Cleanup the task, as required for legal drop.
            task.as_mut().clear();

            // By coincidence, the implementation is such that tasks today will become inert
            // immediately after clear. This may no longer be the case tomorrow, in which
            // case we may need more complex test logic for proper cleanup here. An
            // assertion failure here should signal when it is time to complicate the logic.
            assert!(task.is_inert());
        }
    }

    pub fn new_guarded_queue() -> ScopeGuard<Rc<SpawnQueue>, fn(Rc<SpawnQueue>), Always> {
        scopeguard::guard(
            // SAFETY: We are not allowed to drop this without first:
            // 1. Draining the queue.
            // 2. Shutting down the queue.
            // Both of which we try to do below to the best of our ability.
            unsafe { SpawnQueue::new() },
            |queue: Rc<SpawnQueue>| {
                // We drain the tasks but do not process them. This ensures that they cannot spawn
                // new tasks and means we know one drain is enough to clear the queue.

                // SAFETY: We must not drop until they are inert, which we at least assert below.
                let tasks = unsafe { queue.drain(|tasks| tasks.collect::<Vec<_>>()) };

                for mut task in tasks {
                    // Cleanup the task, as required for legal drop.
                    task.as_mut().clear();

                    // By coincidence, the implementation is such that tasks today will become inert
                    // immediately after clear. This may no longer be the case tomorrow, in which
                    // case we may need more complex test logic for proper cleanup here. An
                    // assertion failure here should signal when it is time to complicate the logic.
                    assert!(task.is_inert());
                }

                // All tasks are drained, dropping the queue becomes legal.
                drop(queue);
            },
        )
    }
}