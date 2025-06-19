// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::io::ErrorKind;
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project::pin_project;

use crate::Error;

/// A future that races between a future and a deadline.
///
/// - If the future completes before the deadline, the future's output is returned.
/// - If the deadline completes before the future, an error is returned.
#[pin_project]
#[derive(Debug)]
pub struct Timeout<F, D> {
    #[pin]
    future: F,
    #[pin]
    deadline: D,
}

impl<F, D> Timeout<F, D> {
    pub(super) const fn new(future: F, deadline: D) -> Self {
        Self { future, deadline }
    }
}

impl<F: Future, D: Future> Future for Timeout<F, D> {
    type Output = Result<F::Output, Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        match this.future.poll(cx) {
            Poll::Ready(v) => Poll::Ready(Ok(v)),
            Poll::Pending => match this.deadline.poll(cx) {
                Poll::Ready(_) => {
                    let io_err = std::io::Error::new(ErrorKind::TimedOut, "future timed out");
                    Poll::Ready(Err(Error::from_other(io_err)))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}