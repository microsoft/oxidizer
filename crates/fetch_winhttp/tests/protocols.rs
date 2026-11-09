// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests protocol negotiation and version-specific framing over localhost.
//!
//! Covers the negotiated protocol, the reported protocol, chunked HTTP/1.1
//! uploads, HTTP/2 and HTTP/3 stream framing and trailers, and required HTTP/3.

#![cfg(windows)]

use std::error::Error;

use bytes::Bytes;
use bytesbuf::BytesView;
use fetch_winhttp::WinHttpTlsConfig;
use fetch_winhttp_impl::testing::{Http3Server, ResponsePlan, TestServer, client, collect_frames};
use http::{HeaderMap, HeaderValue, Version};
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[test]
fn unknown_length_upload_is_streamed_before_the_response() {
    let server = TestServer::http([ResponsePlan::ok("stream received")]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
    let chunks = [
        Ok(BytesView::copied_from_slice(b"streamed ", &test_client.body_builder)),
        Ok(BytesView::copied_from_slice(b"request", &test_client.body_builder)),
    ];
    let body = test_client.body_builder.stream(futures::stream::iter(chunks));

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
    // A chunked upload can carry trailers, but this transport does not send
    // them. The final chunk must not contain trailers.
    assert!(snapshot.requests[0].trailers.is_none());
}

#[cfg_attr(miri, ignore)]
#[test]
fn http1_is_negotiated_and_reported() {
    // A negotiated-protocol value of `0` makes `parse_protocol_used` query
    // `WINHTTP_OPTION_HTTP_VERSION`. Unit tests use a scripted `"HTTP/1.1"`
    // value. This live test proves that WinHTTP returns an accepted value.
    let server = TestServer::https([ResponsePlan::ok("http1")], &["localhost"]);
    let test_client = client(
        &[Version::HTTP_11],
        WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        Clock::new_frozen(),
    );

    let response = futures::executor::block_on(test_client.client.get(server.url("/http1")).fetch()).unwrap();

    assert_eq!(response.version(), Version::HTTP_11);
    assert_eq!(futures::executor::block_on(response.into_body().into_text()).unwrap(), "http1");
    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].version, Version::HTTP_11);
}

#[cfg_attr(miri, ignore)]
#[test]
fn http2_is_negotiated_and_reported() {
    let server = TestServer::https([ResponsePlan::ok("http2")], &["localhost"]);
    let test_client = client(
        &[Version::HTTP_2],
        WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        Clock::new_frozen(),
    );

    let response = futures::executor::block_on(test_client.client.get(server.url("/http2")).fetch()).unwrap();

    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(futures::executor::block_on(response.into_body().into_text()).unwrap(), "http2");
    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].version, Version::HTTP_2);
}

#[cfg_attr(miri, ignore)]
#[test]
fn http2_streams_unknown_length_uploads_and_preserves_response_trailers() {
    let server = TestServer::https(
        [
            ResponsePlan::chunks([Bytes::from_static(b"response")]).trailers(HeaderMap::from_iter([(
                "x-trailer".parse().unwrap(),
                HeaderValue::from_static("value"),
            )])),
        ],
        &["localhost"],
    );
    let test_client = client(
        &[Version::HTTP_2],
        WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        Clock::new_frozen(),
    );
    let body = test_client.body_builder.stream(futures::stream::iter([
        Ok(BytesView::copied_from_slice(b"streamed ", &test_client.body_builder)),
        Ok(BytesView::copied_from_slice(b"request", &test_client.body_builder)),
    ]));

    let response = futures::executor::block_on(test_client.client.post(server.url("/stream")).body(body).fetch()).unwrap();
    let (response_body, trailers) = futures::executor::block_on(collect_frames(response.into_body()));

    assert_eq!(response_body, b"response");
    assert_eq!(trailers.unwrap().get("x-trailer").unwrap(), "value");
    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].body, Bytes::from_static(b"streamed request"));
    assert_eq!(snapshot.requests[0].version, Version::HTTP_2);
}

#[cfg_attr(miri, ignore)]
#[test]
fn http3_streams_an_unknown_length_request_and_reports_the_protocol() {
    let server = Http3Server::start([
        ResponsePlan::chunks([Bytes::from_static(b"http3 response")]).trailers(HeaderMap::from_iter([(
            "x-trailer".parse().unwrap(),
            HeaderValue::from_static("value"),
        )])),
    ]);
    let test_client = client(
        &[Version::HTTP_3],
        WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        Clock::new_frozen(),
    );
    let body = test_client.body_builder.stream(futures::stream::iter([
        Ok(BytesView::copied_from_slice(b"streamed ", &test_client.body_builder)),
        Ok(BytesView::copied_from_slice(b"request", &test_client.body_builder)),
    ]));

    let response = futures::executor::block_on(test_client.client.post(server.url("/stream")).body(body).fetch()).unwrap();

    assert_eq!(response.version(), Version::HTTP_3);
    let (response_body, trailers) = futures::executor::block_on(collect_frames(response.into_body()));
    assert_eq!(response_body, b"http3 response");
    assert_eq!(trailers.unwrap().get("x-trailer").unwrap(), "value");
    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].body, Bytes::from_static(b"streamed request"));
    assert_eq!(snapshot.requests[0].version, Version::HTTP_3);
}

#[cfg_attr(miri, ignore)]
#[test]
fn required_http3_does_not_fall_back_when_quic_is_unavailable() {
    let tcp_only_server = TestServer::https([ResponsePlan::ok("must not fall back")], &["localhost"]);
    let test_client = client(
        &[Version::HTTP_3],
        WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
        Clock::new_frozen(),
    );

    let error = futures::executor::block_on(test_client.client.get(tcp_only_server.url("/http3-required")).fetch()).unwrap_err();
    assert!(
        error_chain_contains_win32_code(&error, &[12029, 12030]),
        "required HTTP/3 failed with an unexpected error: {error:?}"
    );

    assert!(tcp_only_server.finish().requests.is_empty());
}

fn error_chain_contains_win32_code(error: &(dyn Error + 'static), expected_codes: &[u32]) -> bool {
    let mut current = Some(error);

    while let Some(error) = current {
        if expected_codes
            .iter()
            .any(|code| error.to_string().contains(&format!("Win32 error {code}")))
        {
            return true;
        }
        current = error.source();
    }

    false
}
