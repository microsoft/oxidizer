// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Benchmarks for the `observed` crate's hot paths.
//!
//! Run with:
//! `cargo bench -p observed --features test-util --bench observed_benchmarks`.

use std::any::Any;
use std::cell::Cell;
use std::hint::black_box;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{self, Poll};

use criterion::{BatchSize, BenchmarkId, Criterion};
use data_privacy::{DataClass, Sensitive};
use observed::__private::EnrichmentEntry;
use observed::context::{Transfer, Transferred};
use observed::enrichment::{EnrichFnExt, EnrichFutureExt, Enriched};
use observed::{Enrichment, Key, Sink, Value, event};
use opentelemetry::logs::LoggerProvider;
use opentelemetry_sdk::logs::SdkLoggerProvider;

#[path = "../examples/support/otel.rs"]
mod otel;

const BENCH_DC: DataClass = DataClass::new("bench", "public");
const EMIT_PIPELINE: &str = "observed_benchmarks/emit_pipeline";
const EMIT_COMPOSITE: &str = "observed_benchmarks/emit_composite";
const EMIT_ENRICHMENT: &str = "observed_benchmarks/emit_enrichment";
const SINK_OPERATIONS: &str = "observed_benchmarks/sink_operations";
const ALLOCATION: &str = "observed_benchmarks/allocation";
const ENRICHMENT_TRANSFER: &str = "observed_benchmarks/enrichment_transfer";
const ENRICHED_POLLS: u32 = 8;
const BENCH_SINK_IDS: [&str; 8] = [
    "bench-0", "bench-1", "bench-2", "bench-3", "bench-4", "bench-5", "bench-6", "bench-7",
];

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

struct TraversalProcessor;

impl observed::processing::EventProcessor for TraversalProcessor {
    fn is_interested(&self, _description: &observed::metadata::EventDescription) -> bool {
        true
    }

    fn process(&self, event: &observed::processing::EventView<'_>) {
        let _ = black_box(event);
    }

