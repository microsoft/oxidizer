// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! End-to-end launch latency and memory benchmarks.

#![allow(
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::unwrap_used,
    reason = "benchmark code needs no API documentation and should fail fast"
)]

mod support;

use std::hint::black_box;
use std::sync::OnceLock;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion};
#[cfg(target_os = "linux")]
use support::observe_controller_and_templates;
use support::{
    InputShape, PREPARED_1_MIB, PREPARED_64_MIB, PREPARED_EMPTY, TRANSPARENT, accelerated_entry, accelerated_exit, native_entry,
    native_exit, start,
};
#[cfg(target_os = "linux")]
use support::{observe_accelerated_memory, observe_native_memory, write_memory_observations};
use zygote_control::Launcher;
#[cfg(target_os = "linux")]
use zygote_control::{PreforkPoolConfig, Zygote};

const REQUEST_TO_EXIT_GROUP: &str = "zygote_benchmarks_launch/request_to_exit";

fn representative_launcher() -> &'static Launcher {
    static LAUNCHER: OnceLock<Launcher> = OnceLock::new();
    LAUNCHER.get_or_init(|| {
        let zygote = Box::leak(Box::new(start(PREPARED_1_MIB, 1)));
        zygote.launcher()
    })
}

#[metabench::benchmark(REPRESENTATIVE_LAUNCH, REQUEST_TO_EXIT_GROUP, "prepared_1m/accelerated/representative")]
fn representative_launch() -> u64 {
    accelerated_exit(representative_launcher(), InputShape::Representative)
}

fn criterion_benchmarks(c: &mut Criterion) {
    let _representative_launcher = representative_launcher();
    let fixtures = [
        ("transparent", TRANSPARENT),
        ("prepared_0", PREPARED_EMPTY),
        ("prepared_1m", PREPARED_1_MIB),
        ("prepared_64m", PREPARED_64_MIB),
    ];
    for input in InputShape::ALL {
        eprintln!(
            "launch input={} argument+environment raw_bytes={} (excluding argv[0], separators, Windows quoting and wire framing)",
            input.name(),
            input.payload_bytes()
        );
    }
    eprintln!("new-template startup is measured separately; warm samples below do not represent a cold filesystem cache");
    let mut startup = c.benchmark_group("zygote_benchmarks_launch/new_template");
    startup.sample_size(10);
    for (mode, program) in fixtures {
        startup.bench_function(mode, |b| b.iter(|| start(program, 1).shutdown().unwrap()));
    }
    startup.finish();

    let mut entry = c.benchmark_group("zygote_benchmarks_launch/request_to_entry");
    entry.sample_size(10);
    entry.warm_up_time(Duration::from_secs(1));
    entry.measurement_time(Duration::from_secs(5));
    for (mode, program) in fixtures {
        let zygote = start(program, 1);
        let launcher = zygote.launcher();
        for input in InputShape::ALL {
            entry.bench_function(BenchmarkId::new(format!("{mode}/native"), input.name()), |b| {
                b.iter(|| black_box(native_entry(program, input)));
            });
            entry.bench_function(BenchmarkId::new(format!("{mode}/accelerated"), input.name()), |b| {
                b.iter(|| black_box(accelerated_entry(&launcher, input)));
            });
        }
        zygote.shutdown().unwrap();
    }
    entry.finish();

    let mut exit = c.benchmark_group(REQUEST_TO_EXIT_GROUP);
    exit.sample_size(10);
    exit.warm_up_time(Duration::from_secs(1));
    exit.measurement_time(Duration::from_secs(5));
    for (mode, program) in fixtures {
        let zygote = start(program, 1);
        let launcher = zygote.launcher();
        for input in InputShape::ALL {
            exit.bench_function(BenchmarkId::new(format!("{mode}/native"), input.name()), |b| {
                b.iter(|| black_box(native_exit(program, input)));
            });
            exit.bench_function(BenchmarkId::new(format!("{mode}/accelerated"), input.name()), |b| {
                if mode == "prepared_1m" && matches!(input, InputShape::Representative) {
                    b.iter(|| black_box(representative_launch()));
                } else {
                    b.iter(|| black_box(accelerated_exit(&launcher, input)));
                }
            });
        }
        zygote.shutdown().unwrap();
    }
    exit.finish();
    let mut alternating = c.benchmark_group("zygote_benchmarks_launch/alternating");
    alternating.sample_size(10);
    for (mode, program) in fixtures {
        let zygote = start(program, 1);
        let launcher = zygote.launcher();
        alternating.bench_function(BenchmarkId::new(mode, "empty_then_large"), |b| {
            b.iter(|| {
                black_box(accelerated_exit(&launcher, InputShape::Empty));
                black_box(accelerated_exit(&launcher, InputShape::Large));
            });
        });
        zygote.shutdown().unwrap();
    }
    alternating.finish();
    #[cfg(target_os = "linux")]
    memory_observation();
    #[cfg(target_os = "linux")]
    prefork_observation(c);
}

#[cfg(target_os = "linux")]
fn prefork_observation(c: &mut Criterion) {
    let mut group = c.benchmark_group("zygote_benchmarks_launch/prefork_pool");
    group.sample_size(10);
    for size in [0_usize, 1, 2, 4, 8, 16] {
        let mut builder = Zygote::builder(PREPARED_1_MIB);
        builder.prefork_pool(PreforkPoolConfig::new(size).unwrap());
        let zygote = builder.spawn().unwrap();
        let launcher = zygote.launcher();
        eprintln!("prefork size={size} initial_health={:?}", zygote.health());
        observe_controller_and_templates();
        group.bench_function(BenchmarkId::new("request_to_entry", size), |b| {
            b.iter(|| black_box(accelerated_entry(&launcher, InputShape::Representative)));
        });
        group.bench_function(BenchmarkId::new("request_to_exit", size), |b| {
            b.iter(|| black_box(accelerated_exit(&launcher, InputShape::Representative)));
        });
        eprintln!("prefork size={size} final_health={:?}", zygote.health());
        zygote.shutdown().unwrap();
    }
    group.finish();
}

#[cfg(target_os = "linux")]
fn memory_observation() {
    let fixtures = [
        ("transparent", TRANSPARENT),
        ("prepared_0", PREPARED_EMPTY),
        ("prepared_1m", PREPARED_1_MIB),
        ("prepared_64m", PREPARED_64_MIB),
    ];
    let mut observations = Vec::with_capacity(fixtures.len() * 2);
    for (mode, program) in fixtures {
        let zygote = start(program, 1);
        let launcher = zygote.launcher();
        eprintln!("memory mode={mode} controller and template snapshot:");
        observe_controller_and_templates();
        observations.push((mode, "native", observe_native_memory(program)));
        observations.push((mode, "accelerated", observe_accelerated_memory(&launcher)));
        zygote.shutdown().unwrap();
    }
    write_memory_observations(&observations);
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [REPRESENTATIVE_LAUNCH],);
