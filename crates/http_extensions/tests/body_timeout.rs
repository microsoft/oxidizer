// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Idle timeout behavior for streaming and transformed HTTP bodies.

use std::cell::Cell;
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use futures::channel::mpsc;
use futures::executor::block_on;
use futures::{FutureExt, Stream, stream};
use http_body::{Body, Frame};
use http_body_util::{BodyExt, Full, StreamBody};
use http_extensions::body::{Content, StreamingBody};
use http_extensions::{HttpBody, HttpBodyBuilder, HttpBodyOptions, HttpError};
use ohno::{ErrorExt, Labeled};
use tick::ClockControl;

#[test]
fn body_size_fits_the_storage_and_policy_budget() {
    let budget = size_of::<Option<BytesView>>() + size_of::<HttpBodyOptions>() + size_of::<tick::Clock>();
    assert!(size_of::<http_extensions::HttpBody>() <= budget);
}

#[test]
fn body_debug_hides_payload_and_stream_implementation() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let bytes = BytesView::copied_from_slice(b"sensitive payload", &builder);

    let debug = format!("{:?}", builder.bytes(bytes.clone()));
    assert!(debug.starts_with("HttpBody"));
    assert!(!debug.contains("sensitive payload"));
    assert_eq!(format!("{:?}", Content::Bytes(bytes)), "Bytes");
    assert_eq!(format!("{:?}", StreamingBody::new(PendingBody)), "StreamingBody { .. }");
    assert_eq!(
        format!("{:?}", Content::Streaming(StreamingBody::new(PendingBody))),
        "Streaming(StreamingBody { .. })"
    );
}

#[test]
fn transform_buffered_and_empty_bodies() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());

    for (body, expected) in [
        (builder.empty(), b"" as &[u8]),
        (builder.text("source"), b"source"),
        (builder.slice(b"bytes"), b"bytes"),
    ] {
        let calls = Cell::new(0);
        let transformed = body
            .map(|content| {
                Ok(match content {
                    Content::Bytes(bytes) => {
                        calls.set(calls.get() + 1);
                        assert_eq!(bytes, expected);
                        Content::Bytes(BytesView::from_views([bytes, BytesView::copied_from_slice(b"!", &builder)]))
                    }
                    Content::Streaming(_) => panic!("buffered fixtures must use the bytes path"),
                })
            })
            .unwrap();

        assert_eq!(calls.get(), 1);
        assert!(transformed.try_clone().is_some());
        let mut result = expected.to_vec();
        result.push(b'!');
        assert_eq!(block_on(transformed.into_bytes()).unwrap().to_vec(), result);
    }
}

#[test]
fn transform_rejects_a_consumed_buffered_body() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let mut body = builder.text("source");
    let frame = body.frame().now_or_never().unwrap().unwrap().unwrap();
    assert_eq!(frame.into_data().unwrap(), b"source");
    let called = Cell::new(false);
    assert!(body.is_end_stream());
    assert!(body.frame().now_or_never().unwrap().is_none());
    let error = body
        .map(|content| {
            called.set(true);
            Ok(content)
        })
        .unwrap_err();

    assert!(!called.get());
    assert!(error.to_string().contains("already consumed"));
}

#[test]
fn transform_propagates_callback_errors() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    for body in [
        builder.empty(),
        builder.text("source"),
        builder.body(PendingBody, &HttpBodyOptions::new()),
    ] {
        let expected: HttpError = std::io::Error::other("map failed").into();
        let expected_label = expected.label().to_string();
        let expected_message = expected.message().to_string();
        let error = body.map(|_| Err(expected)).unwrap_err();

        assert_eq!(error.label().to_string(), expected_label);
        assert_eq!(error.message(), expected_message);
    }
}

#[test]
fn transform_accepts_an_empty_streaming_body() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let body = builder.body(http_body_util::Empty::new(), &HttpBodyOptions::new());
    assert!(body.is_end_stream());
    let called = Cell::new(false);
    let transformed = body
        .map(|content| {
            Ok(match content {
                Content::Streaming(inner) => {
                    called.set(true);
                    Content::Streaming(inner)
                }
                Content::Bytes(_) => panic!("a streaming fixture must stay on the streaming path"),
            })
        })
        .unwrap();

    assert!(called.get());
    assert!(block_on(transformed.into_bytes()).unwrap().is_empty());
}

