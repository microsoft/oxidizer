// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Builds an HTTP client on the WinHTTP transport and issues one request.
//!
//! Two things differ from the usual `fetch` client setup. The transport is selected through
//! the `HttpClientWinHttpExt` extension trait instead of `HttpClient::builder`, and its
//! environment dependencies - a clock, a memory pool, and a telemetry sink - are required
//! constructor arguments. Nothing defaults, so the transport never picks a clock or an
//! allocator on the caller's behalf.
//!
//! The example runs on Tokio. The transport itself is runtime-neutral - its I/O completes on
//! WinHTTP's own worker threads - but a real application still supplies the clock its runtime
//! drives, here via `Clock::new_tokio()`.
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
            // The fixture speaks plaintext, which production callers should not allow.
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
