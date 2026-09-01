// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for the `observed` crate's hot paths.
//!
//! Measures:
//! - Full emit pipeline (event -> log record via `OTel` provider)
//! - Enrichment resolution (context lookup + Vec building)
//! - Metric dimension building
//! - Sink operations (construction, emit through a no-op sink)
//!
//! Run with:
//! ```sh
//! cargo bench -p observed --features test-util
//! ```

use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};
use std::time::Instant;

use alloc_tracker::Allocator;
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use data_privacy::{DataClass, Sensitive};
use observed::__private::EnrichmentEntry;
use observed::enrichment::{EnrichFnExt, EnrichFutureExt};
use observed::{Enrichment, Key, Sink, Value, event};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::SdkLoggerProvider;

#[path = "../examples/support/otel.rs"]
mod otel;

const BENCH_DC: DataClass = DataClass::new("bench", "public");

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

#[event("bench.simple_log")]
#[info]
struct SimpleLogEvent {
    #[data_class(BENCH_DC)]
    status: i64,
    #[data_class(BENCH_DC)]
    retries: i64,
}

#[event("bench.many_fields")]
#[info]
#[expect(clippy::struct_field_names, reason = "benchmark helper struct, field_ prefix is intentional")]
struct ManyFieldsEvent {
    #[data_class(BENCH_DC)]
    field_a: i64,
    #[data_class(BENCH_DC)]
    field_b: i64,
    #[data_class(BENCH_DC)]
    field_c: i64,
    #[data_class(BENCH_DC)]
    field_d: i64,
    #[data_class(BENCH_DC)]
    field_e: i64,
    #[data_class(BENCH_DC)]
    field_f: i64,
    #[data_class(BENCH_DC)]
    field_g: i64,
    #[data_class(BENCH_DC)]
    field_h: i64,
}

#[event("bench.with_metric")]
#[info]
#[histogram(duration_ms, name = "bench_duration")]
struct MetricEvent {
    // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
    #[unredacted]
    duration_ms: f64,
    #[data_class(BENCH_DC)]
    region: i64,
    #[data_class(BENCH_DC)]
    service: i64,
}

#[event("bench.body_event")]
#[info("Request completed")]
struct BodyEvent {
    #[data_class(BENCH_DC)]
    code: i64,
}

/// An event whose fields go through the redaction path and produce strings.
///
/// Every other event here carries only `i64` fields, whose `Value` needs no
/// string at all. This one is the shape that pays for redaction on every emit.
#[event("bench.redacted_strings")]
#[info]
struct RedactedStringsEvent {
    #[data_class(BENCH_DC)]
    request_id: &'static str,
    #[data_class(BENCH_DC)]
    user_agent: &'static str,
}

// ---------------------------------------------------------------------------
// Enrichment types for benchmarks
// ---------------------------------------------------------------------------

#[derive(Enrichment)]
struct BenchEnrich1 {
    #[data_class(BENCH_DC)]
    val: i64,
}

#[derive(Enrichment)]
struct BenchEnrich3 {
    #[data_class(BENCH_DC)]
    service: i64,
    #[data_class(BENCH_DC)]
    region: i64,
    #[data_class(BENCH_DC)]
    env: i64,
}

#[derive(Enrichment)]
struct BenchEnrich5 {
    #[data_class(BENCH_DC)]
    service: i64,
    #[data_class(BENCH_DC)]
    region: i64,
    #[data_class(BENCH_DC)]
    env: i64,
    #[data_class(BENCH_DC)]
    cluster: i64,
    #[data_class(BENCH_DC)]
    pod: i64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A simple processor that forwards events to an `OTel` logger provider.
struct SimpleLogProcessor {
    logger: opentelemetry_sdk::logs::SdkLogger,
    redaction_engine: data_privacy::RedactionEngine,
}

impl observed::processing::EventProcessor for SimpleLogProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        use opentelemetry::logs::Logger;

        let mut record = self.logger.create_log_record();
        if otel::populate_log_record(&mut record, event, &self.redaction_engine) {
            self.logger.emit(record);
        }
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
}

