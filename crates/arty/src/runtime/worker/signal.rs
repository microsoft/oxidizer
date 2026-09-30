// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::{Arc, Condvar, Mutex};
use std::task::Wake;
use std::time::Duration;

const LOCK_INVARIANT: &str = "worker notification lock is never held while invoking user code";

/// A coalescing notification shared by task wakers and the command dispatcher.
#[derive(Debug, Default)]
pub(in crate::runtime) struct WorkerSignal {
    notified: Mutex<bool>,
    ready: Condvar,
}

impl WorkerSignal {
    pub(in crate::runtime) fn wait(&self, timeout: Duration) {
        let notified = self.notified.lock().expect(LOCK_INVARIANT);
        let (mut notified, _) = self
            .ready
            .wait_timeout_while(notified, timeout, |notified| !*notified)
            .expect(LOCK_INVARIANT);
        *notified = false;
    }

    fn notify(&self) {
        *self.notified.lock().expect(LOCK_INVARIANT) = true;
        self.ready.notify_one();
    }
}

impl Wake for WorkerSignal {
    fn wake(self: Arc<Self>) {
        self.notify();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.notify();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::sync::mpsc;
    use std::task::Waker;
    use std::thread;

    use testing_aids::TEST_TIMEOUT;

    use super::*;

    #[test]
    fn notification_before_wait_is_retained_and_consumed() {
        let signal = Arc::new(WorkerSignal::default());
        Waker::from(Arc::clone(&signal)).wake();
        assert!(*signal.notified.lock().unwrap());
        signal.wait(Duration::ZERO);
        assert!(!*signal.notified.lock().unwrap());
    }

    #[test]
    fn remote_wake_releases_a_waiter() {
        let signal = Arc::new(WorkerSignal::default());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            waiter.wait(TEST_TIMEOUT.saturating_mul(2));
            done_tx.send(()).unwrap();
        });
        ready_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        Waker::from(signal).wake_by_ref();
        done_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn an_unnotified_wait_can_time_out() {
        let signal = WorkerSignal::default();
        let timeout = Duration::from_millis(20);
        let start = std::time::Instant::now();
        signal.wait(timeout);
        assert!(start.elapsed() >= timeout);
        assert!(!*signal.notified.lock().unwrap());
    }
}