#[test]
fn body_transform_keeps_the_running_timer() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let timeout = Duration::from_millis(100);
    let mut source = builder.body(PendingBody, &HttpBodyOptions::new().timeout(timeout));
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut source).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(75));
    let mut body = source.map(Ok).unwrap();
    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(25));

    let Poll::Ready(Some(Err(error))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("transform must keep the original deadline");
    };
    assert_eq!(error.label(), "body_timeout");
}

#[test]
fn body_transform_times_out_after_the_source_finishes() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let timeout = Duration::from_millis(100);
    let source = builder.body(
        Full::new(BytesView::copied_from_slice(b"source", &builder)),
        &HttpBodyOptions::new().timeout(timeout),
    );
    let mut body = map_stream(source, |body| {
        StreamBody::new(stream::once(async move {
            let _collected = body.collect().await?;
            futures::future::pending::<Result<Frame<BytesView>, HttpError>>().await
        }))
    });
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(timeout);

    let Poll::Ready(Some(Err(error))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("the output must still time out after the source finishes");
    };
    assert_eq!(error.label(), "body_timeout");
    assert!(body.is_end_stream());
}

#[test]
fn body_transform_preserves_the_buffer_limit() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen()).with_options(HttpBodyOptions::new().buffer_limit(99));
    let options = HttpBodyOptions::new().buffer_limit(3);
    let body = builder
        .body(Full::new(BytesView::copied_from_slice(b"abc", &builder)), &options)
        .map(Ok)
        .unwrap();
    assert_eq!(block_on(body.into_bytes()).unwrap(), b"abc");

    let body = builder
        .body(Full::new(BytesView::copied_from_slice(b"abcd", &builder)), &options)
        .map(Ok)
        .unwrap();
    let error = block_on(body.into_bytes()).unwrap_err();
    assert_eq!(error.label(), "body_size_limit");
    assert_eq!(error.message(), "body size exceeds the limit of 3 bytes");
}

#[test]
fn transform_does_not_apply_builder_defaults() {
    let control = ClockControl::new();
    let timeout = Duration::from_millis(100);
    let builder =
        HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock()).with_options(HttpBodyOptions::new().timeout(timeout).buffer_limit(1));
    let body = builder.text("source").map(Ok).unwrap();
    assert_eq!(block_on(body.into_text()).unwrap(), "source");

    let mut body = builder.text("source").map(Ok).unwrap();
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    control.advance(Duration::from_secs(2));
    let Poll::Ready(Some(Ok(frame))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("the original buffered body must still contain its data");
    };
    assert_eq!(frame.into_data().unwrap(), b"source");
    assert!(body.is_end_stream());
}

#[test]
fn policies_survive_buffering_cloning_and_buffered_transformations() {
    let control = ClockControl::new();
    let timeout = Duration::from_millis(100);
    let builder =
        HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock()).with_options(HttpBodyOptions::new().timeout(timeout).buffer_limit(3));
    let source = builder.stream(
        stream::iter([Ok(BytesView::copied_from_slice(b"abc", &builder))]),
        &HttpBodyOptions::new(),
    );
    let buffered = block_on(source.into_buffered()).unwrap();
    let buffered = buffered.map(Ok).unwrap();
    let clone = buffered.try_clone().unwrap();
    let mut stalled = clone
        .map(|content| {
            Ok(match content {
                Content::Bytes(_) => Content::Streaming(StreamingBody::new(PendingBody)),
                Content::Streaming(inner) => Content::Streaming(inner),
            })
        })
        .unwrap();
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut stalled).poll_frame(&mut cx).is_pending());
    control.advance(timeout);
    let Poll::Ready(Some(Err(error))) = Pin::new(&mut stalled).poll_frame(&mut cx) else {
        panic!("buffering and cloning must keep the original timeout clock");
    };
    assert_eq!(error.label(), "body_timeout");

    let expanded = buffered
        .map(|content| {
            Ok(match content {
                Content::Bytes(_) => Content::Streaming(StreamingBody::new(Full::new(BytesView::copied_from_slice(b"abcd", &builder)))),
                Content::Streaming(inner) => Content::Streaming(inner),
            })
        })
        .unwrap();
    let error = block_on(expanded.into_buffered()).unwrap_err();
    assert_eq!(error.label(), "body_size_limit");
    assert_eq!(error.message(), "body size exceeds the limit of 3 bytes");
}

