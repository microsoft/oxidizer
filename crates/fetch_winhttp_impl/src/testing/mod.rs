// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Localhost fixtures for exercising the transport out of process.
//!
//! `TestServer` serves plaintext or TLS traffic over TCP, negotiating HTTP/1.1
//! or HTTP/2; `Http3Server` serves HTTP/3 over QUIC. Both are scripted with
//! `ResponsePlan` values and observed through a `ServerSnapshot`. No fixture
//! depends on wall-clock time: a plan that must stay in flight stalls
//! indefinitely and is aborted at shutdown.
//!
//! The module is not part of any supported API. It is reached only through the
//! `private-test-util` feature, which `fetch_winhttp` turns on in its own
//! dev-dependency on this crate, so the integration tests, benchmarks and
//! examples that drive these fixtures can live beside the public API they
//! exercise. See
//! [`docs/private-test-utils.md`](https://github.com/microsoft/oxidizer/blob/main/docs/private-test-utils.md).

#![expect(
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::unwrap_used,
    reason = "fixture code: a failed assumption should abort the run, and its panics are not a contract"
)]

mod http3_server;
mod recording;
mod server;

use std::future::poll_fn;
use std::io::Read as _;
use std::pin::Pin;
use std::time::Duration;

use bytesbuf::mem::GlobalPool;
use fetch::{HttpBodyBuilder, HttpClient, HttpClientBuilder};
use http::{HeaderMap, Version};
use http_body::Body as _;
pub use http3_server::Http3Server;
use observed::Sink;
pub use recording::{RecordedRequest, ResponseFrame, ResponsePlan, ResponseScript, ServerSnapshot};
pub use server::TestServer;
use tick::Clock;

use crate::{HttpClientWinHttpExt as _, WinHttpDeps, WinHttpTlsConfig};

/// A client built over the WinHTTP transport, paired with its body builder.
///
/// The body builder is returned alongside the client because response body
/// options are applied per request rather than at client build time.
#[derive(Debug)]
pub struct TestClient {
    /// The client under test.
    pub client: HttpClient,
    /// Builds the response body options each request is issued with.
    pub body_builder: HttpBodyBuilder,
}

/// Builds a ready-to-use client over the WinHTTP transport.
///
/// An empty `versions` slice leaves protocol selection to the `fetch` default.
/// The connect timeout is unbounded so a fixture is never decided by wall-clock
/// time; callers supply the clock so tests can freeze it while examples drive a
/// real runtime clock.
pub fn client(versions: &[Version], tls: WinHttpTlsConfig, clock: Clock) -> TestClient {
    let (builder, body_builder) = client_builder(versions, tls, clock);

    TestClient {
        client: builder.build(),
        body_builder,
    }
}

/// Builds an unfinished client, for callers that need to configure it further.
///
/// Plain HTTP is allowed so a fixture can be reached without TLS.
pub fn client_builder(versions: &[Version], tls: WinHttpTlsConfig, clock: Clock) -> (HttpClientBuilder, HttpBodyBuilder) {
    let global_pool = GlobalPool::new();
    let body_builder = HttpBodyBuilder::new(global_pool.clone(), &clock);
    let deps = WinHttpDeps::builder(clock, global_pool, Sink::noop()).tls(tls).build();
    let mut builder = HttpClient::builder_winhttp(deps)
        .insecure_allow_http()
        .connect_timeout(Duration::MAX)
        .minimal_pipeline();
    if !versions.is_empty() {
        builder = builder.supported_http_versions(versions);
    }

    (builder, body_builder)
}

/// Drains a response body frame by frame, separating the data from the trailer section.
///
/// `HttpBody::into_bytes` and `into_text` discard trailers, so any test that asserts on a trailer
/// section has to poll the frames itself.
pub async fn collect_frames(mut body: fetch::HttpBody) -> (Vec<u8>, Option<HeaderMap>) {
    let mut data = Vec::new();
    let mut trailers = None;

    while let Some(frame) = poll_fn(|context| Pin::new(&mut body).poll_frame(context)).await {
        let frame = frame.unwrap();
        match frame.into_data() {
            Ok(mut bytes) => {
                bytes.read_to_end(&mut data).unwrap();
            }
            Err(frame) => trailers = Some(frame.into_trailers().unwrap()),
        }
    }

    (data, trailers)
}
