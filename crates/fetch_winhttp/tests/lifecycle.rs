// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests client, connection, and response-body lifetimes over localhost.
//!
//! Covers connection pool ownership and reuse, an abandoned download, and
//! dispatch through the full `fetch` pipeline.

#![cfg(windows)]

use std::future::Future as _;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use bytesbuf::BytesView;
use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch::HttpClient;
use fetch::options::{ConnectionPoolOptions, PoolSelection};
use fetch_winhttp::{HttpClientWinHttpExt as _, WinHttpDeps, WinHttpTlsConfig};
use fetch_winhttp_impl::testing::{ResponsePlan, TestServer, client, client_builder};
use futures::TryStreamExt as _;
use http::Version;
use observed::Sink;
use tick::Clock;

#[cfg_attr(miri, ignore)]
#[test]
fn clients_reuse_only_their_own_connection_pools() {
    let shared_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let shared = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
    let shared_clone = shared.client.clone();
    futures::executor::block_on(shared.client.get(shared_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(shared_clone.get(shared_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(shared_server.finish().connections, 1);

    let isolated_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let first = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
    let second = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
    futures::executor::block_on(first.client.get(isolated_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(second.client.get(isolated_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(isolated_server.finish().connections, 2);

    let builder_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
    let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
    let first = builder.clone().build();
    let second = builder.build();
    futures::executor::block_on(first.get(builder_server.url("/one")).fetch_text_body()).unwrap();
    futures::executor::block_on(second.get(builder_server.url("/two")).fetch_text_body()).unwrap();
    assert_eq!(builder_server.finish().connections, 2);

    let pools_server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two"), ResponsePlan::ok("three")]);
    let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
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
    // WinHTTP rejects an out-of-range idle window and permanently fails the
    // transport. Only a live Windows test can show that both extreme values are
    // accepted. Mock tests only show the value that the transport sends.
    // Connection reuse also shows that the configuration keeps pooling enabled.
    for idle_timeout in [None, Some(Duration::ZERO)] {
        let server = TestServer::http([ResponsePlan::ok("one"), ResponsePlan::ok("two")]);
        let (builder, _body_builder) = client_builder(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
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
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());
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
    let test_client = client(&[Version::HTTP_11], WinHttpTlsConfig::default(), Clock::new_frozen());

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
    let memory = OpaqueMemory::new(GlobalPool::new());
    let deps = WinHttpDeps::builder(clock, memory, Sink::noop()).build();
    let client = HttpClient::builder_winhttp(deps)
        .insecure_allow_http()
        .supported_http_versions(&[Version::HTTP_11])
        // The retry layer uses `Clock::delay` for backoff. This test uses a
        // frozen clock that no test logic advances. A retry waits forever
        // because both `HttpTimeout` layers and the connect deadline use that
        // clock. WinHTTP native timers are unlimited. Set the retry budget to
        // zero so all standard layers remain present but no backoff can start.
        .standard_pipeline(|pipeline, _context| pipeline.retry(|retry| retry.max_retry_attempts(0)))
        .build();

    let response = futures::executor::block_on(client.get(server.url("/full-pipeline")).fetch_text_body()).unwrap();

    assert_eq!(response, "full pipeline");
    assert_eq!(server.finish().requests.len(), 1);
}