#[test]
fn buffered_builder_policies_apply_after_conversion_to_streaming() {
    let control = ClockControl::new();
    let timeout = Duration::from_millis(100);
    let builder =
        HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock()).with_options(HttpBodyOptions::new().timeout(timeout).buffer_limit(3));
    let source = builder.text("abc");
    let mut stalled = source
        .map(|content| {
            Ok(match content {
                Content::Bytes(_) => Content::Streaming(StreamingBody::new(PendingBody)),
                Content::Streaming(inner) => Content::Streaming(inner),
            })
        })
        .unwrap();
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut stalled).poll_frame(&mut cx).is_pending());
    control.advance(timeout);
    let Poll::Ready(Some(Err(error))) = Pin::new(&mut stalled).poll_frame(&mut cx) else {
        panic!("buffered bodies must keep their builder's timeout policy");
    };
    assert_eq!(error.label(), "body_timeout");
}

#[test]
fn transform_can_turn_buffered_bytes_into_streaming_content() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let body = builder
        .text("source")
        .map(|content| {
            Ok(match content {
                Content::Bytes(bytes) => Content::Streaming(StreamingBody::new(Full::new(bytes))),
                Content::Streaming(inner) => Content::Streaming(inner),
            })
        })
        .unwrap();

    assert!(body.try_clone().is_none());
    assert_eq!(body.content_length(), Some(6));
    assert_eq!(block_on(body.into_bytes()).unwrap(), b"source");
}

#[test]
fn transform_can_complete_streaming_content_and_release_its_source() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let dropped = Arc::new(AtomicBool::new(false));
    let mut source = builder.body(
        DropObservedBody(Arc::clone(&dropped)),
        &HttpBodyOptions::new().timeout(Duration::from_millis(100)).buffer_limit(1),
    );
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(Pin::new(&mut source).poll_frame(&mut cx).is_pending());

    let body = source
        .map(|content| {
            Ok(match content {
                Content::Streaming(_) => Content::Bytes(BytesView::copied_from_slice(b"result", &builder)),
                Content::Bytes(bytes) => Content::Bytes(bytes),
            })
        })
        .unwrap();

    assert!(dropped.load(Ordering::Relaxed));
    assert!(body.try_clone().is_some());
    control.advance(Duration::from_secs(1));
    assert_eq!(block_on(body.into_bytes()).unwrap(), b"result");
}

#[test]
fn streaming_body_pins_a_non_unpin_source_and_delegates_its_state() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let mut body = StreamingBody::new(PinnedBody {
        bytes: BytesView::copied_from_slice(b"pinned", &builder),
        yielded: Cell::new(false),
        _pin: PhantomPinned,
    });
    assert_eq!(body.size_hint().exact(), Some(6));
    assert!(!body.is_end_stream());
    let frame = body.frame().now_or_never().unwrap().unwrap().unwrap();

    assert_eq!(frame.into_data().unwrap(), b"pinned");
    assert_eq!(body.size_hint().exact(), Some(0));
    assert!(body.is_end_stream());
    assert!(body.frame().now_or_never().unwrap().is_none());
}

#[test]
fn streaming_body_converts_source_errors() {
    let source = StreamBody::new(stream::iter([Err::<Frame<BytesView>, _>(std::io::Error::other("stream failed"))]));
    let mut body = StreamingBody::new(source);
    let error = body.frame().now_or_never().unwrap().unwrap().unwrap_err();

    assert!(error.to_string().contains("stream failed"));
    assert!(body.frame().now_or_never().unwrap().is_none());
}

#[test]
fn stream_body_returns_data_before_timeout() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let chunks = vec![Ok(BytesView::copied_from_slice(b"streamed data", &builder))];
    let body = builder.stream(stream::iter(chunks), &HttpBodyOptions::new().timeout(Duration::from_secs(30)));

    assert_eq!(block_on(body.into_bytes()).unwrap(), b"streamed data");
}

