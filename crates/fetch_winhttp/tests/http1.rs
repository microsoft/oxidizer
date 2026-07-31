// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(windows)]
#![expect(
    clippy::unwrap_used,
    reason = "integration tests use unwrap to surface failures through the test harness"
)]

//! HTTP/1.1 localhost integration tests for the WinHTTP transport.

mod common;

use std::future::Future as _;
use std::task::{Context, Poll};

use bytes::Bytes;
use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use common::{ResponsePlan, TestServer, client, client_builder};
use fetch::options::{ConnectionPoolOptions, PoolSelection};
use fetch::{HttpClient, HttpError};
use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps, WinHttpTlsConfig};
use futures::TryStreamExt as _;
use http::header::{CONTENT_ENCODING, COOKIE, LOCATION, SET_COOKIE, WWW_AUTHENTICATE};
use http::{HeaderMap, HeaderValue, StatusCode, Version};
use http_body::Frame;
use http_body_util::StreamBody;
use http_extensions::HttpBodyOptions;
use observed::Sink;
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[test]
fn small_and_large_get_and_post_round_trips() {
    let large = "large response ".repeat(32 * 1024);
    let server = TestServer::http([
        ResponsePlan::ok("small"),
        ResponsePlan::ok(large.clone()),
        ResponsePlan::ok("known upload received"),
        ResponsePlan::ok("large upload received"),
    ]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());

    let small = futures::executor::block_on(test_client.client.get(server.url("/small")).fetch_text_body()).unwrap();
    let large_response = futures::executor::block_on(test_client.client.get(server.url("/large")).fetch_text_body()).unwrap();
    let known = futures::executor::block_on(
        test_client
            .client
            .post(server.url("/known-upload"))
            .text("known body")
            .fetch_text_body(),
    )
    .unwrap();
    let large_upload =
        futures::executor::block_on(test_client.client.post(server.url("/large-upload")).text(&large).fetch_text_body()).unwrap();

    assert_eq!(small, "small");
    assert_eq!(large_response, large);
    assert_eq!(known, "known upload received");
    assert_eq!(large_upload, "large upload received");

    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 4);
    assert_eq!(snapshot.requests[0].uri.path(), "/small");
    assert_eq!(snapshot.requests[1].uri.path(), "/large");
    assert_eq!(snapshot.requests[2].body, Bytes::from_static(b"known body"));
    assert_eq!(snapshot.requests[3].body, Bytes::from(large));
    assert!(snapshot.requests.iter().all(|request| request.version == Version::HTTP_11));
}

#[cfg_attr(miri, ignore)]
#[test]
fn unknown_length_upload_is_streamed_before_the_response() {
    let server = TestServer::http([ResponsePlan::ok("stream received")]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let chunks = [
        Ok(BytesView::copied_from_slice(b"streamed ", &test_client.body_builder)),
        Ok(BytesView::copied_from_slice(b"request", &test_client.body_builder)),
    ];
    let body = test_client
        .body_builder
        .stream(futures::stream::iter(chunks), &HttpBodyOptions::default());

    let response = futures::executor::block_on(test_client.client.post(server.url("/stream")).body(body).fetch_text_body()).unwrap();

    assert_eq!(response, "stream received");
    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].body, Bytes::from_static(b"streamed request"));
    assert_eq!(
        snapshot.requests[0]
            .headers
            .get("transfer-encoding")
            .and_then(|value| value.to_str().ok()),
        Some("chunked")
    );
    assert!(!snapshot.requests[0].headers.contains_key("content-length"));
}

