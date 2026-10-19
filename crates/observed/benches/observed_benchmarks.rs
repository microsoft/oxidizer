// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for the `observed` crate's hot paths.
//!
//! Run with:
//! `cargo bench -p observed --features test-util --bench observed_benchmarks`.

use std::hint::black_box;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};

use criterion::{BenchmarkId, Criterion};
use data_privacy::{DataClass, Sensitive};
use observed::__private::EnrichmentEntry;
use observed::context::Transfer;
use observed::enrichment::{EnrichFnExt, EnrichFutureExt};
use observed::{Enrichment, Key, Sink, Value, event};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::SdkLoggerProvider;

#[path = "../examples/support/otel.rs"]
mod otel;

const BENCH_DC: DataClass = DataClass::new("bench", "public");
const EMIT_PIPELINE: &str = "observed_benchmarks/emit_pipeline";
const EMIT_ENRICHMENT: &str = "observed_benchmarks/emit_enrichment";
const SINK_OPERATIONS: &str = "observed_benchmarks/sink_operations";
const ALLOCATION: &str = "observed_benchmarks/allocation";
const ENRICHMENT_TRANSFER: &str = "observed_benchmarks/enrichment_transfer";
const BENCH_SINK_IDS: [&str; 3] = ["bench-0", "bench-1", "bench-2"];

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

#[event("bench.redacted_strings")]
#[info]
struct RedactedStringsEvent {
    #[data_class(BENCH_DC)]
    request_id: &'static str,
    #[data_class(BENCH_DC)]
    user_agent: &'static str,
}

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

struct EmitState {
    sink: Sink,
    _provider: SdkLoggerProvider,
}

fn replacing_engine() -> data_privacy::RedactionEngine {
    data_privacy::RedactionEngine::builder()
        .set_fallback_redactor(data_privacy::simple_redactor::SimpleRedactor::new())
        .build()
}

fn emit_state() -> EmitState {
    emit_state_with(data_privacy::RedactionEngine::default())
}

fn redacted_emit_state() -> EmitState {
    emit_state_with(replacing_engine())
}

fn emit_state_with(redaction_engine: data_privacy::RedactionEngine) -> EmitState {
    let provider = SdkLoggerProvider::builder().with_simple_exporter(NoOpLogExporter).build();
    let logger = provider.logger("bench");
    let processor = SimpleLogProcessor { logger, redaction_engine };
    let sink = Sink::builder("bench").processors(vec![Arc::new(processor)]).build();
    EmitState { sink, _provider: provider }
}

fn processor_free_sink() -> Sink {
    Sink::builder("bench").build()
}

fn transfer_with_entries(slot_count: usize) -> Transfer {
    let sink = if slot_count == 1 {
        processor_free_sink()
    } else {
        Sink::composite(BENCH_SINK_IDS[..slot_count].iter().map(|&id| Sink::builder(id).build()))
    };

    (|| sink.transfer_context()).enrich(&sink, BenchEnrich1 { val: 42 })()
}

#[metabench::benchmark(EMIT_SIMPLE_LOG, EMIT_PIPELINE, "simple_log_2_fields")]
#[bench::run(&emit_state())]
fn emit_simple_log(state: &EmitState) {
    observed::emit!(&state.sink, SimpleLogEvent { status: 200, retries: 0 });
}

