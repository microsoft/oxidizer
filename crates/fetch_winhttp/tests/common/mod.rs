// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    dead_code,
    reason = "each integration-test binary compiles this shared module independently and sees helpers used only by sibling binaries"
)]
#![allow(unused_imports, reason = "shared integration-test helpers are used by different test binaries")]

mod http3_server;
mod server;

use std::time::Duration;

use bytesbuf::mem::GlobalPool;
use fetch::{HttpBodyBuilder, HttpClient, HttpClientBuilder};
use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps, WinHttpTlsConfig};
use http::Version;
pub(crate) use http3_server::Http3Server;
use observed::Sink;
pub(crate) use server::{ResponsePlan, TestServer};
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
