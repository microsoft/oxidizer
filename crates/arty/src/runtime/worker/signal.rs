// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc as StdArc, OnceLock};
#[cfg(test)]
use std::sync::mpsc;
use std::task::{Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use performables::arc::Arc;

/// A coalescing notification shared by task wakers and the command dispatcher.
#[derive(Debug, Default)]
pub(crate) struct WorkerSignal {
    notified: AtomicBool,
    worker: OnceLock<Thread>,
    #[cfg(test)]
    test_hooks: WorkerSignalTestHooks,
}

#[cfg(test)]
#[derive(Debug, Default, Clone)]
pub(crate) struct WorkerSignalTestHooks {
    idle_wait_notifier: Option<mpsc::Sender<()>>,
    notification_wait_released: Option<mpsc::Sender<()>>,
}

#[cfg(test)]
impl WorkerSignalTestHooks {
    pub(crate) fn with_wait_observers(idle_wait_notifier: mpsc::Sender<()>, notification_wait_released: mpsc::Sender<()>) -> Self {
        Self {
            idle_wait_notifier: Some(idle_wait_notifier),
            notification_wait_released: Some(notification_wait_released),
        }
    }

    #[cfg(test)]
    fn notify_idle_wait(&self) {
        if let Some(notifier) = &self.idle_wait_notifier {
            _ = notifier.send(());
        }
    }

    fn notify_notification_release(&self) {
        if let Some(notifier) = &self.notification_wait_released {
            _ = notifier.send(());
        }
    }
}

fn wait_until_deadline(
    timeout: Duration,
    mut waiting: impl FnMut() -> bool,
    mut elapsed: impl FnMut() -> Duration,
    mut park: impl FnMut(Duration),
) {
    while waiting() {
        let remaining = timeout.saturating_sub(elapsed());
        if remaining.is_zero() {
            break;
        }
        park(remaining);
    }
}

impl WorkerSignal {
    #[cfg(test)]
    pub(crate) fn new(test_hooks: WorkerSignalTestHooks) -> Self {
        Self {
            notified: AtomicBool::new(false),
            worker: OnceLock::new(),
            test_hooks,
        }
    }

    pub(crate) fn wait_for_notification(&self) {
        self.worker.get_or_init(thread::current);
        loop {
            if self.notified.swap(false, Ordering::Acquire) {
                #[cfg(test)]
                self.test_hooks.notify_notification_release();
                return;
            }
            #[cfg(test)]
            self.test_hooks.notify_idle_wait();
            thread::park();
        }
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        // Register before checking the latch: a concurrent notifier either leaves
        // the latch set or sees this handle and supplies the thread's unpark token.
        self.worker.get_or_init(thread::current);
        let start = std::time::Instant::now();
        wait_until_deadline(
            timeout,
            || !self.notified.swap(false, Ordering::Acquire),
            || start.elapsed(),
            thread::park_timeout,
        );
    }

    pub(crate) fn waker(signal: &Arc<Self>) -> Waker {
        // Wake requires a standard Arc; both handles share the same allocation.
        Waker::from(Arc::into_std_arc(Arc::clone(signal)))
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn is_notified(&self) -> bool {
        self.notified.load(Ordering::Acquire)
    }

    fn notify(&self) {
        if !self.notified.swap(true, Ordering::Release) {
            self.wake_waiter();
        }
    }

    #[cfg_attr(test, mutants::skip)] // Removing the native notification strands the smoke test; flag handling is tested separately.
    fn wake_waiter(&self) {
        if let Some(worker) = self.worker.get() {
            worker.unpark();
        }
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
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::mpsc;

    use testing_aids::{TEST_TIMEOUT, is_mutation_testing};

    use super::*;

    #[test]
    fn notification_before_wait_is_retained_and_consumed() {
        let signal = Arc::new(WorkerSignal::default());
        let waker = WorkerSignal::waker(&signal);
        waker.wake_by_ref();
        assert!(signal.is_notified());
        signal.wait(Duration::ZERO);
        assert!(!signal.is_notified());
        waker.wake();
        assert!(signal.is_notified());
        signal.wait(Duration::ZERO);
        assert!(!signal.is_notified());
    }

    #[test]
    fn notification_bursts_coalesce_before_waiter_registration() {
        let signal = WorkerSignal::default();
        for _ in 0..16 {
            signal.notify();
        }
        assert!(signal.worker.get().is_none());
        assert!(signal.is_notified());
        signal.wait(Duration::ZERO);
        assert!(!signal.is_notified());
    }

    #[test]
    fn remote_wake_after_waiter_registration_is_retained() {
        if is_mutation_testing() {
            return;
        }
        let signal = Arc::new(WorkerSignal::default());
        let (done, received) = mpsc::channel();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            waiter.wait(TEST_TIMEOUT.saturating_mul(2));
            done.send(()).unwrap();
        });
        let deadline = std::time::Instant::now() + TEST_TIMEOUT;
        while signal.worker.get().is_none() {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        signal.notify();
        received.recv_timeout(TEST_TIMEOUT).unwrap();
        worker.join().unwrap();
        assert!(!signal.is_notified());
    }

    #[test]
    fn repeated_notifications_racing_with_wait_are_not_lost() {
        if is_mutation_testing() {
            return;
        }
        let rounds = if cfg!(miri) { 2 } else { 64 };
        let signal = Arc::new(WorkerSignal::default());
        let (ready, about_to_wait) = mpsc::channel();
        let (done, received) = mpsc::channel();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            for _ in 0..rounds {
                ready.send(()).unwrap();
                waiter.wait(TEST_TIMEOUT.saturating_mul(2));
                done.send(()).unwrap();
            }
        });
        for _ in 0..rounds {
            about_to_wait.recv_timeout(TEST_TIMEOUT).unwrap();
            signal.notify();
            received.recv_timeout(TEST_TIMEOUT).unwrap();
        }
        worker.join().unwrap();
    }

    #[test]
    fn remote_wake_releases_an_indefinite_waiter() {
        let signal = Arc::new(WorkerSignal::default());
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            waiter.wait_for_notification();
            done_tx.send(()).unwrap();
        });
        let deadline = std::time::Instant::now() + TEST_TIMEOUT;
        while signal.worker.get().is_none() {
            assert!(std::time::Instant::now() < deadline);
            thread::yield_now();
        }
        assert_eq!(
            done_rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        WorkerSignal::waker(&signal).wake_by_ref();
        done_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn notification_before_indefinite_wait_is_retained_and_consumed() {
        let signal = Arc::new(WorkerSignal::default());
        WorkerSignal::waker(&signal).wake_by_ref();

        signal.wait_for_notification();

        assert!(!signal.is_notified());
    }

    #[test]
    fn an_unnotified_wait_can_time_out() {
        if is_mutation_testing() {
            return;
        }
        let signal = WorkerSignal::default();
        let timeout = Duration::from_millis(20);
        let start = std::time::Instant::now();
        signal.wait(timeout);
        assert!(start.elapsed() >= timeout);
        assert!(!signal.is_notified());
    }

    #[test]
    fn spurious_unpark_does_not_finish_an_unnotified_wait() {
        if is_mutation_testing() {
            return;
        }
        let signal = WorkerSignal::default();
        let timeout = Duration::from_millis(20);
        thread::current().unpark();
        let start = std::time::Instant::now();
        signal.wait(timeout);
        assert!(start.elapsed() >= timeout);
        assert!(!signal.is_notified());
    }

    #[test]
    fn spurious_wakes_reduce_the_remaining_wait_budget() {
        let timeout = Duration::from_millis(10);
        let elapsed = Cell::new(Duration::from_millis(4));
        let mut elapsed_after_wait = VecDeque::from([Duration::from_millis(7), Duration::from_millis(11)]);
        let mut wait_budgets = Vec::new();

        wait_until_deadline(
            timeout,
            || true,
            || elapsed.get(),
            |budget| {
                wait_budgets.push(budget);
                elapsed.set(elapsed_after_wait.pop_front().unwrap());
            },
        );

        assert_eq!(wait_budgets, [Duration::from_millis(6), Duration::from_millis(3)]);
        assert_eq!(elapsed.get(), Duration::from_millis(11));
        assert!(elapsed_after_wait.is_empty());
    }
}
