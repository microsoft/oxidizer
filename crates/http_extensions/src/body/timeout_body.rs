// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytesbuf::BytesView;
use http_body::{Body, Frame, SizeHint};
use pin_project::pin_project;
use tick::{Clock, Delay};

use crate::{HttpError, Result};

/// Applies an idle timeout while polling a streaming body.
#[pin_project]
pub(super) struct TimeoutBody<B> {
    #[pin]
    inner: Option<B>,
    timeout: Duration,
    clock: Clock,
    current_delay: Option<Delay>,
}

impl<B> TimeoutBody<B> {
    pub(super) fn new(inner: B, timeout: Duration, clock: &Clock) -> Self {
        Self {
            inner: Some(inner),
            timeout,
            clock: clock.clone(),
            current_delay: None,
        }
    }
}

impl<B> Body for TimeoutBody<B>
where
    B: Body<Data = BytesView>,
    B::Error: Into<HttpError>,
{
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
        let mut this = self.project();

        let Some(inner) = this.inner.as_mut().as_pin_mut() else {
            return Poll::Ready(None);
        };

        if let Poll::Ready(result) = inner.poll_frame(cx) {
            *this.current_delay = None;
            return Poll::Ready(result.map(|result| result.map_err(Into::into)));
        }

        let delay = this.current_delay.get_or_insert_with(|| Delay::new(this.clock, *this.timeout));
        if Pin::new(delay).poll(cx).is_ready() {
            *this.current_delay = None;
            this.inner.set(None);
            return Poll::Ready(Some(Err(HttpError::timeout_for_body(*this.timeout))));
        }

        Poll::Pending
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.as_ref().map_or_else(|| SizeHint::with_exact(0), Body::size_hint)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.as_ref().is_none_or(Body::is_end_stream)
    }
}
