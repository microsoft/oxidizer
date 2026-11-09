// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Public behavior of HTTP body mapping.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::cell::Cell;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use futures::executor::block_on;
use futures::{FutureExt, stream};
use http_body::Body;
use http_body_util::{BodyExt, Full};
use http_extensions::body::{Content, StreamingBody};
use http_extensions::{HttpBodyBuilder, HttpBodyOptions, HttpError};
use ohno::{ErrorExt, Labeled};
use tick::ClockControl;

#[test]
fn maps_buffered_and_streaming_content() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());

    let buffered = builder
        .text("buffered")
        .map(|content| {
            let Content::Bytes(bytes) = content else {
                panic!("text content must be buffered");
            };
            Ok(Content::Bytes(BytesView::from_views([
                bytes,
                BytesView::copied_from_slice(b" output", &builder),
            ])))
        })
        .unwrap();
    assert_eq!(block_on(buffered.into_text()).unwrap(), "buffered output");

    let selected_streaming = Cell::new(false);
    let source = builder.stream(
        stream::iter([Ok(BytesView::copied_from_slice(b"streaming", &builder))]),
        &HttpBodyOptions::new(),
    );
    let streaming = source
        .map(|content| {
            let Content::Streaming(inner) = content else {
                panic!("stream content must be streaming");
            };
            selected_streaming.set(true);
            Ok(Content::Streaming(StreamingBody::new(inner.map_frame(|frame| frame))))
        })
        .unwrap();

    assert!(selected_streaming.get());
    assert_eq!(block_on(streaming.into_text()).unwrap(), "streaming");
}

#[test]
fn propagates_callback_and_consumed_body_errors() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen());
    let error = builder
        .text("source")
        .map(|_| Err(HttpError::validation("map failed")))
        .unwrap_err();
    assert_eq!(error.message(), "map failed");
    assert_eq!(error.label(), "validation");

    let mut consumed = builder.text("source");
    consumed.frame().now_or_never().unwrap().unwrap().unwrap();
    let called = Cell::new(false);
    let error = consumed
        .map(|content| {
            called.set(true);
            Ok(content)
        })
        .unwrap_err();

    assert!(!called.get());
    assert_eq!(error.label(), "body_consumed");
}

#[test]
fn preserves_the_buffer_limit_for_streaming_output() {
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &tick::Clock::new_frozen()).with_options(HttpBodyOptions::new().buffer_limit(3));
    let transformed = builder
        .text("abc")
        .map(|content| {
            let Content::Bytes(_) = content else {
                panic!("text content must be buffered");
            };
            Ok(Content::Streaming(StreamingBody::new(Full::new(BytesView::copied_from_slice(
                b"four", &builder,
            )))))
        })
        .unwrap();

    let error = block_on(transformed.into_buffered()).unwrap_err();
    assert_eq!(error.label(), "body_size_limit");
}

#[test]
fn preserves_an_active_timeout_and_its_clock() {
    let control = ClockControl::new();
    let timeout = Duration::from_millis(100);
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let mut source = builder.stream(stream::pending(), &HttpBodyOptions::new().timeout(timeout));
    let waker = futures::task::noop_waker();
    let mut cx = Context::from_waker(&waker);

    assert!(Pin::new(&mut source).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(75));

    let mut transformed = source
        .map(|content| {
            let Content::Streaming(inner) = content else {
                panic!("pending content must be streaming");
            };
            Ok(Content::Streaming(inner))
        })
        .unwrap();
    assert!(Pin::new(&mut transformed).poll_frame(&mut cx).is_pending());
    control.advance(Duration::from_millis(25));

    let Poll::Ready(Some(Err(error))) = Pin::new(&mut transformed).poll_frame(&mut cx) else {
        panic!("the original timeout must fire");
    };
    assert_eq!(error.label(), "body_timeout");
}
