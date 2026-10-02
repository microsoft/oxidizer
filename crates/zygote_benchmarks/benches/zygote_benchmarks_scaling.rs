// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Sequential and concurrent launch-scaling benchmarks.

#![allow(
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::unwrap_used,
    reason = "benchmark code needs no API documentation and should fail fast"
)]

mod support;

use std::hint::black_box;
use std::io::Read;
#[cfg(target_os = "linux")]
use std::io::Write;
use std::sync::{Arc, Barrier, OnceLock};
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput};
use support::{ConcurrentRunner, InputShape, TRANSPARENT, accelerated_exit, start};
#[cfg(target_os = "linux")]
use zygote_control::PreforkPoolConfig;
use zygote_control::{Launcher, Stdio, Zygote};

const LAUNCHES_GROUP: &str = "zygote_benchmarks_scaling/launches";

#[cfg(target_os = "linux")]
fn controller_cpu_us() -> i128 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the complete output structure on success.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) }, 0);
    // SAFETY: the successful getrusage call initialized the structure.
    let usage = unsafe { usage.assume_init() };
    i128::from(usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) * 1_000_000 + i128::from(usage.ru_utime.tv_usec + usage.ru_stime.tv_usec)
}

fn entry_latency(launcher: &Launcher) -> Duration {
    let mut command = launcher.command();
    command
        .arg("entry")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let submitted = Instant::now();
    let mut child = command.spawn().unwrap();
    let mut marker = [0];
    child.stdout.as_mut().unwrap().read_exact(&mut marker).unwrap();
    assert_eq!(marker, [b'E']);
    let elapsed = submitted.elapsed();
    assert!(child.wait().unwrap().success());
    elapsed
}

