// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc as StdArc;
use std::task::{Wake, Waker};
use std::time::Duration;

use performables::arc::Arc;
use performables::sync::condition::Condvar;
use performables::sync::mutex::Mutex;
use seismograph_runtime::worker::WorkerHandle;

/// A coalescing notification shared by task wakers and the command dispatcher.
#[derive(Debug, Default)]
pub(in crate::runtime) struct WorkerSignal {
    notified: Mutex<bool>,
    ready: Condvar,
    telemetry: Option<WorkerHandle>,
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
    pub(in crate::runtime) fn with_telemetry(telemetry: WorkerHandle) -> Self {
        Self {
            notified: Mutex::new(false),
            ready: Condvar::new(),
            telemetry: Some(telemetry),
        }
    }

    pub(in crate::runtime) fn wait(&self, timeout: Duration) {
        let start = std::time::Instant::now();
        let mut notified = Some(self.notified.lock());
        let notification = notified.as_mut().expect("the wait starts with its notification guard");
        if **notification {
            **notification = false;
            return;
        }
        if let Some(telemetry) = &self.telemetry {
            telemetry.parked();
        }
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
                let (guard, elapsed) = self.ready.wait_timeout(
                    notified
                        .take()
                        .expect("the previous timed wait always replaced its notification guard"),
                    remaining,
                );
                *notified = Some(guard);
                elapsed.timed_out()
            },
        );
        let mut notified = notified.expect("the final timed wait always replaced its notification guard");
        *notified = false;
        if let Some(telemetry) = &self.telemetry {
            telemetry.unparked();
        }
    }

    pub(in crate::runtime) fn waker(signal: &Arc<Self>) -> Waker {
        // Wake requires a standard Arc; both handles share the same allocation.
        Waker::from(Arc::into_std_arc(Arc::clone(signal)))
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub(in crate::runtime) fn is_notified(&self) -> bool {
        *self.notified.lock()
    }

    fn notify(&self) {
        *self.notified.lock() = true;
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
    use std::sync::mpsc;
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
    fn remote_wake_releases_a_waiter() {
        if is_mutation_testing() {
            return;
        }
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
