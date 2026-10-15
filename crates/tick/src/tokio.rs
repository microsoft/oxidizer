// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::Instant;

use ::tokio::sync::Notify;

use crate::Clock;

#[derive(Debug)]
pub(crate) struct DriverLifecycle {
    notify: Arc<Notify>,
}

impl Drop for DriverLifecycle {
    fn drop(&mut self) {
        self.notify.notify_one();
    }
}

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
        let (mut clock, mut driver) = crate::runtime::InactiveClock::new_shared().activate(driver_waker);
        clock.set_tokio_driver(Arc::new(DriverLifecycle {
            notify: Arc::clone(&notify),
        }));

        let join_handle = ::tokio::spawn(async move {
            while let Ok(next_timer) = driver.advance_timers(Instant::now()) {
                match next_timer {
                    Some(deadline) => {
                        _ = ::tokio::time::timeout_at(::tokio::time::Instant::from_std(deadline), notify.notified()).await;
                    }
                    None => notify.notified().await,
                }
            }
        });

        (clock, join_handle)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use super::*;

    #[cfg_attr(miri, ignore)] // The logic we call talks to the real OS, which Miri cannot do.
    #[::tokio::test]
    async fn hidden_driver_task_finishes_when_clock_is_dropped() {
        // Unit access is required because the public API intentionally hides the driver task's
        // join handle; public timing and relocation behavior is covered by integration tests.
        let (clock, handle) = Clock::new_tokio_core();

        clock.delay(Duration::from_millis(15)).await;
        drop(clock);

        handle.await.unwrap();
    }
}
