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
/// On every [`poll_frame`][Body::poll_frame] call the deadline is checked
/// **before** the inner body is polled; if the deadline has already elapsed a
/// timeout error is returned immediately, even if the inner body would have
/// data ready. When the inner body is not ready, a [`Delay`] is created for
/// the remaining time until the deadline and cached for subsequent polls. The
/// cached delay is cleared whenever the inner body yields a frame, so the next
/// pending poll will create a new delay with the updated remaining time. This
/// means the timeout shrinks as time passes rather than resetting on every
/// poll.
#[pin_project]
pub(crate) struct TimeoutBody<B> {
    #[pin]
    inner: B,
    deadline_at: Instant,
    timeout_duration: Duration,
    clock: Clock,
    /// Cached delay for the remaining time; created on the first pending poll
    /// and reused until the inner body makes progress or the delay fires.
    current_delay: Option<Delay>,
}

impl<B> TimeoutBody<B> {
    pub(crate) fn new(inner: B, deadline_at: Instant, timeout_duration: Duration, clock: &Clock) -> Self {
        Self {
            inner,
            deadline_at,
            timeout_duration,
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

        // Check the deadline *before* accepting any data from the inner body so
        // that frames arriving after the deadline consistently yield a timeout
        // error rather than silently slipping through.
        let now = this.clock.instant();
        let remaining = this.deadline_at.saturating_duration_since(now);

        if remaining.is_zero() {
            *this.current_delay = None;
            return Poll::Ready(Some(Err(HttpError::timeout_for_body(*this.timeout_duration))));
        }

        // Deadline has not elapsed yet — poll the inner body for data.
        // Drop any in-flight delay so the next poll recomputes the remaining time.
        if let Poll::Ready(result) = this.inner.poll_frame(cx) {
            *this.current_delay = None;
            return Poll::Ready(result);
        }

        // Inner body is pending — enforce the deadline via a delay.
        // Reuse the existing delay if we are re-polled without the inner body
        // making progress, or create a new one for the remaining time.
        // `Delay` implements `Unpin` (a deliberate guarantee from the `tick` crate),
        // so we can poll it through `Pin::new` without needing a pinned projection.
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
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::task::{Context, Poll};
    use std::time::Duration;

    use bytesbuf::BytesView;
    use bytesbuf::mem::GlobalPool;
    use futures::executor::block_on;
    use http_body::{Body, Frame};
    use tick::ClockControl;

    use crate::testing::create_stream_body;
    use crate::{BodyOptions, HttpBodyBuilder, HttpError, Result};

    #[test]
    fn stream_body_returns_data_before_deadline() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        // Stream yields data immediately — well within the generous timeout,
        // exercising the TimeoutBody happy path via stream with timeout options.
        let chunks: Vec<Result<BytesView>> = vec![Ok(BytesView::copied_from_slice(b"streamed data", &builder))];
        let options = BodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.stream(futures::stream::iter(chunks), &options);
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"streamed data");
    }

