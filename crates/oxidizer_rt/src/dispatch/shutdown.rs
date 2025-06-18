// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::{mem, thread};

use async_lock::Mutex;

use crate::once_event;

/// Asynchronously waits for the runtime to shut down, allowing for multiple concurrent waiters.
#[cfg_attr(test, mockall::automock)]
pub trait WaitForShutdown {
    fn wait(&self) -> impl Future<Output = ()> + 'static;
}

#[derive(Debug)]
pub struct ThreadWaiter {
    state: Arc<Mutex<ThreadWaiterState>>,
}

#[derive(Debug)]
enum ThreadWaiterState {
    /// Nobody has started waiting for the threads to exit yet.
    Initialized {
        threads: Vec<thread::JoinHandle<()>>,
    },

    /// The first waiter has shown up and started the wait operation.
    /// Additional waiters can add themselves to the list of waiters.
    Waiting {
        completed_txs: Vec<once_event::shared::InefficientSender<()>>,
    },

    /// All threads have already exited - no more waiting is happening.
    Completed,
}

impl ThreadWaiter {
    pub fn new(threads: Vec<thread::JoinHandle<()>>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ThreadWaiterState::Initialized { threads })),
        }
    }
}

impl WaitForShutdown for ThreadWaiter {
    fn wait(&self) -> impl Future<Output = ()> + 'static {
        let state_arc = Arc::clone(&self.state);

        async move {
            let mut state = state_arc.lock().await;

            match &mut *state {
                ThreadWaiterState::Initialized { .. } => {
                    // We are the first waiter, so we need to wait for the threads to exit.
                    let (tx, rx) = once_event::shared::new_inefficient();

                    let completed_txs = vec![tx];
                    let initialized_state =
                        mem::replace(&mut *state, ThreadWaiterState::Waiting { completed_txs });

                    // The waiting occurs in a new thread since it is a synchronous operation.
                    thread::spawn({
                        let state = Arc::clone(&state_arc);

                        move || {
                            let ThreadWaiterState::Initialized { threads } = initialized_state
                            else {
                                unreachable!(
                                    "we already matched this and know the state we are in"
                                );
                            };

                            // TODO: What to do if a thread panicked and this returns an error?
                            // For now, we ignore it but we might want a more thought-through plan.
                            for thread in threads {
                                _ = thread.join();
                            }

                            // All threads have exited. Signal the waiters!
                            let mut state = state.lock_blocking();

                            let ThreadWaiterState::Waiting { completed_txs } = &mut *state else {
                                unreachable!(
                                    "there is nothing else that could transition us out of the waiting state"
                                );
                            };

                            for tx in completed_txs.drain(..) {
                                tx.set(());
                            }

                            // The state is locked, so nothing can add an extra waiter before we get here.
                            *state = ThreadWaiterState::Completed;
                        }
                    });

                    // Drop the lock and wait for the "transitioned into Completed state" signal.
                    drop(state);
                    rx.await;

                    // We have received the completion signal and are good to continue.
                }
                ThreadWaiterState::Waiting { completed_txs } => {
                    // Add ourselves to the list of waiters.
                    let (tx, rx) = once_event::shared::new_inefficient();
                    completed_txs.push(tx);

                    // Drop the lock and wait for the "transitioned into Completed state" signal.
                    drop(state);
                    rx.await;

                    // We have received the completion signal and are good to continue.
                }
                ThreadWaiterState::Completed => {
                    // Nothing to do, all threads have already exited.
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use core::task;

    use futures::task::noop_waker_ref;
    use oxidizer_testing::execute_or_abandon;

    use super::*;

    #[test]
    fn thread_waiter_no_threads_is_ok() {
        // Yes, a ThreadWaiter with no threads is relatively pointless but a valid thing to have
        // in terms of meeting the API requirements, so let's verify that it works fine.
        let threads = Vec::new();
        let waiter = ThreadWaiter::new(threads);

        execute_or_abandon(move || futures::executor::block_on(waiter.wait())).unwrap();
    }

    #[test]
    fn thread_waiter_some_threads_is_ok() {
        let thread_1 = thread::spawn(|| {});
        let thread_2 = thread::spawn(|| {});

        let threads = vec![thread_1, thread_2];
        let waiter = Arc::new(ThreadWaiter::new(threads));

        execute_or_abandon({
            let waiter = Arc::clone(&waiter);
            move || futures::executor::block_on(waiter.wait())
        })
        .unwrap();

        // Calling it a second time is also fine, we just return immediately.
        execute_or_abandon({
            let waiter = Arc::clone(&waiter);
            move || futures::executor::block_on(waiter.wait())
        })
        .unwrap();
    }

    #[test]
    fn thread_waiter_pending_if_not_yet_completed() {
        // We start the wait when the thread we are waiting for has not yet completed.

        let (release_tx, release_rx) = oneshot::channel();

        let thread = thread::spawn(move || {
            // Wait for the release signal before we complete.
            _ = release_rx.recv();
        });

        execute_or_abandon(move || {
            futures::executor::block_on(async move {
                let waiter = Arc::new(ThreadWaiter::new(vec![thread]));

                let mut wait_fut = Box::pin(waiter.wait());

                let mut cx = task::Context::from_waker(noop_waker_ref());
                let first_poll_result = wait_fut.as_mut().poll(&mut cx);

                // This (and not the above call) starts the actual wait operation.
                // The thread is still running, so the wait task must be pending.
                assert_eq!(first_poll_result, task::Poll::Pending);

                // Now we release the thread and expect the wait task to complete.
                _ = release_tx.send(());

                wait_fut.await;
            });
        })
        .unwrap();
    }

    #[test]
    fn thread_waiter_late_joining_waiter_ok() {
        // We start the wait when the thread we are waiting for has not yet completed,
        // then before the thread completes we add a second waiter.

        let (release_tx, release_rx) = oneshot::channel();

        let thread = thread::spawn(move || {
            // Wait for the release signal before we complete.
            _ = release_rx.recv();
        });

        execute_or_abandon(move || {
            futures::executor::block_on(async move {
                let waiter = Arc::new(ThreadWaiter::new(vec![thread]));

                let mut wait_fut = Box::pin(waiter.wait());

                let mut cx = task::Context::from_waker(noop_waker_ref());
                let first_poll_result = wait_fut.as_mut().poll(&mut cx);

                // This (and not the above call) starts the actual wait operation.
                // The thread is still running, so the wait task must be pending.
                assert_eq!(first_poll_result, task::Poll::Pending);

                // We add the second waiter here.
                // Note that we have to poll it first to actually cause it to wire itself up!
                let mut wait_fut_2 = Box::pin(waiter.wait());

                let mut cx = task::Context::from_waker(noop_waker_ref());
                let second_poll_result = wait_fut_2.as_mut().poll(&mut cx);

                // The second waiter is also pending since the thread is still running.
                assert_eq!(second_poll_result, task::Poll::Pending);

                // Now we release the thread and expect the wait task to complete.
                _ = release_tx.send(());

                wait_fut.await;
                wait_fut_2.await;
            });
        })
        .unwrap();
    }
}