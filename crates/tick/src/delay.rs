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
/// The delay uses the current thread's scheduler to schedule its ticks. The precision
/// of the delay is affected by the load on this thread. There are no guarantees about the
/// precision of the delay other than that it will eventually complete. When the thread is healthy,
/// the delay should be close to the specified duration.
///
/// Note: `Delay` is not affected by changes in the system clock.
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
pub struct Delay {
    // Currently scheduled timer. This value is not initialized before
    // actually calling the `Future::poll` method.
    current_timer: Option<TimerKey>,
    clock: Clock,
    duration: Duration,
}

impl Delay {
    /// Creates a new delay that will complete after the specified duration.
    ///
    /// If the duration is [`Duration::ZERO`], the delay completes immediately.
    /// If the duration is [`Duration::MAX`], the delay never completes.
    ///
    /// > **Note**: Consider using [`Clock::delay()`] as a shortcut for creating delays.
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
    #[must_use]
    pub fn new(clock: &Clock, duration: Duration) -> Self {
        Self {
            duration,
            current_timer: None,
            clock: clock.clone(),
        }
    }

    fn register_timer(&mut self, waker: &Waker) -> Poll<()> {
        let when = self.clock.timer_instant().checked_add(self.duration);

        if let Some(when) = when {
            self.current_timer = Some(self.clock.register_timer(when, waker.clone()));
        } else {
            // We have moved past the maximum instant value; this delay never completes.
            self.duration = Duration::MAX;
            if let Some(key) = self.current_timer.take() {
                self.clock.unregister_timer(key);
            }
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
            Some(key) if key.tick() <= this.clock.timer_instant() || !this.clock.contains_timer(key) => {
                this.current_timer = None;

                // Unregister the timer if precise time reached the deadline before the driver did.
                // If the driver already consumed the key, its wake is authoritative even when a
                // coarse driver timestamp led the precise clock.
                this.clock.unregister_timer(key);

                Poll::Ready(())
            }
            Some(_) => Poll::Pending,
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
}
