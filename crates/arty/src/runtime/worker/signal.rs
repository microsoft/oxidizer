// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc as StdArc;
#[cfg(test)]
use std::sync::mpsc;
use std::task::{Wake, Waker};
use std::time::Duration;

use performables::arc::Arc;
use performables::sync::condition::Condvar;
use performables::sync::mutex::Mutex;

/// A coalescing notification shared by task wakers and the command dispatcher.
#[derive(Debug, Default)]
pub(crate) struct WorkerSignal {
    notified: Mutex<bool>,
    ready: Condvar,
    driver_waker: Mutex<Option<Waker>>,
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
    fn notify_wait_started(&self) {
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

fn should_finish_wait(timed_out: bool, elapsed: Duration, timeout: Duration) -> bool {
    timed_out && elapsed >= timeout
}

fn wait_until_deadline<S>(
    state: &mut S,
    timeout: Duration,
    mut waiting: impl FnMut(&S) -> bool,
    mut elapsed: impl FnMut(&S) -> Duration,
    mut timed_wait: impl FnMut(&mut S, Duration) -> bool,
) {
    while waiting(state) {
        let remaining = timeout.saturating_sub(elapsed(state));
        if remaining.is_zero() {
            break;
        }
        let timed_out = timed_wait(state, remaining);
        if should_finish_wait(timed_out, elapsed(state), timeout) {
            break;
        }
    }
}

impl WorkerSignal {
    #[cfg(test)]
    pub(crate) fn new(test_hooks: WorkerSignalTestHooks) -> Self {
        Self {
            notified: Mutex::new(false),
            ready: Condvar::new(),
            driver_waker: Mutex::new(None),
            test_hooks,
        }
    }

    #[cfg(test)]
    pub(crate) fn wait_for_notification(&self) {
        let mut notified = self.notified.lock();
        loop {
            if *notified {
                *notified = false;
                #[cfg(test)]
                self.test_hooks.notify_notification_release();
                return;
            }
            #[cfg(test)]
            self.test_hooks.notify_wait_started();
            notified = self.ready.wait(notified);
        }
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        let start = std::time::Instant::now();
        let mut notified = Some(self.notified.lock());
        wait_until_deadline(
            &mut notified,
            timeout,
            |notified| {
                !**notified
                    .as_ref()
                    .expect("the wait always retains or immediately replaces its notification guard")
            },
            |_| start.elapsed(),
            |notified, remaining| {
                #[cfg(test)]
                self.test_hooks.notify_wait_started();
                let (guard, elapsed) = self.ready.wait_timeout(
                    notified
                        .take()
                        .expect("the previous timed wait always replaced its notification guard"),
                    remaining,
                );
                #[cfg(test)]
                if *guard {
                    self.test_hooks.notify_notification_release();
                }
                *notified = Some(guard);
                elapsed.timed_out()
            },
        );
        let mut notified = notified.expect("the final timed wait always replaced its notification guard");
        *notified = false;
    }

    pub(crate) fn waker(signal: &Arc<Self>) -> Waker {
        // Wake requires a standard Arc; both handles share the same allocation.
        Waker::from(Arc::into_std_arc(Arc::clone(signal)))
    }

    pub(crate) fn set_driver_waker(&self, waker: Waker) {
        assert!(
            self.driver_waker.lock().replace(waker).is_none(),
            "a worker installs its I/O driver interrupt waker once"
        );
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn is_notified(&self) -> bool {
        *self.notified.lock()
    }

    #[cfg_attr(test, mutants::skip)] // Removing all worker and driver wake effects strands runtime tests; each effect is tested directly.
    fn notify(&self) {
        *self.notified.lock() = true;
        if let Some(driver_waker) = self.driver_waker.lock().as_ref() {
            driver_waker.wake_by_ref();
        }
        self.wake_waiter();
    }

    #[cfg_attr(test, mutants::skip)] // Removing the native notification strands the smoke test; flag handling is tested separately.
    fn wake_waiter(&self) {
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
    use std::collections::VecDeque;
    use std::thread;

    use testing_aids::{TEST_TIMEOUT, is_mutation_testing};

    use super::*;

    #[test]
    fn notification_before_wait_is_retained_and_consumed() {
        let signal = Arc::new(WorkerSignal::default());
        let waker = WorkerSignal::waker(&signal);
        waker.wake_by_ref();
        assert!(*signal.notified.lock());
        signal.wait(Duration::ZERO);
        assert!(!*signal.notified.lock());
        waker.wake();
        assert!(*signal.notified.lock());
        signal.wait(Duration::ZERO);
        assert!(!*signal.notified.lock());
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
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn notification_before_indefinite_wait_is_retained_and_consumed() {
        let signal = Arc::new(WorkerSignal::default());
        WorkerSignal::waker(&signal).wake_by_ref();

        signal.wait_for_notification();

        assert!(!*signal.notified.lock());
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
        assert!(!*signal.notified.lock());
    }

    #[test]
    fn wait_timeout_decision_requires_both_expiration_and_deadline() {
        let timeout = Duration::from_millis(20);
        assert!(!should_finish_wait(false, timeout, timeout));
        assert!(!should_finish_wait(true, timeout.saturating_sub(Duration::from_millis(1)), timeout));
        assert!(should_finish_wait(true, timeout, timeout));
    }

    #[test]
    fn spurious_wakes_reduce_the_remaining_wait_budget() {
        struct Script {
            elapsed: Duration,
            elapsed_after_wait: VecDeque<Duration>,
            wait_budgets: Vec<Duration>,
        }

        let timeout = Duration::from_millis(10);
        let mut script = Script {
            elapsed: Duration::from_millis(4),
            elapsed_after_wait: VecDeque::from([Duration::from_millis(7), Duration::from_millis(11)]),
            wait_budgets: Vec::new(),
        };

        wait_until_deadline(
            &mut script,
            timeout,
            |_| true,
            |script| script.elapsed,
            |script, budget| {
                script.wait_budgets.push(budget);
                script.elapsed = script.elapsed_after_wait.pop_front().unwrap();
                false
            },
        );

        assert_eq!(script.wait_budgets, [Duration::from_millis(6), Duration::from_millis(3)]);
        assert_eq!(script.elapsed, Duration::from_millis(11));
        assert!(script.elapsed_after_wait.is_empty());
    }
}
