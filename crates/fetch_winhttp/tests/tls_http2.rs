// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(windows)]

//! TLS and HTTP/2 localhost integration tests for the `WinHTTP` transport.

mod common;

use std::future::poll_fn;
use std::io::Read as _;
use std::pin::Pin;

use bytes::Bytes;
use bytesbuf::BytesView;
use common::{ResponsePlan, TestServer, client};
use fetch_winhttp::WinHttpTlsConfig;
use http::{HeaderMap, HeaderValue, Version};
use http_body::Body as _;
use http_extensions::HttpBodyOptions;

#[cfg_attr(miri, ignore)]
#[test]
fn tls_validation_relaxations_are_independent() {
    let valid_name = TestServer::https([ResponsePlan::ok("valid name accepted")], &["localhost"]);
    let strict = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    futures::executor::block_on(strict.client.get(valid_name.url("/strict")).fetch())
        .expect_err("a self-signed certificate is rejected by default");
    let invalid_certs = client(&[Version::HTTP_11], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());
    let response = futures::executor::block_on(invalid_certs.client.get(valid_name.url("/accepted")).fetch_text_body())
        .expect("relaxing certificate validation accepts a valid host name");
    assert_eq!(response, "valid name accepted");

    let wrong_name = TestServer::https([ResponsePlan::ok("both accepted")], &["different.invalid"]);
    let invalid_certs = client(&[Version::HTTP_11], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());
    futures::executor::block_on(invalid_certs.client.get(wrong_name.url("/certificate-only")).fetch())
        .expect_err("certificate relaxation does not disable host-name validation");
    let invalid_hostnames = client(
        &[Version::HTTP_11],
        WinHttpTlsConfig::builder().accept_invalid_hostnames(true).build(),
    );
    futures::executor::block_on(invalid_hostnames.client.get(wrong_name.url("/hostname-only")).fetch())
        .expect_err("host-name relaxation does not trust a self-signed certificate");
    let both = client(
        &[Version::HTTP_11],
        WinHttpTlsConfig::builder()
            .accept_invalid_certs(true)
            .accept_invalid_hostnames(true)
            .build(),
    );
    let response = futures::executor::block_on(both.client.get(wrong_name.url("/both")).fetch_text_body())
        .expect("both independent faults are accepted only when both relaxations are enabled");
    assert_eq!(response, "both accepted");
}

#[cfg_attr(miri, ignore)]
#[test]
fn http2_is_negotiated_and_reported() {
    let server = TestServer::https([ResponsePlan::ok("http2")], &["localhost"]);
    let test_client = client(&[Version::HTTP_2], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());

    let response = futures::executor::block_on(test_client.client.get(server.url("/http2")).fetch()).expect("HTTP/2 request succeeds");

    assert_eq!(response.version(), Version::HTTP_2);
    assert_eq!(
        futures::executor::block_on(response.into_body().into_text()).expect("HTTP/2 response body reads"),
        "http2"
    );
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
                "x-trailer".parse().expect("valid name"),
                HeaderValue::from_static("value"),
            )])),
        ],
        &["localhost"],
    );
    let test_client = client(&[Version::HTTP_2], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());
    let body = test_client.body_builder.stream(
        futures::stream::iter([
            Ok(BytesView::copied_from_slice(b"streamed ", &test_client.body_builder)),
            Ok(BytesView::copied_from_slice(b"request", &test_client.body_builder)),
        ]),
        &HttpBodyOptions::default(),
    );

    let response = futures::executor::block_on(test_client.client.post(server.url("/stream")).body(body).fetch())
        .expect("HTTP/2 streaming request succeeds");
    let (response_body, trailers) = futures::executor::block_on(collect_frames(response.into_body()));

    assert_eq!(response_body, b"response");
    assert_eq!(
        trailers
            .expect("HTTP/2 trailers are present")
            .get("x-trailer")
            .expect("expected trailer"),
        "value"
    );
    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].body, Bytes::from_static(b"streamed request"));
    assert_eq!(snapshot.requests[0].version, Version::HTTP_2);
}

async fn collect_frames(mut body: fetch::HttpBody) -> (Vec<u8>, Option<HeaderMap>) {
    let mut data = Vec::new();
    let mut trailers = None;

    while let Some(frame) = poll_fn(|context| Pin::new(&mut body).poll_frame(context)).await {
        let frame = frame.expect("response body frame succeeds");
        match frame.into_data() {
            Ok(mut bytes) => {
                bytes.read_to_end(&mut data).expect("response body bytes are readable");
            }
            Err(frame) => trailers = Some(frame.into_trailers().expect("the non-data frame contains trailers")),
        }
    }

    (data, trailers)
}
