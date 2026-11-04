// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Prepared cases with unmeasured calls to annotated functions during setup.

use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion};

// Allocating destruction must follow collection; return this value to defer it.
struct Output(Vec<u8>);

impl Drop for Output {
    fn drop(&mut self) {
        black_box(self.0.len());
        drop(black_box(vec![42; 64]));
    }
}

// Preparation and allocating destruction stay outside collection by returning ownership.
struct Input {
    size: usize,
    prepared: Vec<u8>,
}

impl Drop for Input {
    fn drop(&mut self) {
        black_box(self.prepared.len());
        drop(black_box(vec![42; 64]));
    }
}

fn input(size: usize) -> Input {
    Input {
        size,
        prepared: vec![42; 11],
    }
}

#[expect(
    clippy::unnecessary_box_returns,
    reason = "the fixture verifies that boxed input setup and destruction stay outside measurement"
)]
fn prepare(size: usize) -> Box<Input> {
    if std::env::var_os("METABENCH_TEST_TIMEOUT").is_some() {
        eprintln!("selected workload is waiting");
        std::thread::sleep(std::time::Duration::from_mins(1));
    }
    assert!(std::env::var_os("METABENCH_TEST_FAIL_SETUP").is_none(), "native setup failed");
    drop(other());
    drop(allocate(input(5)));
    drop(allocate(input(size)));
    Box::new(input(size))
}

#[metabench::benchmark(ALLOCATE, "boundaries", "allocate", gungraun_setup = prepare)]
#[bench::small(32)]
#[bench::large(128)]
fn allocate<S: std::borrow::BorrowMut<Input>>(mut input: S) -> (S, Output) {
    let output = Output(black_box(vec![42; black_box(input.borrow_mut().size)]));
    (input, output)
}

#[metabench::benchmark(OTHER, "boundaries", "other")]
fn other() -> Vec<u8> {
    black_box(vec![42; 7])
}

#[metabench::benchmark(CONST, "boundaries", "const")]
const fn constant() -> u64 {
    42
}

fn criterion_benchmarks(criterion: &mut Criterion) {
    assert!(
        std::env::var_os("METABENCH_TEST_NO_CRITERION").is_none(),
        "Criterion must not run for native probes"
    );
    if std::env::var_os("METABENCH_TEST_REQUIRE_UNTRACKED").is_some() {
        let session = alloc_tracker::Session::new().no_stdout().no_file();
        let span = session.operation("untracked").measure_process().iterations(1);
        drop(black_box(vec![42; 128]));
        drop(span);
        let report = session.to_report();
        for (_, operation) in report.operations() {
            assert_eq!(operation.total_allocations_count(), 0, "Criterion must bypass allocation tracking");
            assert_eq!(operation.total_bytes_allocated(), 0, "Criterion must bypass allocation tracking");
        }
    }
    drop(other());
    drop(allocate(input(3)));
    let mut group = criterion.benchmark_group(ALLOCATE.group_name());
    for (case, size) in [("small", 32), ("large", 128)] {
        group.bench_function(BenchmarkId::new(ALLOCATE.benchmark_name(), case), |bencher| {
            bencher.iter_batched_ref(|| prepare(size), |input| allocate(input.as_mut()).1, BatchSize::SmallInput);
        });
    }
    group.bench_function(CONST.benchmark_name(), |bencher| bencher.iter(constant));
    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [ALLOCATE, OTHER, CONST],);
