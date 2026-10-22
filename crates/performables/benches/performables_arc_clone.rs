// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Separates Arc cloning from the logical-count work performed during drop.
//!
//! Compare std and `PerProcess` with `PerThread` storage containing 0, 1, 8, or
//! 64 partitions. The shared-64 case aliases one allocation in every partition;
//! the other populated cases use distinct allocations. Thread creation and
//! storage construction are outside the measured region. All workloads are
//! single-threaded and uncontended.
//!
//! Run with `cargo bench -p performables --bench performables_arc_clone`.
//! Repeat with `--features seismograph` to compare disabled recording with
//! runtime-enabled recording in the `_recording` cases. Every object is
//! recorded, without backtraces. The bounded event ring overwrites old events
//! during sustained measurement; there is no export or concurrent reader.
//! Metabench also supplies instruction counts on Linux.
//!
//! Clone-only retains outputs until after timing; non-final-drop prepares
//! clones before timing and retains another owner. These two groups include
//! output buffering overhead (which depends on handle size), not output
//! destruction; their times should not be added to predict clone-and-drop.

#![allow(missing_docs, reason = "benchmark code")]
#![expect(
    clippy::exit,
    clippy::missing_docs_in_private_items,
    unused_qualifications,
    reason = "Triggered by Gungraun macro expansion on every platform."
)]

use std::hint::black_box;
use std::sync::Arc as StdArc;
use std::thread;

use criterion::measurement::WallTime;
use criterion::{BatchSize, BenchmarkGroup, BenchmarkId, Criterion};
use performables::arc::{Arc, PerProcess, PerThread};
use thread_aware::ThreadBuilder;

const CLONE_ONLY: &str = "performables_arc_clone/clone_only";
const CLONE_DROP: &str = "performables_arc_clone/clone_drop";
const DROP_NONFINAL: &str = "performables_arc_clone/drop_nonfinal";
// Bound retained outputs independently of Criterion's adaptive iteration count.
const BATCH_SIZE: BatchSize = BatchSize::NumIterations(1_024);
const PARTITIONS: [(usize, &str); 4] = [
    (0, "per_thread_empty"),
    (1, "per_thread_distinct_1"),
    (8, "per_thread_distinct_8"),
    (64, "per_thread_distinct_64"),
];

#[derive(Clone, Copy)]
enum Allocations {
    Distinct,
    Shared,
}

#[derive(Clone, Copy)]
enum Recording {
    Disabled,
    #[cfg(feature = "seismograph")]
    Enabled,
}

impl Recording {
    fn suffix(self) -> &'static str {
        match self {
            Self::Disabled => "",
            #[cfg(feature = "seismograph")]
            Self::Enabled => "_recording",
        }
    }

    fn configure(self) {
        #[cfg(feature = "seismograph")]
        {
            use seismograph::recorder::{Configuration, RecordingPolicy};

            let configuration = match self {
                Self::Disabled => Configuration::default(),
                Self::Enabled => Configuration {
                    general_events: RecordingPolicy::all(false),
                    ..Configuration::default()
                },
            };
            seismograph::recorder(configuration);
        }
        #[cfg(not(feature = "seismograph"))]
        let _ = self;
    }
}

fn configured<T: Clone>(value: T, recording: Recording) -> T {
    recording.configure();
    // Initialize the thread-local event ring before timing enabled cases.
    drop(black_box(value.clone()));
    value
}

#[derive(Clone, Copy)]
enum Operation {
    CloneOnly,
    CloneDrop,
    DropNonfinal,
}

impl Operation {
    fn identity(self) -> metabench::BenchmarkIdentity {
        match self {
            Self::CloneOnly => ARC_CLONE_ONLY,
            Self::CloneDrop => ARC_CLONE_DROP,
            Self::DropNonfinal => ARC_DROP_NONFINAL,
        }
    }
}

fn standard() -> StdArc<u64> {
    StdArc::new(7)
}

