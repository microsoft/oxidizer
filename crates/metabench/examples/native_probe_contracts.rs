// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native iterator cases and lifecycle hooks without Criterion registration.

use std::hint::black_box;
use std::io::Write as _;
use std::sync::OnceLock;

static TEST_OPTIONS: OnceLock<TestOptions> = OnceLock::new();

struct TestOptions {
    require_untracked: bool,
    ordering: bool,
    panic_workload: bool,
}

fn event(value: &str) {
    if let Some(path) = std::env::var_os("METABENCH_TEST_LIFECYCLE") {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .expect("open lifecycle log");
        writeln!(file, "{value}").expect("write lifecycle log");
    }
}

fn suite_setup() {
    TEST_OPTIONS.get_or_init(|| TestOptions {
        require_untracked: std::env::var_os("METABENCH_TEST_REQUIRE_UNTRACKED").is_some(),
        ordering: std::env::var_os("METABENCH_TEST_ORDERING").is_some(),
        panic_workload: std::env::var_os("METABENCH_TEST_PANIC_WORKLOAD").is_some(),
    });
    println!("native suite setup");
    event("suite setup");
}
fn suite_teardown() {
    event("suite teardown");
}
fn group_setup() {
    event("group setup");
}
fn group_teardown() {
    event("group teardown");
}

fn prepare(size: usize) -> Vec<u8> {
    event("case setup");
    black_box(vec![1; size])
}

fn cleanup(output: (Vec<u8>, Vec<u8>)) {
    event("case teardown");
    assert_eq!(output.0.len(), output.1.len());
    drop(output);
    drop(black_box(vec![1; 256]));
}

#[metabench::benchmark(ITER, "contracts", "iterator", gungraun_setup = prepare, gungraun_teardown = cleanup)]
#[benches::sizes(iter = [8, 16])]
#[cfg_attr(all(), bench::conditional(args = (24), setup = prepare, teardown = cleanup))]
#[cfg_attr(any(), bench::disabled(32))]
fn allocate(input: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    // Windows environment lookups allocate even when the variable is absent.
    let options = TEST_OPTIONS
        .get()
        .expect("suite setup initializes test options before workloads run");
    if options.require_untracked {
        let session = alloc_tracker::Session::new().no_stdout().no_file();
        let span = session.operation("untracked").measure_process().iterations(1);
        drop(black_box(vec![42; 128]));
        drop(span);
        for (_, operation) in session.to_report().operations() {
            assert_eq!(
                operation.total_allocations_count(),
                0,
                "hardware workers must bypass allocation tracking"
            );
            assert_eq!(
                operation.total_bytes_allocated(),
                0,
                "hardware workers must bypass allocation tracking"
            );
        }
    }
    if options.ordering {
        event("workload entry");
        assert!(!options.panic_workload, "native workload panic");
    }
    let output = black_box(vec![42; black_box(input.len())]);
    if options.ordering {
        event("workload exit");
    }
    (input, output)
}

#[expect(clippy::panic, reason = "process fixture fails if a probe executes Criterion registration")]
fn criterion_benchmarks(_: &mut criterion::Criterion) {
    panic!("native probes must not execute Criterion registration");
}

metabench::main!(
    criterion = criterion_benchmarks,
    gungraun = { setup = suite_setup(); teardown = suite_teardown(); },
    groups = {
        CONTRACTS {
            benchmarks = [ITER],
            gungraun_setup = group_setup(),
            gungraun_teardown = group_teardown(),
        },
    },
);
