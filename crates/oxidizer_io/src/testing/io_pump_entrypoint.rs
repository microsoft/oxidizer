// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{self, Poll};
use std::time::{Duration, Instant};

use pin_project::pin_project;
use tracing::{Level, event};

use crate::Driver;

/// A future that wraps an entrypoint future and an I/O driver.
///
/// This ensures that the I/O driver is
/// kept alive until the entrypoint future completes and performing the safe I/O driver shutdown
/// process once the entrypoint has completed.
///
/// This is a minimum-effort wrapper that enables I/O to be used while the entrypoint is executing
/// and ensures safe shutdown. It is implemented inefficiently, busy-looping even when there is
/// nothing to do, as it lacks the insight that the runtime has (a proper runtime integration would
/// wait longer for I/O activity when there is nothing else to do).
#[derive(Debug)]
#[pin_project]
pub struct IoPumpEntrypoint<E, R>
where
    E: Future<Output = R>,
{
    driver: Rc<RefCell<Driver>>,

    mode: IoPumpMode,

    #[pin]
    entrypoint: E,

    // When the entrypoint returns Poll::Ready, we will set this to the result to remember
    // that we are not meant to poll the entrypoint anymore and are allowed to complete ourselves.
    result: Option<R>,

    // Depending on the use case, there may be a timeout we want to apply for the entire scenario.
    // This timeout is not applied after we progress into the shutdown phase - they are independent.
    execute_timeout: Option<Duration>,

    execute_started: Instant,

    // We will typically want to terminate a test if it takes too long to shut down,
    // which suggests that something has deadlocked.
    shutdown_timeout: Option<Duration>,

    shutdown_started: Option<Instant>,
}

impl<E, R> IoPumpEntrypoint<E, R>
where
    E: Future<Output = R>,
{
    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    pub fn new(
        driver: Rc<RefCell<Driver>>,
        mode: IoPumpMode,
        execute_timeout: Option<Duration>,
        shutdown_timeout: Option<Duration>,
        entrypoint: E,
    ) -> Self {
        event!(
            Level::TRACE,
            message = "new",
            ?mode,
            ?execute_timeout,
            ?shutdown_timeout
        );

        Self {
            driver,
            mode,
            entrypoint,
            result: None,
            execute_timeout,
            execute_started: Instant::now(),
            shutdown_timeout,
            shutdown_started: None,
        }
    }
}

impl<E, R> Future for IoPumpEntrypoint<E, R>
where
    E: Future<Output = R>,
{
    type Output = R;

    #[cfg_attr(test, mutants::skip)] // This is test code, we do not care about mutating it.
    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<R> {
        let self_projection = self.project();

        {
            let driver = self_projection.driver.borrow();

            if *self_projection.mode == IoPumpMode::Always || self_projection.result.is_some() {
                // We always process I/O completions first. We use a wait time of 0 - yes, this may burn
                // CPU doing nothing but at least it does not slow down the examples. A real runtime would
                // be smarter here and pass nonzero if the runtime has no useful work to do itself.
                driver.process_completions(0);
            }
        }

        if self_projection.result.is_some() {
            {
                let driver = self_projection.driver.borrow();
                if driver.is_inert() {
                    event!(Level::TRACE, message = "ready");
                    return Poll::Ready(
                        self_projection
                            .result
                            .take()
                            .expect("must have stored result if entrypoint has already completed"),
                    );
                }
            }

            if let Some(shutdown_timeout) = *self_projection.shutdown_timeout {
                let shutdown_started = self_projection.shutdown_started.expect(
                    "must have stored shutdown start time if entrypoint has already completed",
                );

                assert!(
                    shutdown_started.elapsed() < shutdown_timeout,
                    "I/O subsystem shutdown took too long, likely deadlocked"
                );
            }
        } else {
            if let Some(execute_timeout) = *self_projection.execute_timeout {
                assert!(
                    self_projection.execute_started.elapsed() < execute_timeout,
                    "I/O subsystem test harness timed out in test execution phase"
                );
            }

            if let Poll::Ready(result) = self_projection.entrypoint.poll(cx) {
                *self_projection.result = Some(result);
                *self_projection.shutdown_started = Some(Instant::now());
            }
        }

        // We immediately wake ourselves up again. This self-wakeup logic here exists to allow the
        // entrypoint wrapper to be hosted in a runtime that also processes other tasks besides the
        // entrypoint, with this self-wake here acting as a basic yield mechanism. A real runtime
        // would have more sophisticated logic here to avoid busy-waiting.
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// Determines when the I/O test harness will process I/O events during the lifecycle of the test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IoPumpMode {
    /// I/O events are automatically processed during execution and shutdown.
    /// This is equivalent to real-world apps and is intended for use in examples.
    Always,

    /// I/O events are only processed automatically during shutdown.
    /// This may be useful in unit tests which want to process I/O events manually during the test.
    ShutdownOnly,
}