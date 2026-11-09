// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests transport policy on the wire over localhost.
//!
//! Covers server addressing, request methods, body framing, request-trailer
//! rejection, selective response decoding, redirects, cookies, and
//! authentication. These rules apply to every protocol. Plaintext HTTP/1.1 is
//! the smallest fixture that shows them.

#![cfg(windows)]

use bytes::Bytes;
use bytesbuf::BytesView;
use fetch::HttpError;
use fetch_winhttp::WinHttpTlsConfig;
use fetch_winhttp_impl::testing::{ResponsePlan, TestServer, client};
use http::header::{CONTENT_ENCODING, CONTENT_LENGTH, COOKIE, LOCATION, SET_COOKIE, TRANSFER_ENCODING, WWW_AUTHENTICATE};
use http::{HeaderMap, HeaderValue, Method, StatusCode, Version};
use http_body::Frame;
use http_body_util::StreamBody;
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
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

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
    assert_eq!(snapshot.requests[0].method, Method::GET);
    assert_eq!(snapshot.requests[1].method, Method::GET);
    assert_eq!(snapshot.requests[2].method, Method::POST);
    assert_eq!(snapshot.requests[3].method, Method::POST);
    assert!(snapshot.requests.iter().all(|request| request.version == Version::HTTP_11));
}

