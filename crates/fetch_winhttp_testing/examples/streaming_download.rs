// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumes a response body frame by frame and reads the trailer section that follows it.
//!
//! Response bodies are pulled, not pushed: the transport issues a WinHTTP read only when the
//! consumer polls for the next frame, so a slow consumer applies backpressure without any
//! buffering in between. Abandoning the body mid-stream cancels the request.
//!
//! The transport also preserves a response trailer section as a final frame instead of
//! discarding it. Trailers are an HTTP/2 and HTTP/3 feature as WinHTTP exposes them, so this
//! example uses an HTTP/2 fixture over TLS.
//!
//! Run with `cargo run -p fetch_winhttp_testing --example streaming_download`.

fn main() {
    #[cfg(windows)]
    example::run();

    #[cfg(not(windows))]
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use std::future::Future as _;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_testing::{ResponsePlan, TestServer, client, collect_frames};
    use futures::TryStreamExt as _;
    use http::{HeaderMap, HeaderValue, Version};

    pub(super) fn run() {
        read_frames_and_trailers();
        abandon_a_download();
    }

    fn read_frames_and_trailers() {
        let trailers = HeaderMap::from_iter([(http::HeaderName::from_static("x-checksum"), HeaderValue::from_static("d41d8cd9"))]);
        let server = TestServer::https(
            [ResponsePlan::chunks([Bytes::from_static(b"first "), Bytes::from_static(b"second")]).trailers(trailers)],
            &["localhost"],
        );
        // The fixture presents a self-signed certificate, so certificate validation is relaxed.
        let test_client = client(&[Version::HTTP_2], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());

        let response =
            futures::executor::block_on(test_client.client.get(server.url("/download")).fetch()).expect("the fixture answers the request");
        let (body, trailers) = futures::executor::block_on(collect_frames(response.into_body()));

        println!("body: {}", String::from_utf8_lossy(&body));
        println!("trailers: {trailers:?}");
        drop(server.finish());
    }

    fn abandon_a_download() {
        // The fixture sends one chunk and then leaves the response in flight forever. Nothing
        // here waits for it: the consumer polls once, sees `Pending`, and drops the stream.
        let server = TestServer::http([
            ResponsePlan::chunks([Bytes::from_static(b"partial")]).stall_after_frames(),
            ResponsePlan::ok("the connection pool is still usable"),
        ]);
        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());

        let response =
            futures::executor::block_on(test_client.client.get(server.url("/stall")).fetch()).expect("the fixture answers the request");
        let mut stream = response.into_body().into_stream();
        let first = futures::executor::block_on(stream.try_next())
            .expect("the first chunk arrives")
            .expect("the stream is not yet finished");
        println!("first chunk: {} bytes", first.len());

        let mut pending = Box::pin(stream.try_next());
        let waker = futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        let still_waiting = matches!(pending.as_mut().poll(&mut context), Poll::Pending);
        println!("second chunk still outstanding: {still_waiting}");

        // Dropping the body cancels the WinHTTP request and releases its handles.
        drop(pending);
        drop(stream);

        let after = futures::executor::block_on(test_client.client.get(server.url("/after")).fetch_text_body())
            .expect("cancellation does not poison the client");
        println!("after cancellation: {after}");
        drop(server.finish());
    }
}
