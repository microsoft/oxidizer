// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Captured-output size and concurrency benchmarks.

#![allow(
    missing_docs,
    clippy::missing_docs_in_private_items,
    clippy::unwrap_used,
    reason = "benchmark code needs no API documentation and should fail fast"
)]

mod support;

use std::hint::black_box;
#[cfg(any(target_os = "linux", windows))]
use std::sync::Arc;
use std::sync::OnceLock;
#[cfg(any(target_os = "linux", windows))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};
use support::{ConcurrentRunner, TRANSPARENT, start};
use zygote_control::Launcher;

// The stress tier reaches 256 MiB of captured data at concurrency 32.
const STRESS_BYTES_PER_STREAM: usize = 4 << 20;
const CAPTURED_GROUP: &str = "zygote_benchmarks_output/captured";
const REPRESENTATIVE_BYTES: usize = 1 << 20;
const REPRESENTATIVE_CONCURRENCY: usize = 8;

fn capture(launcher: &Launcher, bytes_per_stream: usize) -> u64 {
    let size = bytes_per_stream.to_string();
    let output = launcher.command().args(["emit", size.as_str(), size.as_str()]).output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), bytes_per_stream);
    assert_eq!(output.stderr.len(), bytes_per_stream);
    assert!(output.stdout.iter().all(|byte| *byte == b'o'));
    assert!(output.stderr.iter().all(|byte| *byte == b'e'));
    u64::try_from(output.stdout.len() + output.stderr.len()).unwrap()
}

fn representative_runner() -> &'static ConcurrentRunner {
    static RUNNER: OnceLock<ConcurrentRunner> = OnceLock::new();
    RUNNER.get_or_init(|| {
        let zygote = Box::leak(Box::new(start(TRANSPARENT, 1)));
        let launcher = zygote.launcher();
        ConcurrentRunner::new(REPRESENTATIVE_CONCURRENCY, move |_| capture(&launcher, REPRESENTATIVE_BYTES))
    })
}

#[cfg(target_os = "linux")]
fn process_stats() -> (u64, u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let value = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .unwrap()
            .split_ascii_whitespace()
            .next()
            .unwrap()
            .parse::<u64>()
            .unwrap()
    };
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the complete output structure on success.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) }, 0);
    // SAFETY: the successful getrusage call initialized the structure.
    let usage = unsafe { usage.assume_init() };
    (
        value("VmRSS:"),
        value("Threads:"),
        (usage.ru_nvcsw + usage.ru_nivcsw).try_into().unwrap(),
    )
}

#[cfg(target_os = "linux")]
fn observe_output(runner: &ConcurrentRunner, size: usize, concurrency: usize) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let before = process_stats();
    let observer = std::thread::spawn(move || {
        let mut peak_rss = 0;
        let mut peak_threads = 0;
        while !flag.load(Ordering::Relaxed) {
            let (rss, threads, _) = process_stats();
            peak_rss = peak_rss.max(rss);
            peak_threads = peak_threads.max(threads);
            std::thread::sleep(Duration::from_millis(1));
        }
        (peak_rss, peak_threads)
    });
    let started = std::time::Instant::now();
    runner.run();
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (peak_rss, peak_threads) = observer.join().unwrap();
    let after = process_stats();
    eprintln!(
        "output bytes_per_stream={size} concurrency={concurrency} elapsed_us={} sampled_peak_rss_kib={} sampled_peak_threads={} process_context_switches_delta={} captured_bytes={} (1ms sampler)",
        elapsed.as_micros(),
        peak_rss.max(before.0).max(after.0),
        peak_threads.max(before.1).max(after.1),
        after.2.saturating_sub(before.2),
        size * 2 * concurrency
    );
}