#[cfg_attr(miri, ignore)]
#[test]
fn request_trailers_fail_explicitly() {
    let request_server = TestServer::http([]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let request_body = StreamBody::new(futures::stream::iter([
        Ok::<_, HttpError>(Frame::data(BytesView::copied_from_slice(b"data", &test_client.body_builder))),
        Ok(Frame::trailers(HeaderMap::from_iter([(
            "x-request-trailer".parse().unwrap(),
            HeaderValue::from_static("value"),
        )]))),
    ]));
    let request_body = test_client.body_builder.body(request_body, &HttpBodyOptions::default());

    let error = futures::executor::block_on(
        test_client
            .client
            .post(request_server.url("/request-trailers"))
            .body(request_body)
            .fetch(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cannot submit request trailer frames"));
    request_server.finish();
}

#[cfg_attr(miri, ignore)]
#[test]
fn compression_redirects_and_cookies_follow_transport_policy() {
    const GZIP_HELLO: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x86, 0xa6, 0x10, 0x36, 0x05,
        0x00, 0x00, 0x00,
    ];
    const DEFLATE_HELLO: &[u8] = &[0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00];

    let destination = TestServer::http([ResponsePlan::ok("must not be reached")]);
    let redirect_target = destination.url("/redirected");
    let server = TestServer::http([
        ResponsePlan::ok(Bytes::from_static(GZIP_HELLO)).header(CONTENT_ENCODING, HeaderValue::from_static("gzip")),
        ResponsePlan::ok(Bytes::from_static(DEFLATE_HELLO)).header(CONTENT_ENCODING, HeaderValue::from_static("deflate")),
        ResponsePlan::ok(Bytes::from_static(b"opaque brotli")).header(CONTENT_ENCODING, HeaderValue::from_static("br")),
        ResponsePlan::ok(Bytes::from_static(b"opaque zstd")).header(CONTENT_ENCODING, HeaderValue::from_static("zstd")),
        ResponsePlan::status(StatusCode::FOUND).header(LOCATION, HeaderValue::from_str(&redirect_target).unwrap()),
        ResponsePlan::ok("cookie set").header(SET_COOKIE, HeaderValue::from_static("session=secret; Path=/")),
        ResponsePlan::ok("cookie check"),
    ]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());

    let gzip = futures::executor::block_on(test_client.client.get(server.url("/gzip")).fetch_text_body()).unwrap();
    let deflate = futures::executor::block_on(test_client.client.get(server.url("/deflate")).fetch_text_body()).unwrap();
    let brotli = futures::executor::block_on(test_client.client.get(server.url("/brotli")).fetch()).unwrap();
    let zstd = futures::executor::block_on(test_client.client.get(server.url("/zstd")).fetch()).unwrap();
    let redirect = futures::executor::block_on(test_client.client.get(server.url("/redirect")).fetch()).unwrap();
    futures::executor::block_on(test_client.client.get(server.url("/set-cookie")).fetch_text_body()).unwrap();
    futures::executor::block_on(test_client.client.get(server.url("/check-cookie")).fetch_text_body()).unwrap();

    assert_eq!(gzip, "hello");
    assert_eq!(deflate, "hello");
    assert_eq!(brotli.headers().get(CONTENT_ENCODING).unwrap(), "br");
    assert_eq!(zstd.headers().get(CONTENT_ENCODING).unwrap(), "zstd");
    assert_eq!(
        futures::executor::block_on(brotli.into_body().into_bytes()).unwrap(),
        BytesView::copied_from_slice(b"opaque brotli", &test_client.body_builder)
    );
    assert_eq!(
        futures::executor::block_on(zstd.into_body().into_bytes()).unwrap(),
        BytesView::copied_from_slice(b"opaque zstd", &test_client.body_builder)
    );
    assert_eq!(redirect.status(), StatusCode::FOUND);
    assert_eq!(redirect.headers().get(LOCATION).unwrap(), redirect_target.as_str());

    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 7);
    assert!(!snapshot.requests[6].headers.contains_key(COOKIE));
    assert!(destination.finish().requests.is_empty());
}

#[cfg_attr(miri, ignore)]
#[test]
fn authentication_challenges_are_surfaced_without_automatic_retry() {
    let server = TestServer::http([
        ResponsePlan::status(StatusCode::UNAUTHORIZED).header(WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"test\""))
    ]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());

    let response = futures::executor::block_on(test_client.client.get(server.url("/authentication")).fetch()).unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers().get(WWW_AUTHENTICATE).unwrap(), "Basic realm=\"test\"");
    assert_eq!(server.finish().requests.len(), 1);
}