    fn flush(&self) -> Result<(), observed::FlushError> {
        Ok(())
    }
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

/// Discards exported records so pipeline benchmarks do not retain unbounded output.
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

/// Keeps the provider alive for as long as its sink is used by a benchmark.
struct EmitState {
    sink: Sink,
    _provider: SdkLoggerProvider,
}

struct EnrichedEmitState {
    // Drop the guard before the state that owns the sink and provider.
    _guard: Box<dyn Any>,
    state: EmitState,
}

struct EnrichedSinkState {
    // Drop the guard before the sink whose slot it restores.
    _guard: Box<dyn Any>,
    sink: Sink,
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

fn traversal_sink(leaf_count: usize) -> Sink {
    let leaves = BENCH_SINK_IDS[..leaf_count]
        .iter()
        .map(|&id| Sink::builder(id).processors(vec![Arc::new(TraversalProcessor)]).build());
    if leaf_count == 1 {
        leaves
            .into_iter()
            .next()
            .expect("leaf_count is one in the single-sink benchmark case")
    } else {
        Sink::composite(leaves)
    }
}

fn enriched_emit_state(depth: usize) -> EnrichedEmitState {
    let state = emit_state();
    let guard = applied_enrichment(&state.sink, depth);
    EnrichedEmitState { _guard: guard, state }
}

fn enriched_sink_state(depth: usize) -> EnrichedSinkState {
    enriched_sink_state_with_slots(1, depth)
}

fn enriched_sink_state_with_slots(slot_count: usize, depth: usize) -> EnrichedSinkState {
    let sink = sink_with_slots(slot_count);
    let guard = applied_enrichment(&sink, depth);
    EnrichedSinkState { _guard: guard, sink }
}

fn applied_enrichment(sink: &Sink, depth: usize) -> Box<dyn Any> {
    let mut transfer = sink.transfer_context();
    if depth == 3 {
        transfer = transfer.with_enrichment(BenchEnrich3 {
            service: 1,
            region: 2,
            env: 3,
        });
    } else {
        for value in 0..depth {
            transfer = transfer.with_enrichment(BenchEnrich1 {
                val: i64::try_from(value).expect("benchmark enrichment depth fits in i64"),
            });
        }
    }
    Box::new(transfer.apply_current_thread())
}

fn transfer_with_entries(slot_count: usize) -> Transfer {
    let sink = sink_with_slots(slot_count);

    (|| sink.transfer_context()).enrich(&sink, BenchEnrich1 { val: 42 })()
}

fn sink_with_slots(slot_count: usize) -> Sink {
    if slot_count == 1 {
        processor_free_sink()
    } else {
        composite_sink(slot_count)
    }
}

struct AttachedPollState {
    future: Pin<Box<Transferred<AlwaysPending>>>,
}

fn attached_poll_state(slot_count: usize) -> AttachedPollState {
    AttachedPollState {
        future: Box::pin(AlwaysPending.attach(transfer_with_entries(slot_count))),
    }
}

struct PendingEnrichedPollState {
    future: Pin<Box<Enriched<AlwaysPending>>>,
}

fn pending_enriched_poll_state(sink: &Sink) -> PendingEnrichedPollState {
    let mut state = PendingEnrichedPollState {
        future: Box::pin(AlwaysPending.enrich(sink, BenchEnrich1 { val: 42 })),
    };
    let waker = task::Waker::noop();
    let mut cx = task::Context::from_waker(waker);
    let _ = state.future.as_mut().poll(&mut cx);
    state
}

struct CaptureOnPoll {
    sink: Sink,
    captured: Rc<Cell<Option<Transfer>>>,
}

impl Future for CaptureOnPoll {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.captured.set(Some(self.sink.transfer_context()));
        Poll::Pending
    }
}

struct CapturingEnrichedPollState {
    future: Pin<Box<Enriched<CaptureOnPoll>>>,
    captured: Rc<Cell<Option<Transfer>>>,
}

fn capturing_enriched_poll_state(sink: &Sink) -> CapturingEnrichedPollState {
    let captured = Rc::new(Cell::new(None));
    let mut state = CapturingEnrichedPollState {
        future: Box::pin(
            CaptureOnPoll {
                sink: sink.clone(),
                captured: Rc::clone(&captured),
            }
            .enrich(sink, BenchEnrich1 { val: 42 }),
        ),
        captured,
    };

    let waker = task::Waker::noop();
    let mut cx = task::Context::from_waker(waker);
    let _ = state.future.as_mut().poll(&mut cx);
    drop(black_box(state.captured.take()));
    state
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
/// Measures log mapping for an event that also declares metric metadata.
///
/// The fixture installs only a log processor, so this excludes instrument
/// creation and metric recording.
fn emit_log_of_metric_event(state: &EmitState) {
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

#[metabench::benchmark(EMIT_COMPOSITE_TRAVERSAL, EMIT_COMPOSITE, "emit_composite_traversal")]
#[bench::single(&traversal_sink(1))]
#[bench::composite_3(&traversal_sink(3))]
#[bench::composite_8(&traversal_sink(8))]
/// Measures sink traversal and one minimal processor call per leaf.
///
/// The event payload stays fixed across cases so the 3- and 8-leaf cases
/// expose composite interest, sampling, and dispatch overhead.
fn emit_composite_traversal(sink: &Sink) {
    observed::emit!(sink, SimpleLogEvent { status: 200, retries: 0 });
}

#[metabench::benchmark(EMIT_WITH_ENRICHMENTS, EMIT_ENRICHMENT, "emit_with_3_enrichments")]
#[bench::run(&enriched_emit_state(3))]
fn emit_with_enrichments(state: &EnrichedEmitState) {
    emit_simple_log(&state.state);
}

#[metabench::benchmark(EMIT_DEEP_ENRICHMENTS, EMIT_ENRICHMENT, "emit_with_10_nested_enrichments")]
#[bench::run(&enriched_emit_state(10))]
fn emit_deep_enrichments(state: &EnrichedEmitState) {
    emit_simple_log(&state.state);
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
#[bench::three_entries(&enriched_sink_state(3))]
#[bench::ten_nested(&enriched_sink_state(10))]
fn enrichment_collect(state: &EnrichedSinkState) -> Vec<EnrichmentEntry> {
    black_box(state.sink.current_enrichments())
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
    let waker = task::Waker::noop();
    let mut cx = task::Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    for _ in 0..ENRICHED_POLLS {
        let _ = black_box(future.as_mut().poll(&mut cx));
    }
}

fn composite_sink(slot_count: usize) -> Sink {
    Sink::composite(BENCH_SINK_IDS[..slot_count].iter().map(|&id| Sink::builder(id).build()))
}

#[metabench::benchmark(PENDING_ENRICHED_FUTURE, ALLOCATION, "pending_enriched_future_8_polls")]
#[bench::single(&processor_free_sink())]
#[bench::composite_3(&composite_sink(3))]
#[bench::composite_8(&composite_sink(8))]
/// Compares per-poll enrichment work without executor scheduling overhead.
///
/// The future stays pending and is driven directly with a no-op waker; the
/// cases isolate one sink from representative composite fan-out.
fn pending_enriched_future(sink: &Sink) {
    poll_pending(AlwaysPending.enrich(sink, BenchEnrich1 { val: 42 }));
}

#[metabench::benchmark(POLL_PENDING_ENRICHED_FUTURE, ALLOCATION, "poll_pending_enriched_future_8_polls")]
#[bench::single(pending_enriched_poll_state(&processor_free_sink()))]
#[bench::composite_3(pending_enriched_poll_state(&composite_sink(3)))]
#[bench::composite_8(pending_enriched_poll_state(&composite_sink(8)))]
/// Isolates steady-state enriched polling after construction and TLS first touch.
fn poll_pending_enriched_future(mut state: PendingEnrichedPollState) -> PendingEnrichedPollState {
    let waker = task::Waker::noop();
    let mut cx = task::Context::from_waker(waker);
    for _ in 0..ENRICHED_POLLS {
        let _ = black_box(state.future.as_mut().poll(&mut cx));
    }
    state
}

#[metabench::benchmark(POLL_CAPTURED_ENRICHED_FUTURE, ENRICHMENT_TRANSFER, "poll_captured_enriched_future_8_polls")]
#[bench::single(capturing_enriched_poll_state(&processor_free_sink()))]
#[bench::composite_3(capturing_enriched_poll_state(&composite_sink(3)))]
#[bench::composite_8(capturing_enriched_poll_state(&composite_sink(8)))]
/// Measures polling when the active enrichment snapshot is captured each time.
///
/// Each capture outlives the poll guard, forcing the future to evict its shared
/// reusable nodes. The transfer is then consumed before the next poll rebuilds
/// those nodes, preserving captured snapshot immutability without accumulating
/// transfers across iterations.
fn poll_captured_enriched_future(mut state: CapturingEnrichedPollState) -> CapturingEnrichedPollState {
    let waker = task::Waker::noop();
    let mut cx = task::Context::from_waker(waker);
    for _ in 0..ENRICHED_POLLS {
        let _ = black_box(state.future.as_mut().poll(&mut cx));
        drop(black_box(state.captured.take()));
    }
    state
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
/// Compares intermediate `String` construction with direct rendering into `Value`.
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
#[bench::depth_0(&enriched_emit_state(0))]
#[bench::depth_5(&enriched_emit_state(5))]
/// Uses representative low and higher depths to compare aggregate emit cost.
///
/// These endpoints are not intended to report a per-layer marginal cost.
fn emit_enrichment_depth(state: &EnrichedEmitState) {
    emit_simple_log(&state.state);
}

#[metabench::benchmark(POLL_ATTACHED, ENRICHMENT_TRANSFER, "poll_attached")]
#[bench::single_slot(attached_poll_state(1))]
#[bench::three_slots(attached_poll_state(3))]
#[bench::eight_slots(attached_poll_state(8))]
/// Measures the attached-future hot path that applies borrowed transfer state.
///
/// The single leaf is the lower bound, the small composite represents common
/// fan-out, and eight slots exercise storage beyond the inline capacity.
fn poll_attached(mut state: AttachedPollState) -> AttachedPollState {
    let waker = task::Waker::noop();
    let mut cx = task::Context::from_waker(waker);
    let _ = black_box(state.future.as_mut().poll(&mut cx));
    state
}

#[metabench::benchmark(CAPTURE_ENRICHMENT_CONTEXT, ENRICHMENT_TRANSFER, "capture_enrichment_context")]
#[bench::single(&enriched_sink_state_with_slots(1, 1))]
#[bench::composite_3(&enriched_sink_state_with_slots(3, 1))]
#[bench::composite_8(&enriched_sink_state_with_slots(8, 1))]
/// Isolates current-slot lookup, captured-head cloning, and transfer assembly.
fn capture_enrichment_context(state: &EnrichedSinkState) -> Transfer {
    black_box(state.sink.transfer_context())
}

fn criterion_emit_pipeline(criterion: &mut Criterion) {
    let mut emit_pipeline = criterion.benchmark_group(EMIT_PIPELINE);
    let emit = emit_state();
    emit_pipeline.bench_function(BenchmarkId::new(EMIT_SIMPLE_LOG.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_simple_log(black_box(&emit)));
    });
    emit_pipeline.bench_function(BenchmarkId::new(EMIT_MANY_FIELDS.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_many_fields(black_box(&emit)));
    });
    emit_pipeline.bench_function(BenchmarkId::new(EMIT_WITH_BODY.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_with_body(black_box(&emit)));
    });
    emit_pipeline.bench_function(BenchmarkId::new(EMIT_METRIC_EVENT.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_log_of_metric_event(black_box(&emit)));
    });
    let redacted = redacted_emit_state();
    emit_pipeline.bench_function(BenchmarkId::new(EMIT_REDACTED_STRINGS.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_redacted_strings(black_box(&redacted)));
    });
    emit_pipeline.finish();
}

