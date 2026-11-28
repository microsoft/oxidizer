// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests that the `observed` sink of the deps reaches the request pipelines and the transports of a client.

#![allow(clippy::unwrap_used, reason = "test code")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use bytesbuf::mem::{GlobalPool, OpaqueMemory};
use fetch::custom::{CustomContext, CustomDeps, Isolation, create_builder};
use fetch::fake::{FakeDeps, FakeHandler};
use fetch::{HttpClient, HttpClientBuilder, HttpRequest, HttpResponseBuilder};
use futures::executor::block_on;
use http::StatusCode;
use layered::{Intercept, Stack};
use observed::{Severity, Sink, SinkId, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::{ExpectedEvent, test_emitter};
use thread_aware::{ThreadAware, ThreadBuilder};
use tick::ClockControl;

const URI: &str = "http://example.com";

/// The number of other threads that each send one request.
const THREADS: usize = 4;

#[test]
fn configured_sink_reaches_custom_pipeline() {
    let (sink, processor) = test_emitter(SinkId::new("configured"));
    let (client, _builds) = probe_client(HttpClient::builder_fake(StatusCode::OK, FakeDeps::new(sink)));

    block_on(client.get(URI).fetch()).unwrap();
    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(processor.events(), probes(2));
}

#[test]
fn configured_sink_reaches_custom_transport() {
    let (sink, processor) = test_emitter(SinkId::new("transport"));
    let deps = CustomDeps {
        clock: ClockControl::new().to_clock(),
        memory: OpaqueMemory::new(GlobalPool::new()),
        sink,
        extras: (),
    };
    let client = create_builder(
        "test-runtime",
        "test",
        |context: CustomContext| {
            let sink = context.sink;
            FakeHandler::from_fn(move |_| {
                emit!(sink, ProbeEvent::new(1));
                HttpResponseBuilder::new_fake().status(StatusCode::OK).build()
            })
        },
        Isolation::Shared,
        deps,
    )
    .insecure_allow_http()
    .minimal_pipeline()
    .build();

    block_on(client.get(URI).fetch()).unwrap();
    block_on(client.get(URI).fetch()).unwrap();

    assert_eq!(processor.events(), probes(2));
}

#[test]
fn fake_deps_with_clock_keeps_clock_and_sink() {
    let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let deps = FakeDeps::with_clock(&ClockControl::new_at(time).to_clock(), Sink::builder("with_clock").build());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_in_pipeline = Arc::clone(&seen);

    let _client = HttpClient::builder_fake(StatusCode::OK, deps)
        .custom_pipeline(move |dispatch, context| {
            seen_in_pipeline
                .lock()
                .unwrap()
                .push((context.clock().system_time(), context.sink().id()));
            dispatch
        })
        .build();

    assert_eq!(*seen.lock().unwrap(), [(time, SinkId::new("with_clock"))]);
}

#[test]
fn isolated_pipeline_keeps_supplied_sink() {
    let (sink, processor) = test_emitter(SinkId::new("isolated"));
    let deps = CustomDeps {
        clock: ClockControl::new().to_clock(),
        memory: OpaqueMemory::new(GlobalPool::new()),
        sink,
        extras: (),
    };
    let builder = create_builder(
        "test-runtime",
        "test",
        |_: CustomContext| FakeHandler::from(StatusCode::OK),
        Isolation::Isolated,
        deps,
    );
    let (client, builds) = probe_client(builder);

    send_from_other_threads(&client);

    // The client builds one pipeline on this thread and one on each other thread.
    assert_eq!(builds.load(Ordering::Relaxed), THREADS + 1);
    assert_eq!(processor.events(), probes(THREADS));
}

#[test]
fn shared_pipeline_keeps_supplied_sink() {
    let (sink, processor) = test_emitter(SinkId::new("shared"));
    let (client, builds) = probe_client(HttpClient::builder_fake(StatusCode::OK, FakeDeps::new(sink)));

    send_from_other_threads(&client);

    assert_eq!(builds.load(Ordering::Relaxed), 1);
    assert_eq!(processor.events(), probes(THREADS));
}

/// Builds a client whose pipeline emits one probe through the sink of its context for each request.
///
/// The counter tells how many pipelines the client built.
fn probe_client(builder: HttpClientBuilder) -> (HttpClient, Arc<AtomicUsize>) {
    let builds = Arc::new(AtomicUsize::new(0));
    let builds_in_pipeline = Arc::clone(&builds);
    let client = builder
        .insecure_allow_http()
        .custom_pipeline(move |dispatch, context| {
            builds_in_pipeline.fetch_add(1, Ordering::Relaxed);
            let sink = context.sink().clone();
            (
                Intercept::layer().on_input(move |_: &HttpRequest| emit!(sink, ProbeEvent::new(1))),
                dispatch,
            )
                .into_service()
        })
        .build();
    (client, builds)
}

/// Moves a clone of the client to each of [`THREADS`] other threads and sends one request from each.
fn send_from_other_threads(client: &HttpClient) {
    let builder = ThreadBuilder::default();
    let handles = (0..THREADS)
        .map(|_| {
            let mut client = client.clone();
            let builder = builder.clone();
            std::thread::spawn(move || {
                client.relocate(None, &builder.build(std::thread::current().id()));
                block_on(client.get(URI).fetch()).unwrap();
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().unwrap();
    }
}

fn probes(count: usize) -> Vec<ExpectedEvent> {
    (0..count)
        .map(|_| ExpectedEvent::new("test.probe", Severity::Info).dimension("value", "1"))
        .collect()
}

testing_aids::init_tracing!();
