// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use super::Clock;
use super::timers::TimerKey;

/// Asynchronously delays for the specified duration.
///
/// # Precision
///
/// The delay uses the scheduler of the current thread. Thread load affects its
/// precision. The delay will complete, but it does not guarantee an exact
/// completion time. Under normal load, it completes near the requested duration.
///
/// Changes to the system clock do not affect `Delay`.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use tick::{Clock, Delay};
///
/// # async fn delay_example(clock: &Clock) {
/// let stopwatch = clock.stopwatch();
///
/// // Delay for 10 milliseconds
/// clock.delay(Duration::from_millis(10)).await;
///
/// assert!(stopwatch.elapsed() >= Duration::from_millis(10));
/// # }
/// ```
#[derive(Debug)]
#[must_use = "futures do nothing unless awaited or polled"]
pub struct Delay {
    // Currently scheduled timer. This value is not initialized before
    // actually calling the `Future::poll` method.
    current_timer: Option<TimerKey>,
    current_waker: Option<Waker>,
    clock: Clock,
    duration: Duration,
}

impl Delay {
    /// Creates a new delay that will complete after the specified duration.
    ///
    /// If the duration is [`Duration::ZERO`], the delay completes immediately.
    /// If the duration is [`Duration::MAX`], the delay never completes.
    ///
    /// You can use [`Clock::delay()`] as a shorter form.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use tick::{Clock, Delay};
    ///
    /// # async fn delay_example(clock: &Clock) {
    /// let stopwatch = clock.stopwatch();
    ///
    /// // Delay for 10 milliseconds
    /// Delay::new(clock, Duration::from_millis(10)).await;
    ///
    /// assert!(stopwatch.elapsed() >= Duration::from_millis(10));
    /// # }
    /// ```
    #[must_use = "futures do nothing unless awaited or polled"]
    pub fn new(clock: &Clock, duration: Duration) -> Self {
        Self {
            duration,
            current_timer: None,
            current_waker: None,
            clock: clock.clone(),
        }
    }

    fn register_timer(&mut self, waker: &Waker) -> Poll<()> {
        let when = self.clock.timer_instant().checked_add(self.duration);

        if let Some(when) = when {
            self.current_timer = Some(self.clock.register_timer(when, waker.clone()));
            self.current_waker = Some(waker.clone());
        } else {
            // We have moved past the maximum instant value; this delay never completes.
            self.duration = Duration::MAX;
            if let Some(key) = self.current_timer.take() {
                self.clock.unregister_timer(key);
            }
            self.current_waker = None;
        }

        Poll::Pending
    }
}

impl Future for Delay {
    type Output = ();

    #[cfg_attr(test, mutants::skip)] // some mutations never finish and cause timeouts
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        match this.current_timer {
            None if this.duration == Duration::MAX => Poll::Pending,
            None if this.duration == Duration::ZERO => Poll::Ready(()),
            None => this.register_timer(cx.waker()),
            Some(key) if key.tick() <= this.clock.timer_instant() => {
                this.current_timer = None;
                this.current_waker = None;

                // Unregister the timer, just in case this call was explicit
                // and not due to timers advancing.
                this.clock.unregister_timer(key);

                Poll::Ready(())
            }
            Some(key) => {
                if this.current_waker.as_ref().is_some_and(|waker| waker.will_wake(cx.waker())) {
                    return Poll::Pending;
                }

                if this.clock.update_timer_waker(key, cx.waker()) {
                    this.current_waker = Some(cx.waker().clone());
                    Poll::Pending
                } else {
                    // The driver can remove and wake the timer between the time check above
                    // and this replacement. A missing registration therefore means completion.
                    this.current_timer = None;
                    this.current_waker = None;
                    Poll::Ready(())
                }
            }
        }
    }
}

impl Drop for Delay {
    fn drop(&mut self) {
        if let Some(key) = self.current_timer {
            self.clock.unregister_timer(key);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::ClockControl;

    #[test]
    fn private_overflowing_reregistration_clears_the_existing_timer_key() {
        // Public callers cannot mutate a polled delay's duration to force this recovery branch.
        let clock = ClockControl::new().to_clock();
        let mut delay = Delay::new(&clock, Duration::from_millis(1));
        let waker = Waker::noop();
        assert_eq!(delay.register_timer(waker), Poll::Pending);
        assert!(delay.current_timer.is_some());
        assert_eq!(clock.clock_state().timers_len(), 1);

        delay.duration = Duration::MAX;
        assert_eq!(delay.register_timer(waker), Poll::Pending);

        assert_eq!(delay.current_timer, None);
        assert_eq!(clock.clock_state().timers_len(), 0);
    }

    #[test]
    fn missing_timer_is_treated_as_fired_when_the_task_changes() {
        #[derive(Debug)]
        struct CountingWake(std::sync::atomic::AtomicUsize);

        impl std::task::Wake for CountingWake {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let clock = ClockControl::new().to_clock();
        let mut delay = Delay::new(&clock, Duration::from_millis(1));
        let first_waker = Waker::from(Arc::new(CountingWake(std::sync::atomic::AtomicUsize::new(0))));
        let mut first_context = Context::from_waker(&first_waker);
        assert_eq!(Pin::new(&mut delay).poll(&mut first_context), Poll::Pending);
        clock.unregister_timer(delay.current_timer.unwrap());

        let second_waker = Waker::from(Arc::new(CountingWake(std::sync::atomic::AtomicUsize::new(0))));
        let mut second_context = Context::from_waker(&second_waker);

        assert_eq!(Pin::new(&mut delay).poll(&mut second_context), Poll::Ready(()));
    }
}