fn criterion_emit_composite(criterion: &mut Criterion) {
    let mut emit_composite = criterion.benchmark_group(EMIT_COMPOSITE);
    let traversal_single = traversal_sink(1);
    let traversal_3 = traversal_sink(3);
    let traversal_8 = traversal_sink(8);
    for (case, sink) in [
        ("single", &traversal_single),
        ("composite_3", &traversal_3),
        ("composite_8", &traversal_8),
    ] {
        emit_composite.bench_function(BenchmarkId::new(EMIT_COMPOSITE_TRAVERSAL.benchmark_name(), case), |bencher| {
            bencher.iter(|| emit_composite_traversal(black_box(sink)));
        });
    }
    emit_composite.finish();
}

fn criterion_emit_enrichment(criterion: &mut Criterion) {
    let mut enrichment = criterion.benchmark_group(EMIT_ENRICHMENT);
    let enriched = enriched_emit_state(3);
    enrichment.bench_function(BenchmarkId::new(EMIT_WITH_ENRICHMENTS.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_with_enrichments(black_box(&enriched)));
    });
    let deeply_enriched = enriched_emit_state(10);
    enrichment.bench_function(BenchmarkId::new(EMIT_DEEP_ENRICHMENTS.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_deep_enrichments(black_box(&deeply_enriched)));
    });
    let sink = processor_free_sink();
    for (case, count) in [("three_entries", 3), ("five_entries", 5)] {
        enrichment.bench_function(BenchmarkId::new(ENRICH_PUSH_POP.benchmark_name(), case), |bencher| {
            bencher.iter(|| enrich_push_pop(black_box(&sink), count));
        });
    }
    enrichment.finish();
}

