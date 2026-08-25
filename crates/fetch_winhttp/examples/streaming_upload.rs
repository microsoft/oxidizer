// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Streams a request body whose length is not known in advance, then shows the one framing
//! construct the transport refuses.
//!
//! WinHTTP frames an unknown-length upload itself, so a streamed body needs no
//! `Content-Length` and no caller-managed chunking. What WinHTTP offers no API for is
//! submitting a request *trailer* section, so a body that yields a trailer frame fails the
//! request outright rather than dropping the trailers silently. Callers that would otherwise
//! send request trailers must carry that data in headers instead.
//!
//! Run with `cargo run -p fetch_winhttp --example streaming_upload`.

fn main() {
    #[cfg(windows)]
    example::run();

    #[cfg(not(windows))]
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use bytesbuf::BytesView;
    use fetch::HttpError;
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{ResponsePlan, TestServer, client};
    use http::header::CONTENT_LENGTH;
    use http::{HeaderMap, HeaderName, HeaderValue, Version};
    use http_body::Frame;
    use http_body_util::StreamBody;
    use http_extensions::HttpBodyOptions;

    pub(super) fn run() {
        stream_an_unknown_length_body();
        reject_request_trailers();
    }

    fn stream_an_unknown_length_body() {
        let server = TestServer::http([ResponsePlan::ok("upload received")]);
        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());

        // A stream carries no length, so neither the caller nor `fetch` declares one.
        let body = test_client.body_builder.stream(
            futures::stream::iter([
                Ok(BytesView::copied_from_slice(b"streamed ", &test_client.body_builder)),
                Ok(BytesView::copied_from_slice(b"request", &test_client.body_builder)),
            ]),
            &HttpBodyOptions::default(),
        );

        let response = futures::executor::block_on(test_client.client.post(server.url("/upload")).body(body).fetch_text_body())
            .expect("the fixture accepts the upload");

        println!("response: {response}");
        let snapshot = server.finish();
        // WinHTTP chunked the upload; the peer reassembled the whole body.
        println!("body observed on the wire: {:?}", snapshot.requests[0].body);
        println!("Content-Length sent: {:?}", snapshot.requests[0].headers.get(CONTENT_LENGTH));
    }

    fn reject_request_trailers() {
        let server = TestServer::http([]);
        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
        let body = StreamBody::new(futures::stream::iter([
            Ok::<_, HttpError>(Frame::data(BytesView::copied_from_slice(b"data", &test_client.body_builder))),
            Ok(Frame::trailers(HeaderMap::from_iter([(
                HeaderName::from_static("x-request-trailer"),
                HeaderValue::from_static("value"),
            )]))),
        ]));
        let body = test_client.body_builder.body(body, &HttpBodyOptions::default());

        let error = futures::executor::block_on(test_client.client.post(server.url("/trailers")).body(body).fetch())
            .expect_err("WinHTTP cannot submit request trailers");

        println!("request trailers are refused: {error}");
        drop(server.finish());
    }
}