fn per_process() -> Arc<u64, PerProcess> {
    Arc::new(7)
}

fn per_thread(partitions: usize, allocations: Allocations) -> Arc<u64, PerThread> {
    if partitions == 0 {
        return Arc::new_with(|| 7);
    }

    let builder = ThreadBuilder::default();
    let current = builder.build(thread::current().id());
    let shared = Arc::new(7);
    let values = (0..partitions).map(|index| {
        let coordinate = if index == 0 {
            current.clone()
        } else {
            let id = thread::spawn(|| thread::current().id())
                .join()
                .expect("coordinate thread only reads its ID");
            builder.build(id)
        };
        let value = match allocations {
            Allocations::Distinct => Arc::new(7),
            Allocations::Shared => shared.clone(),
        };
        (coordinate, value)
    });
    let value = Arc::<u64, PerThread>::try_from_values(&current, values)
        .expect("coordinates have one owner, distinct IDs, and include the current thread");
    drop(shared);
    assert_eq!(Arc::strong_count(&value), 1);
    value
}

fn drop_input<T: Clone>(value: T) -> (T, T) {
    let cloned = value.clone();
    (value, cloned)
}

#[metabench::benchmark(ARC_CLONE_ONLY, CLONE_ONLY, "arc")]
#[bench::std(&configured(standard(), Recording::Disabled))]
#[bench::per_process(&configured(per_process(), Recording::Disabled))]
#[bench::per_thread_empty(&configured(per_thread(0, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_distinct_1(&configured(per_thread(1, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_distinct_8(&configured(per_thread(8, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_distinct_64(&configured(per_thread(64, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_shared_64(&configured(per_thread(64, Allocations::Shared), Recording::Disabled))]
#[cfg_attr(feature = "seismograph", bench::std_recording(&configured(standard(), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_process_recording(&configured(per_process(), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_empty_recording(&configured(per_thread(0, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_distinct_1_recording(&configured(per_thread(1, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_distinct_8_recording(&configured(per_thread(8, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_distinct_64_recording(&configured(per_thread(64, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_shared_64_recording(&configured(per_thread(64, Allocations::Shared), Recording::Enabled)))]
fn clone_only<T: Clone>(value: &T) -> T {
    black_box(black_box(value).clone())
}

#[metabench::benchmark(ARC_CLONE_DROP, CLONE_DROP, "arc")]
#[bench::std(&configured(standard(), Recording::Disabled))]
#[bench::per_process(&configured(per_process(), Recording::Disabled))]
#[bench::per_thread_empty(&configured(per_thread(0, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_distinct_1(&configured(per_thread(1, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_distinct_8(&configured(per_thread(8, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_distinct_64(&configured(per_thread(64, Allocations::Distinct), Recording::Disabled))]
#[bench::per_thread_shared_64(&configured(per_thread(64, Allocations::Shared), Recording::Disabled))]
#[cfg_attr(feature = "seismograph", bench::std_recording(&configured(standard(), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_process_recording(&configured(per_process(), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_empty_recording(&configured(per_thread(0, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_distinct_1_recording(&configured(per_thread(1, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_distinct_8_recording(&configured(per_thread(8, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_distinct_64_recording(&configured(per_thread(64, Allocations::Distinct), Recording::Enabled)))]
#[cfg_attr(feature = "seismograph", bench::per_thread_shared_64_recording(&configured(per_thread(64, Allocations::Shared), Recording::Enabled)))]
fn clone_drop<T: Clone>(value: &T) -> &T {
    drop(black_box(black_box(value).clone()));
    value
}