/// A redaction engine that actually rewrites the value.
///
/// The default engine erases, so its output is always empty and hides the cost
/// of moving redacted bytes around. `Replace('*')` produces an output the same
/// length as the input, which is what a real deployment pays for.
fn replacing_engine() -> data_privacy::RedactionEngine {
    data_privacy::RedactionEngine::builder()
        .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::new())
        .build()
}

fn make_log_processor() -> (SimpleLogProcessor, SdkLoggerProvider) {
    make_log_processor_with(data_privacy::RedactionEngine::default())
}

fn make_log_processor_with(redaction_engine: data_privacy::RedactionEngine) -> (SimpleLogProcessor, SdkLoggerProvider) {
    // Use a no-op log exporter to avoid retaining millions of `LogRecord`s
    // during high-iteration benchmarks (the default `InMemoryLogExporter`
    // accumulates every emitted record and OOMs on agents with limited RAM).
    let logger_provider = SdkLoggerProvider::builder().with_simple_exporter(NoOpLogExporter).build();

    let logger = logger_provider.logger("bench");

    let processor = SimpleLogProcessor { logger, redaction_engine };

    (processor, logger_provider)
}

/// No-op log exporter for benchmarks.
///
/// Discards every batch immediately so that benches measuring the
/// `emit -> OTel` pipeline don't accumulate `LogRecord`s in memory across
/// millions of iterations.
#[derive(Debug)]
struct NoOpLogExporter;

impl opentelemetry_sdk::logs::LogExporter for NoOpLogExporter {
    fn export(
        &self,
        _batch: opentelemetry_sdk::logs::LogBatch<'_>,
    ) -> impl Future<Output = opentelemetry_sdk::error::OTelSdkResult> + Send {
        std::future::ready(Ok(()))
    }

    fn shutdown_with_timeout(&self, _timeout: std::time::Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }
}

/// No-op log exporter for benchmarks.
///
/// Helper to run a benchmark with `alloc_tracker` and `all_the_time` tracking.
fn bench_with_tracking(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
    name: &str,
    mut body: impl FnMut(),
) {
    let allocs_op = allocs.operation(name);
    let time_op = time.operation(name);
    group.bench_function(name, |b| {
        b.iter_custom(|iters| {
            let _alloc = allocs_op.measure_thread().iterations(iters);
            let _clock = time_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                body();
            }

            start.elapsed()
        });
    });
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

fn entrypoint(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();
    let time = all_the_time::Session::new();

    // --- Emit pipeline benchmarks ---
    {
        let mut group = c.benchmark_group("emit_pipeline");
        bench_emit_simple_log(&mut group, &allocs, &time);
        bench_emit_many_fields(&mut group, &allocs, &time);
        bench_emit_with_body(&mut group, &allocs, &time);
        bench_emit_log_of_metric_event(&mut group, &allocs, &time);
        bench_emit_redacted_strings(&mut group, &allocs, &time);
        group.finish();
    }

    // --- Enrichment benchmarks ---
    {
        let mut group = c.benchmark_group("emit_enrichment");
        bench_emit_with_enrichments(&mut group, &allocs, &time);
        bench_emit_deeply_nested_enrichments(&mut group, &allocs, &time);
        bench_enrich_push_pop(&mut group, &allocs, &time);
        group.finish();
    }

    // --- Sink operation benchmarks ---
    {
        let mut group = c.benchmark_group("sink_operations");
        bench_construct_processor_free_sink(&mut group, &allocs, &time);
        bench_emit_to_noop_sink(&mut group, &allocs, &time);
        group.finish();
    }

    // --- Allocation-focused benchmarks (infinity_pool exploration) ---
    {
        let mut group = c.benchmark_group("emit_alloc");
        bench_enrichment_vec_collect(&mut group, &allocs, &time);
        bench_enrichment_vec_collect_deep(&mut group, &allocs, &time);
        bench_enrichment_entry_clone(&mut group, &allocs, &time);
        bench_arc_enrichment_node_churn(&mut group, &allocs, &time);
        bench_pending_enriched_future_polls(&mut group, &allocs, &time);
        bench_key_value_creation(&mut group, &allocs, &time);
        bench_redacted_value_creation(&mut group, &allocs, &time);
        bench_emit_varying_enrichment_depth(&mut group, &allocs, &time);
        group.finish();
    }

    // Both sessions are measured for every benchmark, so report both when
    // dropped: the allocation table is the point of the `emit_alloc` group and
    // of the enrichment benchmarks generally.
}