#[test]
fn stream_body_times_out_when_pending() {
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);
    let body = builder.stream(stream::pending(), &HttpBodyOptions::new().timeout(Duration::from_millis(100)));

    let error = block_on(body.into_bytes()).unwrap_err();
    assert!(error.to_string().contains("body data was not fully received"));
    assert_eq!(error.label(), "body_timeout");
}

#[test]
fn body_timeout_chains_with_buffer_limit() {
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock).with_options(HttpBodyOptions::new().buffer_limit(1024));
    let body = builder.body(PendingBody, &HttpBodyOptions::new().timeout(Duration::from_secs(30)));

    let error = block_on(body.into_bytes()).unwrap_err();
    assert!(error.to_string().contains("body data was not fully received"));
}

#[test]
fn size_hint_delegates_to_inner() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let chunks = vec![Ok(BytesView::copied_from_slice(b"hello", &builder))];
    let body = builder.stream(stream::iter(chunks), &HttpBodyOptions::new());

    assert_eq!(body.size_hint().lower(), 0);
}

#[test]
fn size_hint_delegates_through_body_timeout() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let body = builder.body(
        Full::new(BytesView::copied_from_slice(b"hello", &builder)),
        &HttpBodyOptions::new().timeout(Duration::from_secs(30)),
    );

    assert_eq!(body.size_hint().lower(), 5);
    assert_eq!(body.size_hint().upper(), Some(5));
}

#[test]
fn is_end_stream_true_when_inner_is_empty() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let body = builder.body(
        http_body_util::Empty::new(),
        &HttpBodyOptions::new().timeout(Duration::from_secs(1)),
    );

    assert!(body.is_end_stream());
}

#[test]
fn is_end_stream_false_when_inner_has_data() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let body = builder.body(
        Full::new(BytesView::copied_from_slice(b"data", &builder)),
        &HttpBodyOptions::new().timeout(Duration::from_secs(1)),
    );

    assert!(!body.is_end_stream());
}

#[test]
fn poll_frame_returns_data_through_body_timeout() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let body = builder.body(
        Full::new(BytesView::copied_from_slice(b"payload", &builder)),
        &HttpBodyOptions::new().timeout(Duration::from_secs(30)),
    );

    assert_eq!(block_on(body.into_bytes()).unwrap(), b"payload");
}

#[test]
fn poll_frame_times_out_when_pending_with_short_timeout() {
    let clock = ClockControl::new().auto_advance_timers(true).to_clock();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &clock);
    let body = builder.body(PendingBody, &HttpBodyOptions::new().timeout(Duration::from_millis(1)));

    let error = block_on(body.into_bytes()).unwrap_err();
    assert!(error.to_string().contains("body data was not fully received"));
}

#[test]
fn poll_frame_returns_data_even_when_clock_advanced_past_timeout() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let body = builder.body(
        Full::new(BytesView::copied_from_slice(b"ready data", &builder)),
        &HttpBodyOptions::new().timeout(Duration::from_millis(1)),
    );

    control.advance(Duration::from_mins(1));

    assert_eq!(block_on(body.into_bytes()).unwrap(), b"ready data");
}

#[test]
fn poll_frame_times_out_via_delay_when_inner_body_advances_clock() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let body = builder.body(
        ClockAdvancingBody { control, poll_count: 0 },
        &HttpBodyOptions::new().timeout(Duration::from_millis(100)),
    );

    let error = block_on(body.into_bytes()).unwrap_err();
    assert!(error.to_string().contains("body data was not fully received"));
}

#[test]
fn poll_frame_returns_error_after_timeout() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let timeout = Duration::from_millis(50);
    let dropped = Arc::new(AtomicBool::new(false));
    let mut body = builder.body(DropObservedBody(Arc::clone(&dropped)), &HttpBodyOptions::new().timeout(timeout));
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(timeout);
    let Poll::Ready(Some(Err(error))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("the body must report its idle timeout");
    };
    assert_eq!(error.label(), "body_timeout");
    assert!(dropped.load(Ordering::Relaxed));
    assert!(body.is_end_stream());
    assert_eq!(body.size_hint().exact(), Some(0));
    assert_eq!(body.content_length(), Some(0));
    assert!(matches!(Pin::new(&mut body).poll_frame(&mut cx), Poll::Ready(None)));
}

