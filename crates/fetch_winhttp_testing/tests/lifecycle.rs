// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Client, connection and response-body lifetimes over localhost.
//!
//! Covers connection pool ownership and reuse, what happens when a caller abandons an in-flight
//! download, and that a client assembled through the full `fetch` pipeline dispatches through this
//! transport.

#![cfg(windows)]

// The standard pipeline exercised here emits `tracing` events through `fetch`'s logging
// handler, and integration binaries link the library with `cfg(test)` false, so no crate-root
// initialization runs. Install it directly. See docs/tracing-tests.md.
testing_aids::init_tracing!();

use std::future::Future as _;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use bytesbuf::BytesView;
use bytesbuf::mem::GlobalPool;
use fetch::HttpClient;
use fetch::options::{ConnectionPoolOptions, PoolSelection};
use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps, WinHttpTlsConfig};
use fetch_winhttp_testing::{ResponsePlan, TestServer, client, client_builder};
use futures::TryStreamExt as _;
use http::Version;
use observed::Sink;
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[test]
fn clients_reuse_only_their_own_connection_pools() {
    let shared_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let shared = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let shared_clone = shared.client.clone();
    futures::executor::block_on(shared.client.get(shared_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(shared_clone.get(shared_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(shared_server.finish().connections, 1);

    let isolated_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let first = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let second = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    futures::executor::block_on(first.client.get(isolated_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(second.client.get(isolated_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(isolated_server.finish().connections, 2);

    let builder_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let first = builder.clone().build();
    let second = builder.build();
    futures::executor::block_on(first.get(builder_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(second.get(builder_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(builder_server.finish().connections, 2);

    let pools_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two"), ResponsePlan::ok("three")]);
    let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let client = builder
        .connection_pool_options(ConnectionPoolOptions::default().multiple_pools(2, PoolSelection::round_robin()))
        .build();
    for path in ["/one", "/two", "/three"] {
        futures::executor::block_on(client.get(pools_server.url(path)).fetch_text_body()).unwrap();
    }
    assert_eq!(pools_server.finish().connections, 2);
}

#[cfg_attr(miri, ignore)]
#[test]
fn extreme_idle_windows_are_accepted_and_still_pool() {
    // The idle window is applied through an option WinHTTP rejects outright for
    // an out-of-range value, and a rejected session option permanently fails the
    // transport. Only a real platform can show that the two extremes of the
    // generic option survive that check; mock-based tests establish which value
    // the transport sends, not whether Windows takes it. Reusing a connection
    // additionally shows the option did not disable pooling as a side effect.
    for idle_timeout in [None, Some(Duration::ZERO)] {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
        let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default());
        let client = builder
            .connection_pool_options(ConnectionPoolOptions::default().connection_idle_timeout(idle_timeout))
            .build();

        for path in ["/one", "/two"] {
            futures::executor::block_on(client.get(server.url(path)).fetch_text_body()).unwrap();
        }

        assert_eq!(server.finish().connections, 1);
    }
}

#[cfg_attr(miri, ignore)]
#[test]
fn dropping_a_pending_download_cancels_it_without_blocking_later_requests() {
    let server = TestServer::http([
        ResponsePlan::chunks([Bytes::from_static(b"first")]).stall_after_frames(),
        ResponsePlan::ok("after cancellation"),
    ]);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());
    let response = futures::executor::block_on(test_client.client.get(server.url("/stall")).fetch()).unwrap();
    let mut stream = response.into_body().into_stream();
    let first = futures::executor::block_on(stream.try_next()).unwrap().unwrap();
    assert_eq!(first, BytesView::copied_from_slice(b"first", &test_client.body_builder));

    let mut pending = Box::pin(stream.try_next());
    let waker = futures::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(matches!(pending.as_mut().poll(&mut context), Poll::Pending));
    drop(pending);
    drop(stream);

    let response = futures::executor::block_on(test_client.client.get(server.url("/after-cancellation")).fetch_text_body()).unwrap();
    assert_eq!(response, "after cancellation");
    assert_eq!(server.finish().connections, 2);
}

#[cfg_attr(miri, ignore)]
#[test]
fn mixed_completion_cancellation_and_body_drops_remain_usable() {
    let mut responses = Vec::new();
    for index in 0..18 {
        responses.push(match index % 3 {
            0 => ResponsePlan::ok(format!("complete {index}")),
            1 => ResponsePlan::chunks([Bytes::from_static(b"partial")]).stall_after_frames(),
            _ => ResponsePlan::ok("dropped"),
        });
    }
    responses.push(ResponsePlan::ok("final"));
    let server = TestServer::http(responses);
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default());

    for index in 0..18 {
        let response = futures::executor::block_on(test_client.client.get(server.url(&format!("/soak/{index}"))).fetch()).unwrap();
        match index % 3 {
            0 => {
                let body = futures::executor::block_on(response.into_body().into_text()).unwrap();
                assert_eq!(body, format!("complete {index}"));
            }
            1 => {
                let mut stream = response.into_body().into_stream();
                let first = futures::executor::block_on(stream.try_next()).unwrap().unwrap();
                assert_eq!(first, BytesView::copied_from_slice(b"partial", &test_client.body_builder));
                let mut pending = Box::pin(stream.try_next());
                let waker = futures::task::noop_waker();
                let mut context = Context::from_waker(&waker);
                assert!(matches!(pending.as_mut().poll(&mut context), Poll::Pending));
                drop(pending);
                drop(stream);
            }
            _ => drop(response),
        }
    }

    let final_response = futures::executor::block_on(test_client.client.get(server.url("/final")).fetch_text_body()).unwrap();
    assert_eq!(final_response, "final");
    assert_eq!(server.finish().requests.len(), 19);
}

#[cfg_attr(miri, ignore)]
#[test]
fn full_fetch_pipeline_uses_the_winhttp_transport() {
    let server = TestServer::http([ResponsePlan::ok("full pipeline")]);
    let clock = Clock::new_frozen();
    let global_pool = GlobalPool::new();
    let deps = WinHttpDeps::builder(clock, global_pool, Sink::noop()).build();
    let client = HttpClient::builder_winhttp(deps)
        .insecure_allow_http()
        .supported_http_versions(&[Version::HTTP_11])
        // The standard pipeline's retry layer awaits `Clock::delay` for its backoff between
        // attempts. This test drives a frozen clock, which never advances on its own and which no
        // test logic advances, so a single retried attempt would block forever: the two
        // `HttpTimeout` layers and the transport's connect deadline all run on that same frozen
        // clock, and `WinHTTP`'s native timers are deliberately unlimited (see
        // crates/fetch_winhttp/docs/design.md, "Timeouts and time"). A transient localhost failure
        // would therefore turn a test failure into an indefinite hang instead of an assertion.
        // Capping the retry budget at zero leaves every standard layer in the stack - including
        // the retry layer and its recovery classification - while making it structurally
        // impossible for a backoff delay to be awaited, because the retry loop breaks on the first
        // attempt.
        .standard_pipeline(|pipeline, _context| pipeline.retry(|retry| retry.max_retry_attempts(0)))
        .build();

    let response = futures::executor::block_on(client.get(server.url("/full-pipeline")).fetch_text_body()).unwrap();

    assert_eq!(response, "full pipeline");
    assert_eq!(server.finish().requests.len(), 1);
}
