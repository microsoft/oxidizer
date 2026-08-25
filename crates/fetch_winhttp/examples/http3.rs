// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Issues a request over HTTP/3.
//!
//! Protocol selection is explicit. `supported_http_versions` lists what the client may
//! negotiate, and listing HTTP/3 alone makes it mandatory: the request fails if HTTP/3
//! cannot be established, rather than quietly falling back to HTTP/1.1 or HTTP/2. That
//! failure is what a caller who requires HTTP/3 wants to see, so the example provokes it
//! too by pointing the same client at a fixture that only speaks HTTP over TCP.
//!
//! Run with `cargo run -p fetch_winhttp --example http3`.

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
    use bytes::Bytes;
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{Http3Server, ResponsePlan, TestServer, client, collect_frames};
    use http::{HeaderMap, HeaderName, HeaderValue, Version};
    use tick::Clock;

    pub(super) async fn run() -> Result<(), ohno::AppError> {
        succeed_over_http3().await?;
        fail_without_http3().await?;
        Ok(())
    }

    async fn succeed_over_http3() -> Result<(), ohno::AppError> {
        let trailers = HeaderMap::from_iter([(HeaderName::from_static("x-served-by"), HeaderValue::from_static("http3"))]);
        let server = Http3Server::start([ResponsePlan::chunks([Bytes::from_static(b"http3 response")]).trailers(trailers)]);
        // The HTTP/3 fixture presents a self-signed certificate.
        let test_client = client(
            &[Version::HTTP_3],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
            Clock::new_tokio(),
        );

        let response = test_client.client.get(server.url("/h3")).fetch().await?;

        println!("negotiated version: {:?}", response.version());
        let (body, trailers) = collect_frames(response.into_body()).await;
        println!("body: {}", String::from_utf8_lossy(&body));
        println!("trailers: {trailers:?}");
        drop(server.finish());
        Ok(())
    }

    async fn fail_without_http3() -> Result<(), ohno::AppError> {
        let server = TestServer::https([ResponsePlan::ok("never reached")], &["localhost"]);
        let test_client = client(
            &[Version::HTTP_3],
            WinHttpTlsConfig::builder().accept_invalid_certs(true).build(),
            Clock::new_tokio(),
        );

        let Err(error) = test_client.client.get(server.url("/h3")).fetch().await else {
            ohno::bail!("a peer without HTTP/3 should not satisfy a required HTTP/3 request");
        };

        println!("required HTTP/3 does not fall back: {error}");
        drop(server.finish());
        Ok(())
    }
}
