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
//! The client is driven here without any async runtime installed, which is what
//! runtime-neutrality buys: the transport's I/O completes on WinHTTP's own worker threads.
//!
//! Run with `cargo run -p fetch_winhttp_testing --example quick_start`.

fn main() {
    #[cfg(windows)]
    example::run();

    #[cfg(not(windows))]
    println!("fetch_winhttp is a Windows-only transport, so this example does nothing here.");
}

#[cfg(windows)]
mod example {
    use bytesbuf::mem::GlobalPool;
    use fetch::HttpClient;
    use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps};
    use fetch_winhttp_testing::{ResponsePlan, TestServer};
    use http::Version;
    use observed::Sink;
    use tick::Clock;

    pub(super) fn run() {
        let server = TestServer::http([ResponsePlan::ok("hello from the fixture")]);

        // A frozen clock keeps the example free of wall-clock dependencies. A real caller
        // passes the clock its async runtime drives, such as `Clock::new_tokio()`.
        let deps = WinHttpDeps::builder(Clock::new_frozen(), GlobalPool::new(), Sink::noop()).build();

        let client = HttpClient::builder_winhttp(deps)
            // The fixture speaks plaintext, which production callers should not allow.
            .insecure_allow_http()
            .supported_http_versions(&[Version::HTTP_11])
            .minimal_pipeline()
            .build();

        let response = futures::executor::block_on(client.get(server.url("/hello")).fetch()).expect("the fixture answers every request");

        println!("negotiated version: {:?}", response.version());
        let body = futures::executor::block_on(response.into_body().into_text()).expect("the fixture sends a complete body");
        println!("body: {body}");

        let snapshot = server.finish();
        println!("requests served: {}", snapshot.requests.len());
    }
}