#[cfg_attr(miri, ignore)]
#[test]
fn full_fetch_pipeline_uses_the_winhttp_transport() {
    let server = TestServer::http([ResponsePlan::ok("full pipeline")]);
    let clock = Clock::new_frozen();
    let global_pool = GlobalPool::new();
    let deps = WinHttpDeps::builder(clock, global_pool, Sink::noop()).build();
    let client = HttpClient::builder_winhttp(deps)
        .insecure_allow_http()
        .supported_http_versions(&[Version::HTTP_11])
        .build();

    let response = futures::executor::block_on(client.get(server.url("/full-pipeline")).fetch_text_body()).unwrap();

    assert_eq!(response, "full pipeline");
    assert_eq!(server.finish().requests.len(), 1);
}

#[cfg_attr(miri, ignore)]
#[test]
fn clients_reuse_only_their_own_connection_pools() {
    let shared_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let shared = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let shared_clone = shared.client.clone();
    futures::executor::block_on(shared.client.get(shared_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(shared_clone.get(shared_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(shared_server.finish().connections, 1);

    let isolated_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let first = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let second = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    futures::executor::block_on(first.client.get(isolated_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(second.client.get(isolated_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(isolated_server.finish().connections, 2);

    let builder_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let first = builder.clone().build();
    let second = builder.build();
    futures::executor::block_on(first.get(builder_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(second.get(builder_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(builder_server.finish().connections, 2);

    let pools_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two"), ResponsePlan::ok("three")]);
    let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let client = builder
        .connection_pool_options(ConnectionPoolOptions::default().multiple_pools(2, PoolSelection::round_robin()))
        .build();
    for path in ["/one", "/two", "/three"] {
        futures::executor::block_on(client.get(pools_server.url(path)).fetch_text_body()).unwrap();
    }
    assert_eq!(pools_server.finish().connections, 2);
}

#[cfg_attr(miri, ignore)]
#[test]
fn dropping_a_pending_download_cancels_it_without_blocking_later_requests() {
    let server = TestServer::http([
        ResponsePlan::chunks([Bytes::from_static(b"first")]).stall_after_frames(),
        ResponsePlan::ok("after cancellation"),
    ]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let response = futures::executor::block_on(test_client.client.get(server.url("/stall")).fetch()).unwrap();
    let mut stream = response.into_body().into_stream();
    let first = futures::executor::block_on(stream.try_next()).unwrap().unwrap();
    assert_eq!(first, BytesView::copied_from_slice(b"first", &test_client.body_builder));

    let mut pending = Box::pin(stream.try_next());
    let waker = futures::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(pending.as_mut().poll(&mut context), Poll::Pending));
    drop(pending);
    drop(stream);

    let response = futures::executor::block_on(test_client.client.get(server.url("/after-cancellation")).fetch_text_body()).unwrap();
    assert_eq!(response, "after cancellation");
    assert_eq!(server.finish().connections, 2);
}

#[cfg_attr(miri, ignore)]
#[test]
fn mixed_completion_cancellation_and_body_drops_remain_usable() {
    let mut responses = Vec::new();
    for index in 0..18 {
        responses.push(match index % 3 {
            0 => ResponsePlan::ok(format!("complete {index}")),
            1 => ResponsePlan::chunks([Bytes::from_static(b"partial")]).stall_after_frames(),
            _ => ResponsePlan::ok("dropped"),
        });
    }
    responses.push(ResponsePlan::ok("final"));
    let server = TestServer::http(responses);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());

    for index in 0..18 {
        let response = futures::executor::block_on(test_client.client.get(server.url(&format!("/soak/{index}"))).fetch()).unwrap();
        match index % 3 {
            0 => {
                let body = futures::executor::block_on(response.into_body().into_text()).unwrap();
                assert_eq!(body, format!("complete {index}"));
            }
            1 => {
                let mut stream = response.into_body().into_stream();
                let first = futures::executor::block_on(stream.try_next()).unwrap().unwrap();
                assert_eq!(first, BytesView::copied_from_slice(b"partial", &test_client.body_builder));
                let mut pending = Box::pin(stream.try_next());
                let waker = futures::task::noop_waker();
                let mut context = Context::from_waker(&waker);
                assert!(matches!(pending.as_mut().poll(&mut context), Poll::Pending));
                drop(pending);
                drop(stream);
            }
            _ => drop(response),
        }
    }

    let final_response = futures::executor::block_on(test_client.client.get(server.url("/final")).fetch_text_body()).unwrap();
    assert_eq!(final_response, "final");
    assert_eq!(server.finish().requests.len(), 19);
}