// ---------------------------------------------------------------------------
// Pipeline benchmarks
// ---------------------------------------------------------------------------

fn bench_emit_simple_log(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "simple_log_2_fields";
    let (processor, _provider) = make_log_processor();
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    bench_with_tracking(group, allocs, time, ID, || {
        observed::emit!(&sink, SimpleLogEvent { status: 200, retries: 0 });
    });
}

fn bench_emit_many_fields(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "log_8_fields";
    let (processor, _provider) = make_log_processor();
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    bench_with_tracking(group, allocs, time, ID, || {
        observed::emit!(
            &sink,
            ManyFieldsEvent {
                field_a: 1,
                field_b: 2,
                field_c: 3,
                field_d: 4,
                field_e: 5,
                field_f: 6,
                field_g: 7,
                field_h: 8,
            }
        );
    });
}

fn bench_emit_with_body(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "log_with_body";
    let (processor, _provider) = make_log_processor();
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    bench_with_tracking(group, allocs, time, ID, || {
        observed::emit!(&sink, BodyEvent { code: 42 });
    });
}

/// Benchmarks emitting an event that declares **both** a log and a metric
/// signal, through a log-only processor.
///
/// This is deliberately not a log-plus-metric measurement: no processor here
/// owns a meter or records an instrument, so what it captures is the cost of
/// log mapping for an event whose descriptor also carries metric metadata -
/// the field visit yields metric descriptors the processor then skips.
/// Use it as a full-pipeline log baseline for that event shape rather than as
/// an isolated metric-descriptor comparison.
fn bench_emit_log_of_metric_event(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "log_of_metric_event";
    let (processor, _provider) = make_log_processor();
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    bench_with_tracking(group, allocs, time, ID, || {
        observed::emit!(
            &sink,
            MetricEvent {
                duration_ms: 42.5,
                region: 1,
                service: 2,
            }
        );
    });
}

// ---------------------------------------------------------------------------
// Enrichment benchmarks
// ---------------------------------------------------------------------------

/// Benchmarks emitting an event whose fields are classified strings.
///
/// This is a full emit-pipeline case in which each field is rendered through
/// the redaction engine and stored as a string `Value`.
fn bench_emit_redacted_strings(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "log_2_redacted_strings";
    let (processor, _provider) = make_log_processor_with(replacing_engine());
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    bench_with_tracking(group, allocs, time, ID, || {
        observed::emit!(
            &sink,
            RedactedStringsEvent {
                request_id: "6f1c2f10-6a5b-4f0a-9c1f-2f106a5b4f0a",
                user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
            }
        );
    });
}

fn bench_emit_with_enrichments(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "emit_with_3_enrichments";
    let (processor, _provider) = make_log_processor();
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    // Establish an enrichment context, then benchmark emission within it.
    (|| {
        bench_with_tracking(group, allocs, time, ID, || {
            observed::emit!(&sink, SimpleLogEvent { status: 200, retries: 0 });
        });
    })
    .enrich(
        &sink,
        BenchEnrich3 {
            service: 1,
            region: 2,
            env: 3,
        },
    )();
}

fn bench_emit_deeply_nested_enrichments(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
) {
    const ID: &str = "emit_with_10_nested_enrichments";
    let (processor, _provider) = make_log_processor();
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    // Create 10 nested enrichment levels to stress the Arc-linked list resolution.
    (|| {
        bench_with_tracking(group, allocs, time, ID, || {
            observed::emit!(&sink, SimpleLogEvent { status: 200, retries: 0 });
        });
    })
    .enrich(&sink, BenchEnrich1 { val: 9 })
    .enrich(&sink, BenchEnrich1 { val: 8 })
    .enrich(&sink, BenchEnrich1 { val: 7 })
    .enrich(&sink, BenchEnrich1 { val: 6 })
    .enrich(&sink, BenchEnrich1 { val: 5 })
    .enrich(&sink, BenchEnrich1 { val: 4 })
    .enrich(&sink, BenchEnrich1 { val: 3 })
    .enrich(&sink, BenchEnrich1 { val: 2 })
    .enrich(&sink, BenchEnrich1 { val: 1 })
    .enrich(&sink, BenchEnrich1 { val: 0 })();
}

