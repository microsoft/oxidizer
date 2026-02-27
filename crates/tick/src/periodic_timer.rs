// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use futures_core::Stream;

use super::Clock;
use super::timers::TimerKey;
use crate::timers::TIMER_RESOLUTION;

/// A timer that periodically ticks.
///
/// A periodic timer can be created using the [`PeriodicTimer::new()`] constructor,
/// which requires a reference to a [`Clock`].
///
/// # Precision
///
/// The timer uses the current thread's scheduler to schedule its ticks. The precision
/// of the timer is affected by the load on this thread. There are no guarantees about the
/// precision of the timer other than that it will eventually tick. When the thread is healthy,
/// the timer's period should be close to the specified one.
///
/// > **Note**: The periodic timer is not affected by adjustments to the system clock.
///
/// # Stream Behavior
///
/// `PeriodicTimer` implements [`Stream`] and will never complete. The stream produces
/// a tick every period indefinitely. Use stream combinators like [`StreamExt::take`]
/// to limit the number of ticks.
///
/// [`StreamExt::take`]: https://docs.rs/futures/latest/futures/stream/trait.StreamExt.html#method.take
///
/// # Examples
///
/// ## Create a periodic timer
///
/// ```
/// use std::time::Duration;
///
/// use futures::StreamExt;
/// use tick::{Clock, PeriodicTimer, Stopwatch};
///
/// # async fn periodic_timer_example(clock: &Clock) {
/// let timer = PeriodicTimer::new(clock, Duration::from_millis(1));
///
/// timer
///     .take(3)
///     .for_each(async |()| {
///         // Do something every 1ms
///     })
///     .await;
/// # }
/// ```
///
/// ## Create a periodic timer with initial delay
///
/// ```
/// use std::time::Duration;
///
/// use futures::StreamExt;
/// use tick::{Clock, PeriodicTimer};
///
/// # async fn periodic_timer_example(clock: &Clock) {
/// // Delay for 10ms before the timer starts ticking
/// clock.delay(Duration::from_millis(10)).await;
///
/// let timer = PeriodicTimer::new(clock, Duration::from_millis(1));
///
/// timer
///     .take(3)
///     .for_each(async |()| {
///         // Do something every 1ms
///     })
///     .await;
/// # }
/// ```
#[derive(Debug)]
pub struct PeriodicTimer {
    period: Duration,
    clock: Clock,
    // Currently scheduled timer. This value is not initialized until
    // the first call to the `Stream::poll_next` method.
    current_timer: Option<TimerKey>,
}

impl PeriodicTimer {
    /// Creates a timer that fires periodically.
    ///
    /// > **Note**: The minimum precision of the timer is 1ms. If a smaller period is specified,
    /// > it will be adjusted to 1ms.
    #[must_use]
    pub fn new(clock: &Clock, period: Duration) -> Self {
        let period = period.max(TIMER_RESOLUTION);

        Self {
            // The timer is not registered yet; it will be registered on the first
            // call to `Stream::poll_next`.
            current_timer: None,
            period,
            clock: clock.clone(),
        }
    }

    fn register_timer(&mut self, waker: Waker) {
        match self.clock.instant().checked_add(self.period) {
            Some(when) => {
                self.current_timer = Some(self.clock.register_timer(when, waker));
            }
            None => {
                // The timer would tick so far in the future that we can assume
                // it never fires. For this reason, there is no point in registering it.
                // The period is set to Duration::MAX to prevent further registrations.
                self.period = Duration::MAX;
            }
        }
    }
}

impl Stream for PeriodicTimer {
    type Item = ();

    #[cfg_attr(test, mutants::skip)] // cannot reliably check that poll_tick has been called
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if this.period == Duration::MAX {
            return Poll::Pending;
        }

