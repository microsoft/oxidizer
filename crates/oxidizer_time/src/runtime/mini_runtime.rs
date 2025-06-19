// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use std::time::Instant;

use futures::future::{Either, select};

use crate::runtime::{ClockDriver, InactiveClock};

/// A helper used in tests that can execute async code that uses clock-related operations.
#[derive(Debug)]
pub struct MiniRuntime;

impl MiniRuntime {
    pub fn execute<F, FF, R>(execute: F) -> R
    where
        F: FnOnce(crate::Clock) -> FF,
        FF: Future<Output = R>,
    {
        let (clock, driver) = InactiveClock::default().activate();
        let mini_runtime = AdvanceTimersFuture::new(driver);
        let future = pin!(execute(clock));
        let poll_timers = pin!(mini_runtime);

        match futures::executor::block_on(select(future, poll_timers)) {
            Either::Left((result, _)) => result,
            Either::Right(_) => unreachable!(),
        }
    }
}

struct AdvanceTimersFuture {
    driver: ClockDriver,
    started: Instant,
}

impl AdvanceTimersFuture {
    fn new(driver: ClockDriver) -> Self {
        Self {
            driver,
            started: Instant::now(),
        }
    }
}

impl Future for AdvanceTimersFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.started.elapsed() > std::time::Duration::from_secs(30) {
            assert!(
                self.started.elapsed() <= std::time::Duration::from_secs(30),
                "the execution took more than 30s"
            );
        }

        _ = self.driver.advance_timers(Instant::now());
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{Delay, Stopwatch};

    #[test]
    fn test_clock_executor() {
        let duration = Duration::from_millis(10);

        MiniRuntime::execute(async move |clock| {
            let watch = Stopwatch::with_clock(&clock);
            Delay::with_clock(&clock, duration).await;
            assert!(watch.elapsed() >= duration);
        });
    }
}