// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg(windows)]

//! HTTP/3 localhost integration tests for the `WinHTTP` transport.

mod common;

use std::error::Error;
use std::future::poll_fn;
use std::io::Read as _;
use std::pin::Pin;

use bytes::Bytes;
use bytesbuf::BytesView;
use common::{Http3Server, ResponsePlan, TestServer, client};
use fetch_winhttp::WinHttpTlsConfig;
use http::{HeaderMap, Version};
use http_body::Body as _;
use http_extensions::HttpBodyOptions;

#[cfg_attr(miri, ignore)]
#[test]
fn http3_streams_an_unknown_length_request_and_reports_the_protocol() {
    let server = Http3Server::start([Bytes::from_static(b"http3 response")]);
    let test_client = client(&[Version::HTTP_3], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());
    let body = test_client.body_builder.stream(
        futures::stream::iter([
            Ok(BytesView::copied_from_slice(b"streamed ", &test_client.body_builder)),
            Ok(BytesView::copied_from_slice(b"request", &test_client.body_builder)),
        ]),
        &HttpBodyOptions::default(),
    );

    let response = futures::executor::block_on(test_client.client.post(server.url("/stream")).body(body).fetch())
        .expect("HTTP/3 streaming request succeeds");

    assert_eq!(response.version(), Version::HTTP_3);
    let (response_body, trailers) = futures::executor::block_on(collect_frames(response.into_body()));
    assert_eq!(response_body, b"http3 response");
    assert_eq!(
        trailers
            .expect("HTTP/3 trailers are present")
            .get("x-trailer")
            .expect("expected trailer"),
        "value"
    );
    let snapshot = server.finish();
    assert_eq!(snapshot.requests.len(), 1);
    assert_eq!(snapshot.requests[0].body, Bytes::from_static(b"streamed request"));
    assert_eq!(snapshot.requests[0].version, Version::HTTP_3);
}

#[cfg_attr(miri, ignore)]
#[test]
fn required_http3_does_not_fall_back_when_quic_is_unavailable() {
    let tcp_only_server = TestServer::https([ResponsePlan::ok("must not fall back")], &["localhost"]);
    let test_client = client(&[Version::HTTP_3], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());

    let error = futures::executor::block_on(test_client.client.get(tcp_only_server.url("/http3-required")).fetch())
        .expect_err("required HTTP/3 fails instead of falling back to the TCP server");
    assert!(
        error_chain_contains_win32_code(&error, &[12029, 12030]),
        "required HTTP/3 failed with an unexpected error: {error:?}"
    );

    assert!(tcp_only_server.finish().requests.is_empty());
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
