// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native storage for queued blocking functions and active worker slots.

use std::collections::VecDeque;
#[cfg(not(all(test, feature = "test-util")))]
use std::sync::Condvar;
use std::sync::Mutex;

#[cfg(all(test, feature = "test-util"))]
use tests::ScriptedCondvar as Condvar;

use crate::runtime::monitoring::{BlockingJob, BlockingPoolMonitoring};
use crate::runtime::worker::blocking::LocalWorker;

pub(super) struct QueuedJob {
    pub(super) body: BlockingJob,
    pub(super) shutdown: Option<crate::runtime::shutdown::Shutdown>,
}

#[derive(Default)]
struct State {
    jobs: VecDeque<QueuedJob>,
    stopped: bool,
}

pub(super) struct Queue {
    state: Mutex<State>,
    ready: Condvar,
    pub(super) monitoring: BlockingPoolMonitoring,
}

impl Queue {
    pub(super) fn new(runtime: Option<seismograph_runtime::Runtime>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            ready: Condvar::default(),
            monitoring: BlockingPoolMonitoring::new(runtime),
        }
    }

    pub(super) fn push(&self, mut job: QueuedJob) -> bool {
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        if state.stopped {
            drop(state);
            drop(job);
            return false;
        }
        job.body
            .enqueue(u64::try_from(state.jobs.len()).expect("blocking queue length fits u64"));
        state.jobs.push_back(job);
        drop(state);
        self.ready.notify_one();
        true
    }

    pub(super) fn pop(&self) -> Option<QueuedJob> {
        let mut state = self
            .state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock");
        loop {
            #[cfg(all(test, feature = "test-util"))]
            self.ready.check_progress();
            if let Some(mut job) = state.jobs.pop_front() {
                if let Some(worker) = LocalWorker::monitoring() {
                    job.body.started_on(&worker);
                }
                return Some(job);
            }
            if state.stopped {
                return None;
            }
            if let Some(worker) = LocalWorker::monitoring() {
                worker.parked();
            }
            state = self
                .ready
                .wait(state)
                .expect("a blocking queue operation must not panic while holding its lock");
            if let Some(worker) = LocalWorker::monitoring() {
                worker.unparked();
            }
        }
    }

    pub(super) fn stop(&self) {
        self.state
            .lock()
            .expect("a blocking queue operation must not panic while holding its lock")
            .stopped = true;
        self.ready.notify_all();
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
#[cfg(feature = "test-util")]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, LockResult, MutexGuard};

    use super::*;

    #[derive(Default)]
    pub(super) struct ScriptedCondvar {
        native: std::sync::Condvar,
        script: Mutex<Option<WaitScript>>,
        waiting: AtomicUsize,
    }

    struct WaitScript {
        remaining_steps: usize,
        on_wait: Box<dyn FnMut(&mut State) + Send>,
    }

    impl ScriptedCondvar {
        fn script(&self, steps: usize, on_wait: impl FnMut(&mut State) + Send + 'static) {
            *self.script.lock().unwrap() = Some(WaitScript {
                remaining_steps: steps,
                on_wait: Box::new(on_wait),
            });
        }

        pub(super) fn check_progress(&self) {
            if let Some(script) = self.script.lock().unwrap().as_mut() {
                assert!(script.remaining_steps > 0, "an empty queue must wait instead of spinning");
                script.remaining_steps -= 1;
            }
        }

        pub(super) fn wait<'a>(&self, mut state: MutexGuard<'a, State>) -> LockResult<MutexGuard<'a, State>> {
            if let Some(script) = self.script.lock().unwrap().as_mut() {
                (script.on_wait)(&mut state);
                return Ok(state);
            }
            self.native.wait(state)
        }

        pub(super) fn notify_one(&self) {
            self.waiting
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |waiting| Some(waiting.saturating_sub(1)))
                .unwrap();
            self.native.notify_one();
        }

        pub(super) fn notify_all(&self) {
            self.waiting.store(0, Ordering::SeqCst);
            self.native.notify_all();
        }
    }

    #[test]
    fn a_stopped_queue_rejects_work_and_releases_the_rejected_callback() {
        let queue = Arc::new(Queue::default());
        queue.ready.script(1, |_| panic!("a stopped queue must not wait for more work"));
        let capture = Arc::new(());
        let lifetime = Arc::downgrade(&capture);
        queue.stop();

        assert!(!queue.push(QueuedJob {
            body: BlockingJob::Plain(Box::new(move || drop(capture))),
            shutdown: None,
        }));

        assert!(lifetime.upgrade().is_none());
        assert!(queue.pop().is_none());
    }

    #[test]
    fn stopping_the_queue_wakes_every_waiting_worker() {
        let queue = Queue::default();
        queue.ready.waiting.store(2, Ordering::SeqCst);

        queue.stop();

        assert_eq!(queue.ready.waiting.load(Ordering::SeqCst), 0);
    }
}