fn bench_enrich_push_pop(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "enrich_push_pop_3_entries";
    let noop = Sink::noop();

    bench_with_tracking(group, allocs, time, ID, || {
        (|| {
            black_box(());
        })
        .enrich(
            &noop,
            BenchEnrich3 {
                service: 1,
                region: 2,
                env: 3,
            },
        )();
    });
}

// ---------------------------------------------------------------------------
// Sink operation benchmarks
// ---------------------------------------------------------------------------

fn bench_construct_processor_free_sink(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
) {
    const ID: &str = "construct_processor_free_sink";
    const EMPTY_PROCESSORS: Vec<Arc<dyn observed::processing::EventProcessor>> = Vec::new();

    bench_with_tracking(group, allocs, time, ID, || {
        let sink = Sink::new("bench", EMPTY_PROCESSORS, tick::SimpleClock::new_frozen());
        drop(sink);
    });
}

fn bench_emit_to_noop_sink(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "emit_to_noop_sink";

    // Benchmark the emit path through an explicit no-op sink.
    let noop = Sink::noop();
    bench_with_tracking(group, allocs, time, ID, || {
        observed::emit!(&noop, SimpleLogEvent { status: 200, retries: 0 });
    });
}

// ===========================================================================
// Allocation-focused benchmarks for infinity_pool exploration
//
// These benchmarks isolate the allocation-heavy operations in the emit hot
// path to establish a baseline before exploring pooled alternatives:
//
// 1. Vec<EnrichmentEntry> collection (resolve_enrichments / to_vec)
// 2. Arc<EnrichmentNode> creation in enrich() scope push/pop
// 3. Key/Value object creation and cloning
// 4. Scaling behavior with enrichment depth
// ===========================================================================

/// Benchmarks the allocation cost of collecting enrichments into a Vec
/// (the `resolve_enrichments` path) with 3 enrichments in a flat scope.
fn bench_enrichment_vec_collect(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "enrichment_vec_collect_3";
    let (processor, _provider) = make_log_processor();
    let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

    // Single enrich scope with 3 entries - measures the Vec allocation
    // and EnrichmentEntry cloning that happens on every emit.
    (|| {
        // Only read enrichments, don't go through the full emit pipeline.
        bench_with_tracking(group, allocs, time, ID, || {
            let entries = sink.current_enrichments();
            black_box(entries);
        });
    })
    .enrich(
        &sink,
        BenchEnrich3 {
            service: 1,
            region: 2,
            env: 3,
        },
    )();
}

/// Benchmarks Vec<EnrichmentEntry> collection with 10 entries across 10
/// nested scopes - stresses the Arc-linked-list traversal + Vec reallocation.
fn bench_enrichment_vec_collect_deep(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
) {
    const ID: &str = "enrichment_vec_collect_10_nested";
    let noop = Sink::noop();

    // Build 10 nested enrichment scopes (1 entry each) then collect.
    (|| {
        bench_with_tracking(group, allocs, time, ID, || {
            let entries = noop.current_enrichments();
            black_box(entries);
        });
    })
    .enrich(&noop, BenchEnrich1 { val: 9 })
    .enrich(&noop, BenchEnrich1 { val: 8 })
    .enrich(&noop, BenchEnrich1 { val: 7 })
    .enrich(&noop, BenchEnrich1 { val: 6 })
    .enrich(&noop, BenchEnrich1 { val: 5 })
    .enrich(&noop, BenchEnrich1 { val: 4 })
    .enrich(&noop, BenchEnrich1 { val: 3 })
    .enrich(&noop, BenchEnrich1 { val: 2 })
    .enrich(&noop, BenchEnrich1 { val: 1 })
    .enrich(&noop, BenchEnrich1 { val: 0 })();
}

