// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests for enrichment: scoped context, stacking, unwinding, targeted enrichment,
//! enrichment isolation, typed enrichment structs, and enrichment field-level attributes.
//!
//! Covers DESIGN.md requirements:
//! - Scoped, automatic enrichment
//! - Flat composition (no nesting indentation explosion)
//! - Enrichment isolation for library emitters
//! - Per-sink targeted enrichment
//! - Typed enrichment structs via `#[derive(Enrichment)]`
//! - Enrichment field-level attributes (`dimension`, `unredacted`, `data_class`)

use std::sync::Arc;

use observed::enrichment::{EnrichFnExt, EnrichFutureExt};
use observed::{Enrichment, Sink, SinkId, emit};
use observed_testing::events::ProbeEvent;
use observed_testing::types::{PublicBool, PublicI64};
use observed_testing::{ExpectedEvent, TEST_ID, test_emitter};

#[derive(Debug, Enrichment)]
struct TenantContext {
    tenant: PublicI64,
}

#[derive(Debug, Enrichment)]
struct RequestContext {
    request_id: PublicI64,
    attempt: PublicI64,
    is_retry: PublicBool,
}

#[derive(Debug, Enrichment)]
struct ServiceContext {
    service: PublicI64,
}

#[test]
fn enrichment_appears_as_dimensions() {
    let (sink, processor) = test_emitter(TEST_ID);

    (|| {
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(&sink, TenantContext { tenant: PublicI64(42) })();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("tenant", "42")
            .dimension("value", "1")
            .log(),
    );
}

#[test]
fn enrichments_stack_and_unwind() {
    #[derive(Debug, Enrichment)]
    struct Inner {
        request_id: PublicI64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    (|| {
        // Inner scope: both enrichments visible
        (|| {
            emit!(sink, ProbeEvent::new(10));
        })
        .enrich(&sink, Inner { request_id: PublicI64(42) })();

        // Outer scope only: inner enrichment is gone
        emit!(sink, ProbeEvent::new(20));
    })
    .enrich(&sink, ServiceContext { service: PublicI64(1) })();

    let events = processor.events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("request_id", "42")
            .dimension("service", "1")
            .dimension("value", "10")
            .log(),
    );
    assert_eq!(
        events[1],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("service", "1")
            .dimension("value", "20")
            .log(),
    );
}

#[test]
fn multiple_enrichment_entries_in_single_call() {
    let (sink, processor) = test_emitter(TEST_ID);

    (|| {
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(
        &sink,
        RequestContext {
            request_id: PublicI64(7),
            attempt: PublicI64(1),
            is_retry: PublicBool(false),
        },
    )();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("attempt", "1")
            .dimension("is_retry", "false")
            .dimension("request_id", "7")
            .dimension("value", "1")
            .log(),
    );
}

#[test]
fn targeted_enrichment_visible_to_specific_emitter() {
    static APP: SinkId = SinkId::new("app_targeted");
    static LIB: SinkId = SinkId::new("lib_targeted");

    #[derive(Debug, Enrichment)]
    struct LibVersion {
        #[dimension(log = "lib.version")]
        version: PublicI64,
    }

    let (app_emitter, app_processor) = test_emitter(APP);
    let (lib_emitter, lib_processor) = test_emitter(LIB);

    (|| {
        emit!(app_emitter, ProbeEvent::new(1));
        emit!(lib_emitter, ProbeEvent::new(2));
    })
    .enrich_for(&lib_emitter, LIB, LibVersion { version: PublicI64(1) })();

    let app_events = app_processor.events();
    let lib_events = lib_processor.events();

    assert_eq!(app_events.len(), 1);
    assert_eq!(lib_events.len(), 1);

    // Targeted enrichment should appear on lib sink's events
    assert_eq!(
        lib_events[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("lib.version", "1")
            .dimension("value", "2")
            .log(),
    );

    // Targeted enrichment should NOT appear on app sink's events
    assert_eq!(
        app_events[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("value", "1")
            .log(),
    );
}

#[test]
fn isolated_enrichment_excludes_global_context() {
    let processor = observed_testing::MockProcessor::new();
    let sink = Sink::new_isolated("test", vec![Arc::new(processor.clone())], tick::SimpleClock::new_frozen());

    (|| {
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(&sink, TenantContext { tenant: PublicI64(99) })();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("value", "1")
            .log(),
    );
}

// ---------------------------------------------------------------------------
// Typed enrichment structs - #[derive(Enrichment)]
// ---------------------------------------------------------------------------

#[test]
fn typed_enrichment_struct_adds_dimensions() {
    let (sink, processor) = test_emitter(TEST_ID);

    let ctx = RequestContext {
        request_id: PublicI64(42),
        attempt: PublicI64(1),
        is_retry: PublicBool(false),
    };

    (|| {
        emit!(sink, ProbeEvent::new(100));
    })
    .enrich(&sink, ctx)();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("request_id", "42")
            .dimension("attempt", "1")
            .dimension("is_retry", "false")
            .dimension("value", "100")
            .log(),
    );
}

#[test]
fn typed_enrichment_stacking() {
    let (sink, processor) = test_emitter(TEST_ID);

    let ctx = RequestContext {
        request_id: PublicI64(7),
        attempt: PublicI64(2),
        is_retry: PublicBool(true),
    };

    (|| {
        (|| {
            emit!(sink, ProbeEvent::new(200));
        })
        .enrich(&sink, TenantContext { tenant: PublicI64(55) })();
    })
    .enrich(&sink, ctx)();

    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("attempt", "2")
            .dimension("is_retry", "true")
            .dimension("request_id", "7")
            .dimension("tenant", "55")
            .dimension("value", "200")
            .log(),
    );
}

// ---------------------------------------------------------------------------
// Enrichment field-level attributes
// ---------------------------------------------------------------------------

#[test]
fn enrichment_field_level_attributes() {
    #[derive(Debug, observed::Enrichment)]
    struct Ctx {
        #[dimension(log = "ctx.trace_id")]
        trace_id: PublicI64,
        #[dimension(log = exclude)]
        metrics_only_tag: PublicI64,
        logs_only_detail: PublicI64,
    }

    let (sink, processor) = test_emitter(TEST_ID);

    let ctx = Ctx {
        trace_id: PublicI64(999),
        metrics_only_tag: PublicI64(1),
        logs_only_detail: PublicI64(2),
    };

    (|| {
        emit!(sink, ProbeEvent::new(1));
    })
    .enrich(&sink, ctx)();

    // trace_id is renamed to "ctx.trace_id"; all fields appear as dimensions
    // (the exclude flags are routing hints consumed by processors, not dimension removal).
    assert_eq!(
        processor.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("ctx.trace_id", "999")
            .dimension("logs_only_detail", "2")
            .dimension("metrics_only_tag", "1")
            .dimension("value", "1")
            .log(),
    );
}

// ---------------------------------------------------------------------------
// Enrichment preserved across future polls
// ---------------------------------------------------------------------------

#[test]
fn enrichment_preserved_when_future_polled() {
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use testing_aids::FutureTestExt;

    #[derive(Debug, Clone, Enrichment)]
    struct Scope {
        scope: PublicI64,
    }

    /// A future that returns [`Poll::Pending`] for `remaining` polls,
    /// then emits a [`ProbeEvent`] and returns [`Poll::Ready`].
    struct EmitAfterPending {
        remaining: u32,
        sink: Sink,
        value: PublicI64,
    }

    impl Future for EmitAfterPending {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
            if self.remaining > 0 {
                self.remaining -= 1;
                Poll::Pending
            } else {
                emit!(self.sink, ProbeEvent { value: self.value });
                Poll::Ready(())
            }
        }
    }

    let (sink, processor) = test_emitter(TEST_ID);

    let mut fut1 = EmitAfterPending {
        remaining: 1,
        sink: sink.clone(),
        value: PublicI64(1),
    }
    .enrich(&sink, Scope { scope: PublicI64(1) });

    let mut fut2 = EmitAfterPending {
        remaining: 1,
        sink: sink.clone(),
        value: PublicI64(2),
    }
    .enrich(&sink, Scope { scope: PublicI64(2) });

    // First poll: both futures pend, no events emitted yet.
    assert!(testing_aids::poll_once(&mut fut1).is_pending());
    assert!(testing_aids::poll_once(&mut fut2).is_pending());
    assert!(processor.events().is_empty());

    // Second poll: both futures complete and emit with their enrichment.
    fut1.unwrap_ready();
    fut2.unwrap_ready();

    let events = processor.events();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("scope", "1")
            .dimension("value", "1")
            .log(),
    );
    assert_eq!(
        events[1],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("scope", "2")
            .dimension("value", "2")
            .log(),
    );
}

// ---------------------------------------------------------------------------
// Composite sink: enrichment broadcast
// ---------------------------------------------------------------------------

/// `.enrich(&composite, ...)` broadcasts the entry to every child's slot,
/// so records dispatched through each child carry the enrichment dimension.
#[test]
fn composite_enrich_appears_on_every_child_record() {
    static APP: observed::SinkId = observed::SinkId::new("composite_enrich_app");
    static AUDIT: observed::SinkId = observed::SinkId::new("composite_enrich_audit");

    let (app, app_proc) = observed_testing::test_emitter(APP);
    let (audit, audit_proc) = observed_testing::test_emitter(AUDIT);
    let composite = Sink::composite([app, audit]);

    (|| {
        emit!(composite, ProbeEvent::new(7));
    })
    .enrich(&composite, TenantContext { tenant: PublicI64(99) })();

    // Both children received the event with the enrichment dimension.
    assert_eq!(
        app_proc.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("tenant", "99")
            .dimension("value", "7")
            .log(),
    );
    assert_eq!(
        audit_proc.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("tenant", "99")
            .dimension("value", "7")
            .log(),
    );
}

/// Pushing an enrichment via the composite, then opening a nested scope on
/// one of its children, stacks correctly: the nested scope sees both the
/// outer (composite-broadcast) entry and the inner (child-only) entry on
/// records emitted through that child; records emitted through the other
/// child only see the outer entry. Both entries unwind correctly.
#[test]
fn composite_enrich_stacks_with_per_child_scope() {
    static APP: observed::SinkId = observed::SinkId::new("stack_app");
    static AUDIT: observed::SinkId = observed::SinkId::new("stack_audit");

    #[derive(Debug, Enrichment)]
    struct Inner {
        request_id: PublicI64,
    }

    let (app, app_proc) = observed_testing::test_emitter(APP);
    let (audit, audit_proc) = observed_testing::test_emitter(AUDIT);
    let composite = Sink::composite([app.clone(), audit.clone()]);

    (|| {
        // Outer scope: composite-broadcast entry visible on both children.
        (|| {
            // Nested scope: inner entry pushed only on `app`.
            emit!(app, ProbeEvent::new(100));
            emit!(audit, ProbeEvent::new(200));
        })
        .enrich(&app, Inner { request_id: PublicI64(42) })();

        // Outer scope, after inner unwinds: only the broadcast entry remains.
        emit!(composite, ProbeEvent::new(300));
    })
    .enrich(&composite, ServiceContext { service: PublicI64(1) })();

    let app_events = app_proc.events();
    let audit_events = audit_proc.events();
    assert_eq!(app_events.len(), 2);
    assert_eq!(audit_events.len(), 2);

    // app[0]: in nested scope — sees both outer (service) and inner (request_id).
    assert_eq!(
        app_events[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("request_id", "42")
            .dimension("service", "1")
            .dimension("value", "100")
            .log(),
    );
    // audit[0]: in nested scope, but inner was pushed only on app — sees only outer.
    assert_eq!(
        audit_events[0],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("service", "1")
            .dimension("value", "200")
            .log(),
    );
    // app[1]: outer scope only — only outer.
    assert_eq!(
        app_events[1],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("service", "1")
            .dimension("value", "300")
            .log(),
    );
    // audit[1]: outer scope only — only outer.
    assert_eq!(
        audit_events[1],
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("service", "1")
            .dimension("value", "300")
            .log(),
    );
}

/// `.enrich_for(&composite, ID, ...)` pushes a targeted entry onto every
/// child's slot, but the entry is only emitted from the child whose scope
/// matches `ID`. Other children's records do not carry it.
#[test]
fn composite_enrich_for_targets_one_child_only() {
    static APP: observed::SinkId = observed::SinkId::new("target_app");
    static AUDIT: observed::SinkId = observed::SinkId::new("target_audit");

    #[derive(Debug, Enrichment)]
    struct AuditCtx {
        audit_id: PublicI64,
    }

    let (app, app_proc) = observed_testing::test_emitter(APP);
    let (audit, audit_proc) = observed_testing::test_emitter(AUDIT);
    let composite = Sink::composite([app, audit]);

    (|| {
        emit!(composite, ProbeEvent::new(1));
    })
    .enrich_for(&composite, AUDIT, AuditCtx { audit_id: PublicI64(7) })();

    // App's record: enrichment was on app's slot, but the entry's target is
    // AUDIT, so app's `visit_enrichments` filters it out.
    assert_eq!(
        app_proc.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("value", "1")
            .log(),
    );
    // Audit's record: target matches its scope, so the entry is emitted.
    assert_eq!(
        audit_proc.single_event(),
        ExpectedEvent::new("test.probe", observed::Severity::Info)
            .dimension("audit_id", "7")
            .dimension("value", "1")
            .log(),
    );
}