    #[test]
    fn stream_body_times_out_when_pending() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        // A body that never yields data.
        let options = BodyOptions::default().timeout(Duration::from_millis(100));
        let body = builder.body(PendingBody, &options);
        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received"),
            "expected body timeout error, got: {err}"
        );
    }

    #[test]
    fn body_timeout_chains_with_buffer_limit() {
        let clock = ClockControl::new().auto_advance_timers(true).to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock).with_options(BodyOptions::default().buffer_limit(1024));

        assert_eq!(builder.options, BodyOptions::default().buffer_limit(1024));

        // Timeout is applied per-body, not on the builder.
        let options = BodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.body(PendingBody, &options);
        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(err.to_string().contains("body data was not fully received"));
    }

    #[test]
    fn size_hint_delegates_to_inner() {
        let builder = HttpBodyBuilder::new_fake();

        // Stream body has unknown size hint.
        let body = create_stream_body(&builder, b"hello", &BodyOptions::default());
        let hint = body.size_hint();
        assert_eq!(hint.lower(), 0);
    }

    #[test]
    fn size_hint_delegates_through_timeout_body() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        // Full body has an exact size hint; verify it passes through TimeoutBody.
        let options = BodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.body(
            http_body_util::Full::new(BytesView::copied_from_slice(b"hello", &builder)),
            &options,
        );
        let hint = body.size_hint();
        assert_eq!(hint.lower(), 5);
        assert_eq!(hint.upper(), Some(5));
    }

    #[test]
    fn is_end_stream_true_when_inner_is_empty() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        let options = BodyOptions::default().timeout(Duration::from_secs(1));
        let body = builder.body(http_body_util::Empty::new(), &options);
        assert!(body.is_end_stream());
    }

    #[test]
    fn is_end_stream_false_when_inner_has_data() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        let options = BodyOptions::default().timeout(Duration::from_secs(1));
        let body = builder.body(http_body_util::Full::new(BytesView::copied_from_slice(b"data", &builder)), &options);
        assert!(!body.is_end_stream());
    }

    #[test]
    fn poll_frame_returns_data_through_timeout_body() {
        let clock = ClockControl::new().to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        let options = BodyOptions::default().timeout(Duration::from_secs(30));
        let body = builder.body(
            http_body_util::Full::new(BytesView::copied_from_slice(b"payload", &builder)),
            &options,
        );
        let bytes = block_on(body.into_bytes()).unwrap();
        assert_eq!(bytes, b"payload");
    }

    #[test]
    fn poll_frame_times_out_immediately_when_deadline_already_passed() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        // Construct with a short timeout so the deadline is near.
        let options = BodyOptions::default().timeout(Duration::from_millis(1));
        let body = builder.body(PendingBody, &options);

        // Advance the clock well past the deadline before polling.
        control.advance(Duration::from_secs(60));

        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received"),
            "expected body timeout error, got: {err}"
        );
    }

    #[test]
    fn poll_frame_times_out_even_when_inner_body_has_data_ready() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);

        // Use a body that has data immediately available (Full is always ready).
        let options = BodyOptions::default().timeout(Duration::from_millis(1));
        let body = builder.body(
            http_body_util::Full::new(BytesView::copied_from_slice(b"ready data", &builder)),
            &options,
        );

        // Advance the clock well past the deadline before polling.
        control.advance(Duration::from_secs(60));

        // Even though the inner body would return data, the elapsed deadline
        // must take precedence and produce a timeout error.
        let err = block_on(body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received"),
            "expected body timeout error, got: {err}"
        );
    }

    #[test]
    fn poll_frame_times_out_via_delay_when_inner_body_advances_clock() {
        let control = ClockControl::new();
        let clock = control.to_clock();
        let timeout = Duration::from_millis(100);
        let deadline = clock.instant().checked_add(timeout).expect("timeout does not overflow");

        // Body that returns Pending on the first poll (so the delay is created
        // and registered), then advances the clock past the deadline on the
        // second poll before returning Pending again. This makes the cached
        // delay fire on re-poll while `remaining` at the top of poll_frame is
        // still non-zero, exercising the delay-fires path.
        let body = ClockAdvancingBody {
            control,
            advance_by: Duration::from_secs(60),
            poll_count: AtomicU32::new(0),
        };

        let timeout_body = super::TimeoutBody::new(body, deadline, timeout, &clock);
        let http_body = HttpBodyBuilder::new_fake().body(timeout_body, &BodyOptions::default());

        let err = block_on(http_body.into_bytes()).unwrap_err();
        assert!(
            err.to_string().contains("body data was not fully received"),
            "expected body timeout error, got: {err}"
        );
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

    /// Body that returns [`Poll::Pending`] but advances the clock on the second poll,
    /// allowing the cached delay to fire before the `remaining.is_zero()` check catches it.
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
                // the deadline so the already-registered delay expires.
                self.control.advance(self.advance_by);
            }
            // Wake ourselves so the executor re-polls after the first Pending.
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}
