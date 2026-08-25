// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Issues a request over HTTP/3.
//!
//! Protocol selection is explicit. `supported_http_versions` lists what the client may
//! negotiate, and listing HTTP/3 alone makes it mandatory: WinHTTP is asked for QUIC and the
//! request fails if QUIC cannot be established, rather than quietly falling back to TCP. That
//! failure is what a caller who requires HTTP/3 wants to see, so the example provokes it too
//! by pointing the same client at a TCP-only fixture.
//!
//! Run with `cargo run -p fetch_winhttp --example http3`.

fn main() {
    #[cfg(windows)]
    example::run();

    #[cfg(not(windows))]
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use bytes::Bytes;
    use fetch_winhttp::WinHttpTlsConfig;
    use fetch_winhttp_impl::testing::{Http3Server, ResponsePlan, TestServer, client, collect_frames};
    use http::{HeaderMap, HeaderName, HeaderValue, Version};

    pub(super) fn run() {
        succeed_over_quic();
        fail_without_quic();
    }

    fn succeed_over_quic() {
        let trailers = HeaderMap::from_iter([(HeaderName::from_static("x-served-by"), HeaderValue::from_static("quic"))]);
        let server = Http3Server::start([ResponsePlan::chunks([Bytes::from_static(b"http3 response")]).trailers(trailers)]);
        // The QUIC fixture presents a self-signed certificate.
        let test_client = client(&[Version::HTTP_3], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());

        let response = futures::executor::block_on(test_client.client.get(server.url("/h3")).fetch()).expect("the QUIC fixture answers");

        println!("negotiated version: {:?}", response.version());
        let (body, trailers) = futures::executor::block_on(collect_frames(response.into_body()));
        println!("body: {}", String::from_utf8_lossy(&body));
        println!("trailers: {trailers:?}");
        drop(server.finish());
    }

    fn fail_without_quic() {
        let server = TestServer::https([ResponsePlan::ok("never reached")], &["localhost"]);
        let test_client = client(&[Version::HTTP_3], WinHttpTlsConfig::builder().accept_invalid_certs(true).build());

        let error = futures::executor::block_on(test_client.client.get(server.url("/h3")).fetch())
            .expect_err("a TCP-only peer cannot satisfy a required HTTP/3 request");

        println!("required HTTP/3 does not fall back: {error}");
        drop(server.finish());
    }
}