/// Benchmarks the cost of cloning `EnrichmentEntry` objects (Key + Value
/// cloning), which happens on every enrichment resolution.
fn bench_enrichment_entry_clone(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "enrichment_entry_clone_5";

    let entries: Vec<observed::__private::EnrichmentEntry> = vec![
        EnrichmentEntry::new("service", Sensitive::new("api-gateway", BENCH_DC)),
        EnrichmentEntry::new("region", Sensitive::new("us-west-2", BENCH_DC)),
        EnrichmentEntry::new("env", Sensitive::new("production", BENCH_DC)),
        EnrichmentEntry::new("tenant", Sensitive::new("contoso", BENCH_DC)),
        EnrichmentEntry::new("version", Sensitive::new("1.2.3", BENCH_DC)),
    ];

    bench_with_tracking(group, allocs, time, ID, || {
        let cloned: Vec<observed::__private::EnrichmentEntry> = entries.clone();
        black_box(cloned);
    });
}

/// Benchmarks the Arc<EnrichmentNode> creation that happens in `enrich()`
/// scope push - this is the allocation that `infinity_pool` could pool.
///
/// Measures repeated push/pop of a single enrichment scope (1 entry).
fn bench_arc_enrichment_node_churn(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
) {
    const ID: &str = "arc_enrichment_node_churn_1_entry";
    const NAME_5: &str = "arc_enrichment_node_churn_5_entries";
    let noop = Sink::noop();

    bench_with_tracking(group, allocs, time, ID, || {
        (|| {
            black_box(());
        })
        .enrich(&noop, BenchEnrich1 { val: 42 })();
    });

    bench_with_tracking(group, allocs, time, NAME_5, || {
        (|| {
            black_box(());
        })
        .enrich(
            &noop,
            BenchEnrich5 {
                service: 1,
                region: 2,
                env: 3,
                cluster: 4,
                pod: 5,
            },
        )();
    });
}

