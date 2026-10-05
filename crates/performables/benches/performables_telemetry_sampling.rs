// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime telemetry sampling overhead for `Arc::deref`.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::hint::black_box;

use criterion::Criterion;
use performables::arc::Arc;
use seismograph::recorder::{Configuration, EventSampling};

const OBJECT_COUNT: usize = 4_096;
const ARC_DEREF: &str = "performables_telemetry_sampling/arc_deref";

struct Input {
    values: Vec<Arc<u64>>,
    index: usize,
}

fn input(configuration: Configuration) -> Input {
    seismograph::recorder(Configuration::default());
    let values = (0..OBJECT_COUNT).map(|value| Arc::new(value as u64)).collect::<Vec<_>>();
    seismograph::recorder(configuration);
    Input { values, index: 0 }
}

#[metabench::benchmark(EVENTS_OFF, ARC_DEREF, "events_off")]
#[bench::configured(&mut input(Configuration::default()))]
fn events_off(input: &mut Input) -> u64 {
    dereference(input)
}

#[metabench::benchmark(EVENTS_1_IN_100, ARC_DEREF, "events_1_in_100")]
#[bench::configured(&mut input(recording_configuration(100, false)))]
fn events_1_in_100(input: &mut Input) -> u64 {
    dereference(input)
}

#[metabench::benchmark(EVENTS_1_IN_20, ARC_DEREF, "events_1_in_20")]
#[bench::configured(&mut input(recording_configuration(20, false)))]
fn events_1_in_20(input: &mut Input) -> u64 {
    dereference(input)
}

#[metabench::benchmark(EVENTS_1_IN_1, ARC_DEREF, "events_1_in_1")]
#[bench::configured(&mut input(recording_configuration(1, false)))]
fn events_1_in_1(input: &mut Input) -> u64 {
    dereference(input)
}

#[metabench::benchmark(EVENTS_1_IN_1_WITH_BACKTRACES, ARC_DEREF, "events_1_in_1_with_backtraces")]
#[bench::configured(&mut input(recording_configuration(1, true)))]
fn events_1_in_1_with_backtraces(input: &mut Input) -> u64 {
    dereference(input)
}

fn dereference(input: &mut Input) -> u64 {
    let value = &input.values[input.index];
    input.index = (input.index + 1) & (OBJECT_COUNT - 1);
    black_box(**value)
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group(ARC_DEREF);

    benchmark_mode(&mut group, EVENTS_OFF.benchmark_name(), input(Configuration::default()), events_off);
    benchmark_mode(
        &mut group,
        EVENTS_1_IN_100.benchmark_name(),
        input(recording_configuration(100, false)),
        events_1_in_100,
    );
    benchmark_mode(
        &mut group,
        EVENTS_1_IN_20.benchmark_name(),
        input(recording_configuration(20, false)),
        events_1_in_20,
    );
    benchmark_mode(
        &mut group,
        EVENTS_1_IN_1.benchmark_name(),
        input(recording_configuration(1, false)),
        events_1_in_1,
    );
    benchmark_mode(
        &mut group,
        EVENTS_1_IN_1_WITH_BACKTRACES.benchmark_name(),
        input(recording_configuration(1, true)),
        events_1_in_1_with_backtraces,
    );

    group.finish();
    seismograph::recorder(Configuration::default());
}

fn recording_configuration(sampling_one_in: usize, capture_backtraces: bool) -> Configuration {
    Configuration {
        arc_dereferences: seismograph::recorder::RecordingPolicy {
            enabled: true,
            capture_backtraces,
            event_sampling: EventSampling::one_in(sampling_one_in)
                .expect("benchmark sampling denominators are within the supported nonzero range"),
        },
        ..Configuration::default()
    }
}

fn benchmark_mode(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
    mut input: Input,
    benchmark: fn(&mut Input) -> u64,
) {
    group.bench_function(name, |bencher| bencher.iter(|| benchmark(&mut input)));
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [
        EVENTS_OFF,
        EVENTS_1_IN_100,
        EVENTS_1_IN_20,
        EVENTS_1_IN_1,
        EVENTS_1_IN_1_WITH_BACKTRACES,
    ],
);