fn criterion_sink_operations(criterion: &mut Criterion) {
    let mut sink_operations = criterion.benchmark_group(SINK_OPERATIONS);
    let clock = tick::SimpleClock::new_system();
    sink_operations.bench_function(BenchmarkId::new(CONSTRUCT_PROCESSOR_FREE_SINK.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| construct_processor_free_sink(black_box(&clock)));
    });
    let noop = Sink::noop();
    sink_operations.bench_function(BenchmarkId::new(EMIT_TO_NOOP_SINK.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| emit_to_noop_sink(black_box(&noop)));
    });
    sink_operations.finish();
}

fn criterion_allocation(criterion: &mut Criterion) {
    let mut allocation = criterion.benchmark_group(ALLOCATION);
    for (case, state) in [("three_entries", enriched_sink_state(3)), ("ten_nested", enriched_sink_state(10))] {
        allocation.bench_function(BenchmarkId::new(ENRICHMENT_COLLECT.benchmark_name(), case), |bencher| {
            bencher.iter(|| enrichment_collect(black_box(&state)));
        });
    }
    let entries = enrichment_entries();
    allocation.bench_function(BenchmarkId::new(ENRICHMENT_ENTRY_CLONE.benchmark_name(), "run"), |bencher| {
        bencher.iter(|| enrichment_entry_clone(black_box(&entries)));
    });
    let sink = processor_free_sink();
    let composite_3 = composite_sink(3);
    let composite_8 = composite_sink(8);
    for (case, case_sink) in [("single", &sink), ("composite_3", &composite_3), ("composite_8", &composite_8)] {
        allocation.bench_function(BenchmarkId::new(PENDING_ENRICHED_FUTURE.benchmark_name(), case), |bencher| {
            bencher.iter(|| pending_enriched_future(black_box(case_sink)));
        });
        allocation.bench_function(BenchmarkId::new(POLL_PENDING_ENRICHED_FUTURE.benchmark_name(), case), |bencher| {
            bencher.iter_batched(
                || pending_enriched_poll_state(case_sink),
                |state| black_box(poll_pending_enriched_future(state)),
                BatchSize::SmallInput,
            );
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
    for (case, state) in [("depth_0", enriched_emit_state(0)), ("depth_5", enriched_emit_state(5))] {
        allocation.bench_function(BenchmarkId::new(EMIT_ENRICHMENT_DEPTH.benchmark_name(), case), |bencher| {
            bencher.iter(|| emit_enrichment_depth(black_box(&state)));
        });
    }
    allocation.finish();
}

fn criterion_enrichment_transfer(criterion: &mut Criterion) {
    let mut transfer = criterion.benchmark_group(ENRICHMENT_TRANSFER);
    for (case, slot_count) in [("single_slot", 1), ("three_slots", 3), ("eight_slots", 8)] {
        transfer.bench_function(BenchmarkId::new(POLL_ATTACHED.benchmark_name(), case), |bencher| {
            bencher.iter_batched(
                || attached_poll_state(slot_count),
                |state| black_box(poll_attached(state)),
                BatchSize::SmallInput,
            );
        });
    }
    for (case, slot_count) in [("single", 1), ("composite_3", 3), ("composite_8", 8)] {
        transfer.bench_function(BenchmarkId::new(POLL_CAPTURED_ENRICHED_FUTURE.benchmark_name(), case), |bencher| {
            bencher.iter_batched(
                || capturing_enriched_poll_state(&sink_with_slots(slot_count)),
                |state| black_box(poll_captured_enriched_future(state)),
                BatchSize::SmallInput,
            );
        });
    }
    for (case, state) in [
        ("single", enriched_sink_state_with_slots(1, 1)),
        ("composite_3", enriched_sink_state_with_slots(3, 1)),
        ("composite_8", enriched_sink_state_with_slots(8, 1)),
    ] {
        transfer.bench_function(BenchmarkId::new(CAPTURE_ENRICHMENT_CONTEXT.benchmark_name(), case), |bencher| {
            bencher.iter(|| capture_enrichment_context(black_box(&state)));
        });
    }
    transfer.finish();
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    criterion_emit_pipeline(criterion);
    criterion_emit_composite(criterion);
    criterion_emit_enrichment(criterion);
    criterion_sink_operations(criterion);
    criterion_allocation(criterion);
    criterion_enrichment_transfer(criterion);
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        EMIT_SIMPLE_LOG,
        EMIT_MANY_FIELDS,
        EMIT_WITH_BODY,
        EMIT_METRIC_EVENT,
        EMIT_REDACTED_STRINGS,
        EMIT_COMPOSITE_TRAVERSAL,
        EMIT_WITH_ENRICHMENTS,
        EMIT_DEEP_ENRICHMENTS,
        ENRICH_PUSH_POP,
        CONSTRUCT_PROCESSOR_FREE_SINK,
        EMIT_TO_NOOP_SINK,
        ENRICHMENT_COLLECT,
        ENRICHMENT_ENTRY_CLONE,
        PENDING_ENRICHED_FUTURE,
        POLL_PENDING_ENRICHED_FUTURE,
        KEY_VALUE_CREATION,
        REDACTED_VALUE_CREATION,
        EMIT_ENRICHMENT_DEPTH,
        POLL_ATTACHED,
        POLL_CAPTURED_ENRICHED_FUTURE,
        CAPTURE_ENRICHMENT_CONTEXT,
    ],
);