#[test]
fn transformed_body_times_out_after_the_source_finishes() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let timeout = Duration::from_millis(100);
    let source = builder.body(
        Full::new(BytesView::copied_from_slice(b"source", &builder)),
        &HttpBodyOptions::new().timeout(timeout),
    );
    let mut body = map_stream(source, |body| {
        StreamBody::new(stream::once(async move {
            let _collected = body.collect().await?;
            futures::future::pending::<Result<Frame<BytesView>, HttpError>>().await
        }))
    });
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(timeout);

    let Poll::Ready(Some(Err(error))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("a stalled wrapper must time out even after its source finishes");
    };
    assert_eq!(error.label(), "body_timeout");
    assert!(body.is_end_stream());
    assert!(matches!(Pin::new(&mut body).poll_frame(&mut cx), Poll::Ready(None)));
}

#[test]
fn source_frames_do_not_reset_the_mapped_output_timeout() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let options = HttpBodyOptions::default().timeout(Duration::from_millis(100));
    let (sender, receiver) = mpsc::unbounded();
    let source = builder.stream(receiver, &options);
    let mut body = Box::pin(
        source
            .map(|content| {
                Ok(match content {
                    Content::Streaming(body) => Content::Streaming(StreamingBody::new(StreamBody::new(stream::once(async move {
                        body.collect()
                            .await
                            .map(|collected| Frame::data(BytesView::from(collected.to_bytes())))
                    })))),
                    Content::Bytes(bytes) => Content::Bytes(bytes),
                })
            })
            .unwrap(),
    );
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(body.as_mut().poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(75));
    sender.unbounded_send(Ok(BytesView::copied_from_slice(b"first", &builder))).unwrap();
    assert!(body.as_mut().poll_frame(&mut cx).is_pending());

    control.advance(Duration::from_millis(75));
    sender
        .unbounded_send(Ok(BytesView::copied_from_slice(b"second", &builder)))
        .unwrap();
    let Poll::Ready(Some(Err(error))) = body.as_mut().poll_frame(&mut cx) else {
        panic!("source progress must not reset the timeout for mapped frames");
    };
    assert_eq!(error.label(), "body_timeout");
}

#[test]
fn rewrapping_uses_the_builder_timeout_and_clock_when_the_source_has_none() {
    let control = ClockControl::new();
    let timeout = Duration::from_millis(100);
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock()).with_options(HttpBodyOptions::new().timeout(timeout));
    let source_builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let source = source_builder.body(PendingBody, &HttpBodyOptions::new());
    let mut body = builder.rewrap(source, |body| body);
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(timeout);

    let Poll::Ready(Some(Err(error))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("the builder timeout must apply to the wrapped body");
    };
    assert_eq!(error.label(), "body_timeout");
}

#[test]
fn rewrapping_does_not_add_a_second_timeout_to_the_output() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let timeout = Duration::from_millis(100);
    let (sender, receiver) = mpsc::unbounded();
    let source = builder.stream(receiver, &HttpBodyOptions::new().timeout(timeout));
    let mut body = builder.rewrap(source, |body| {
        StreamBody::new(stream::once(async move { body.into_bytes().await.map(Frame::data) }))
    });
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(75));
    sender.unbounded_send(Ok(BytesView::copied_from_slice(b"first", &builder))).unwrap();
    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());

    control.advance(Duration::from_millis(75));
    sender
        .unbounded_send(Ok(BytesView::copied_from_slice(b"second", &builder)))
        .unwrap();
    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    drop(sender);

    let Poll::Ready(Some(Ok(frame))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("source progress must keep the wrapped body alive");
    };
    assert_eq!(frame.into_data().unwrap(), b"firstsecond");
}

