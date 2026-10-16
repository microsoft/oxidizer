// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::Instant;

use ::tokio::sync::Notify;

use crate::Clock;
use crate::runtime::ClockDriver;

#[derive(Debug)]
struct DriverWake(Arc<Notify>);

impl Wake for DriverWake {
    fn wake(self: Arc<Self>) {
        self.0.notify_one();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.notify_one();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitOutcome {
    Notified,
    Deadline,
}

async fn wait_for_driver_event(notify: &Notify, next_timer: Option<Instant>) -> WaitOutcome {
    if let Some(deadline) = next_timer {
        match ::tokio::time::timeout_at(::tokio::time::Instant::from_std(deadline), notify.notified()).await {
            Ok(()) => WaitOutcome::Notified,
            Err(_) => WaitOutcome::Deadline,
        }
    } else {
        notify.notified().await;
        WaitOutcome::Notified
    }
}

async fn run_driver(
    mut driver: ClockDriver,
    notify: Arc<Notify>,
    mut now: impl FnMut() -> Instant,
    mut wait_observer: impl FnMut(Option<Instant>, Option<WaitOutcome>),
) {
    while let Ok(next_timer) = driver.advance_timers(now()) {
        wait_observer(next_timer, None);
        let outcome = wait_for_driver_event(&notify, next_timer).await;
        wait_observer(next_timer, Some(outcome));
    }
}

impl Clock {
    /// Creates a new clock driven by the Tokio runtime.
    ///
    /// # Panics
    ///
    /// Panics if called outside of a Tokio runtime context.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Causes test timeout.
    pub fn new_tokio() -> Self {
        Self::new_tokio_core().0
    }

    fn new_tokio_core() -> (Self, ::tokio::task::JoinHandle<()>) {
        // The Tokio clock is driven by a single background task that advances a shared timer
        // set. We use the shared inactive-clock variant because the single driver must observe
        // timers registered by clones on any Tokio worker.
        let notify = Arc::new(Notify::new());
        let driver_waker = Waker::from(Arc::new(DriverWake(Arc::clone(&notify))));
        let (clock, driver) = crate::runtime::InactiveClock::new_shared().activate_with_driver_waker(driver_waker);

        let join_handle = ::tokio::spawn(run_driver(driver, notify, Instant::now, |_, _| {}));

        (clock, join_handle)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use super::*;

    #[derive(Debug)]
    struct CountingWake(AtomicUsize);

    async fn next_driver_event(
        observed_events: &mut ::tokio::sync::mpsc::UnboundedReceiver<(Option<Instant>, Option<WaitOutcome>)>,
    ) -> (Option<Instant>, Option<WaitOutcome>) {
        ::tokio::time::timeout(Duration::from_secs(1), observed_events.recv())
            .await
            .unwrap()
            .unwrap()
    }

    impl Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn driver_wake_notifies_waiter() {
        let notify = Arc::new(Notify::new());
        let mut notified = std::pin::pin!(notify.notified());
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(notified.as_mut().poll(&mut context), Poll::Pending);

        Waker::from(Arc::new(DriverWake(Arc::clone(&notify)))).wake();

        assert_eq!(notified.as_mut().poll(&mut context), Poll::Ready(()));
    }

    #[::tokio::test(start_paused = true)]
    async fn no_deadline_wait_remains_parked_without_periodic_wakes() {
        let notify = Notify::new();
        let mut waiter = std::pin::pin!(wait_for_driver_event(&notify, None));
        let wake_count = Arc::new(CountingWake(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&wake_count));
        let mut context = Context::from_waker(&waker);
        assert_eq!(waiter.as_mut().poll(&mut context), Poll::Pending);

        ::tokio::time::advance(Duration::from_millis(100)).await;
        ::tokio::task::yield_now().await;
        assert_eq!(wake_count.0.load(Ordering::Relaxed), 0);
        assert_eq!(waiter.as_mut().poll(&mut context), Poll::Pending);

        notify.notify_one();

        assert_eq!(wake_count.0.load(Ordering::Relaxed), 1);
        assert_eq!(waiter.as_mut().poll(&mut context), Poll::Ready(WaitOutcome::Notified));
    }

    #[::tokio::test(start_paused = true)]
    async fn earlier_timer_interrupts_an_established_long_deadline_wait() {
        let notify = Arc::new(Notify::new());
        let driver_waker = Waker::from(Arc::new(DriverWake(Arc::clone(&notify))));
        let (clock, driver) = crate::runtime::InactiveClock::new_shared().activate_with_driver_waker(driver_waker);
        let fixed_now = Instant::now();
        let (events, mut observed_events) = ::tokio::sync::mpsc::unbounded_channel();
        let driver_task = ::tokio::spawn(run_driver(
            driver,
            notify,
            move || fixed_now,
            move |deadline, outcome| {
                events.send((deadline, outcome)).unwrap();
            },
        ));

        assert_eq!(next_driver_event(&mut observed_events).await, (None, None));

        let long_deadline = fixed_now + Duration::from_secs(5);
        let long_timer = clock.register_timer(long_deadline, Waker::noop().clone());
        assert_eq!(next_driver_event(&mut observed_events).await, (None, Some(WaitOutcome::Notified)));
        assert_eq!(next_driver_event(&mut observed_events).await, (Some(long_deadline), None));

        let tokio_now = ::tokio::time::Instant::now();
        let short_deadline = fixed_now + Duration::from_millis(20);
        let short_timer = clock.register_timer(short_deadline, Waker::noop().clone());
        assert_eq!(
            next_driver_event(&mut observed_events).await,
            (Some(long_deadline), Some(WaitOutcome::Notified))
        );
        assert_eq!(next_driver_event(&mut observed_events).await, (Some(short_deadline), None));
        assert_eq!(::tokio::time::Instant::now(), tokio_now);

        driver_task.abort();
        assert!(driver_task.await.unwrap_err().is_cancelled());
        clock.unregister_timer(short_timer);
        clock.unregister_timer(long_timer);
        drop(clock);
    }

    #[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
    #[::tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hidden_driver_task_finishes_when_clock_is_dropped() {
        // Unit access is required because the public API intentionally hides the driver task's
        // join handle; public timing and relocation behavior is covered by integration tests.
        let (clock, handle) = Clock::new_tokio_core();

        ::tokio::time::timeout(Duration::from_secs(1), clock.delay(Duration::from_millis(15)))
            .await
            .unwrap();
        drop(clock);

        ::tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("the hidden driver task must terminate after its final clock owner is dropped")
            .unwrap();
    }
}
