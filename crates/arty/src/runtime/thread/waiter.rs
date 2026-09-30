// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;

#[cfg_attr(test, mockall::automock)]
pub(in crate::runtime) trait WaitForShutdown {
    fn wait(&self);
}

/// Joins workers on one waiting caller; other callers wait for that join to finish.
#[derive(Debug)]
pub(in crate::runtime) struct ThreadWaiter {
    shared: Arc<(Mutex<State>, Condvar)>,
}

#[derive(Debug)]
enum State {
    Ready(Vec<thread::JoinHandle<()>>),
    Joining,
    Completed,
}

impl ThreadWaiter {
    pub(in crate::runtime) fn new(threads: Vec<thread::JoinHandle<()>>) -> Self {
        Self {
            shared: Arc::new((Mutex::new(State::Ready(threads)), Condvar::new())),
        }
    }

    fn wait_locked<'a>(&'a self, mut state_guard: MutexGuard<'a, State>) {
        let (state, completed) = &*self.shared;
        loop {
            match &mut *state_guard {
                State::Ready(threads) => {
                    let threads = std::mem::take(threads);
                    *state_guard = State::Joining;
                    drop(state_guard);
                    for worker in threads {
                        // Worker panic diagnostics are emitted by the thread entry wrapper.
                        let _ = worker.join();
                    }
                    *state.lock().expect("shutdown state is never held while executing user code") = State::Completed;
                    completed.notify_all();
                    return;
                }
                State::Joining => {
                    state_guard = completed
                        .wait(state_guard)
                        .expect("shutdown state is never held while executing user code");
                }
                State::Completed => return,
            }
        }
    }
}

impl WaitForShutdown for ThreadWaiter {
    fn wait(&self) {
        let (state, _) = &*self.shared;
        let state_guard = state.lock().expect("shutdown state is never held while executing user code");
        self.wait_locked(state_guard);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::mpsc;

    use testing_aids::TEST_TIMEOUT;

    use super::*;

    #[test]
    fn empty_and_repeated_waits_complete() {
        let waiter = ThreadWaiter::new(Vec::new());
        waiter.wait();
        waiter.wait();
    }

    #[test]
    fn joining_waits_for_completion_notification() {
        let waiter = ThreadWaiter::new(Vec::new());
        let (state, _) = &*waiter.shared;
        let mut state_guard = state.lock().unwrap();
        *state_guard = State::Joining;

        let shared = Arc::clone(&waiter.shared);
        let completing = thread::spawn(move || {
            let (state, completed) = &*shared;
            let mut state_guard = state.lock().unwrap();
            assert!(matches!(*state_guard, State::Joining));
            *state_guard = State::Completed;
            completed.notify_all();
        });

        // Completion cannot acquire the mutex until the condition-variable wait releases it.
        waiter.wait_locked(state_guard);
        completing.join().unwrap();
        assert!(matches!(*state.lock().unwrap(), State::Completed));
        waiter.wait();
    }

    #[test]
    fn concurrent_waiters_wait_for_worker_completion() {
        let (release, receive) = mpsc::channel();
        let worker = thread::spawn(move || receive.recv().unwrap());
        let waiter = Arc::new(ThreadWaiter::new(vec![worker]));
        let (done, finished) = mpsc::channel();
        let joiners: [_; 2] = std::array::from_fn(|_| {
            let waiter = Arc::clone(&waiter);
            let done = done.clone();
            thread::spawn(move || {
                waiter.wait();
                done.send(()).unwrap();
            })
        });
        assert!(finished.try_recv().is_err());
        release.send(()).unwrap();
        for _ in 0..2 {
            finished.recv_timeout(TEST_TIMEOUT).unwrap();
        }
        for joiner in joiners {
            joiner.join().unwrap();
        }
        waiter.wait();
    }

    #[test]
    fn worker_panic_does_not_strand_waiters() {
        let waiter = ThreadWaiter::new(vec![thread::spawn(|| panic!("worker already reported its panic"))]);
        waiter.wait();
        waiter.wait();
    }
}
