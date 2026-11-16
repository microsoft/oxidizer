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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytesbuf::BytesView;
    use bytesbuf::mem::{GlobalPool, OpaqueMemory};
    use futures::executor::block_on;
    use http_body::{Body, Frame};
    use tick::ClockControl;

    use crate::testing::create_stream_body;
    use crate::{HttpBodyBuilder, HttpBodyOptions, HttpError, Result};

    #[test]
    fn gamma_survivor_timeout_clears_cached_delay_and_inner_body() {
        let control = ClockControl::new();
        let mut body = super::TimeoutBody::new(PendingBody, Duration::from_millis(100), &control.to_clock());
        let mut context = Context::from_waker(futures::task::noop_waker_ref());
        assert!(Pin::new(&mut body).poll_frame(&mut context).is_pending());
        assert!(body.current_delay.is_some());
        control.advance(Duration::from_millis(100));
        assert!(matches!(Pin::new(&mut body).poll_frame(&mut context), Poll::Ready(Some(Err(_)))));
        assert!(body.current_delay.is_none());
        assert!(body.inner.is_none());
        assert!(body.is_end_stream());
        assert!(matches!(Pin::new(&mut body).poll_frame(&mut context), Poll::Ready(None)));
    }

    #[test]
    fn stream_body_returns_data_before_timeout() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        // The stream yields data before the long timeout.
        // exercising the TimeoutBody happy path via stream with timeout options.
        let chunks: Vec<Result<BytesView>> = vec![Ok(BytesView::copied_from_slice(b"streamed data", &builder))];
        let options = HttpBodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.with_options(options).stream(futures::stream::iter(chunks));
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"streamed data");
    }

    #[test]
    fn stream_body_times_out_when_pending() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        // A body that never yields data.
        let options = HttpBodyOptions::default().timeout(Duration::from_millis(100));
        let body = builder.with_options(options).body(PendingBody);
        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received"),
            "expected body timeout error, got: {err}"
        );
    }

    #[test]
    fn body_timeout_chains_with_buffer_limit() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let builder =
            HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock).with_options(HttpBodyOptions::default().buffer_limit(1024));

        assert_eq!(builder.options, HttpBodyOptions::default().buffer_limit(1024));

        let options = HttpBodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.with_options(options).body(PendingBody);
        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(err.to_string().contains("body data was not fully received"));
    }

    #[test]
    fn size_hint_delegates_to_inner() {
        let builder = HttpBodyBuilder::new_fake();

        // Stream body has unknown size hint.
        let body = create_stream_body(&builder, b"hello", &HttpBodyOptions::default());
        let hint = body.size_hint();
        assert_eq!(hint.lower(), 0);
    }

    #[test]
    fn size_hint_delegates_through_timeout_body() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        // A full body has an exact size hint. TimeoutBody keeps this hint.
        let options = HttpBodyOptions::default().timeout(Duration::from_secs(30));
        let builder = builder.with_options(options);
        let body = builder.body(http_body_util::Full::new(BytesView::copied_from_slice(b"hello", &builder)));
        let hint = body.size_hint();
        assert_eq!(hint.lower(), 5);
        assert_eq!(hint.upper(), Some(5));
    }

    #[test]
    fn is_end_stream_true_when_inner_is_empty() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        let options = HttpBodyOptions::default().timeout(Duration::from_secs(1));
        let body = builder.with_options(options).body(http_body_util::Empty::new());
        assert!(body.is_end_stream());
    }

    #[test]
    fn is_end_stream_false_when_inner_has_data() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        let options = HttpBodyOptions::default().timeout(Duration::from_secs(1));
        let builder = builder.with_options(options);
        let body = builder.body(http_body_util::Full::new(BytesView::copied_from_slice(b"data", &builder)));
        assert!(!body.is_end_stream());
    }

    #[test]
    fn poll_frame_returns_data_through_timeout_body() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        let options = HttpBodyOptions::default().timeout(Duration::from_secs(30));
        let builder = builder.with_options(options);
        let body = builder.body(http_body_util::Full::new(BytesView::copied_from_slice(b"payload", &builder)));
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"payload");
    }

    #[test]
    fn poll_frame_times_out_when_pending_with_short_timeout() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        // A body that never yields data with a very short timeout.
        let options = HttpBodyOptions::default().timeout(Duration::from_millis(1));
        let body = builder.with_options(options).body(PendingBody);

        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received"),
            "expected body timeout error, got: {err}"
        );
    }

    #[test]
    fn poll_frame_returns_data_even_when_clock_advanced_past_timeout() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let builder = HttpBodyBuilder::new(OpaqueMemory::new(GlobalPool::new()), &clock);

        // Use a body that has data immediately available (Full is always ready).
        let options = HttpBodyOptions::default().timeout(Duration::from_millis(1));
        let builder = builder.with_options(options);
        let body = builder.body(http_body_util::Full::new(BytesView::copied_from_slice(b"ready data", &builder)));

        // Advance the clock past the timeout before polling.
        control.advance(Duration::from_mins(1));

        // Return data from the inner body regardless of elapsed time.
        // The idle timeout fires only when the inner body is pending.
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"ready data");
    }

    #[test]
    fn poll_frame_times_out_via_delay_when_inner_body_advances_clock() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let timeout = Duration::from_millis(100);

        // The first poll returns Pending and registers the delay.
        // The second poll advances the clock past the timeout and returns Pending.
        // The next poll fires the cached delay.
        let body = ClockAdvancingBody {
            control,
            advance_by: Duration::from_mins(1),
            poll_count: AtomicU32::new(0),
        };

        let timeout_body = super::TimeoutBody::new(body, timeout, &clock);
        let http_body = HttpBodyBuilder::new_fake().body(timeout_body);

        let err = block_on(http_body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received"),
            "expected body timeout error, got: {err}"
        );
    }

    #[test]
    fn poll_frame_returns_error_after_timeout() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let timeout = Duration::from_millis(50);

        let mut timeout_body = super::TimeoutBody::new(PendingBody, timeout, &clock);
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        // First poll: inner returns Pending, delay is created and auto-advanced.
        assert!(Pin::new(&mut timeout_body).poll_frame(&mut cx).is_pending());

        // The second poll fires the delay, returns an error, and drops the inner body.
        let result = Pin::new(&mut timeout_body).poll_frame(&mut cx);
        assert!(matches!(result, Poll::Ready(Some(Err(_)))));

        // Third poll: inner is gone, returns None (end-of-stream) consistent
        // with is_end_stream returning true when inner is None.
        let result = Pin::new(&mut timeout_body).poll_frame(&mut cx);
        assert!(matches!(result, Poll::Ready(None)));
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

    /// A body that advances the clock on its second pending poll.
    ///
    /// The cached delay fires before the next poll completes.
    struct ClockAdvancingBody {
        control: ClockControl,
        advance_by: Duration,
        poll_count: AtomicU32,
    }

    impl Body for ClockAdvancingBody {
        type Data = BytesView;
        type Error = HttpError;

        fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
            let count = self.poll_count.fetch_add(1, Ordering::Relaxed);
            if count >= 1 {
                // On the second (and subsequent) polls, advance the clock past
                // the timeout so the already-registered delay expires.
                self.control.advance(self.advance_by);
            }
            // Wake ourselves so the executor re-polls after the first Pending.
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
