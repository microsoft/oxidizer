// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Noop versus active telemetry, without private exporter dependencies.
//!
//! The active processor synchronously reads/redacts every field and enrichment, then
//! discards it. It does not buffer, export, or retain an unbounded stream of events.
//! These are Arty-only overhead measurements, not an equivalently instrumented Tokio comparison.

use std::hint::black_box;
use std::num::NonZeroUsize;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use arty::runtime::{BlockingPoolPolicy, ProcessorCount, Runtime};
use arty::task::{JoinHandle, TaskScheduler};
use criterion::{BenchmarkId, Criterion, Throughput};
use data_privacy::RedactionEngine;
use observed::metadata::{EventDescription, FieldDescriptor};
use observed::processing::{EventProcessor, EventView};
use observed::{FlushError, Sink};
use tick::SimpleClock;

#[derive(Debug)]
struct ReadFields(RedactionEngine);

impl EventProcessor for ReadFields {
    fn is_interested(&self, _: &EventDescription) -> bool {
        true
    }

    fn process(&self, event: &EventView<'_>) {
        let mut read = |_: &FieldDescriptor, value: &observed::processing::FieldValueFn<'_>| {
            black_box(value(&self.0));
            ControlFlow::Continue(())
        };
        let _ = event.visit_fields(&mut read);
        let _ = event.visit_enrichments(&mut read);
    }

    fn flush(&self) -> Result<(), FlushError> {
        Ok(())
    }
}

fn active_sink() -> Sink {
    let redactor = RedactionEngine::builder()
        .suppress_redaction(data_privacy::DataClass::new("arty", "SystemMetadata"))
        .build();
    Sink::new("arty-benchmark", vec![Arc::new(ReadFields(redactor))], SimpleClock::new_frozen())
}

fn runtime(sink: Sink) -> Runtime {
    Runtime::builder()
        .processor_count(ProcessorCount::exactly(NonZeroUsize::MIN))
        .blocking_pool_policy(BlockingPoolPolicy::shared(1))
        .sink(sink)
        .build()
        .expect("benchmark requires one available processor")
}

#[derive(Debug)]
struct Case {
    _runtime: Runtime,
    scheduler: TaskScheduler,
    handles: Vec<JoinHandle<()>>,
    count: usize,
}

impl Case {
    fn new(count: usize, active: bool) -> Self {
        let runtime = runtime(if active { active_sink() } else { Sink::noop() });
        let scheduler = runtime.task_scheduler();
        let mut case = Self {
            _runtime: runtime,
            scheduler,
            handles: Vec::with_capacity(count),
            count,
        };
        case.spawn();
        case
    }

    fn spawn(&mut self) {
        self.handles.clear();
        self.handles
            .extend((0..self.count).map(|_| self.scheduler.spawn(async |_| black_box(()))));
        for handle in &mut self.handles {
            futures::executor::block_on(black_box(handle));
        }
    }
}

#[metabench::benchmark(NOOP, "arty_telemetry/spawn", "noop")]
#[bench::one(&mut Case::new(1, false))]
#[bench::hundred(&mut Case::new(100, false))]
fn noop(case: &mut Case) {
    case.spawn();
}

#[metabench::benchmark(ACTIVE, "arty_telemetry/spawn", "active")]
#[bench::one(&mut Case::new(1, true))]
#[bench::hundred(&mut Case::new(100, true))]
fn active(case: &mut Case) {
    case.spawn();
}

#[metabench::benchmark(LIFECYCLE_NOOP, "arty_telemetry/lifecycle", "noop")]
fn lifecycle_noop() {
    drop(runtime(Sink::noop()));
}

#[metabench::benchmark(LIFECYCLE_ACTIVE, "arty_telemetry/lifecycle", "active")]
#[bench::one(&active_sink())]
fn lifecycle_active(sink: &Sink) {
    drop(runtime(sink.clone()));
}

fn configured_criterion() -> Criterion {
    Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2))
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(NOOP.group_name());
    for (count, name) in [(1, "one"), (100, "hundred")] {
        group.throughput(Throughput::Elements(u64::try_from(count).expect("benchmark counts fit in u64")));
        group.bench_function(BenchmarkId::new(NOOP.benchmark_name(), name), |bencher| {
            let mut case = Case::new(count, false);
            bencher.iter(|| noop(&mut case));
        });
        group.bench_function(BenchmarkId::new(ACTIVE.benchmark_name(), name), |bencher| {
            let mut case = Case::new(count, true);
            bencher.iter(|| active(&mut case));
        });
    }
    group.finish();
    let mut lifecycle = criterion.benchmark_group(LIFECYCLE_NOOP.group_name());
    lifecycle.bench_function(LIFECYCLE_NOOP.benchmark_name(), |bencher| bencher.iter(lifecycle_noop));
    lifecycle.bench_function(BenchmarkId::new(LIFECYCLE_ACTIVE.benchmark_name(), "one"), |bencher| {
        let sink = active_sink();
        bencher.iter(|| lifecycle_active(&sink));
    });
    lifecycle.finish();
}

metabench::main!(
    criterion = {
        factory = configured_criterion,
        benchmarks = criterion_benchmarks,
        unit = "ns",
    },
    benchmarks = [NOOP, ACTIVE, LIFECYCLE_NOOP, LIFECYCLE_ACTIVE],
);
