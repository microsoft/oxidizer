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
/// The delay uses the current thread scheduler to schedule its ticks. The precision
/// of the delay is affected by the load on this thread. There are no guarantees about the
/// precision of the delay other than that it will eventually finish. When the thread is healthy,
/// the delay should be close to the specified one.
///
/// # Examples
///
/// ```
/// use oxidizer_time::{Clock, Stopwatch, Delay};
/// use std::time::Duration;
///
/// async fn delay_example(clock: &Clock) {
///     let stopwatch = Stopwatch::with_clock(&clock);
///
///     // Delay for 10 millis
///     Delay::with_clock(&clock, Duration::from_millis(10)).await;
///
///     assert!(stopwatch.elapsed() >= Duration::from_millis(10));
/// }
///
/// # fn main() {
/// #     let clock = Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance_timers(true));
/// #     futures::executor::block_on(delay_example(&clock));
/// # }
/// ```
#[derive(Debug)]
pub struct Delay {
    // Currently scheduled timer. This value is not initialized before
    // actually calling the "Future::poll" method.
    current_timer: Option<TimerKey>,
    clock: Clock,
    duration: Duration,
}

impl Delay {
    /// Creates a new delay that will finish after the specified duration.
    ///
    /// If the duration is [`Duration::ZERO`], the delay does nothing.
    /// If the duration is [`Duration::MAX`], the delay never finishes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxidizer_time::{Clock, Stopwatch, Delay};
    /// use std::time::Duration;
    ///
    /// async fn delay_example(clock: &Clock) {
    ///     let stopwatch = Stopwatch::with_clock(&clock);
    ///
    ///     // Delay for 10 millis
    ///     Delay::with_clock(&clock, Duration::from_millis(10)).await;
    ///
    ///     assert!(stopwatch.elapsed() >= Duration::from_millis(10));
    /// }
    ///
    /// # fn main() {
    /// #     let clock = Clock::with_control(&oxidizer_time::ClockControl::new().auto_advance_timers(true));
    /// #     futures::executor::block_on(delay_example(&clock));
    /// # }
    /// ```
    #[must_use]
    pub fn with_clock(clock: &Clock, duration: Duration) -> Self {
        Self {
            duration,
            current_timer: None,
            clock: clock.clone(),
        }
    }

    fn register_timer(&mut self, waker: &Waker) -> Poll<()> {
        let when = self.clock.instant_now().checked_add(self.duration);

        if let Some(when) = when {
            self.current_timer = Some(self.clock.register_timer(when, waker.clone()));
        } else {
            // We have moved past the maximum instant value, this delay never finishes.
            self.duration = Duration::MAX;
            self.current_timer = None;
        }

        Poll::Pending
    }
}

impl Future for Delay {
    type Output = ();

    #[mutants::skip] // some mutations never finish and cause timeouts
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        match this.current_timer {
            None if this.duration == Duration::MAX => Poll::Pending,
            None if this.duration == Duration::ZERO => Poll::Ready(()),
            None => this.register_timer(cx.waker()),
            Some(key) if key.tick() <= this.clock.instant_now() => {
                this.current_timer = None;

                // Unregister timer, just in case this call was explicit
                // and not due to timers advancing.
                this.clock.unregister_timer(key);

                Poll::Ready(())
            }
            Some(_) => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Instant;

    use futures::task::noop_waker;

    use super::*;
    use crate::ClockControl;
    use crate::runtime::MiniRuntime;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(Delay: Send, Sync);
    }

    #[test]
    fn delay_ok() {
        MiniRuntime::execute(async move |clock| {
            let now = Instant::now();
            Delay::with_clock(&clock, Duration::from_millis(5)).await;
            assert!(now.elapsed() >= Duration::from_millis(5));
        });
    }

    #[test]
    fn delay_with_control() {
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);
        let mut delay = Delay::with_clock(&clock, Duration::from_millis(1));

        assert_eq!(poll_delay(&mut delay), Poll::Pending);
        thread::sleep(Duration::from_millis(1));
        assert_eq!(poll_delay(&mut delay), Poll::Pending);

        let len = control.timers_len();
        control.advance(Duration::from_millis(2));
        assert_eq!(control.timers_len(), len - 1);
        assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
    }

    #[test]
    fn delay_zero() {
        let clock = Clock::new_dormant();
        let mut delay = Delay::with_clock(&clock, Duration::ZERO);
        assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
    }

    #[test]
    fn delay_max() {
        let clock = Clock::new_dormant();

        let result = poll_delay(&mut Delay::with_clock(&clock, Duration::MAX));

        assert_eq!(result, Poll::Pending);
    }

    #[test]
    fn delay_zero_ensure_timer_not_registered() {
        let clock = Clock::new_dormant();
        assert!(
            Delay::with_clock(&clock, Duration::ZERO)
                .current_timer
                .is_none()
        );
    }

    #[test]
    fn delay_max_ensure_timer_not_registered() {
        let clock = Clock::new_dormant();
        assert!(
            Delay::with_clock(&clock, Duration::MAX)
                .current_timer
                .is_none()
        );
    }

    #[test]
    fn delay_close_to_max_ensure_timer_not_registered() {
        let clock = Clock::new_dormant();
        let mut delay = Delay::with_clock(
            &clock,
            Duration::MAX.saturating_sub(Duration::from_millis(1)),
        );

        assert_eq!(poll_delay(&mut delay), Poll::Pending);
        assert_eq!(delay.duration, Duration::MAX);
        assert!(delay.current_timer.is_none());
    }

    #[test]
    fn ready_without_advancing_timers_ensure_timer_unregistered() {
        let clock = Clock::new_dormant();
        let period = Duration::from_millis(1);
        let mut delay = Delay::with_clock(&clock, period);

        assert_eq!(poll_delay(&mut delay), Poll::Pending);
        assert_eq!(clock.clock_state().timers_len(), 1);
        thread::sleep(period);
        assert_eq!(poll_delay(&mut delay), Poll::Ready(()));
        assert_eq!(delay.current_timer, None);
        assert_eq!(clock.clock_state().timers_len(), 0);
    }

    fn poll_delay(delay: &mut Delay) -> Poll<()> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let delay = std::pin::pin!(delay);

        delay.poll(&mut cx)
    }
}