// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error as StdError;
use std::fmt::{self, Display};
use std::thread::{self, ThreadId};

use performables::arc::Arc;
use performables::sync::condition::Condvar;
use performables::sync::mutex::{Mutex, MutexGuard};

use crate::runtime::Error;

#[cfg_attr(test, mockall::automock)]
pub(in crate::runtime) trait WaitForShutdown {
    fn wait(&self) -> Result<(), Error>;
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
    Completed(Option<ThreadId>),
}

#[derive(Debug)]
struct WorkerPanicked(ThreadId);

impl Display for WorkerPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime worker {:?} panicked", self.0)
    }
}

impl StdError for WorkerPanicked {}

fn completion_result(failed_worker: Option<ThreadId>) -> Result<(), Error> {
    failed_worker.map_or(Ok(()), |worker| Err(Error::new(WorkerPanicked(worker))))
}

impl ThreadWaiter {
    pub(in crate::runtime) fn new(threads: Vec<thread::JoinHandle<()>>) -> Self {
        Self {
            shared: Arc::new((Mutex::new(State::Ready(threads)), Condvar::new())),
        }
    }

    fn wait_locked<'a>(&'a self, mut state_guard: MutexGuard<'a, State>) -> Result<(), Error> {
        let (state, completed) = &*self.shared;
        loop {
            match &mut *state_guard {
                State::Ready(threads) => {
                    let threads = std::mem::take(threads);
                    *state_guard = State::Joining;
                    drop(state_guard);
                    let mut failed_worker = None;
                    for worker in threads {
                        let worker_id = worker.thread().id();
                        // Worker panic diagnostics are emitted by the thread entry wrapper.
                        if worker.join().is_err() && failed_worker.is_none() {
                            failed_worker = Some(worker_id);
                        }
                    }
                    *state.lock() = State::Completed(failed_worker);
                    completed.notify_all();
                    return completion_result(failed_worker);
                }
                State::Joining => {
                    state_guard = completed.wait(state_guard);
                }
                State::Completed(failed_worker) => return completion_result(*failed_worker),
            }
        }
    }
}

impl WaitForShutdown for ThreadWaiter {
    fn wait(&self) -> Result<(), Error> {
        let (state, _) = &*self.shared;
        let state_guard = state.lock();
        self.wait_locked(state_guard)
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
        waiter.wait().unwrap();
        waiter.wait().unwrap();
    }

    #[test]
    fn joining_waits_for_completion_notification() {
        let waiter = ThreadWaiter::new(Vec::new());
        let (state, _) = &*waiter.shared;
        let mut state_guard = state.lock();
        *state_guard = State::Joining;

        let shared = Arc::clone(&waiter.shared);
        let completing = thread::spawn(move || {
            let (state, completed) = &*shared;
            let mut state_guard = state.lock();
            assert!(matches!(*state_guard, State::Joining));
            *state_guard = State::Completed(None);
            completed.notify_all();
        });

        // Completion cannot acquire the mutex until the condition-variable wait releases it.
        waiter.wait_locked(state_guard).unwrap();
        completing.join().unwrap();
        assert!(matches!(*state.lock(), State::Completed(None)));
        waiter.wait().unwrap();
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
                waiter.wait().unwrap();
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
        waiter.wait().unwrap();
    }

    #[test]
    fn worker_panic_does_not_strand_waiters() {
        let waiter = ThreadWaiter::new(vec![thread::spawn(|| panic!("worker already reported its panic"))]);
        let first = waiter.wait().unwrap_err().to_string();
        let repeated = waiter.wait().unwrap_err().to_string();
        assert_eq!(first, repeated);
        assert!(first.contains("panicked"));
    }

    #[test]
    fn failed_shutdown_joins_remaining_workers_and_reports_the_same_result_to_waiters() {
        let panicking = thread::spawn(|| panic!("first worker failed"));
        let (release, receive) = mpsc::channel();
        let (joined, complete) = mpsc::channel();
        let remaining = thread::spawn(move || {
            receive.recv().unwrap();
            joined.send(()).unwrap();
        });
        let waiter = Arc::new(ThreadWaiter::new(vec![panicking, remaining]));
        let (reported, outcomes) = mpsc::channel();
        let callers: [_; 2] = std::array::from_fn(|_| {
            let waiter = Arc::clone(&waiter);
            let reported = reported.clone();
            thread::spawn(move || {
                reported.send(waiter.wait().unwrap_err().to_string()).unwrap();
            })
        });
        assert_eq!(outcomes.try_recv(), Err(mpsc::TryRecvError::Empty));
        release.send(()).unwrap();
        complete.recv_timeout(TEST_TIMEOUT).unwrap();
        let first = outcomes.recv_timeout(TEST_TIMEOUT).unwrap();
        let second = outcomes.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("panicked"));
        for caller in callers {
            caller.join().unwrap();
        }
    }
}