#[metabench::benchmark(ARC_DROP_NONFINAL, DROP_NONFINAL, "arc")]
#[bench::std(drop_input(configured(standard(), Recording::Disabled)))]
#[bench::per_process(drop_input(configured(per_process(), Recording::Disabled)))]
#[bench::per_thread_empty(drop_input(configured(per_thread(0, Allocations::Distinct), Recording::Disabled)))]
#[bench::per_thread_distinct_1(drop_input(configured(per_thread(1, Allocations::Distinct), Recording::Disabled)))]
#[bench::per_thread_distinct_8(drop_input(configured(per_thread(8, Allocations::Distinct), Recording::Disabled)))]
#[bench::per_thread_distinct_64(drop_input(configured(per_thread(64, Allocations::Distinct), Recording::Disabled)))]
#[bench::per_thread_shared_64(drop_input(configured(per_thread(64, Allocations::Shared), Recording::Disabled)))]
#[cfg_attr(
    feature = "seismograph",
    bench::std_recording(drop_input(configured(standard(), Recording::Enabled)))
)]
#[cfg_attr(
    feature = "seismograph",
    bench::per_process_recording(drop_input(configured(per_process(), Recording::Enabled)))
)]
#[cfg_attr(
    feature = "seismograph",
    bench::per_thread_empty_recording(drop_input(configured(per_thread(0, Allocations::Distinct), Recording::Enabled)))
)]
#[cfg_attr(
    feature = "seismograph",
    bench::per_thread_distinct_1_recording(drop_input(configured(per_thread(1, Allocations::Distinct), Recording::Enabled)))
)]
#[cfg_attr(
    feature = "seismograph",
    bench::per_thread_distinct_8_recording(drop_input(configured(per_thread(8, Allocations::Distinct), Recording::Enabled)))
)]
#[cfg_attr(
    feature = "seismograph",
    bench::per_thread_distinct_64_recording(drop_input(configured(per_thread(64, Allocations::Distinct), Recording::Enabled)))
)]
#[cfg_attr(
    feature = "seismograph",
    bench::per_thread_shared_64_recording(drop_input(configured(per_thread(64, Allocations::Shared), Recording::Enabled)))
)]
fn drop_nonfinal<T>((retained, cloned): (T, T)) -> T {
    drop(black_box(cloned));
    black_box(retained)
}

fn benchmark_case<T: Clone>(group: &mut BenchmarkGroup<'_, WallTime>, operation: Operation, case: &str, value: T, recording: Recording) {
    let value = configured(value, recording);
    let case = format!("{case}{}", recording.suffix());
    let id = BenchmarkId::new(operation.identity().benchmark_name(), case);
    group.bench_function(id, |bencher| match operation {
        Operation::CloneOnly => {
            bencher.iter_batched(|| (), |()| clone_only(&value), BATCH_SIZE);
        }
        Operation::CloneDrop => bencher.iter(|| clone_drop(&value)),
        Operation::DropNonfinal => {
            bencher.iter_batched(|| drop_input(value.clone()), drop_nonfinal, BATCH_SIZE);
        }
    });
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    for operation in [Operation::CloneOnly, Operation::CloneDrop, Operation::DropNonfinal] {
        let mut group = criterion.benchmark_group(operation.identity().group_name());
        for recording in [
            Recording::Disabled,
            #[cfg(feature = "seismograph")]
            Recording::Enabled,
        ] {
            benchmark_case(&mut group, operation, "std", standard(), recording);
            benchmark_case(&mut group, operation, "per_process", per_process(), recording);
            for (partitions, case) in PARTITIONS {
                benchmark_case(
                    &mut group,
                    operation,
                    case,
                    per_thread(partitions, Allocations::Distinct),
                    recording,
                );
            }
            benchmark_case(
                &mut group,
                operation,
                "per_thread_shared_64",
                per_thread(64, Allocations::Shared),
                recording,
            );
        }
        group.finish();
    }
    Recording::Disabled.configure();
}

metabench::main!(
    criterion = criterion_benchmarks,
    benchmarks = [ARC_CLONE_ONLY, ARC_CLONE_DROP, ARC_DROP_NONFINAL],
);