#[test]
fn buffering_a_transformed_body_enforces_its_timeout() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let timeout = Duration::from_millis(100);
    let source = builder.stream(stream::pending(), &HttpBodyOptions::new().timeout(timeout));
    let body = map_stream(
        source,
        |_| StreamBody::new(stream::pending::<Result<Frame<BytesView>, HttpError>>()),
    );
    let mut buffered = Box::pin(body.into_buffered());
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(buffered.as_mut().poll_unpin(&mut cx).is_pending());
    control.advance(timeout);

    let Poll::Ready(Err(error)) = buffered.as_mut().poll_unpin(&mut cx) else {
        panic!("buffering must enforce the transformed body's timeout");
    };
    assert_eq!(error.label(), "body_timeout");
}

#[test]
fn transforming_keeps_the_running_timer_and_source_clock() {
    let source_control = ClockControl::new();
    let source_builder = HttpBodyBuilder::new(GlobalPool::new(), &source_control.to_clock());
    let wrapper_control = ClockControl::new();
    let timeout = Duration::from_millis(100);
    let mut source = source_builder.body(PendingBody, &HttpBodyOptions::new().timeout(timeout));
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut source).poll_frame(&mut cx).is_pending());
    source_control.advance(Duration::from_millis(75));
    let mut body = source.map(Ok).unwrap();
    body = body.map(Ok).unwrap();

    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    wrapper_control.advance(Duration::from_secs(2));
    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    source_control.advance(Duration::from_millis(25));

    let Poll::Ready(Some(Err(error))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("transforming must keep the original deadline and clock");
    };
    assert_eq!(error.label(), "body_timeout");
}

#[test]
fn transformed_output_resets_the_timer_without_a_source_timeout() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let timeout = Duration::from_millis(100);
    let source = builder.stream(stream::pending(), &HttpBodyOptions::new().timeout(timeout));
    let (sender, mut receiver) = mpsc::unbounded();
    let mut body = map_stream(source, |mut source| {
        StreamBody::new(stream::poll_fn(move |cx| {
            let output = Pin::new(&mut receiver).poll_next(cx);
            if output.is_ready() {
                return output;
            }
            Pin::new(&mut source).poll_frame(cx)
        }))
    });
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(75));
    sender
        .unbounded_send(Ok(Frame::data(BytesView::copied_from_slice(b"output", &builder))))
        .unwrap();
    assert!(matches!(Pin::new(&mut body).poll_frame(&mut cx), Poll::Ready(Some(Ok(_)))));
    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(25));
    assert!(Pin::new(&mut body).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(75));
    let Poll::Ready(Some(Err(error))) = Pin::new(&mut body).poll_frame(&mut cx) else {
        panic!("the mapped output must report its reset idle timeout");
    };
    assert_eq!(error.label(), "body_timeout");
}

#[expect(clippy::panic, reason = "this integration-test helper requires a streaming fixture")]
fn map_stream<B>(body: HttpBody, map_content: impl FnOnce(StreamingBody) -> B) -> HttpBody
where
    B: Body<Data = BytesView, Error: Into<HttpError>> + Send + 'static,
{
    body.map(|content| {
        Ok(match content {
            Content::Streaming(inner) => Content::Streaming(StreamingBody::new(map_content(inner))),
            Content::Bytes(_) => panic!("the fixture must be a streaming body"),
        })
    })
    .unwrap()
}

struct PinnedBody {
    bytes: BytesView,
    yielded: Cell<bool>,
    _pin: PhantomPinned,
}

impl Body for PinnedBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.as_ref().get_ref();
        if this.yielded.replace(true) {
            Poll::Ready(None)
        } else {
            Poll::Ready(Some(Ok(Frame::data(this.bytes.clone()))))
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::with_exact(if self.yielded.get() { 0 } else { self.bytes.len() as u64 })
    }

    fn is_end_stream(&self) -> bool {
        self.yielded.get()
    }
}

struct PendingBody;

impl Body for PendingBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Pending
    }
}

struct ClockAdvancingBody {
    control: ClockControl,
    poll_count: u32,
}

impl Body for ClockAdvancingBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.poll_count > 0 {
            self.control.advance(Duration::from_mins(1));
        }
        self.poll_count += 1;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

struct DropObservedBody(Arc<AtomicBool>);

impl Drop for DropObservedBody {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Body for DropObservedBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Pending
    }
}
