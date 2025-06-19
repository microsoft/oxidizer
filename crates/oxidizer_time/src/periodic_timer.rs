// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use futures::Stream;

use super::timers::TimerKey;
use super::{Clock, TIMER_RESOLUTION};

/// A timer that periodically ticks.
///
/// Periodic timer can be created by using the [`PeriodicTimer::with_clock`] constructor
/// that requires a reference to [`Clock`][`Clock`].
///
/// # Precision
///
/// The timer uses the current thread scheduler to schedule its ticks. The precision
/// of the timer is affected by the load on this thread. There are no guarantees about the
/// precision of the timer other than that it will eventually tick. When the thread is healthy,
/// the period of the timer should be close to the specified one.
///
/// # Examples
///
/// ### Create periodic timer
///
/// ```
/// use oxidizer_time::{Clock, Stopwatch, PeriodicTimer};
/// use std::time::Duration;
/// use futures::StreamExt;
///
/// async fn periodic_timer_example(clock: &Clock) {
///     let timer = PeriodicTimer::with_clock(clock, Duration::from_millis(1));
///
///     timer
///         .take(3)
///         .for_each(async |()| {
///             // Do something every 1ms
///         })
///         .await;
/// }
///
/// # fn main() {
/// #     let control = oxidizer_time::ClockControl::new().auto_advance_timers(true);
/// #     let clock = Clock::with_control(&control);
/// #     futures::executor::block_on(periodic_timer_example(&clock));
/// # }
/// ```
///
/// ### Create periodic timer with initial delay
///
/// ```
/// use oxidizer_time::{Clock, Stopwatch, PeriodicTimer, Delay};
/// use std::time::Duration;
/// use futures::StreamExt;
///
/// async fn periodic_timer_example(clock: &Clock) {
///     // Delay for 10ms before timer starts ticking
///     Delay::with_clock(clock, Duration::from_millis(10)).await;
///
///     let timer = PeriodicTimer::with_clock(clock, Duration::from_millis(1));
///
///     timer
///         .take(3)
///         .for_each(async |()|  {
///             // Do something every 1ms
///         })
///         .await;
/// }
///
/// # fn main() {
/// #     let control = oxidizer_time::ClockControl::new().auto_advance_timers(true);
/// #     let clock = Clock::with_control(&control);
/// #     futures::executor::block_on(periodic_timer_example(&clock));
/// # }
/// ```
#[derive(Debug)]
pub struct PeriodicTimer {
    period: Duration,
    clock: Clock,
    // Currently scheduled timer. This value is not initialized before
    // actually calling the "Stream::poll_next" method.
    current_timer: Option<TimerKey>,
}

impl PeriodicTimer {
    /// Creates a timer that fires periodically.
    ///
    /// This constructor automatically adjusts the provided period to the minimum allowed value.
    /// Currently, this minimum period is 1ms but this may change in the future. Be aware of this
    /// fact when creating timers with very short periods as these won't have the desired precision.
    #[must_use]
    pub fn with_clock(clock: &Clock, period: Duration) -> Self {
        let mut period = period;

        if period < TIMER_RESOLUTION {
            period = TIMER_RESOLUTION;
        }

        Self {
            // The timer is not registered yet, it will be done on the first
            // call to the Stream::poll_next.
            current_timer: None,
            period,
            clock: clock.clone(),
        }
    }

    fn register_timer(&mut self, waker: Waker) {
        match self.clock.instant_now().checked_add(self.period) {
            Some(when) => {
                self.current_timer = Some(self.clock.register_timer(when, waker));
            }
            None => {
                // The timer would tick so far in the future that we can assume
                // it never fires. For this reason, there is no point of even registering id.
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
            Some(key) if key.tick() <= this.clock.instant_now() => {
                // Reset the timer. It will be registered again in the next poll.
                this.current_timer = None;

                // Unregister timer, just in case this call was explicit and not due to
                // timers advancing.
                this.clock.unregister_timer(key);

                Poll::Ready(Some(()))
            }
            // Timer is registered and will fire later in the future.
            Some(_) => Poll::Pending,

            // Timer is not registered yet, let's register it.
            // The registration is lazy, when someone polls the future. This means
            // that the work between two ticks is not taken into account when scheduling
            // the next tick. When thread is busy, the timer may tick later than expected.
            None => {
                this.register_timer(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use futures::StreamExt;
    use futures::task::noop_waker;

    use super::*;
    use crate::ClockControl;
    use crate::runtime::MiniRuntime;

    #[test]
    fn assert_types() {
        static_assertions::assert_impl_all!(PeriodicTimer: Send, Sync);
    }

    #[test]
    fn next_ensure_awaited() {
        MiniRuntime::execute(async move |clock| {
            let mut timer = PeriodicTimer::with_clock(&clock, Duration::from_millis(1));
            assert_eq!(timer.next().await, Some(()));
            assert_eq!(timer.next().await, Some(()));
        });
    }

    #[test]
    fn next_with_control() {
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);
        let mut timer = PeriodicTimer::with_clock(&clock, Duration::from_millis(1));

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
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);

        let mut timer = PeriodicTimer::with_clock(&clock, Duration::from_millis(1));

        assert_eq!(poll_timer(&mut timer), Poll::Pending);
    }

    #[test]
    fn new_zero_duration_period_adjusted() {
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);

        let timer = PeriodicTimer::with_clock(&clock, Duration::ZERO);

        assert_eq!(timer.period, Duration::from_millis(1));
    }

    #[test]
    fn new_duration_near_max_never_fires() {
        let control = ClockControl::new();
        let clock = Clock::with_control(&control);

        let mut timer = PeriodicTimer::with_clock(
            &clock,
            Duration::MAX.saturating_sub(Duration::from_millis(1)),
        );

        assert_eq!(poll_timer(&mut timer), Poll::Pending);
        assert_eq!(poll_timer(&mut timer), Poll::Pending);

        assert_eq!(timer.period, Duration::MAX);
        assert_eq!(timer.current_timer, None);
    }

    #[test]
    fn ready_without_advancing_timers_ensure_timer_unregistered() {
        MiniRuntime::execute(async move |clock| {
            let period = Duration::from_millis(1);
            let mut timer = PeriodicTimer::with_clock(&clock, period);

            assert_eq!(poll_timer(&mut timer), Poll::Pending);
            thread::sleep(period);
            assert_eq!(poll_timer(&mut timer), Poll::Ready(Some(())));

            assert_eq!(timer.current_timer, None);
            assert_eq!(clock.clock_state().timers_len(), 0);
        });
    }

    fn poll_timer(delay: &mut PeriodicTimer) -> Poll<Option<()>> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let delay = std::pin::pin!(delay);

        delay.poll_next(&mut cx)
    }
}