// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc as StdArc;
#[cfg(test)]
use std::sync::mpsc;
use std::task::{Wake, Waker};
use std::time::Duration;

use performables::arc::Arc;
use performables::sync::notify::Notify;

/// A coalescing notification shared by task wakers and the command dispatcher.
#[derive(Debug, Default)]
pub(crate) struct WorkerSignal {
    ready: Notify,
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

impl WorkerSignal {
    #[cfg(test)]
    pub(crate) fn new(test_hooks: WorkerSignalTestHooks) -> Self {
        Self {
            ready: Notify::new(),
            test_hooks,
        }
    }

    pub(crate) fn wait_for_notification(&self) {
        if !self.ready.try_wait() {
            #[cfg(test)]
            self.test_hooks.notify_idle_wait();
            self.ready.wait();
        }
        #[cfg(test)]
        self.test_hooks.notify_notification_release();
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        _ = self.ready.wait_timeout(timeout);
    }

    pub(crate) fn waker(signal: &Arc<Self>) -> Waker {
        // Wake requires a standard Arc; both handles share the same allocation.
        Waker::from(Arc::into_std_arc(Arc::clone(signal)))
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn is_notified(&self) -> bool {
        self.ready.is_notified()
    }

    fn notify(&self) {
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
    use std::thread;

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
    fn remote_wake_releases_an_indefinite_waiter() {
        let signal = Arc::new(WorkerSignal::default());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            waiter.wait_for_notification();
            done_tx.send(()).unwrap();
        });
        ready_rx.recv_timeout(TEST_TIMEOUT).unwrap();
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
}
