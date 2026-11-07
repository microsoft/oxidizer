// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Source-side idle timeouts and buffering policies survive body transformations.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use futures::channel::{mpsc, oneshot};
use futures::executor::block_on;
use futures::stream;
use http_body::{Body, Frame};
use http_body_util::StreamBody;
use http_extensions::{HttpBody, HttpBodyBuilder, HttpBodyOptions, HttpError};
use ohno::{ErrorExt as _, Labeled as _};
use tick::ClockControl;

fn poll_frame(body: &mut HttpBody) -> Poll<Option<Result<Frame<BytesView>, HttpError>>> {
    Pin::new(body).poll_frame(&mut Context::from_waker(futures::task::noop_waker_ref()))
}

#[test]
fn inherited_timeout_reaches_the_observer_before_drop_and_then_the_consumer() {
    #[derive(Debug, PartialEq, Eq)]
    enum Observation {
        Error(String, String),
        Dropped,
    }

    struct Observer {
        inner: HttpBody,
        observations: Arc<Mutex<Vec<Observation>>>,
    }

    impl Body for Observer {
        type Data = BytesView;
        type Error = HttpError;

        fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            let result = Pin::new(&mut self.inner).poll_frame(cx);
            if let Poll::Ready(Some(Err(error))) = &result {
                self.observations
                    .lock()
                    .unwrap()
                    .push(Observation::Error(error.label().to_string(), error.message()));
            }
            result
        }
    }

    impl Drop for Observer {
        fn drop(&mut self) {
            self.observations.lock().unwrap().push(Observation::Dropped);
        }
    }

    let control = ClockControl::new();
    let source_builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let builder = source_builder
        .clone()
        .with_options(HttpBodyOptions::new().timeout(Duration::from_millis(100)));
    let source = source_builder.stream(stream::pending(), &HttpBodyOptions::new());
    let observations = Arc::new(Mutex::new(Vec::new()));
    let mut body = builder.rewrap(source, |inner| Observer {
        inner,
        observations: Arc::clone(&observations),
    });

    assert!(poll_frame(&mut body).is_pending());
    control.advance(Duration::from_millis(99));
    assert!(poll_frame(&mut body).is_pending());
    assert!(observations.lock().unwrap().is_empty());
    control.advance(Duration::from_millis(2));

    let Poll::Ready(Some(Err(error))) = poll_frame(&mut body) else {
        panic!("the inherited source timeout must reach the consumer");
    };
    assert_eq!(error.label(), "body_timeout");
    assert_eq!(error.message(), "body data was not fully received, timeout: 100ms");
    assert_eq!(
        *observations.lock().unwrap(),
        [Observation::Error(
            "body_timeout".to_owned(),
            "body data was not fully received, timeout: 100ms".to_owned()
        )]
    );
    drop(body);
    assert_eq!(
        *observations.lock().unwrap(),
        [
            Observation::Error(
                "body_timeout".to_owned(),
                "body data was not fully received, timeout: 100ms".to_owned()
            ),
            Observation::Dropped
        ]
    );
}

#[test]
fn inherited_timeout_tracks_source_progress_while_a_transform_combines_frames() {
    let control = ClockControl::new();
    let source_builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let builder = source_builder
        .clone()
        .with_options(HttpBodyOptions::new().timeout(Duration::from_millis(100)));
    let (sender, receiver) = mpsc::unbounded();
    let source = source_builder.stream(receiver, &HttpBodyOptions::new());
    let mut body = builder.rewrap(source, |source| {
        StreamBody::new(stream::once(async move { source.into_bytes().await.map(Frame::data) }))
    });

    assert!(poll_frame(&mut body).is_pending());
    control.advance(Duration::from_millis(75));
    sender.unbounded_send(Ok(BytesView::copied_from_slice(b"first", &builder))).unwrap();
    assert!(poll_frame(&mut body).is_pending());
    control.advance(Duration::from_millis(75));
    sender
        .unbounded_send(Ok(BytesView::copied_from_slice(b"second", &builder)))
        .unwrap();
    assert!(poll_frame(&mut body).is_pending());
    drop(sender);

    let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
        panic!("source progress must keep the combining transformation alive");
    };
    assert_eq!(frame.into_data().unwrap(), b"firstsecond");
    assert!(matches!(poll_frame(&mut body), Poll::Ready(None)));
}

#[test]
fn source_timeout_wins_over_both_shorter_and_longer_builder_timeouts() {
    for (source_ms, builder_ms) in [(200, 50), (100, 1000)] {
        let control = ClockControl::new();
        let source_builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
        let builder = source_builder
            .clone()
            .with_options(HttpBodyOptions::new().timeout(Duration::from_millis(builder_ms)));
        let source = source_builder.stream(stream::pending(), &HttpBodyOptions::new().timeout(Duration::from_millis(source_ms)));
        let mut body = builder.rewrap(source, |source| source);

        assert!(poll_frame(&mut body).is_pending());
        control.advance(Duration::from_millis(source_ms - 1));
        assert!(poll_frame(&mut body).is_pending());
        control.advance(Duration::from_millis(2));

        let Poll::Ready(Some(Err(error))) = poll_frame(&mut body) else {
            panic!("the source timeout must take precedence");
        };
        assert_eq!(error.label(), "body_timeout");
        assert_eq!(error.message(), format!("body data was not fully received, timeout: {source_ms}ms"));
    }
}

