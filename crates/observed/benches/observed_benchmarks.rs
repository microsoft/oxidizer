// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for the `observed` crate's hot paths.
//!
//! Measures:
//! - Full emit pipeline (event -> log record via `OTel` provider)
//! - Enrichment resolution (context lookup + Vec building)
//! - Metric dimension building
//! - Context operations (enrich)
//!
//! Run with:
//! ```sh
//! cargo bench -p observed --features test-util
//! ```

use std::hint::black_box;
use std::sync::Arc;

use alloc_tracker::Allocator;
use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, Criterion, criterion_group, criterion_main};
use data_privacy::{DataClass, Sensitive};
use observed::__private::EnrichmentEntry;
use observed::enrichment::EnrichFnExt;
use observed::{Enrichment, Event, Key, Sink, Value};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::metrics::data::ResourceMetrics;

const BENCH_DC: DataClass = DataClass::new("bench", "public");

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

#[derive(Event)]
#[event(name = "bench.simple_log")]
#[log(severity = info)]
struct SimpleLogEvent {
    #[data_class(BENCH_DC)]
    status: i64,
    #[data_class(BENCH_DC)]
    retries: i64,
}

#[derive(Event)]
#[event(name = "bench.many_fields")]
#[log(severity = info)]
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

#[derive(Event)]
#[event(name = "bench.with_metric")]
#[log(severity = info)]
#[metric(kind = histogram, field = duration_ms, name = "bench_duration")]
struct MetricEvent {
    // TODO: replace #[unredacted] with classified type once metric fields support non-numeric Values
    #[unredacted]
    duration_ms: f64,
    #[data_class(BENCH_DC)]
    region: i64,
    #[data_class(BENCH_DC)]
    service: i64,
}

#[derive(Event)]
#[event(name = "bench.body_event")]
#[log(severity = info, message = "Request completed")]
struct BodyEvent {
    #[data_class(BENCH_DC)]
    code: i64,
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
        use std::ops::ControlFlow;

        use opentelemetry::logs::{AnyValue, LogRecord, Logger};

        let mut record = self.logger.create_log_record();
        record.set_event_name(event.name());
        if let Some(severity) = event.severity() {
            record.set_severity_number(opentelemetry::logs::Severity::from(severity));
            record.set_severity_text(severity.as_str());
        }
        record.set_timestamp(std::time::SystemTime::now());
        if let Some(body) = event.body() {
            record.set_body(AnyValue::String(body.into_owned().into()));
        }

        let engine = &self.redaction_engine;
        let _ = event.visit_fields(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });
        let _ = event.visit_enrichments(&mut |desc, get_value| {
            if let Some(log) = desc.log() {
                let value = get_value(engine);
                let any_value: AnyValue = value.into();
                record.add_attribute(opentelemetry::Key::from(log.key().to_owned()), any_value);
            }
            ControlFlow::Continue(())
        });
        if let Some(file) = event.source_file() {
            record.add_attribute(
                opentelemetry::Key::from_static_str("code.file.path"),
                AnyValue::String(file.into_owned().into()),
            );
        }
        if let Some(line) = event.source_line() {
            record.add_attribute(
                opentelemetry::Key::from_static_str("code.line.number"),
                AnyValue::Int(i64::from(line)),
            );
        }
        self.logger.emit(record);
    }

    fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Ok(())
    }
}

fn make_log_processor() -> (SimpleLogProcessor, SdkLoggerProvider) {
    // Use a no-op log exporter to avoid retaining millions of `LogRecord`s
    // during high-iteration benchmarks (the default `InMemoryLogExporter`
    // accumulates every emitted record and OOMs on agents with limited RAM).
    let logger_provider = SdkLoggerProvider::builder().with_simple_exporter(NoOpLogExporter).build();

    let logger = logger_provider.logger("bench");

    let processor = SimpleLogProcessor {
        logger,
        redaction_engine: data_privacy::RedactionEngine::default(),
    };

    (processor, logger_provider)
}

