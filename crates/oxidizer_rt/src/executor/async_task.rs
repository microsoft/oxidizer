// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;

/// An asynchronous task that can be enqueued to be executed by an async task executor.
///
/// Note that the future's output type is `()`. This is because from the point of view of the async
/// task executor, tasks are merely execution units that do not have return values. It is the
/// responsibility of the task itself to deliver any "outputs" to interested parties.
///
/// # Lifecycle
///
/// There are multiple lifecycle stages that every task will go through:
///
/// 1. Alive - the starting state of a task, during which it may be polled and may make progress.
///    It is not required that the task be polled - this lifecycle stage may end without the task
///    ever being polled (e.g. if it was enqueued during executor shutdown). The task remains in
///    this state even after it has completed its work and returned `Poll::Ready(())`.
/// 2. Dead - the task has been informed that it will never be polled again and it has dropped any
///    state it may have been holding onto (e.g. references to other tasks or shared state). In
///    practice, this primarily means that the future that was the task body has been dropped, along
///    with any captured variables it may have been holding on to.
/// 3. Inert - it is safe to drop the task.
///
/// The transition from Alive to Dead is facilitated by `clear()` which instructs the task that it
/// will never be polled again and is to drop all references it is holding.
///
/// The transition from Dead to Inert happens via unspecified means but callers can identify whether
/// it has taken place by polling `is_inert()`. This will be polled by the async task executor at
/// unspecified times.
///
/// The async task executor guarantees that:
/// 1. `clear()` will be called for all tasks before dropping them; and
/// 2. tasks will only be dropped once `is_inert()` returns true.
pub trait AsyncTask: Future<Output = ()> + 'static {
    /// Returns true if this task was requested to be aborted.
    ///
    /// For aborted tasks, we are sure there is no join handle waiting for the result and executor
    /// will not poll the task again.
    fn is_aborted(&self) -> bool;

    /// Returns true if it is safe to drop the task.
    ///
    /// Must always be true for remote tasks sent from a different thread (because these can be
    /// dropped at any time, as there is no guarantee that anything is even listening on the other
    /// end of the cross-thread message channel).
    fn is_inert(&self) -> bool;

    /// Informs the task that it will never be polled again and it must drop all references it is
    /// holding to any shared state (e.g. through captured variables in the future that makes
    /// up the task body).
    fn clear(self: Pin<&mut Self>);
}