// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::future::poll_fn;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use super::Clock;
use super::timers::TimerKey;
use crate::timers::TIMER_RESOLUTION;

/// A timer that periodically ticks.
///
/// A periodic timer can be created using the [`Timer::new()`] constructor,
/// which requires a reference to a [`Clock`].
///
/// # Precision
///
/// The timer uses the scheduler of the current thread. Thread load affects its
/// precision. The timer will tick, but it does not guarantee an exact tick time.
/// Under normal load, the interval stays near the requested period.
///
/// Changes to the system clock do not affect the timer.
///
/// # Timer behavior
///
/// Call [`next`][Self::next] to wait for a tick. The timer does not stop by
/// itself. The caller selects how many ticks to await.
///
/// # Examples
///
/// ## Create a periodic timer
///
/// ```
/// use std::time::Duration;
///
/// use tick::{Clock, Stopwatch, Timer};
///
/// # async fn periodic_timer_example(clock: &Clock) {
/// let mut timer = Timer::new(clock, Duration::from_millis(1));
///
/// for _ in 0..3 {
///     let tick = timer.next().await;
///     assert!(tick.observed_at() <= clock.instant());
/// }
/// # }
/// ```
///
/// ## Create a periodic timer with initial delay
///
/// ```
/// use std::time::Duration;
///
/// use tick::{Clock, Timer};
///
/// # async fn periodic_timer_example(clock: &Clock) {
/// // Delay for 10ms before the timer starts ticking
/// clock.delay(Duration::from_millis(10)).await;
///
/// let mut timer = Timer::new(clock, Duration::from_millis(1));
///
/// for _ in 0..3 {
///     timer.next().await;
/// }
/// # }
/// ```
///
/// # Stream interoperability
///
/// `tick` does not expose a `futures` trait in its public API. An application that uses
/// [`futures_core::Stream`](https://docs.rs/futures-core/latest/futures_core/stream/trait.Stream.html)
/// can adapt its timer:
///
/// ```
/// use std::time::Duration;
///
/// use futures::{Stream, stream};
/// use tick::{Clock, Timer, TimerTick};
///
/// fn into_stream(timer: Timer) -> impl Stream<Item = TimerTick> {
///     stream::unfold(timer, |mut timer| async move {
///         let tick = timer.next().await;
///         Some((tick, timer))
///     })
/// }
///
/// # fn example(clock: &Clock) {
/// let timer = Timer::new(clock, Duration::from_millis(1));
/// let ticks = into_stream(timer);
/// # let _ = ticks;
/// # }
/// ```
///
/// This adapter keeps the timer in the stream state. It does not add
/// `futures-core` as a dependency of `tick`.
#[derive(Debug)]
#[must_use = "periodic timers do nothing unless `next` is awaited"]
pub struct Timer {
    period: Duration,
    clock: Clock,
    // Currently scheduled timer. This value is not initialized until the first `next` call.
    current: Option<TimerKey>,
    current_waker: Option<Waker>,
}

/// Information recorded when a [`Timer`] fires.
#[derive(Debug)]
#[must_use]
pub struct TimerTick {
    observed_at: Instant,
}

impl TimerTick {
    /// Returns the clock instant observed when the timer tick completed.
    #[must_use]
    pub const fn observed_at(&self) -> Instant {
        self.observed_at
    }
}

impl Timer {
    /// Creates a timer that fires periodically.
    ///
    /// The minimum period is 1 millisecond. A shorter period is increased to
    /// 1 millisecond.
    #[must_use = "periodic timers do nothing unless `next` is awaited"]
    pub fn new(clock: &Clock, period: Duration) -> Self {
        let period = period.max(TIMER_RESOLUTION);

        Self {
            // The timer is not registered yet; it will be registered by the first `next` call.
            current: None,
            current_waker: None,
            period,
            clock: clock.clone(),
        }
    }

    /// Waits for the next timer tick.
    ///
    /// The returned future is cancellation-safe. If it is dropped while pending, the timer keeps
    /// its registered deadline and the next call continues waiting for the same tick.
    pub async fn next(&mut self) -> TimerTick {
        poll_fn(|cx| self.poll_next_tick(cx)).await
    }

    fn register_timer(&mut self, waker: &Waker) {
        if let Some(when) = self.clock.timer_instant().checked_add(self.period) {
            self.current = Some(self.clock.register_timer(when, waker.clone()));
            self.current_waker = Some(waker.clone());
        } else {
            // The timer would tick so far in the future that we can assume
            // it never fires. For this reason, there is no point in registering it.
            // The period is set to Duration::MAX to prevent further registrations.
            self.period = Duration::MAX;
            self.current_waker = None;
        }
    }
    #[cfg_attr(test, mutants::skip)] // cannot reliably check that poll_tick has been called
    fn poll_next_tick(&mut self, cx: &Context<'_>) -> Poll<TimerTick> {
        if self.period == Duration::MAX {
            return Poll::Pending;
        }

        if let Some(key) = self.current {
            let observed_at = self.clock.timer_instant();
            if key.tick() <= observed_at {
                // Reset the timer. It will be registered again on the next poll.
                self.current = None;
                self.current_waker = None;

                // Unregister the timer, just in case this call was explicit and not due to
                // timers advancing.
                self.clock.unregister_timer(key);

                return Poll::Ready(TimerTick { observed_at });
            }

            // Timer is registered and will fire later in the future.
            if self.current_waker.as_ref().is_some_and(|waker| waker.will_wake(cx.waker())) {
                Poll::Pending
            } else if self.clock.update_timer_waker(key, cx.waker()) {
                self.current_waker = Some(cx.waker().clone());
                Poll::Pending
            } else {
                self.current = None;
                self.current_waker = None;
                Poll::Ready(TimerTick {
                    observed_at: self.clock.timer_instant(),
                })
            }
        } else {
            // Timer is not registered yet; let's register it.
            // The registration is lazy, occurring when someone polls the future. This means
            // that the work between two ticks is not taken into account when scheduling
            // the next tick. When the thread is busy, the timer may tick later than expected.
            self.register_timer(cx.waker());
            Poll::Pending
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(key) = self.current {
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
    fn missing_timer_is_treated_as_fired_when_the_task_changes() {
        #[derive(Debug)]
        struct CountingWake(std::sync::atomic::AtomicUsize);

        impl std::task::Wake for CountingWake {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let clock = ClockControl::new().to_clock();
        let mut timer = Timer::new(&clock, Duration::from_millis(1));
        let first_waker = Waker::from(Arc::new(CountingWake(std::sync::atomic::AtomicUsize::new(0))));
        let first_context = Context::from_waker(&first_waker);
        assert!(timer.poll_next_tick(&first_context).is_pending());
        clock.unregister_timer(timer.current.unwrap());

        let second_waker = Waker::from(Arc::new(CountingWake(std::sync::atomic::AtomicUsize::new(0))));
        let second_context = Context::from_waker(&second_waker);

        let Poll::Ready(tick) = timer.poll_next_tick(&second_context) else {
            panic!("the removed timer must be treated as fired");
        };
        assert_eq!(tick.observed_at(), clock.instant());
    }
}