/// Benchmarks the per-poll enrichment cost of a future that stays **pending**.
///
/// `Enriched::poll` pushes the entries on every poll and drops the guard before
/// returning, and each push allocates a fresh `Arc<EnrichmentNode>`. A composite
/// broadcasts that push to every leaf, so a future polled `N` times through `M`
/// children allocates `N x M` nodes - and it pays that even while pending, having
/// emitted no telemetry at all. A frequently awakened request future therefore
/// carries an allocator cost purely for context propagation.
///
/// The single/composite pair is what isolates the fan-out multiplier: compare
/// the two to see whether the per-poll or the per-leaf term dominates before
/// deciding that node reuse or preallocation is worth its complexity.
///
/// Distinct from `arc_enrichment_node_churn_*`, which measures one synchronous
/// push/pop of an immediately invoked closure rather than repeated polling.
fn bench_pending_enriched_future_polls(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
) {
    /// How many times each iteration polls the pending future.
    const POLLS: u32 = 8;
    /// Leaves in the composite case.
    const COMPOSITE_CHILDREN: usize = 3;
    /// A composite leaf needs no processors: the measurement is the enrichment
    /// push/pop, not dispatch.
    const EMPTY_PROCESSORS: Vec<Arc<dyn observed::processing::EventProcessor>> = Vec::new();

    const ID_SINGLE: &str = "pending_enriched_future_8_polls_single";
    const ID_COMPOSITE: &str = "pending_enriched_future_8_polls_composite_3";

    /// A future that reports `Pending` for its whole measured lifetime, so each
    /// poll exercises only the enrichment push/pop the wrapper adds.
    struct AlwaysPending;

    impl Future for AlwaysPending {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    /// Polls `future` `POLLS` times without a runtime, so the measurement covers
    /// the wrapper rather than an executor.
    fn poll_pending(future: impl Future<Output = ()>) {
        let waker = task::Waker::noop();
        let mut cx = task::Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        for _ in 0..POLLS {
            // The future never completes, so the result is discarded on purpose:
            // what is being measured is the enrichment work each poll performs.
            let _pending = black_box(future.as_mut().poll(&mut cx));
        }
    }

    let single = Sink::noop();
    bench_with_tracking(group, allocs, time, ID_SINGLE, || {
        poll_pending(AlwaysPending.enrich(&single, BenchEnrich1 { val: 42 }));
    });

    let children: Vec<Sink> = (0..COMPOSITE_CHILDREN)
        .map(|_| Sink::new("bench", EMPTY_PROCESSORS, tick::SimpleClock::new_frozen()))
        .collect();
    let composite = Sink::composite(children);
    bench_with_tracking(group, allocs, time, ID_COMPOSITE, || {
        poll_pending(AlwaysPending.enrich(&composite, BenchEnrich1 { val: 42 }));
    });
}

/// Benchmarks Key and Value creation - measures the allocation cost of
/// constructing the types that form `EnrichmentEntry` fields.
fn bench_key_value_creation(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "key_value_creation_string";
    const NAME_I64: &str = "key_value_creation_i64";

    bench_with_tracking(group, allocs, time, ID, || {
        let key = Key::from("http.request.method");
        let value = Value::from("GET");
        black_box((key, value));
    });

    bench_with_tracking(group, allocs, time, NAME_I64, || {
        let key = Key::from("http.status_code");
        let value = Value::from(200_i64);
        black_box((key, value));
    });
}

/// Benchmarks building a redacted string `Value`, the per-field cost of the
/// redaction path.
///
/// The two variants are the same work through two constructions:
/// `to_redacted_string` materializes a `String` that is then copied into the
/// `Arc<str>`, while `Value::from_redacted` renders into a reusable scratch
/// buffer and allocates only the `Arc<str>`.
fn bench_redacted_value_creation(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID_VIA_STRING: &str = "redacted_value_via_string";
    const ID_DIRECT: &str = "redacted_value_from_redacted";

    let engine = replacing_engine();

    bench_with_tracking(group, allocs, time, ID_VIA_STRING, || {
        let classified = Sensitive::new(black_box("Mozilla/5.0 (Windows NT 10.0)"), BENCH_DC);
        let value = Value::String(observed::Text::from(observed::__private::RedactedToString::to_redacted_string(
            &classified,
            &engine,
        )));
        black_box(value);
    });

    bench_with_tracking(group, allocs, time, ID_DIRECT, || {
        let classified = Sensitive::new(black_box("Mozilla/5.0 (Windows NT 10.0)"), BENCH_DC);
        let value = Value::from_redacted(&classified, &engine);
        black_box(value);
    });
}

/// Benchmarks the full emit pipeline at enrichment depths 0 and 5, providing
/// representative low- and higher-depth baselines for the current
/// implementation. These endpoints show aggregate overhead for those cases,
/// not a per-level marginal cost or a pooling breakeven point.
fn bench_emit_varying_enrichment_depth(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
) {
    // Depth 0: no enrichments
    {
        const ID: &str = "emit_depth_0";
        let (processor, _provider) = make_log_processor();
        let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

        bench_with_tracking(group, allocs, time, ID, || {
            observed::emit!(&sink, SimpleLogEvent { status: 200, retries: 0 });
        });
    }

    // Depth 5: five enrichment levels
    {
        const ID: &str = "emit_depth_5";
        let (processor, _provider) = make_log_processor();
        let sink = Sink::new("bench", vec![Arc::new(processor)], tick::SimpleClock::new_frozen());

        (|| {
            bench_with_tracking(group, allocs, time, ID, || {
                observed::emit!(&sink, SimpleLogEvent { status: 200, retries: 0 });
            });
        })
        .enrich(&sink, BenchEnrich1 { val: 4 })
        .enrich(&sink, BenchEnrich1 { val: 3 })
        .enrich(&sink, BenchEnrich1 { val: 2 })
        .enrich(&sink, BenchEnrich1 { val: 1 })
        .enrich(&sink, BenchEnrich1 { val: 0 })();
    }
}
