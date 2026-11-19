// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::thread;

use arty_executor::discard_task_panic_payload;
use performables::arc::Arc;
use performables::sync::mutex::Mutex;

use crate::runtime::Error;

/// Joins every runtime worker thread for the external runtime owner.
#[derive(Debug)]
pub(crate) struct WorkerJoiner {
    threads: Arc<Mutex<Option<Vec<thread::JoinHandle<()>>>>>,
}

impl WorkerJoiner {
    pub(crate) fn new(threads: Vec<thread::JoinHandle<()>>) -> Self {
        Self {
            threads: Arc::new(Mutex::new(Some(threads))),
        }
    }

    #[cfg(test)]
    pub(crate) fn clone_for_test(&self) -> Self {
        Self {
            threads: Arc::clone(&self.threads),
        }
    }

    pub(crate) fn join(&self) -> Result<(), Error> {
        let threads = self
            .threads
            .lock()
            .take()
            .expect("the runtime owner joins worker threads exactly once");
        join_workers(threads, thread::JoinHandle::join)
    }
}

fn join_workers<T>(workers: impl IntoIterator<Item = T>, mut join: impl FnMut(T) -> thread::Result<()>) -> Result<(), Error> {
    let mut worker_panicked = false;
    for worker in workers {
        if let Err(panic) = join(worker) {
            worker_panicked = true;
            discard_task_panic_payload(panic);
        }
    }
    if worker_panicked { Err(Error::worker_panicked()) } else { Ok(()) }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
// Runtime owns its joins and cannot expose injected thread failures or repeated joins.
mod tests {
    use std::sync::mpsc;

    use testing_aids::TEST_TIMEOUT;

    use super::*;

    #[test]
    fn gamma_survivor_private_joiner_disposes_panicking_payload_and_joins_remaining_worker() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
                panic!("joined worker panic payload");
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        let mut joined = Vec::new();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                join_workers([0, 1], |worker| {
                    joined.push(worker);
                    if worker == 0 {
                        Err(Box::new(Payload(Arc::clone(&drops))))
                    } else {
                        Ok(())
                    }
                })
            }))
            .unwrap()
            .is_err()
        );
        assert_eq!(joined, [0, 1]);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn joins_every_worker() {
        let (completed, observed) = mpsc::channel();
        let workers = (0..2)
            .map(|_| {
                let completed = completed.clone();
                thread::spawn(move || completed.send(()).unwrap())
            })
            .collect();
        let joiner = WorkerJoiner::new(workers);

        joiner.join().unwrap();

        observed.recv_timeout(TEST_TIMEOUT).unwrap();
        observed.recv_timeout(TEST_TIMEOUT).unwrap();
    }

    #[test]
    fn worker_panic_does_not_skip_remaining_workers() {
        let panicking = thread::spawn(|| panic!("worker already reported its panic"));
        let (completed, observed) = mpsc::channel();
        let remaining = thread::spawn(move || completed.send(()).unwrap());
        let joiner = WorkerJoiner::new(vec![panicking, remaining]);

        let error = joiner.join().unwrap_err();

        observed.recv_timeout(TEST_TIMEOUT).unwrap();
        assert!(error.to_string().contains("panicked"));
    }

    #[test]
    #[should_panic(expected = "the runtime owner joins worker threads exactly once")]
    fn repeated_join_is_a_programming_error() {
        let joiner = WorkerJoiner::new(Vec::new());
        joiner.join().unwrap();
        joiner.join().unwrap();
    }
}