#[cfg_attr(miri, ignore)]
#[test]
fn a_bracketed_ipv6_authority_reaches_the_server() {
    let server = TestServer::http_ipv6([ResponsePlan::ok("ipv6")]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

    let body = futures::executor::block_on(test_client.client.get(server.url("/ipv6")).fetch_text_body()).unwrap();

    assert_eq!(body, "ipv6");
    assert_eq!(server.finish().requests.len(), 1);
}

#[cfg_attr(miri, ignore)]
#[test]
fn a_caller_supplied_content_length_reaches_the_wire_exactly_once() {
    // Eleven bytes fit below the `DWORD` limit, so the transport uses
    // `dwTotalLength` instead of the ignore-total sentinel.
    const BODY: &str = "framed body";

    let server = TestServer::http([ResponsePlan::ok("declared length received")]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

    // Below the `DWORD` limit, the transport keeps an agreeing
    // `Content-Length` and passes the same value as `dwTotalLength`. MSDN
    // requires both inputs. Only the server can prove that WinHTTP sends one
    // framing directive.
    //
    // The non-canonical `0011` value proves that the caller supplied this
    // header. A received `11` proves that reconciliation normalized it.
    let response = futures::executor::block_on(
        test_client
            .client
            .post(server.url("/declared-length"))
            .header(CONTENT_LENGTH, HeaderValue::from_static("0011"))
            .text(BODY)
            .fetch_text_body(),
    )
    .unwrap();

    assert_eq!(response, "declared length received");

    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    let request = &snapshot.requests[0];
    assert_eq!(request.body, Bytes::from_static(BODY.as_bytes()));
    // `hyper` accepts repeated identical `Content-Length` values. Count every
    // received value so a second copy cannot pass unnoticed.
    assert_eq!(
        request.headers.get_all(CONTENT_LENGTH).iter().count(),
        1,
        "expected exactly one Content-Length on the wire, received {:?}",
        request.headers.get_all(CONTENT_LENGTH).iter().collect::<Vec<_>>()
    );
    assert_eq!(
        request.headers.get(CONTENT_LENGTH).unwrap().to_str().unwrap(),
        BODY.len().to_string()
    );
    // The other framing directive must be absent. RFC 9112 section 6.1 gives
    // transfer coding priority when both directives are present.
    assert!(!request.headers.contains_key(TRANSFER_ENCODING));
}

#[cfg_attr(miri, ignore)]
#[test]
fn an_empty_request_body_is_framed_as_a_zero_length() {
    // A zero-length body and `WINHTTP_IGNORE_REQUEST_TOTAL_LENGTH` both use
    // zero. Only the server can show how WinHTTP frames an empty POST.
    let server = TestServer::http([ResponsePlan::ok("no body received"), ResponsePlan::ok("empty body received")]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

    let absent = futures::executor::block_on(test_client.client.post(server.url("/absent")).fetch_text_body()).unwrap();
    let empty = futures::executor::block_on(test_client.client.post(server.url("/empty")).text("").fetch_text_body()).unwrap();

    assert_eq!(absent, "no body received");
    assert_eq!(empty, "empty body received");

    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 2);
    for request in &snapshot.requests {
        assert_eq!(request.method, Method::POST);
        assert!(request.body.is_empty());
        assert_eq!(
            request.headers.get(CONTENT_LENGTH).map(|value| value.to_str().unwrap()),
            Some("0"),
            "expected a zero Content-Length on {}, received {:?}",
            request.uri.path(),
            request.headers
        );
        assert!(!request.headers.contains_key(TRANSFER_ENCODING));
    }
}

#[cfg_attr(miri, ignore)]
#[test]
fn a_disagreeing_caller_supplied_content_length_never_reaches_the_wire() {
    // Use a value shorter than the body. A longer value can leave the server
    // waiting forever if reconciliation regresses. A shorter value fails
    // promptly.
    const DECLARED: &str = "4";
    const BODY: &str = "framed body";

    let server = TestServer::http([ResponsePlan::ok("must not be reached")]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

    let error = futures::executor::block_on(
        test_client
            .client
            .post(server.url("/disagreeing-length"))
            .header(CONTENT_LENGTH, HeaderValue::from_static(DECLARED))
            .text(BODY)
            .fetch(),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("Content-Length header must equal the exact request body length (11)"),
        "unexpected error: {error}"
    );
    assert!(server.finish().requests.is_empty());
}

#[cfg_attr(miri, ignore)]
#[test]
fn request_trailers_fail_explicitly() {
    let request_server = TestServer::http([]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
    let request_body = StreamBody::new(futures::stream::iter([
        Ok::<_, HttpError>(Frame::data(BytesView::copied_from_slice(b"data", &test_client.body_builder))),
        Ok(Frame::trailers(HeaderMap::from_iter([(
            "x-request-trailer".parse().unwrap(),
            HeaderValue::from_static("value"),
        )]))),
    ]));
    let request_body = test_client.body_builder.body(request_body);

    let error = futures::executor::block_on(
        test_client
            .client
            .post(request_server.url("/request-trailers"))
            .body(request_body)
            .fetch(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cannot submit request trailer frames"));
    // The transport writes the data frame before it rejects the trailer frame.
    // The fixture records only complete requests. An empty recording proves
    // that no final chunk or trailer section reached the peer.
    assert!(request_server.finish().requests.is_empty());
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
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

    let gzip = futures::executor::block_on(test_client.client.get(server.url("/gzip")).fetch()).unwrap();
    let deflate = futures::executor::block_on(test_client.client.get(server.url("/deflate")).fetch()).unwrap();
    let brotli = futures::executor::block_on(test_client.client.get(server.url("/brotli")).fetch()).unwrap();
    let zstd = futures::executor::block_on(test_client.client.get(server.url("/zstd")).fetch()).unwrap();
    let redirect = futures::executor::block_on(test_client.client.get(server.url("/redirect")).fetch()).unwrap();
    futures::executor::block_on(test_client.client.get(server.url("/set-cookie")).fetch_text_body()).unwrap();
    futures::executor::block_on(test_client.client.get(server.url("/check-cookie")).fetch_text_body()).unwrap();

    // Automatic decoding removes metadata that describes the encoded body.
    assert!(gzip.headers().get(CONTENT_ENCODING).is_none());
    assert!(gzip.headers().get(CONTENT_LENGTH).is_none());
    assert!(deflate.headers().get(CONTENT_ENCODING).is_none());
    assert!(deflate.headers().get(CONTENT_LENGTH).is_none());
    assert_eq!(futures::executor::block_on(gzip.into_body().into_text()).unwrap(), "hello");
    assert_eq!(futures::executor::block_on(deflate.into_body().into_text()).unwrap(), "hello");
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
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

    let response = futures::executor::block_on(test_client.client.get(server.url("/authentication")).fetch()).unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers().get(WWW_AUTHENTICATE).unwrap(), "Basic realm=\"test\"");
    assert_eq!(server.finish().requests.len(), 1);
}
