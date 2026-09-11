// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Incremental HTTP compression, including pauses, trailers and source failures.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::pin::Pin;
use std::task::{Context, Poll};

use bytesbuf::BytesView;
use compressors::format::Format;
use futures::Stream as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::task::noop_waker;
use http::header::CONTENT_TYPE;
use http::{HeaderMap, HeaderValue, StatusCode};
use http_body::{Body, Frame};
use http_compression::{Compression, OriginalBody};
use http_extensions::{
    FakeHandler, HttpBody, HttpBodyBuilder, HttpBodyOptions, HttpError, HttpRequest, HttpResponse, HttpResponseBuilder, Result,
};
use layered::{Layer, Service};
use ohno::{ErrorLabel, Labeled as _};
use seatbelt::RecoveryInfo;

testing_aids::init_tracing!();

const FORMATS: &[Format] = &[Format::Gzip, Format::Brotli, Format::Zstd, Format::Zlib];

struct ChannelBody(UnboundedReceiver<Result<Frame<BytesView>>>);

impl Body for ChannelBody {
    type Data = BytesView;
    type Error = HttpError;

    fn poll_frame(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>>>> {
        Pin::new(&mut self.get_mut().0).poll_next(cx)
    }
}

async fn streaming_response(format: Format) -> (UnboundedSender<Result<Frame<BytesView>>>, HttpResponse) {
    let builder = HttpBodyBuilder::new_fake();
    let server = Compression::server(builder.clone())
        .compress_responses(&[format])
        .layer(FakeHandler::from_fn(|request: HttpRequest| {
            HttpResponseBuilder::new_fake()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, HeaderValue::from_static("application/x-ndjson"))
                .body(request.into_body())
                .build()
        }));
    let client = Compression::client(builder.clone()).decompress_responses(&[format]).layer(server);
    let (sender, receiver) = unbounded();
    let request = http::Request::get("https://example.com/stream")
        .body(builder.body(ChannelBody(receiver), &HttpBodyOptions::default()))
        .unwrap();
    let response = client.execute(request).await.unwrap();
    assert_eq!(response.extensions().get::<OriginalBody>().unwrap().formats(), [format]);
    (sender, response)
}

fn data(bytes: &[u8]) -> Frame<BytesView> {
    Frame::data(BytesView::copied_from_slice(bytes, &HttpBodyBuilder::new_fake()))
}

fn receive_burst(mut body: Pin<&mut HttpBody>, expected: &[u8]) {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut received = Vec::new();
    // Bound polling rather than waiting for EOF or a wall-clock timeout. Cooperative
    // codec yields may return Pending before the source itself pauses.
    for _ in 0..1024 {
        match body.as_mut().poll_frame(&mut cx) {
            Poll::Ready(Some(frame)) => received.extend(frame.unwrap().into_data().unwrap().to_vec()),
            Poll::Ready(None) => break,
            Poll::Pending => {}
        }
        if received.len() >= expected.len() {
            break;
        }
    }
    assert_eq!(received, expected, "the entire burst must be readable before the next one is sent");
}

#[tokio::test]
async fn each_burst_is_decompressed_before_eof_and_trailers_survive() {
    for &format in FORMATS {
        let (sender, response) = streaming_response(format).await;
        let mut body = Box::pin(response.into_body());

        sender.unbounded_send(Ok(data(b"{\"n\":1}\n"))).unwrap();
        receive_burst(body.as_mut(), b"{\"n\":1}\n");

        sender.unbounded_send(Ok(data(b""))).unwrap();
        sender.unbounded_send(Ok(data(b"{\"n\":"))).unwrap();
        sender.unbounded_send(Ok(data(b"2}\n"))).unwrap();
        receive_burst(body.as_mut(), b"{\"n\":2}\n");

        let mut trailers = HeaderMap::new();
        trailers.insert("x-complete", HeaderValue::from_static("true"));
        sender.unbounded_send(Ok(Frame::trailers(trailers))).unwrap();
        drop(sender);

        let mut seen = None;
        while let Some(frame) = std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
            seen = Some(frame.unwrap().into_trailers().unwrap());
        }
        assert_eq!(seen.unwrap().get("x-complete").unwrap(), "true", "{format:?}");
        assert!(body.is_end_stream());
    }
}

#[tokio::test]
async fn a_source_error_after_a_flushed_burst_keeps_its_label_and_ends_the_body() {
    for &format in FORMATS {
        let (sender, response) = streaming_response(format).await;
        let mut body = Box::pin(response.into_body());
        sender.unbounded_send(Ok(data(b"{\"n\":1}\n"))).unwrap();
        receive_burst(body.as_mut(), b"{\"n\":1}\n");

        sender
            .unbounded_send(Err(HttpError::other(
                "producer failed",
                RecoveryInfo::never(),
                ErrorLabel::from_static("source_failed"),
            )))
            .unwrap();
        sender.unbounded_send(Ok(Frame::trailers(HeaderMap::new()))).unwrap();
        drop(sender);

        let error = std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await.unwrap().unwrap_err();
        assert_eq!(error.label(), "source_failed", "{format:?}");
        assert!(std::future::poll_fn(|cx| body.as_mut().poll_frame(cx)).await.is_none());
    }
}