fn report_latency(label: &str, launchers: &[Launcher], threads: usize) {
    let barrier = Arc::new(Barrier::new(threads));
    #[cfg(target_os = "linux")]
    let cpu_before = controller_cpu_us();
    #[cfg(target_os = "linux")]
    let wall_before = Instant::now();
    let samples = std::thread::scope(|scope| {
        (0..threads)
            .map(|index| {
                let barrier = Arc::clone(&barrier);
                let launcher = &launchers[index % launchers.len()];
                scope.spawn(move || {
                    barrier.wait();
                    (0..32).map(|_| entry_latency(launcher)).collect::<Vec<_>>()
                })
            })
            .flat_map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    #[cfg(target_os = "linux")]
    let wall = wall_before.elapsed();
    #[cfg(target_os = "linux")]
    eprintln!(
        "{label} threads={threads} controller_cpu_us={} wall_us={} launches_per_s={} (CPU includes caller/controller threads, excludes child processes)",
        controller_cpu_us() - cpu_before,
        wall.as_micros(),
        (samples.len() as u128 * 1_000_000) / wall.as_micros().max(1)
    );
    let mut samples = samples;
    samples.sort_unstable();
    let percentile = |numerator: usize| samples[(samples.len() * numerator / 100).min(samples.len() - 1)].as_micros();
    eprintln!(
        "{label} threads={threads} request_to_entry_us p50={} p95={} p99={} n={}",
        percentile(50),
        percentile(95),
        percentile(99),
        samples.len()
    );
}

#[cfg(target_os = "linux")]
fn report_occupancy(launcher: &Launcher, held_count: usize, burst_exit: bool) {
    let mut held = Vec::with_capacity(held_count);
    for _ in 0..held_count {
        let mut child = launcher
            .command()
            .arg("hold")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut ready = [0];
        child.stdout.as_mut().unwrap().read_exact(&mut ready).unwrap();
        assert_eq!(ready, [b'R']);
        held.push(child);
    }
    let cpu_before = controller_cpu_us();
    let dispatch = entry_latency(launcher);
    let dispatch_cpu = controller_cpu_us() - cpu_before;
    let exit_cpu_before = controller_cpu_us();
    let mut exits = Vec::new();
    if burst_exit {
        for child in &mut held {
            child.stdin.as_mut().unwrap().write_all(&[0]).unwrap();
        }
    }
    for child in &mut held {
        let start = Instant::now();
        if !burst_exit {
            child.stdin.as_mut().unwrap().write_all(&[0]).unwrap();
        }
        assert!(child.wait().unwrap().success());
        exits.push(start.elapsed());
    }
    eprintln!(
        "occupancy held={held_count} exit_rate={} entry_us={} dispatch_controller_cpu_us={dispatch_cpu} exit_wait_us_total={} exit_controller_cpu_us={} (exit includes reaping and caller bookkeeping)",
        if burst_exit { "burst" } else { "sequential" },
        dispatch.as_micros(),
        exits.iter().sum::<Duration>().as_micros(),
        controller_cpu_us() - exit_cpu_before
    );
}

fn representative_runner() -> &'static ConcurrentRunner {
    static RUNNER: OnceLock<ConcurrentRunner> = OnceLock::new();
    RUNNER.get_or_init(|| {
        let zygote = Box::leak(Box::new(start(TRANSPARENT, 1)));
        let launcher = zygote.launcher();
        ConcurrentRunner::new(4, move |_| accelerated_exit(&launcher, InputShape::Empty))
    })
}

#[metabench::benchmark(REPRESENTATIVE_SCALING, LAUNCHES_GROUP, "one_zygote/4")]
fn representative_scaling() -> u64 {
    representative_runner().run()
}

fn criterion_benchmarks(c: &mut Criterion) {
    let _representative_runner = representative_runner();
    let mut group = c.benchmark_group(LAUNCHES_GROUP);
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));

    for thread_count in [1_usize, 2, 4, 8, 16, 32] {
        group.throughput(Throughput::Elements(thread_count.try_into().unwrap()));

        let single = start(TRANSPARENT, 1);
        let single_launcher = single.launcher();
        report_latency("one_zygote", std::slice::from_ref(&single_launcher), thread_count);
        #[cfg(target_os = "linux")]
        if thread_count == 1 {
            for held in [0, 8, 32] {
                report_occupancy(&single_launcher, held, false);
                if held != 0 {
                    report_occupancy(&single_launcher, held, true);
                }
            }
        }
        let single_runner = ConcurrentRunner::new(thread_count, move |_| accelerated_exit(&single_launcher, InputShape::Empty));
        group.bench_function(BenchmarkId::new("one_zygote", thread_count), |b| {
            if thread_count == 4 {
                b.iter(|| black_box(representative_scaling()));
            } else {
                b.iter(|| black_box(single_runner.run()));
            }
        });
        drop(single_runner);
        single.shutdown().unwrap();

        let sharded = start(TRANSPARENT, thread_count);
        let sharded_launcher = sharded.launcher();
        report_latency("internal_shards", std::slice::from_ref(&sharded_launcher), thread_count);
        let sharded_runner = ConcurrentRunner::new(thread_count, move |_| accelerated_exit(&sharded_launcher, InputShape::Empty));
        group.bench_function(BenchmarkId::new("internal_shards", thread_count), |b| {
            b.iter(|| black_box(sharded_runner.run()));
        });
        drop(sharded_runner);
        sharded.shutdown().unwrap();

        let pooled = (0..thread_count).map(|_| start(TRANSPARENT, 1)).collect::<Vec<_>>();
        let pooled_launchers = pooled.iter().map(Zygote::launcher).collect::<Vec<_>>();
        report_latency("caller_managed_pool", &pooled_launchers, thread_count);
        let pooled_runner = ConcurrentRunner::new(thread_count, move |index| {
            accelerated_exit(&pooled_launchers[index], InputShape::Empty)
        });
        group.bench_function(BenchmarkId::new("caller_managed_pool", thread_count), |b| {
            b.iter(|| black_box(pooled_runner.run()));
        });
        drop(pooled_runner);
        for zygote in pooled {
            zygote.shutdown().unwrap();
        }
    }
    group.finish();

    #[cfg(target_os = "linux")]
    {
        let mut prefork = c.benchmark_group("zygote_benchmarks_scaling/prefork_burst");
        prefork.sample_size(10);
        for pool_size in [0_usize, 1, 2, 4, 8, 16] {
            let mut builder = Zygote::builder(TRANSPARENT);
            builder.prefork_pool(PreforkPoolConfig::new(pool_size).unwrap());
            let zygote = builder.spawn().unwrap();
            let launcher = zygote.launcher();
            report_latency("prefork_burst", std::slice::from_ref(&launcher), 32);
            let runner = ConcurrentRunner::new(32, move |_| accelerated_exit(&launcher, InputShape::Empty));
            prefork.bench_function(BenchmarkId::from_parameter(pool_size), |b| {
                b.iter(|| black_box(runner.run()));
            });
            drop(runner);
            eprintln!("prefork burst size={pool_size} health={:?}", zygote.health());
            zygote.shutdown().unwrap();
        }
        prefork.finish();
    }
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [REPRESENTATIVE_SCALING],);
