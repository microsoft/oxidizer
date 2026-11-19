// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds an HTTP client on the WinHTTP transport and issues one request.
//!
//! Import `HttpClientWinHttpExt` to select this transport instead of
//! `HttpClient::builder`. The constructor requires a clock, memory provider,
//! and telemetry sink. The transport does not select these values for the
//! caller.
//!
//! This example runs on Tokio and supplies `Clock::new_tokio()`. The transport
//! does not depend on Tokio. WinHTTP completes I/O on its own worker threads.
//!
//! Run with `cargo run -p fetch_winhttp --example quick_start`.

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
    use bytesbuf::mem::GlobalPool;
    use fetch::HttpClient;
    use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps};
    use fetch_winhttp_impl::testing::{ResponsePlan, TestServer};
    use http::Version;
    use observed::Sink;
    use tick::Clock;

    pub(super) async fn run() -> Result<(), ohno::AppError> {
        let server = TestServer::http([ResponsePlan::ok("hello from the fixture")]);

        let deps = WinHttpDeps::builder(Clock::new_tokio(), GlobalPool::new(), Sink::noop()).build();

        let client = HttpClient::builder_winhttp(deps)
            // The fixture uses plaintext. Production clients must use TLS.
            .insecure_allow_http()
            .supported_http_versions(&[Version::HTTP_11])
            .minimal_pipeline()
            .build();

        let response = client.get(server.url("/hello")).fetch().await?;

        println!("negotiated version: {:?}", response.version());
        let body = response.into_body().into_text().await?;
        println!("body: {body}");

        let snapshot = server.finish();
        println!("requests served: {}", snapshot.requests.len());
        Ok(())
    }
}