#[metabench::benchmark(EMIT_MANY_FIELDS, EMIT_PIPELINE, "log_8_fields")]
#[bench::run(&emit_state())]
fn emit_many_fields(state: &EmitState) {
    observed::emit!(
        &state.sink,
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
}

#[metabench::benchmark(EMIT_WITH_BODY, EMIT_PIPELINE, "log_with_body")]
#[bench::run(&emit_state())]
fn emit_with_body(state: &EmitState) {
    observed::emit!(&state.sink, BodyEvent { code: 42 });
}

#[metabench::benchmark(EMIT_METRIC_EVENT, EMIT_PIPELINE, "log_of_metric_event")]
#[bench::run(&emit_state())]
fn emit_metric_event(state: &EmitState) {
    observed::emit!(
        &state.sink,
        MetricEvent {
            duration_ms: 42.5,
            region: 1,
            service: 2,
        }
    );
}

#[metabench::benchmark(EMIT_REDACTED_STRINGS, EMIT_PIPELINE, "log_2_redacted_strings")]
#[bench::run(&redacted_emit_state())]
fn emit_redacted_strings(state: &EmitState) {
    observed::emit!(
        &state.sink,
        RedactedStringsEvent {
            request_id: "6f1c2f10-6a5b-4f0a-9c1f-2f106a5b4f0a",
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
        }
    );
}

#[metabench::benchmark(EMIT_WITH_ENRICHMENTS, EMIT_ENRICHMENT, "emit_with_3_enrichments")]
#[bench::run(&emit_state())]
fn emit_with_enrichments(state: &EmitState) {
    (|| emit_simple_log(state)).enrich(
        &state.sink,
        BenchEnrich3 {
            service: 1,
            region: 2,
            env: 3,
        },
    )();
}

#[metabench::benchmark(EMIT_DEEP_ENRICHMENTS, EMIT_ENRICHMENT, "emit_with_10_nested_enrichments")]
#[bench::run(&emit_state())]
fn emit_deep_enrichments(state: &EmitState) {
    (|| emit_simple_log(state))
        .enrich(&state.sink, BenchEnrich1 { val: 9 })
        .enrich(&state.sink, BenchEnrich1 { val: 8 })
        .enrich(&state.sink, BenchEnrich1 { val: 7 })
        .enrich(&state.sink, BenchEnrich1 { val: 6 })
        .enrich(&state.sink, BenchEnrich1 { val: 5 })
        .enrich(&state.sink, BenchEnrich1 { val: 4 })
        .enrich(&state.sink, BenchEnrich1 { val: 3 })
        .enrich(&state.sink, BenchEnrich1 { val: 2 })
        .enrich(&state.sink, BenchEnrich1 { val: 1 })
        .enrich(&state.sink, BenchEnrich1 { val: 0 })();
}

#[metabench::benchmark(ENRICH_PUSH_POP, EMIT_ENRICHMENT, "enrich_push_pop")]
#[bench::three_entries(&processor_free_sink(), 3)]
#[bench::five_entries(&processor_free_sink(), 5)]
fn enrich_push_pop(sink: &Sink, entry_count: usize) {
    match entry_count {
        3 => (|| black_box(())).enrich(
            sink,
            BenchEnrich3 {
                service: 1,
                region: 2,
                env: 3,
            },
        )(),
        5 => (|| black_box(())).enrich(
            sink,
            BenchEnrich5 {
                service: 1,
                region: 2,
                env: 3,
                cluster: 4,
                pod: 5,
            },
        )(),
        _ => unreachable!("benchmark cases only use 3 or 5 entries"),
    }
}

#[metabench::benchmark(CONSTRUCT_PROCESSOR_FREE_SINK, SINK_OPERATIONS, "construct_processor_free_sink")]
#[bench::run(&tick::SimpleClock::new_system())]
fn construct_processor_free_sink(clock: &tick::SimpleClock) -> Sink {
    Sink::builder("bench").clock(clock.clone()).build()
}

#[metabench::benchmark(EMIT_TO_NOOP_SINK, SINK_OPERATIONS, "emit_to_noop_sink")]
#[bench::run(&Sink::noop())]
fn emit_to_noop_sink(sink: &Sink) {
    observed::emit!(sink, SimpleLogEvent { status: 200, retries: 0 });
}

#[metabench::benchmark(ENRICHMENT_COLLECT, ALLOCATION, "enrichment_collect")]
#[bench::three_entries(&processor_free_sink(), 3)]
#[bench::ten_nested(&processor_free_sink(), 10)]
fn enrichment_collect(sink: &Sink, entry_count: usize) -> Vec<EnrichmentEntry> {
    fn collect(sink: &Sink) -> Vec<EnrichmentEntry> {
        black_box(sink.current_enrichments())
    }

    match entry_count {
        3 => (|| collect(sink)).enrich(
            sink,
            BenchEnrich3 {
                service: 1,
                region: 2,
                env: 3,
            },
        )(),
        10 => (|| collect(sink))
            .enrich(sink, BenchEnrich1 { val: 9 })
            .enrich(sink, BenchEnrich1 { val: 8 })
            .enrich(sink, BenchEnrich1 { val: 7 })
            .enrich(sink, BenchEnrich1 { val: 6 })
            .enrich(sink, BenchEnrich1 { val: 5 })
            .enrich(sink, BenchEnrich1 { val: 4 })
            .enrich(sink, BenchEnrich1 { val: 3 })
            .enrich(sink, BenchEnrich1 { val: 2 })
            .enrich(sink, BenchEnrich1 { val: 1 })
            .enrich(sink, BenchEnrich1 { val: 0 })(),
        _ => unreachable!("benchmark cases only use 3 or 10 entries"),
    }
}

fn enrichment_entries() -> Vec<EnrichmentEntry> {
    vec![
        EnrichmentEntry::new("service", Sensitive::new("api-gateway", BENCH_DC)),
        EnrichmentEntry::new("region", Sensitive::new("us-west-2", BENCH_DC)),
        EnrichmentEntry::new("env", Sensitive::new("production", BENCH_DC)),
        EnrichmentEntry::new("tenant", Sensitive::new("contoso", BENCH_DC)),
        EnrichmentEntry::new("version", Sensitive::new("1.2.3", BENCH_DC)),
    ]
}

#[metabench::benchmark(ENRICHMENT_ENTRY_CLONE, ALLOCATION, "enrichment_entry_clone_5")]
#[bench::run(&enrichment_entries())]
fn enrichment_entry_clone(entries: &[EnrichmentEntry]) -> Vec<EnrichmentEntry> {
    entries.to_vec()
}

struct AlwaysPending;

impl Future for AlwaysPending {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

fn poll_pending(future: impl Future<Output = ()>) {
    const POLLS: u32 = 8;

    let waker = task::Waker::noop();
    let mut cx = task::Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    for _ in 0..POLLS {
        let _ = black_box(future.as_mut().poll(&mut cx));
    }
}

fn composite_sink() -> Sink {
    Sink::composite(BENCH_SINK_IDS.iter().map(|&id| Sink::builder(id).build()))
}

#[metabench::benchmark(PENDING_ENRICHED_FUTURE, ALLOCATION, "pending_enriched_future_8_polls")]
#[bench::single(&processor_free_sink())]
#[bench::composite_3(&composite_sink())]
fn pending_enriched_future(sink: &Sink) {
    poll_pending(AlwaysPending.enrich(sink, BenchEnrich1 { val: 42 }));
}

#[metabench::benchmark(KEY_VALUE_CREATION, ALLOCATION, "key_value_creation")]
#[bench::string_value(false)]
#[bench::i64_value(true)]
fn key_value_creation(integer: bool) -> (Key, Value) {
    if integer {
        (Key::from("http.status_code"), Value::from(200_i64))
    } else {
        (Key::from("http.request.method"), Value::from("GET"))
    }
}

#[metabench::benchmark(REDACTED_VALUE_CREATION, ALLOCATION, "redacted_value_creation")]
#[bench::via_string(&replacing_engine(), false)]
#[bench::direct(&replacing_engine(), true)]
fn redacted_value_creation(engine: &data_privacy::RedactionEngine, direct: bool) -> Value {
    let classified = Sensitive::new(black_box("Mozilla/5.0 (Windows NT 10.0)"), BENCH_DC);
    if direct {
        Value::from_redacted(&classified, engine)
    } else {
        Value::String(observed::Text::from(observed::__private::RedactedToString::to_redacted_string(
            &classified,
            engine,
        )))
    }
}

#[metabench::benchmark(EMIT_ENRICHMENT_DEPTH, ALLOCATION, "emit_enrichment_depth")]
#[bench::depth_0(&emit_state(), 0)]
#[bench::depth_5(&emit_state(), 5)]
fn emit_enrichment_depth(state: &EmitState, depth: usize) {
    match depth {
        0 => emit_simple_log(state),
        5 => (|| emit_simple_log(state))
            .enrich(&state.sink, BenchEnrich1 { val: 4 })
            .enrich(&state.sink, BenchEnrich1 { val: 3 })
            .enrich(&state.sink, BenchEnrich1 { val: 2 })
            .enrich(&state.sink, BenchEnrich1 { val: 1 })
            .enrich(&state.sink, BenchEnrich1 { val: 0 })(),
        _ => unreachable!("benchmark cases only use depth 0 or 5"),
    }
}

#[metabench::benchmark(APPLY_TRANSFER, ENRICHMENT_TRANSFER, "apply_current_thread")]
#[bench::single_slot(&transfer_with_entries(1))]
#[bench::three_slots(&transfer_with_entries(3))]
fn apply_transfer(transfer: &Transfer) {
    black_box(transfer.apply_current_thread());
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut emit_pipeline = criterion.benchmark_group(EMIT_PIPELINE);
    let emit = emit_state();
    emit_pipeline.bench_function(EMIT_SIMPLE_LOG.benchmark_name(), |bencher| {
        bencher.iter(|| emit_simple_log(black_box(&emit)));
    });
    emit_pipeline.bench_function(EMIT_MANY_FIELDS.benchmark_name(), |bencher| {
        bencher.iter(|| emit_many_fields(black_box(&emit)));
    });
    emit_pipeline.bench_function(EMIT_WITH_BODY.benchmark_name(), |bencher| {
        bencher.iter(|| emit_with_body(black_box(&emit)));
    });
    emit_pipeline.bench_function(EMIT_METRIC_EVENT.benchmark_name(), |bencher| {
        bencher.iter(|| emit_metric_event(black_box(&emit)));
    });
    let redacted = redacted_emit_state();
    emit_pipeline.bench_function(EMIT_REDACTED_STRINGS.benchmark_name(), |bencher| {
        bencher.iter(|| emit_redacted_strings(black_box(&redacted)));
    });
    emit_pipeline.finish();

    let mut enrichment = criterion.benchmark_group(EMIT_ENRICHMENT);
    enrichment.bench_function(EMIT_WITH_ENRICHMENTS.benchmark_name(), |bencher| {
        bencher.iter(|| emit_with_enrichments(black_box(&emit)));
    });
    enrichment.bench_function(EMIT_DEEP_ENRICHMENTS.benchmark_name(), |bencher| {
        bencher.iter(|| emit_deep_enrichments(black_box(&emit)));
    });
    let sink = processor_free_sink();
    for (case, count) in [("three_entries", 3), ("five_entries", 5)] {
        enrichment.bench_function(BenchmarkId::new(ENRICH_PUSH_POP.benchmark_name(), case), |bencher| {
            bencher.iter(|| enrich_push_pop(black_box(&sink), count));
        });
    }
    enrichment.finish();

    let mut sink_operations = criterion.benchmark_group(SINK_OPERATIONS);
    let clock = tick::SimpleClock::new_system();
    sink_operations.bench_function(CONSTRUCT_PROCESSOR_FREE_SINK.benchmark_name(), |bencher| {
        bencher.iter(|| construct_processor_free_sink(black_box(&clock)));
    });
    let noop = Sink::noop();
    sink_operations.bench_function(EMIT_TO_NOOP_SINK.benchmark_name(), |bencher| {
        bencher.iter(|| emit_to_noop_sink(black_box(&noop)));
    });
    sink_operations.finish();

    let mut allocation = criterion.benchmark_group(ALLOCATION);
    for (case, count) in [("three_entries", 3), ("ten_nested", 10)] {
        allocation.bench_function(BenchmarkId::new(ENRICHMENT_COLLECT.benchmark_name(), case), |bencher| {
            bencher.iter(|| enrichment_collect(black_box(&sink), count));
        });
    }
    let entries = enrichment_entries();
    allocation.bench_function(ENRICHMENT_ENTRY_CLONE.benchmark_name(), |bencher| {
        bencher.iter(|| enrichment_entry_clone(black_box(&entries)));
    });
    let composite = composite_sink();
    for (case, case_sink) in [("single", &sink), ("composite_3", &composite)] {
        allocation.bench_function(BenchmarkId::new(PENDING_ENRICHED_FUTURE.benchmark_name(), case), |bencher| {
            bencher.iter(|| pending_enriched_future(black_box(case_sink)));
        });
    }
    for (case, integer) in [("string_value", false), ("i64_value", true)] {
        allocation.bench_function(BenchmarkId::new(KEY_VALUE_CREATION.benchmark_name(), case), |bencher| {
            bencher.iter(|| key_value_creation(integer));
        });
    }
    let engine = replacing_engine();
    for (case, direct) in [("via_string", false), ("direct", true)] {
        allocation.bench_function(BenchmarkId::new(REDACTED_VALUE_CREATION.benchmark_name(), case), |bencher| {
            bencher.iter(|| redacted_value_creation(black_box(&engine), direct));
        });
    }
    for (case, depth) in [("depth_0", 0), ("depth_5", 5)] {
        allocation.bench_function(BenchmarkId::new(EMIT_ENRICHMENT_DEPTH.benchmark_name(), case), |bencher| {
            bencher.iter(|| emit_enrichment_depth(black_box(&emit), depth));
        });
    }
    allocation.finish();

    let mut transfer = criterion.benchmark_group(ENRICHMENT_TRANSFER);
    let single = transfer_with_entries(1);
    let three = transfer_with_entries(3);
    for (case, context) in [("single_slot", &single), ("three_slots", &three)] {
        transfer.bench_function(BenchmarkId::new(APPLY_TRANSFER.benchmark_name(), case), |bencher| {
            bencher.iter(|| apply_transfer(black_box(context)));
        });
    }
    transfer.finish();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        EMIT_SIMPLE_LOG,
        EMIT_MANY_FIELDS,
        EMIT_WITH_BODY,
        EMIT_METRIC_EVENT,
        EMIT_REDACTED_STRINGS,
        EMIT_WITH_ENRICHMENTS,
        EMIT_DEEP_ENRICHMENTS,
        ENRICH_PUSH_POP,
        CONSTRUCT_PROCESSOR_FREE_SINK,
        EMIT_TO_NOOP_SINK,
        ENRICHMENT_COLLECT,
        ENRICHMENT_ENTRY_CLONE,
        PENDING_ENRICHED_FUTURE,
        KEY_VALUE_CREATION,
        REDACTED_VALUE_CREATION,
        EMIT_ENRICHMENT_DEPTH,
        APPLY_TRANSFER,
    ],
);
