// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Wake notification shared by an asynchronous worker and its callers.

use std::sync::{Arc as StdArc, OnceLock};
use std::task::{Wake, Waker};
use std::time::Duration;

use performables::arc::Arc;
#[cfg(not(test))]
use performables::sync::notify::Notify;
#[cfg(test)]
use tests::CheckedNotify as Notify;

/// A coalescing notification shared by task wakers and the command dispatcher.
#[derive(Debug, Default)]
pub(crate) struct Signal {
    ready: Notify,
    driver_waker: OnceLock<Waker>,
    #[cfg(test)]
    wait_observer: Option<std::sync::mpsc::Sender<WaitEvent>>,
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WaitEvent {
    Waiting,
    Returned,
}

impl Signal {
    #[cfg(test)]
    pub(crate) fn assert_fixture_ready() {
        let signal = Arc::new(Self::default());
        Self::waker(&signal).wake_by_ref();
        assert!(
            signal.is_notified(),
            "a runtime fixture requires wake_by_ref to retain a notification"
        );
    }

    #[cfg(test)]
    pub(crate) fn with_wait_observer(wait_observer: std::sync::mpsc::Sender<WaitEvent>) -> Self {
        Self {
            wait_observer: Some(wait_observer),
            ..Self::default()
        }
    }

    pub(crate) fn wait_for_notification(&self) {
        if !self.ready.try_wait() {
            #[cfg(test)]
            if let Some(wait_observer) = &self.wait_observer {
                _ = wait_observer.send(WaitEvent::Waiting);
            }
            self.ready.wait();
        }
    }

    pub(crate) fn take_notification(&self) -> bool {
        self.ready.try_wait()
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        _ = self.ready.wait_timeout(timeout);
    }

    pub(crate) fn waker(signal: &Arc<Self>) -> Waker {
        // Wake requires a standard Arc; both handles share the same allocation.
        Waker::from(Arc::into_std_arc(Arc::clone(signal)))
    }

    #[cfg_attr(test, mutants::skip)] // Removing driver interruption strands primary waits across unrelated runtime tests.
    pub(crate) fn set_driver_waker(&self, waker: Waker) {
        assert!(
            self.driver_waker.set(waker).is_ok(),
            "a worker installs its I/O driver interrupt waker once"
        );
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn is_notified(&self) -> bool {
        self.ready.is_notified()
    }

    #[cfg_attr(test, mutants::skip)] // Removing all worker and driver wake effects strands runtime tests; each effect is tested directly.
    fn notify(&self) {
        self.ready.notify_one();
        if let Some(driver_waker) = self.driver_waker.get() {
            driver_waker.wake_by_ref();
        }
    }
}

impl Wake for Signal {
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use testing_aids::{TEST_TIMEOUT, is_mutation_testing};

    use super::*;

    #[derive(Debug, Default)]
    pub(super) struct CheckedNotify {
        inner: performables::sync::notify::Notify,
        consumed: AtomicBool,
    }

    impl CheckedNotify {
        pub(super) fn try_wait(&self) -> bool {
            let consumed = self.inner.try_wait();
            self.consumed.store(consumed, Ordering::Relaxed);
            consumed
        }

        pub(super) fn wait(&self) {
            // Signal must not consume a retained permit and then wait for a second one.
            assert!(
                !self.consumed.swap(false, Ordering::Relaxed),
                "notification already consumed before blocking"
            );
            self.inner.wait();
        }

        pub(super) fn wait_timeout(&self, timeout: Duration) -> bool {
            self.consumed.store(false, Ordering::Relaxed);
            self.inner.wait_timeout(timeout)
        }

        pub(super) fn notify_one(&self) {
            self.inner.notify_one();
        }

        pub(super) fn is_notified(&self) -> bool {
            self.inner.is_notified()
        }
    }

    #[test]
    fn notification_before_wait_is_retained_and_consumed() {
        let signal = Arc::new(Signal::default());
        let waker = Signal::waker(&signal);
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
        let (wait_entered, wait_entry) = mpsc::channel();
        let signal = Arc::new(Signal::with_wait_observer(wait_entered));
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            waiter.wait_for_notification();
            done_tx.send(()).unwrap();
        });
        wait_entry.recv_timeout(TEST_TIMEOUT).unwrap();
        let before_wake = done_rx.try_recv();
        Signal::waker(&signal).wake_by_ref();
        let notified = done_rx.recv_timeout(TEST_TIMEOUT);
        if notified.is_err() {
            signal.ready.notify_one();
        }
        worker.join().unwrap();

        assert_eq!((before_wake, notified), (Err(mpsc::TryRecvError::Empty), Ok(())));
    }

    #[test]
    fn notification_before_indefinite_wait_is_retained_and_consumed() {
        let (observed, observations) = mpsc::channel();
        let signal = Arc::new(Signal::with_wait_observer(observed.clone()));
        Signal::waker(&signal).wake_by_ref();
        let waiter = Arc::clone(&signal);
        let worker = thread::spawn(move || {
            waiter.wait_for_notification();
            observed.send(WaitEvent::Returned).unwrap();
        });

        let first = observations.recv_timeout(TEST_TIMEOUT).unwrap();
        if first == WaitEvent::Waiting {
            signal.ready.notify_one();
        }
        worker.join().unwrap();

        assert_eq!((first, signal.is_notified()), (WaitEvent::Returned, false));
    }

    #[test]
    fn taking_a_notification_consumes_it_once() {
        let signal = Arc::new(Signal::default());
        let before_wake = signal.take_notification();
        Signal::waker(&signal).wake_by_ref();

        let first = signal.take_notification();
        let second = signal.take_notification();

        assert_eq!((before_wake, first, second), (false, true, false));
    }

    #[test]
    fn an_unnotified_wait_can_time_out() {
        if is_mutation_testing() {
            return;
        }
        let signal = Signal::default();
        let timeout = Duration::from_millis(20);
        let start = std::time::Instant::now();
        signal.wait(timeout);
        assert!(start.elapsed() >= timeout);
        assert!(!signal.is_notified());
    }
}