        match this.current_timer {
            Some(key) if key.tick() <= this.clock.instant() => {
                // Reset the timer. It will be registered again on the next poll.
                this.current_timer = None;

                // Unregister the timer, just in case this call was explicit and not due to
                // timers advancing.
                this.clock.unregister_timer(key);

                Poll::Ready(Some(()))
            }
            // Timer is registered and will fire later in the future.
            Some(_) => Poll::Pending,

            // Timer is not registered yet; let's register it.
            // The registration is lazy, occurring when someone polls the future. This means
            // that the work between two ticks is not taken into account when scheduling
            // the next tick. When the thread is busy, the timer may tick later than expected.
            None => {
                this.register_timer(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

impl Drop for PeriodicTimer {
    fn drop(&mut self) {
        if let Some(key) = self.current_timer {
            self.clock.unregister_timer(key);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;
    use crate::ClockControl;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(PeriodicTimer: Send, Sync);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn next_ensure_awaited() {
        use futures::StreamExt;

        use crate::FutureExt;

        let clock = Clock::new_tokio();
        let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(1));

        async move {
            assert_eq!(timer.next().await, Some(()));
            assert_eq!(timer.next().await, Some(()));
        }
        .timeout(&clock, Duration::from_secs(5))
        .await
        .unwrap();
    }

    #[test]
    fn next_with_control() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(1));

        assert_eq!(poll_timer(&mut timer), Poll::Pending);
        thread::sleep(Duration::from_millis(1));
        assert_eq!(poll_timer(&mut timer), Poll::Pending);

        let len = control.timers_len();
        control.advance(Duration::from_millis(2));
        assert_eq!(control.timers_len(), len - 1);
        assert_eq!(poll_timer(&mut timer), Poll::Ready(Some(())));
    }

    #[test]
    fn first_poll_next_should_be_pending() {
        let clock = Clock::new_frozen();

        let mut timer = PeriodicTimer::new(&clock, Duration::from_millis(1));

        assert_eq!(poll_timer(&mut timer), Poll::Pending);
    }

    #[test]
    fn new_zero_duration_period_adjusted() {
        let clock = Clock::new_frozen();

        let timer = PeriodicTimer::new(&clock, Duration::ZERO);

        assert_eq!(timer.period, Duration::from_millis(1));
    }

    #[test]
    fn new_duration_near_max_never_fires() {
        let clock = Clock::new_frozen();

        let mut timer = PeriodicTimer::new(&clock, Duration::MAX.saturating_sub(Duration::from_millis(1)));

        assert_eq!(poll_timer(&mut timer), Poll::Pending);
        assert_eq!(poll_timer(&mut timer), Poll::Pending);

        assert_eq!(timer.period, Duration::MAX);
        assert_eq!(timer.current_timer, None);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn ready_without_advancing_timers_ensure_timer_unregistered() {
        let clock = Clock::new_tokio();
        let period = Duration::from_millis(1);
        let mut timer = PeriodicTimer::new(&clock, period);

        assert_eq!(poll_timer(&mut timer), Poll::Pending);
        thread::sleep(period);
        assert_eq!(poll_timer(&mut timer), Poll::Ready(Some(())));

        assert_eq!(timer.current_timer, None);
        assert_eq!(clock.clock_state().timers_len(), 0);
    }

    #[test]
    fn drop_periodic_timer_unregisters_timer() {
        let clock = Clock::new_frozen();
        let period = Duration::from_millis(1);

        // Create and poll the periodic timer to register an elementary timer.
        {
            let mut timer = PeriodicTimer::new(&clock, period);
            assert_eq!(poll_timer(&mut timer), Poll::Pending);
            assert_eq!(clock.clock_state().timers_len(), 1);
            // Periodic timer is dropped here
        }

        // Elementary timer should be unregistered after dropping the periodic timer.
        assert_eq!(clock.clock_state().timers_len(), 0);
    }

    fn poll_timer(delay: &mut PeriodicTimer) -> Poll<Option<()>> {
        let waker = Waker::noop().clone();
        let mut cx = Context::from_waker(&waker);
        let delay = std::pin::pin!(delay);

        delay.poll_next(&mut cx)
    }
}
