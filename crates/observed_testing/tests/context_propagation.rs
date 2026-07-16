// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for cross-thread context transfer and async enrichment propagation.
//!
//! Covers DESIGN.md requirements:
//! - Cross-thread context transfer (`sink.transfer_context()`)
//! - Async enrichment propagation (`.enrich().attach()`)
//! - Enrichments visible on spawned threads/tasks

use observed::enrichment::{EnrichFnExt, EnrichFutureExt};
use observed::{Enrichment, Severity, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::types::PublicI64;
use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};

#[derive(Debug, Clone, Enrichment)]
struct OriginContext {
    origin: PublicI64,
}

#[derive(Debug, Clone, Enrichment)]
struct AsyncContext {
    async_key: PublicI64,
}

// ---- Tests ----

#[test]
fn cross_thread_context_transfer() {
    let (sink, processor) = test_emitter(TEST_ID);

    (|| {
        let transfer = sink.transfer_context();
        let sink = sink.clone();
        let handle = std::thread::spawn(move || {
            let _guard = transfer.apply();
            emit!(sink, ProbeEvent::new(99));
        });
        handle.join().unwrap();
    })
    .enrich(&sink, OriginContext { origin: PublicI64(1) })();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("origin", "1")
            .dimension("value", "99")
            .log(),
    );
}

#[test]
fn context_transfer_does_not_affect_source_thread() {
    let (sink, processor) = test_emitter(TEST_ID);

    (|| {
        let _transfer = sink.transfer_context();
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(&sink, OriginContext { origin: PublicI64(42) })();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("origin", "42")
            .dimension("value", "1")
            .log(),
    );
}

#[cfg_attr(
    miri,
    ignore = "unsupported operation: can't call foreign function `CreateIoCompletionPort` on OS `windows`"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_enrichment_propagation() {
    let (sink, processor) = test_emitter(TEST_ID);

    let emitter_inner = sink.clone();
    let enriched = async move {
        emit!(emitter_inner, ProbeEvent::new(42));
    }
    .enrich(&sink, AsyncContext { async_key: PublicI64(7) });

    enriched.await;

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("async_key", "7")
            .dimension("value", "42")
            .log(),
    );
}

#[cfg_attr(
    miri,
    ignore = "unsupported operation: can't call foreign function `CreateIoCompletionPort` on OS `windows`"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_enrichment_with_context_transfer() {
    let (sink, processor) = test_emitter(TEST_ID);

    let transfer = sink.transfer_context();
    let emitter_inner = sink.clone();

    let enriched = async move {
        emit!(emitter_inner, ProbeEvent::new(7));
    }
    .enrich(&sink, AsyncContext { async_key: PublicI64(99) })
    .attach(transfer);

    tokio::spawn(enriched).await.unwrap();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("async_key", "99")
            .dimension("value", "7")
            .log(),
    );
}
