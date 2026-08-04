// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    dead_code,
    reason = "each integration-test binary compiles this shared module independently and sees helpers used only by sibling binaries; every item here is exercised by at least one binary"
)]
#![allow(unused_imports, reason = "shared integration-test helpers are used by different test binaries")]

mod http3_server;
mod recording;
mod server;

use std::future::poll_fn;
use std::io::Read as _;
use std::pin::Pin;
use std::time::Duration;

use bytesbuf::mem::GlobalPool;
use fetch::{HttpBodyBuilder, HttpClient, HttpClientBuilder};
use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps, WinHttpTlsConfig};
use http::{HeaderMap, Version};
use http_body::Body as _;
pub(crate) use http3_server::Http3Server;
use observed::Sink;
pub(crate) use recording::ResponsePlan;
pub(crate) use server::TestServer;
use tick::Clock;

pub(crate) struct TestClient {
    pub(crate) client: HttpClient,
    pub(crate) body_builder: HttpBodyBuilder,
}

pub(crate) fn client(versions: &[Version], tls: WinHttpTlsConfig) -> TestClient {
    let (builder, body_builder) = client_builder(versions, tls);

    TestClient {
        client: builder.build(),
        body_builder,
    }
}

pub(crate) fn client_builder(versions: &[Version], tls: WinHttpTlsConfig) -> (HttpClientBuilder, HttpBodyBuilder) {
    let clock = Clock::new_frozen();
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
pub(crate) async fn collect_frames(mut body: fetch::HttpBody) -> (Vec<u8>, Option<HeaderMap>) {
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
