// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Runtime telemetry sampling overhead for `Arc::deref`.

use std::hint::black_box;

use criterion::Criterion;
use performables::arc::Arc;
use seismograph::recorder::{Configuration, EventSampling};

const OBJECT_COUNT: usize = 4_096;

fn main() {
    seismograph::recorder(Configuration::default());
    let values = (0..OBJECT_COUNT).map(|value| Arc::new(value as u64)).collect::<Vec<_>>();
    let mut criterion = Criterion::default().configure_from_args();
    let mut group = criterion.benchmark_group("performables_telemetry_sampling/arc_deref");

    benchmark_mode(&mut group, "events_off", Configuration::default(), &values);
    benchmark_mode(&mut group, "events_1_in_100", recording_configuration(100, false), &values);
    benchmark_mode(&mut group, "events_1_in_20", recording_configuration(20, false), &values);
    benchmark_mode(&mut group, "events_1_in_1", recording_configuration(1, false), &values);
    benchmark_mode(
        &mut group,
        "events_1_in_1_with_backtraces",
        recording_configuration(1, true),
        &values,
    );

    group.finish();
    seismograph::recorder(Configuration::default());
    criterion.final_summary();
}

fn recording_configuration(sampling_one_in: usize, capture_backtraces: bool) -> Configuration {
    Configuration {
        general_events: seismograph::recorder::RecordingPolicy {
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
    configuration: Configuration,
    values: &[Arc<u64>],
) {
    seismograph::recorder(configuration);
    let mut index = 0;
    group.bench_function(name, |bencher| {
        bencher.iter(|| {
            let value = &values[index];
            index = (index + 1) & (OBJECT_COUNT - 1);
            black_box(**value)
        });
    });
}
