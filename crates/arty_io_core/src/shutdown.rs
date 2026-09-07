// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::Driver;

/// A future that drives one driver's graceful shutdown.
#[must_use = "futures do nothing unless polled"]
pub struct Shutdown<'a, D: Driver + ?Sized> {
    driver: &'a mut D,
    started: bool,
}

impl<'a, D: Driver + ?Sized> Shutdown<'a, D> {
    pub(crate) fn new(driver: &'a mut D) -> Self {
        Self {
            driver,
            started: false,
        }
    }
}

impl<D: Driver + ?Sized> fmt::Debug for Shutdown<'_, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Shutdown")
            .field("started", &self.started)
            .finish_non_exhaustive()
    }
}

impl<D: Driver + ?Sized> Future for Shutdown<'_, D> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        if !this.started {
            this.driver.begin_shutdown();
            this.started = true;
        }

        this.driver.poll_shutdown(cx)
    }
}