fn make_metric_processor() -> (SimpleLogProcessor, SdkMeterProvider) {
    let exporter = InMemoryMetricExporter::new();
    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter).build();
    let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
    let logger_provider = SdkLoggerProvider::builder().build();

    let logger = logger_provider.logger("bench");

    let processor = SimpleLogProcessor {
        logger,
        redaction_engine: data_privacy::RedactionEngine::default(),
    };

    (processor, meter_provider)
}

/// No-op log exporter for benchmarks.
///
/// Discards every batch immediately so that benches measuring the
/// `emit -> OTel` pipeline don't accumulate `LogRecord`s in memory across
/// millions of iterations.
#[derive(Debug)]
struct NoOpLogExporter;

impl opentelemetry_sdk::logs::LogExporter for NoOpLogExporter {
    async fn export(&self, _batch: opentelemetry_sdk::logs::LogBatch<'_>) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: std::time::Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }
}

/// In-memory metric exporter for benchmarks.
struct InMemoryMetricExporter;

impl InMemoryMetricExporter {
    fn new() -> Self {
        Self
    }
}

impl opentelemetry_sdk::metrics::exporter::PushMetricExporter for InMemoryMetricExporter {
    async fn export(&self, _metrics: &ResourceMetrics) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: std::time::Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        Ok(())
    }

    fn temporality(&self) -> opentelemetry_sdk::metrics::Temporality {
        opentelemetry_sdk::metrics::Temporality::Cumulative
    }
}

/// Helper to run a benchmark with `alloc_tracker` and `all_the_time` tracking.
fn bench_with_tracking(
    group: &mut BenchmarkGroup<'_, WallTime>,
    allocs: &alloc_tracker::Session,
    time: &all_the_time::Session,
    name: &str,
    body: impl Fn(),
) {
    group.bench_function(name, |b| {
        let _alloc = allocs.operation(name).measure_thread();
        let _clock = time.operation(name).measure_thread();
        b.iter(&body);
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
        bench_emit_with_metric(&mut group, &allocs, &time);
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

    // --- Context operation benchmarks ---
    {
        let mut group = c.benchmark_group("emit_context");
        bench_attach_emitter(&mut group, &allocs, &time);
        bench_emit_event_direct(&mut group, &allocs, &time);
        group.finish();
    }

    // --- Allocation-focused benchmarks (infinity_pool exploration) ---
    {
        let mut group = c.benchmark_group("emit_alloc");
        bench_enrichment_vec_collect(&mut group, &allocs, &time);
        bench_enrichment_vec_collect_deep(&mut group, &allocs, &time);
        bench_enrichment_entry_clone(&mut group, &allocs, &time);
        bench_arc_enrichment_node_churn(&mut group, &allocs, &time);
        bench_key_value_creation(&mut group, &allocs, &time);
        bench_emit_varying_enrichment_depth(&mut group, &allocs, &time);
        group.finish();
    }

    time.print_to_stdout();
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

fn bench_emit_with_metric(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "log_plus_metric";
    let (processor, _meter_provider) = make_metric_processor();
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
// Context operation benchmarks
// ---------------------------------------------------------------------------

fn bench_attach_emitter(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "attach_emitter";
    const EMPTY_PROCESSORS: Vec<Arc<dyn observed::processing::EventProcessor>> = Vec::new();

    bench_with_tracking(group, allocs, time, ID, || {
        let sink = Sink::new("bench", EMPTY_PROCESSORS, tick::SimpleClock::new_frozen());
        drop(sink);
    });
}

fn bench_emit_event_direct(group: &mut BenchmarkGroup<'_, WallTime>, allocs: &alloc_tracker::Session, time: &all_the_time::Session) {
    const ID: &str = "emit_event_no_emitter";

    // Benchmark the emit path when no sink is registered - measures the
    // cost of the context lookup + early return.
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

/// Benchmarks the full emit pipeline at varying enrichment depths (0, 3, 5,
/// 10) to quantify the per-enrichment allocation cost. This helps identify
/// the breakeven point for pooling.
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
