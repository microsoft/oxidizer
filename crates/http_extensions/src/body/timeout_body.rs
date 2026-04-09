// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytesbuf::BytesView;
use http_body::{Body, Frame, SizeHint};
use pin_project::pin_project;
use tick::{Clock, Delay};

use crate::{HttpError, Result};

/// Wraps a streaming body to enforce a total timeout on data reception.
///
/// The deadline is an absolute `Instant` computed once at construction time.
/// On each [`poll_frame`][Body::poll_frame] call a fresh [`Delay`] is created
/// for the remaining time until the deadline, so the timeout shrinks as time
/// passes rather than resetting on every poll.
#[pin_project]
pub(crate) struct TimeoutBody<B> {
    #[pin]
    inner: B,
    deadline_at: Instant,
    timeout_duration: Duration,
    clock: Clock,
    /// Per-poll delay; rebuilt on each `poll_frame` with the remaining time.
    current_delay: Option<Delay>,
}

impl<B> TimeoutBody<B> {
    pub(crate) fn new(inner: B, timeout: Duration, clock: &Clock) -> Self {
        let deadline_at = clock
            .instant()
            .checked_add(timeout)
            .expect("timeout duration overflows the monotonic clock");

        Self {
            inner,
            deadline_at,
            timeout_duration: timeout,
            clock: clock.clone(),
            current_delay: None,
        }
    }
}

impl<B> Body for TimeoutBody<B>
where
    B: Body<Data = BytesView, Error = HttpError>,
{
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
        let this = self.project();

        // If the inner body has data ready, return it regardless of the deadline.
        // Drop any in-flight delay so the next poll recomputes the remaining time.
        if let Poll::Ready(result) = this.inner.poll_frame(cx) {
            *this.current_delay = None;
            return Poll::Ready(result);
        }

        // Inner body is pending — check the remaining time until the deadline.
        let now = this.clock.instant();
        let remaining = this.deadline_at.saturating_duration_since(now);

        if remaining.is_zero() {
            *this.current_delay = None;
            return Poll::Ready(Some(Err(HttpError::timeout_for_body(*this.timeout_duration))));
        }

        // Create a fresh delay for the remaining time (or reuse the existing one
        // if we are re-polled without the inner body making progress).
        let delay = this.current_delay.get_or_insert_with(|| Delay::new(this.clock, remaining));

        if Pin::new(delay).poll(cx).is_ready() {
            *this.current_delay = None;
            return Poll::Ready(Some(Err(HttpError::timeout_for_body(*this.timeout_duration))));
        }

        Poll::Pending
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytesbuf::BytesView;
    use futures::executor::block_on;
    use http_body::{Body, Frame};
    use tick::ClockControl;

    use crate::testing::create_stream_body;
    use crate::{HttpBodyBuilder, HttpError, Result};

    #[test]
    fn stream_body_returns_data_before_deadline() {
        let builder = HttpBodyBuilder::new_fake();

        // Stream yields data immediately — well within the timeout.
        let body = create_stream_body(&builder, b"streamed data");
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"streamed data");
    }

    #[test]
    fn stream_body_times_out_when_pending() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let builder = HttpBodyBuilder::new_fake();

        // A body that never yields data.
        let body = builder.custom_body_with_timeout(PendingBody, Duration::from_millis(100), &clock);
        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received within the timeout"),
            "expected body timeout error, got: {err}"
        );
    }

    #[test]
    fn custom_body_with_timeout_chains_with_response_buffer_limit() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let builder = HttpBodyBuilder::new_fake().with_response_buffer_limit(Some(1024));

        assert_eq!(builder.response_buffer_limit, Some(1024));

        // Timeout is applied per-body, not on the builder.
        let body = builder.custom_body_with_timeout(PendingBody, Duration::from_secs(30), &clock);
        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(err.to_string().contains("body data was not fully received within the timeout"));
    }

    #[test]
    fn size_hint_delegates_to_inner() {
        let builder = HttpBodyBuilder::new_fake();

        // Stream body has unknown size hint.
        let body = create_stream_body(&builder, b"hello");
        let hint = body.size_hint();
        assert_eq!(hint.lower(), 0);
    }

    #[test]
    fn is_end_stream_delegates_to_inner() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new_fake();

        let body = builder.custom_body_with_timeout(http_body_util::Empty::new(), Duration::from_secs(1), &clock);
        assert!(body.is_end_stream());
    }

    /// Body that always returns [`Poll::Pending`] to simulate a stalled download.
    struct PendingBody;

    impl Body for PendingBody {
        type Data = BytesView;
        type Error = HttpError;

        fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
            Poll::Pending
        }
    }
}
