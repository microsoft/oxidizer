// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumes a response body frame by frame and reads the trailer section that follows it.
//!
//! The transport reads from WinHTTP only when the consumer polls for the next
//! frame. A slow consumer applies backpressure without extra buffering.
//! Dropping the body during the stream cancels the request.
//!
//! The transport returns response trailers as the final frame. WinHTTP exposes
//! trailers for HTTP/2 and HTTP/3, so this example uses HTTP/2 over TLS.
//!
//! Run with `cargo run -p fetch_winhttp --example streaming_download`.

#[cfg(windows)]
#[tokio::main]
async fn main() -> Result<(), ohno::AppError> {
    example::run().await
}

#[cfg(not(windows))]
fn main() {
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use std::future::Future as _;
    use std::task::{Context, Poll};

    use bytes::Bytes;
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{ResponsePlan, TestServer, client, collect_frames};
    use futures::TryStreamExt as _;
    use http::{HeaderMap, HeaderValue, Version};
    use tick::Clock;

    pub(super) async fn run() -> Result<(), ohno::AppError> {
        read_frames_and_trailers().await?;
        abandon_a_download().await?;
        Ok(())
    }

    async fn read_frames_and_trailers() -> Result<(), ohno::AppError> {
        let trailers = HeaderMap::from_iter([(http::HeaderName::from_static("x-checksum"), HeaderValue::from_static("d41d8cd9"))]);
        let server = TestServer::https(
            [ResponsePlan::chunks([Bytes::from_static(b"first "), Bytes::from_static(b"second")]).trailers(trailers)],
            &["localhost"],
        );
        // The fixture has a self-signed certificate, so the client accepts it.
        let test_client = client(
            &[Version::HTTP_2],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
            Clock::new_tokio(),
        );

        let response = test_client.client.get(server.url("/download")).fetch().await?;
        let (body, trailers) = collect_frames(response.into_body()).await;

        println!("body: {}", String::from_utf8_lossy(&body));
        println!("trailers: {trailers:?}");
        drop(server.finish());
        Ok(())
    }

    async fn abandon_a_download() -> Result<(), ohno::AppError> {
        // The fixture sends one chunk and then stalls. The consumer sees
        // `Pending` on the next poll and drops the stream.
        let server = TestServer::http([
            ResponsePlan::chunks([Bytes::from_static(b"partial")]).stall_after_frames(),
            ResponsePlan::ok("the connection pool is still usable"),
        ]);
        let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_tokio());

        let response = test_client.client.get(server.url("/stall")).fetch().await?;
        let mut stream = response.into_body().into_stream();
        let Some(first) = stream.try_next().await? else {
            ohno::bail!("the stream ended before the first chunk");
        };
        println!("first chunk: {} bytes", first.len());

        {
            let mut pending = Box::pin(stream.try_next());
            let waker = futures::task::noop_waker();
            let mut context = Context::from_waker(&waker);
            let still_waiting = matches!(pending.as_mut().poll(&mut context), Poll::Pending);
            println!("second chunk still outstanding: {still_waiting}");
            // Dropping the body cancels the WinHTTP request and releases its
            // handles. Keep the poll state in this block because `Context` is
            // not `Send`. The next request runs on the Tokio runtime.
        }
        drop(stream);

        let after = test_client.client.get(server.url("/after")).fetch_text_body().await?;
        println!("after cancellation: {after}");
        drop(server.finish());
        Ok(())
    }
}
