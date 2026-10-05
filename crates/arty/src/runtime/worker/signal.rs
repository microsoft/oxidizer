// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc as StdArc;
use std::task::{Wake, Waker};
use std::time::Duration;

use performables::arc::Arc;
use performables::sync::condition::Condvar;
use performables::sync::mutex::Mutex;

/// A coalescing notification shared by task wakers and the command dispatcher.
#[derive(Debug, Default)]
pub(in crate::runtime) struct WorkerSignal {
    notified: Mutex<bool>,
    ready: Condvar,
}

impl WorkerSignal {
    pub(in crate::runtime) fn wait(&self, timeout: Duration) {
        let start = std::time::Instant::now();
        let mut notified = self.notified.lock();
        while !*notified {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                break;
            }
            let (guard, elapsed) = self.ready.wait_timeout(notified, remaining);
            notified = guard;
            if elapsed.timed_out() {
                break;
            }
        }
        *notified = false;
    }

    pub(in crate::runtime) fn waker(signal: &Arc<Self>) -> Waker {
        // Wake requires a standard Arc; both handles share the same allocation.
        Waker::from(Arc::into_std_arc(Arc::clone(signal)))
    }

    fn notify(&self) {
        *self.notified.lock() = true;
        self.ready.notify_one();
    }
}

impl Wake for WorkerSignal {
    fn wake(self: StdArc<Self>) {
        self.notify();
    }

    fn wake_by_ref(self: &StdArc<Self>) {
        self.notify();
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use testing_aids::TEST_TIMEOUT;

    use super::*;

    #[test]
    fn notification_before_wait_is_retained_and_consumed() {
        let signal = Arc::new(WorkerSignal::default());
        WorkerSignal::waker(&signal).wake();
        assert!(*signal.notified.lock());
        signal.wait(Duration::ZERO);
        assert!(!*signal.notified.lock());
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
        WorkerSignal::waker(&signal).wake_by_ref();
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
        assert!(!*signal.notified.lock());
    }

    #[test]
    fn spurious_notifications_do_not_restart_the_timeout() {
        let signal = Arc::new(WorkerSignal::default());
        let timeout = Duration::from_millis(20);
        let (done, finished) = mpsc::channel();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            let start = std::time::Instant::now();
            waiter.wait(timeout);
            done.send(start.elapsed()).unwrap();
        });
        let start = std::time::Instant::now();
        loop {
            signal.ready.notify_one();
            match finished.try_recv() {
                Ok(elapsed) => {
                    assert!(elapsed >= timeout);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    assert!(
                        start.elapsed() < TEST_TIMEOUT,
                        "spurious notifications must not extend the deadline"
                    );
                    thread::yield_now();
                }
                Err(mpsc::TryRecvError::Disconnected) => panic!("the waiter must report its elapsed time"),
            }
        }
        worker.join().unwrap();
    }
}
