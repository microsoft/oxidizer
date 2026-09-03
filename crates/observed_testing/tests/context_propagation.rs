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
            let _guard = transfer.apply_current_thread();
            emit!(sink, ProbeEvent::new(99));
        });
        handle.join().expect("spawned thread does not panic");
    })
    .enrich(&sink, OriginContext { origin: PublicI64(1) })();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("origin", "1")
            .dimension("value", "99"),
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
            .dimension("value", "1"),
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
            .dimension("value", "42"),
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

    tokio::spawn(enriched).await.expect("spawned task does not panic");

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("async_key", "99")
            .dimension("value", "7"),
    );
}

/// The reverse chaining order is equivalent: `Transferred::enrich` re-orders the
/// wrappers so the transfer is restored first and the entries land on top of it.
#[cfg_attr(
    miri,
    ignore = "unsupported operation: can't call foreign function `CreateIoCompletionPort` on OS `windows`"
)]
#[tokio::test]
async fn async_enrichment_after_attach_stays_visible() {
    let (sink, processor) = test_emitter(TEST_ID);

    let transfer = sink.transfer_context();
    let emitter_inner = sink.clone();

    let enriched = async move {
        emit!(emitter_inner, ProbeEvent::new(7));
    }
    .attach(transfer)
    .enrich(&sink, AsyncContext { async_key: PublicI64(99) });

    tokio::spawn(enriched).await.expect("spawned task does not panic");

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("async_key", "99")
            .dimension("value", "7"),
    );
}

/// Targeted enrichment chained after `attach` is likewise preserved, and still
/// only reaches the sink it addresses.
#[cfg_attr(
    miri,
    ignore = "unsupported operation: can't call foreign function `CreateIoCompletionPort` on OS `windows`"
)]
#[tokio::test]
async fn targeted_async_enrichment_after_attach_stays_visible() {
    static APP: observed::SinkId = observed::SinkId::new("attach_target_app");
    static AUDIT: observed::SinkId = observed::SinkId::new("attach_target_audit");

    let (app, app_proc) = test_emitter(APP);
    let (audit, audit_proc) = test_emitter(AUDIT);
    let composite = observed::Sink::composite([app, audit]);

    let transfer = composite.transfer_context();
    let emitter_inner = composite.clone();

    let enriched = async move {
        emit!(emitter_inner, ProbeEvent::new(7));
    }
    .attach(transfer)
    .enrich_for(&composite, AUDIT, AsyncContext { async_key: PublicI64(99) });

    tokio::spawn(enriched).await.expect("spawned task does not panic");

    // Only the addressed sink emits the targeted entry.
    assert_eq!(
        app_proc.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info).dimension("value", "7"),
    );
    assert_eq!(
        audit_proc.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("async_key", "99")
            .dimension("value", "7"),
    );
}

/// `Transfer::with_enrichment_for` is the order-independent escape hatch for
/// *targeted* entries: it survives an `attach` the inherent method never sees,
/// and - unlike `with_enrichment`, which can only produce global entries - it
/// keeps the entry off every sink it is not addressed to.
#[cfg_attr(
    miri,
    ignore = "unsupported operation: can't call foreign function `CreateIoCompletionPort` on OS `windows`"
)]
#[tokio::test]
async fn targeted_enrichment_carried_by_transfer_survives_generic_attach() {
    static APP: observed::SinkId = observed::SinkId::new("transfer_target_app");
    static AUDIT: observed::SinkId = observed::SinkId::new("transfer_target_audit");

    // Stands in for a generic helper that only knows `F: EnrichFutureExt` and so
    // resolves `attach` through the trait.
    fn spawn_attached<F>(future: F, transfer: observed::context::Transfer) -> tokio::task::JoinHandle<F::Output>
    where
        F: EnrichFutureExt + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(future.attach(transfer))
    }

    let (app, app_proc) = test_emitter(APP);
    let (audit, audit_proc) = test_emitter(AUDIT);
    let composite = observed::Sink::composite([app, audit]);

    let transfer = composite
        .transfer_context()
        .with_enrichment_for(AUDIT, AsyncContext { async_key: PublicI64(99) });
    let emitter_inner = composite.clone();

    spawn_attached(
        async move {
            emit!(emitter_inner, ProbeEvent::new(7));
        },
        transfer,
    )
    .await
    .expect("spawned task does not panic");

    // The target is preserved through the transfer: `with_enrichment` would
    // have widened this entry onto `APP` as well.
    assert_eq!(
        app_proc.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info).dimension("value", "7"),
    );
    assert_eq!(
        audit_proc.single_event(),
        ExpectedEvent::new("test.probe", Severity::Info)
            .dimension("async_key", "99")
            .dimension("value", "7"),
    );
}