#[cfg(windows)]
fn process_stats() -> (u64, u64, u64) {
    use std::mem::zeroed;

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount};

    // SAFETY: GetCurrentProcess returns the current process pseudo-handle.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: zero is a valid initial state for this output structure.
    let mut memory: PROCESS_MEMORY_COUNTERS = unsafe { zeroed() };
    let memory_size =
        u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).expect("PROCESS_MEMORY_COUNTERS size always fits the Windows u32 ABI field");
    memory.cb = memory_size;
    // SAFETY: process is the current process and memory is writable for memory_size bytes.
    assert_ne!(unsafe { K32GetProcessMemoryInfo(process, &raw mut memory, memory_size) }, 0);
    let mut handles = 0;
    // SAFETY: process is the current process and handles is writable.
    assert_ne!(unsafe { GetProcessHandleCount(process, &raw mut handles) }, 0);
    // SAFETY: the flags and process identifier are documented snapshot arguments.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    assert_ne!(snapshot, INVALID_HANDLE_VALUE);
    // SAFETY: zero is a valid initial state before setting dwSize.
    let mut entry: THREADENTRY32 = unsafe { zeroed() };
    entry.dwSize = u32::try_from(size_of::<THREADENTRY32>()).expect("THREADENTRY32 size always fits the Windows u32 ABI field");
    let mut threads = 0u64;
    // SAFETY: snapshot is live and entry has the required size.
    if unsafe { Thread32First(snapshot, &raw mut entry) } != 0 {
        loop {
            // SAFETY: GetCurrentProcessId has no pointer arguments.
            if entry.th32OwnerProcessID == unsafe { GetCurrentProcessId() } {
                threads += 1;
            }
            // SAFETY: snapshot and entry remain valid for enumeration.
            if unsafe { Thread32Next(snapshot, &raw mut entry) } == 0 {
                break;
            }
        }
    }
    // SAFETY: snapshot is owned by this function and closed exactly once.
    assert_ne!(unsafe { CloseHandle(snapshot) }, 0);
    (
        u64::try_from(memory.WorkingSetSize).expect("Windows working-set size fits u64") / 1024,
        threads,
        u64::from(handles),
    )
}

#[cfg(windows)]
fn observe_output(runner: &ConcurrentRunner, size: usize, concurrency: usize) {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let before = process_stats();
    let observer = std::thread::spawn(move || {
        let mut peak_working_set = 0;
        let mut peak_threads = 0;
        let mut peak_handles = 0;
        while !flag.load(Ordering::Relaxed) {
            let (working_set, threads, handles) = process_stats();
            peak_working_set = peak_working_set.max(working_set);
            peak_threads = peak_threads.max(threads);
            peak_handles = peak_handles.max(handles);
            std::thread::sleep(Duration::from_millis(1));
        }
        (peak_working_set, peak_threads, peak_handles)
    });
    let started = std::time::Instant::now();
    runner.run();
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (peak_working_set, peak_threads, peak_handles) = observer.join().unwrap();
    let after = process_stats();
    eprintln!(
        "output bytes_per_stream={size} concurrency={concurrency} elapsed_us={} sampled_peak_working_set_kib={} sampled_peak_threads={} sampled_peak_handles={} captured_bytes={} context_switches=unavailable",
        elapsed.as_micros(),
        peak_working_set.max(before.0).max(after.0),
        peak_threads.max(before.1).max(after.1),
        peak_handles.max(before.2).max(after.2),
        size * 2 * concurrency
    );
}

#[metabench::benchmark(REPRESENTATIVE_OUTPUT, CAPTURED_GROUP, "bytes_1048576/concurrency_8")]
fn representative_output() -> u64 {
    representative_runner().run()
}

fn criterion_benchmarks(c: &mut Criterion) {
    let _representative_runner = representative_runner();
    let mut group = c.benchmark_group(CAPTURED_GROUP);
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));

    for bytes_per_stream in [0_usize, 1 << 10, 1 << 20, STRESS_BYTES_PER_STREAM] {
        for concurrency in [1_usize, 8, 32] {
            let bytes_per_iteration = bytes_per_stream
                .checked_mul(2)
                .and_then(|bytes| bytes.checked_mul(concurrency))
                .unwrap();
            group.throughput(Throughput::Bytes(bytes_per_iteration.try_into().unwrap()));

            let zygote = start(TRANSPARENT, 1);
            let launcher = zygote.launcher();
            let runner = ConcurrentRunner::new(concurrency, move |_| capture(&launcher, bytes_per_stream));
            #[cfg(any(target_os = "linux", windows))]
            observe_output(&runner, bytes_per_stream, concurrency);
            group.bench_function(
                BenchmarkId::new(format!("bytes_{bytes_per_stream}"), format!("concurrency_{concurrency}")),
                |b| {
                    if bytes_per_stream == REPRESENTATIVE_BYTES && concurrency == REPRESENTATIVE_CONCURRENCY {
                        b.iter(|| black_box(representative_output()));
                    } else {
                        b.iter(|| black_box(runner.run()));
                    }
                },
            );
            drop(runner);
            zygote.shutdown().unwrap();
        }
    }
    group.finish();
}

metabench::main!(criterion = criterion_benchmarks, benchmarks = [REPRESENTATIVE_OUTPUT],);