#[test]
fn an_existing_source_timeout_does_not_time_delayed_transformed_output() {
    let control = ClockControl::new();
    let source_builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let builder = source_builder
        .clone()
        .with_options(HttpBodyOptions::new().timeout(Duration::from_millis(50)));
    let source = source_builder.stream(
        stream::iter([Ok(BytesView::copied_from_slice(b"source", &source_builder))]),
        &HttpBodyOptions::new().timeout(Duration::from_millis(100)),
    );
    let (release, gate) = oneshot::channel();
    let mut body = builder.rewrap(source, |source| {
        StreamBody::new(stream::once(async move {
            let bytes = source.into_bytes().await?;
            gate.await.unwrap();
            Ok::<_, HttpError>(Frame::data(bytes))
        }))
    });

    assert!(poll_frame(&mut body).is_pending());
    control.advance(Duration::from_secs(1));
    assert!(poll_frame(&mut body).is_pending());
    release.send(()).unwrap();
    let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
        panic!("the transformation must finish without an output timeout");
    };
    assert_eq!(frame.into_data().unwrap(), b"source");
    assert!(matches!(poll_frame(&mut body), Poll::Ready(None)));
}

#[test]
fn a_wholly_untimed_stream_can_resume_after_a_long_pause() {
    let control = ClockControl::new();
    let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
    let (sender, receiver) = mpsc::unbounded();
    let source = builder.stream(receiver, &HttpBodyOptions::new());
    let mut body = builder.rewrap(source, |source| source);

    assert!(poll_frame(&mut body).is_pending());
    control.advance(Duration::from_mins(1));
    assert!(poll_frame(&mut body).is_pending());
    sender
        .unbounded_send(Ok(BytesView::copied_from_slice(b"resumed", &builder)))
        .unwrap();
    drop(sender);
    let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
        panic!("an untimed source must remain readable after a pause");
    };
    assert_eq!(frame.into_data().unwrap(), b"resumed");
    assert!(matches!(poll_frame(&mut body), Poll::Ready(None)));
}

#[test]
fn memory_and_empty_sources_do_not_add_a_timeout_to_transformed_output() {
    for empty in [false, true] {
        let control = ClockControl::new();
        let builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock())
            .with_options(HttpBodyOptions::new().timeout(Duration::from_millis(100)));
        let source = if empty { builder.empty() } else { builder.text("memory") };
        let (release, gate) = oneshot::channel();
        let mut body = builder.rewrap(source, |source| {
            StreamBody::new(stream::once(async move {
                let bytes = source.into_bytes().await?;
                gate.await.unwrap();
                Ok::<_, HttpError>(Frame::data(bytes))
            }))
        });

        assert!(poll_frame(&mut body).is_pending());
        control.advance(Duration::from_secs(1));
        assert!(poll_frame(&mut body).is_pending());
        release.send(()).unwrap();
        let Poll::Ready(Some(Ok(frame))) = poll_frame(&mut body) else {
            panic!("in-memory sources must not time the transformation");
        };
        let expected = if empty { "" } else { "memory" };
        assert_eq!(frame.into_data().unwrap(), expected.as_bytes());
        assert!(matches!(poll_frame(&mut body), Poll::Ready(None)));
    }
}

#[test]
fn later_buffering_enforces_the_source_limit_or_the_builder_fallback() {
    for (source_limit, payload, expected_limit, succeeds) in [
        (Some(7), "12345", 7, true),
        (Some(7), "12345678", 7, false),
        (None, "123", 3, true),
        (None, "1234", 3, false),
    ] {
        for memory in [false, true] {
            if memory && source_limit.is_some() {
                continue;
            }
            let control = ClockControl::new();
            let source_builder = HttpBodyBuilder::new(GlobalPool::new(), &control.to_clock());
            let builder = source_builder
                .clone()
                .with_options(HttpBodyOptions::new().buffer_limit(3).timeout(Duration::from_millis(100)));
            let source = if memory {
                source_builder.text(payload)
            } else {
                let options = source_limit.map_or_else(HttpBodyOptions::new, |limit| HttpBodyOptions::new().buffer_limit(limit));
                source_builder.stream(
                    stream::iter([Ok(BytesView::copied_from_slice(payload.as_bytes(), &source_builder))]),
                    &options,
                )
            };
            let body = builder.rewrap(source, |source| source);
            let result = block_on(body.into_buffered());
            if succeeds {
                assert_eq!(block_on(result.unwrap().into_bytes()).unwrap(), payload.as_bytes());
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.label(), "body_size_limit");
                assert!(error.message().contains(&format!("limit of {expected_limit} bytes")));
            }
        }
    }
}
