// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Adapter exposing a [`tick::Clock`] as a [`hyper::rt::Timer`].

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hyper::rt::Sleep;
use tick::{Clock, Delay};

/// A [`hyper::rt::Timer`] backed by a [`tick::Clock`].
#[derive(Debug, Clone)]
pub(crate) struct ClockTimer(Clock);

impl ClockTimer {
    pub(crate) const fn new(clock: Clock) -> Self {
        Self(clock)
    }
}

impl hyper::rt::Timer for ClockTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Sleep>> {
        Box::pin(DelayWrapper(self.0.delay(duration)))
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Sleep>> {
        let now = self.0.instant();
        let duration = deadline.saturating_duration_since(now);
        self.sleep(duration)
    }
}

#[pin_project::pin_project]
struct DelayWrapper(#[pin] Delay);

impl Sleep for DelayWrapper {}

impl Future for DelayWrapper {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().0.poll(cx)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use hyper::rt::Timer;
    use tick::ClockControl;

    use super::*;

    #[tokio::test]
    async fn sleep_advances_by_duration() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let timer = ClockTimer::new(clock.clone());

        let watch = clock.stopwatch();
        timer.sleep(Duration::from_secs(1)).await;
        assert_eq!(watch.elapsed(), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn sleep_until_advances_to_deadline() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let timer = ClockTimer::new(clock.clone());
        let now = clock.instant();

        let watch = clock.stopwatch();
        timer.sleep_until(now.checked_add(Duration::from_secs(2)).unwrap()).await;
        assert_eq!(watch.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn sleep_until_past_deadline_returns_immediately() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let timer = ClockTimer::new(clock.clone());

        let watch = clock.stopwatch();
        timer.sleep_until(clock.instant()).await;
        assert_eq!(watch.elapsed(), Duration::ZERO);
    }
}
