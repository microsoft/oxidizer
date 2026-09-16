// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::cast_possible_truncation,
    clippy::multiple_unsafe_ops_per_block,
    clippy::needless_pass_by_value,
    reason = "Benchmark callbacks share one signature and use bounded deterministic raw-pointer workloads"
)]

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::hint::black_box;
use std::ptr::NonNull;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use criterion::measurement::WallTime;
use criterion::{BenchmarkGroup, BenchmarkId, Criterion, Throughput};

mod fanout;

const CROSS_THREAD_BATCH_SIZE: usize = 1_024;
const CROSS_THREAD_SIZES: [usize; 6] = [
    16, 4_096, // Retain the 4 KiB page boundary for remote frees.
    16_384, 32_768, 65_536, 98_304,
];
const RETAINED_MEDIUM_BLOCKS: usize = 1_024;
const RETAINED_MEDIUM_SIZE: usize = 32 * 1_024;
const RETAINED_THREAD_COUNTS: [usize; 3] = [1, 8, 64];
const PIPELINE_BATCH: usize = 256;
const PIPELINE_MIXED_SIZES: [usize; 8] = [16_384, 32_768, 32_768, 32_768, 32_768, 65_536, 98_304, 262_144];
const MIXED_LIVE_ALLOCATIONS: usize = 256;
const MIXED_OPERATIONS_PER_THREAD: usize = 100_000;
const MIXED_SMALL_SIZES: [usize; 12] = [8, 16, 24, 42, 64, 96, 256, 1_003, 4_096, 8_192, 12_288, 16_384];
const MIXED_MEDIUM_SIZES: [usize; 6] = [64 * 1_024, 96 * 1_024, 256 * 1_024, 512 * 1_024, 1_024 * 1_024, 2 * 1_024 * 1_024];
const MIXED_LARGE_SIZES: [usize; 2] = [4 * 1_024 * 1_024, 8 * 1_024 * 1_024];
const THREAD_COUNTS: [usize; 2] = [1, 8];

struct OwnedAllocation {
    address: NonNull<u8>,
    layout: Layout,
}

// SAFETY: `OwnedAllocation` uniquely owns the allocation at `address`, carries
// the exact layout needed to free it, and exposes no references into the block.
// Moving it to another thread transfers that unique ownership, and `Drop`
// performs the allocation's only deallocation.
unsafe impl Send for OwnedAllocation {}

impl OwnedAllocation {
    fn new(layout: Layout) -> Self {
        // SAFETY: `layout` is valid and allocation failure is handled below.
        let address = unsafe { alloc(layout) };
        let address = NonNull::new(address).unwrap_or_else(|| handle_alloc_error(layout));
        Self { address, layout }
    }

    fn address(&self) -> usize {
        self.address.as_ptr().addr()
    }

    fn touch_edges(&mut self) {
        // SAFETY: The allocation is live, uniquely owned, and has nonzero size.
        unsafe {
            self.address.as_ptr().write_volatile(0xA5);
            self.address.as_ptr().add(self.layout.size() - 1).write_volatile(0x5A);
        }
    }

    fn touch_pages(&mut self) {
        for offset in (0..self.layout.size()).step_by(4_096) {
            // SAFETY: each offset is inside this uniquely owned allocation.
            unsafe { self.address.as_ptr().add(offset).write_volatile(0xA5) };
        }
    }

    fn read_pages(&self) -> usize {
        (0..self.layout.size())
            .step_by(4_096)
            .map(|offset| {
                // SAFETY: the producer initialized every page marker before
                // transferring this allocation through the synchronized channel.
                usize::from(unsafe { self.address.as_ptr().add(offset).read_volatile() })
            })
            .sum()
    }
}

impl Drop for OwnedAllocation {
    fn drop(&mut self) {
        // SAFETY: This object uniquely owns the live allocation, and `layout`
        // is the same layout that was used to allocate it.
        unsafe { dealloc(self.address.as_ptr(), self.layout) };
    }
}

pub(crate) fn run(file_basename: &str) {
    let mut criterion = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .sample_size(20)
        .configure_from_args();

    #[cfg(feature = "tuning-telemetry")]
    let tuning_output = std::env::var_os("RALLOCATOR_TUNING_OUTPUT").map(|path| {
        assert_eq!(
            file_basename, "rallocator_threaded",
            "tuning diagnostics require the rallocator benchmark"
        );
        rallocator::tuning_telemetry::TuningTelemetry::enable();
        std::path::PathBuf::from(path)
    });
    #[cfg(not(feature = "tuning-telemetry"))]
    assert!(
        std::env::var_os("RALLOCATOR_TUNING_OUTPUT").is_none(),
        "RALLOCATOR_TUNING_OUTPUT requires the tuning-telemetry feature"
    );

    cross_thread_free(&mut criterion, file_basename);
    mixed_scale_throughput(&mut criterion, file_basename);
    retained_medium(&mut criterion, file_basename);
    medium_pipeline(&mut criterion, file_basename);
    medium_fanout(&mut criterion, file_basename);
    criterion.final_summary();

    #[cfg(feature = "tuning-telemetry")]
    if let Some(path) = tuning_output {
        let observation = rallocator::tuning_telemetry::TuningTelemetry::snapshot_if_active()
            .expect("this benchmark explicitly enabled the diagnostic session");
        rallocator::tuning_telemetry::TuningTelemetry::disable();
        // Serialize only after recording stops. The session includes setup,
        // warmup, samples and teardown; diagnostic timing is not a comparison.
        std::fs::write(
            path,
            format!(
                "scope=explicit diagnostic session; includes setup/warmup/samples/teardown\n\
                 counters=independent cumulative events; not occupancy or allocation latency\n\
                 timing=instrumented; do not compare against uninstrumented runs\n{observation}"
            ),
        )
        .expect("diagnostic sidecar must be writable");
    }
}

fn medium_fanout(criterion: &mut Criterion, file_basename: &str) {
    use fanout::{Configuration, Route, Sizes, TailPolicy};

    let mut group = criterion.benchmark_group(format!("{file_basename}/medium_fanout"));
    let available = thread::available_parallelism().expect("benchmark requires CPU count").get();
    #[cfg(target_os = "linux")]
    let processors = allowed_processors();
    #[cfg(not(target_os = "linux"))]
    let processors = Vec::new();
    for threads in [1, 8, 64].into_iter().filter(|&threads| threads <= available) {
        group.throughput(Throughput::Elements((threads * PIPELINE_BATCH) as u64));
        let cases = [
            ("local_32k_w0_p1", Route::Local, Sizes::Uniform32K, TailPolicy::ReclaimUnique, 0, 1),
            ("local_32k_w1_p1", Route::Local, Sizes::Uniform32K, TailPolicy::ReclaimUnique, 1, 1),
            ("local_32k_w4_p1", Route::Local, Sizes::Uniform32K, TailPolicy::ReclaimUnique, 4, 1),
            (
                "remote_32k_w0_p1",
                Route::BroadRemote,
                Sizes::Uniform32K,
                TailPolicy::ReclaimUnique,
                0,
                1,
            ),
            (
                "remote_32k_w1_p1",
                Route::BroadRemote,
                Sizes::Uniform32K,
                TailPolicy::ReclaimUnique,
                1,
                1,
            ),
            (
                "remote_32k_w4_p1",
                Route::BroadRemote,
                Sizes::Uniform32K,
                TailPolicy::ReclaimUnique,
                4,
                1,
            ),
            (
                "remote_32k_w1_p4",
                Route::BroadRemote,
                Sizes::Uniform32K,
                TailPolicy::ReclaimUnique,
                1,
                4,
            ),
            (
                "remote_32k_w1_p16",
                Route::BroadRemote,
                Sizes::Uniform32K,
                TailPolicy::ReclaimUnique,
                1,
                16,
            ),
            (
                "remote_mixed_w1_p1",
                Route::BroadRemote,
                Sizes::Mixed,
                TailPolicy::ReclaimUnique,
                1,
                1,
            ),
            (
                "skewed_32k_w1_p1",
                Route::SkewedRemote,
                Sizes::Uniform32K,
                TailPolicy::ReclaimUnique,
                1,
                1,
            ),
            (
                "detach_32k_w1_p1",
                Route::BroadRemote,
                Sizes::Uniform32K,
                TailPolicy::AlwaysDetach,
                1,
                1,
            ),
        ];
        for (name, route, sizes, tail_policy, retained_bursts, chunks_per_packet) in cases {
            if threads == 1 && route != Route::Local {
                continue;
            }
            let configuration = Configuration {
                threads,
                packets_per_burst: PIPELINE_BATCH / chunks_per_packet,
                chunks_per_packet,
                retained_bursts,
                touched_bytes: 16 * 1024,
                route,
                sizes,
                tail_policy,
            };
            benchmark_fanout(&mut group, name, configuration, &processors);
        }
    }
    group.finish();
}

fn benchmark_fanout(group: &mut BenchmarkGroup<'_, WallTime>, name: &str, configuration: fanout::Configuration, processors: &[usize]) {
    let threads = configuration.threads;
    let placements =
        std::iter::once(("unpinned", Vec::new())).chain((threads <= processors.len()).then(|| ("pinned", processors[..threads].to_vec())));
    for (placement, processors) in placements {
        let id = BenchmarkId::new(format!("{name}_{placement}"), threads);
        let mut workers = None;
        group.bench_function(id, |bencher| {
            // Preserve tails/live windows across warmup and every sample.
            let workers = workers.get_or_insert_with(|| fanout::FanoutWorkers::new(configuration, &processors, fanout_pin_worker));
            bencher.iter_custom(|iterations| {
                #[cfg(target_os = "linux")]
                let before = pipeline_usage();
                let (elapsed, counts) = workers.time(iterations);
                #[cfg(target_os = "linux")]
                if let Some(before) = before {
                    let after = pipeline_usage().expect("metrics stay enabled throughout a sample");
                    eprintln!(
                        "fanout_metrics case={name}_{placement} threads={threads} iterations={iterations} chunks={} raw_allocations={} unique_reuses={} copied_bytes={} minor_faults={} major_faults={} peak_rss_kib={}",
                        counts.chunks,
                        counts.raw_allocations,
                        counts.unique_reuses,
                        counts.copied_bytes,
                        after.ru_minflt - before.ru_minflt,
                        after.ru_majflt - before.ru_majflt,
                        after.ru_maxrss
                    );
                }
                black_box(counts);
                elapsed
            });
        });
    }
}

fn fanout_pin_worker(processor: usize) {
    #[cfg(target_os = "linux")]
    pin_worker(processor);
    #[cfg(not(target_os = "linux"))]
    unreachable!("no pinned placement is registered on this platform: {processor}");
}

fn medium_pipeline(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/medium_pipeline"));
    let available = thread::available_parallelism().expect("benchmark requires CPU count").get();
    #[cfg(target_os = "linux")]
    let processors = allowed_processors();
    #[cfg(target_os = "linux")]
    let interleaved = node_interleaved_processors(&processors);
    for threads in [1, 2, 8, 64].into_iter().filter(|&threads| threads <= available) {
        group.throughput(Throughput::Elements((threads * PIPELINE_BATCH) as u64));
        for remote in [false, true] {
            // A one-worker ring cannot perform a remote free. Keep the genuine
            // single-worker local control rather than mislabel a loopback.
            if remote && threads == 1 {
                continue;
            }
            for mixed in [false, true] {
                let route = if remote { "remote" } else { "local" };
                let sizes = if mixed { "mixed" } else { "32k" };
                let workload = MediumWorkload::Pipeline { remote, mixed };
                benchmark_medium(
                    &mut group,
                    BenchmarkId::new(format!("{route}_{sizes}_unpinned"), threads),
                    threads,
                    &[],
                    workload,
                );
                #[cfg(target_os = "linux")]
                if threads <= processors.len() {
                    benchmark_medium(
                        &mut group,
                        BenchmarkId::new(format!("{route}_{sizes}_pinned"), threads),
                        threads,
                        &processors[..threads],
                        workload,
                    );
                    if remote && let Some(interleaved) = &interleaved {
                        benchmark_medium(
                            &mut group,
                            BenchmarkId::new(format!("{route}_{sizes}_node_interleaved"), threads),
                            threads,
                            &interleaved[..threads],
                            workload,
                        );
                    }
                }
            }
        }
    }
    group.finish();
}

#[derive(Clone, Copy)]
enum MediumWorkload {
    Retained,
    Pipeline { remote: bool, mixed: bool },
}

fn benchmark_medium(
    group: &mut BenchmarkGroup<'_, WallTime>,
    id: BenchmarkId,
    threads: usize,
    processors: &[usize],
    workload: MediumWorkload,
) {
    let mut workers = None;
    group.bench_function(id, |bencher| {
        // Laziness avoids starting workers for filtered-out cases. Keep them
        // across warmup and ALL samples: Criterion also uses wall-clock duration
        // for calibration, so per-sample TLS teardown biases iteration counts.
        let workers = workers.get_or_insert_with(|| MediumWorkers::new(threads, processors, workload));
        bencher.iter_custom(|iterations| workers.time(iterations));
    });
}

struct MediumWorkers {
    starters: Vec<SyncSender<Option<u64>>>,
    finished: mpsc::Receiver<usize>,
    workers: Vec<thread::JoinHandle<()>>,
    workload: MediumWorkload,
}

impl MediumWorkers {
    fn new(threads: usize, processors: &[usize], workload: MediumWorkload) -> Self {
        let barrier = Arc::new(Barrier::new(threads));
        let (ready, ready_workers) = mpsc::sync_channel(0);
        let (finished, finished_workers) = mpsc::sync_channel(threads);
        let (senders, receivers): (Vec<_>, Vec<_>) = (0..threads).map(|_| mpsc::sync_channel::<Vec<OwnedAllocation>>(1)).unzip();

        // A ring gives every worker both roles without oversubscribing 64 cores.
        // Exactly one reusable Vec per worker bounds all live payloads to
        // threads * PIPELINE_BATCH, even if one consumer falls behind.
        // Raw iter_custom is needed for affinity and persistent worker commands.
        let mut starters = Vec::with_capacity(threads);
        let mut workers = Vec::with_capacity(threads);
        for (worker, incoming) in receivers.into_iter().enumerate() {
            let outgoing = senders[(worker + 1) % threads].clone();
            let (start, started) = mpsc::sync_channel::<Option<u64>>(0);
            starters.push(start);
            let ready = ready.clone();
            let finished = finished.clone();
            let barrier = Arc::clone(&barrier);
            let processor = processors.get(worker).copied();
            workers.push(thread::spawn(move || {
                #[cfg(target_os = "linux")]
                if let Some(processor) = processor {
                    pin_worker(processor);
                }
                #[cfg(not(target_os = "linux"))]
                let _ = processor;
                let capacity = match workload {
                    MediumWorkload::Retained => RETAINED_MEDIUM_BLOCKS,
                    MediumWorkload::Pipeline { .. } => PIPELINE_BATCH,
                };
                let mut batch = Vec::with_capacity(capacity);
                ready.send(()).expect("coordinator waits for every worker");
                while let Some(iterations) = started.recv().expect("coordinator sends commands until shutdown") {
                    let mut checksum = 0_usize;
                    for iteration in 0..iterations {
                        for index in 0..capacity {
                            let size = if matches!(workload, MediumWorkload::Pipeline { mixed: true, .. }) {
                                PIPELINE_MIXED_SIZES[(index + worker + iteration as usize) % PIPELINE_MIXED_SIZES.len()]
                            } else {
                                RETAINED_MEDIUM_SIZE
                            };
                            let layout = Layout::from_size_align(size, 16).expect("pipeline layouts are valid");
                            let mut allocation = OwnedAllocation::new(layout);
                            match workload {
                                MediumWorkload::Retained => allocation.touch_edges(),
                                MediumWorkload::Pipeline { .. } => allocation.touch_pages(),
                            }
                            batch.push(allocation);
                        }
                        match workload {
                            MediumWorkload::Retained => {
                                black_box(&batch);
                                barrier.wait();
                                batch.clear();
                                barrier.wait();
                            }
                            MediumWorkload::Pipeline { remote, .. } => {
                                if !remote {
                                    checksum = checksum.wrapping_add(consume_pipeline_batch(&mut batch));
                                }
                                // Both controls send one Vec per burst. Local controls free
                                // before transfer; remote controls free after receipt.
                                outgoing.send(batch).expect("next ring worker remains alive");
                                batch = incoming.recv().expect("previous ring worker sends each burst");
                                if remote {
                                    checksum = checksum.wrapping_add(consume_pipeline_batch(&mut batch));
                                }
                            }
                        }
                    }
                    finished.send(checksum).expect("coordinator collects every result");
                }
            }));
        }
        for _ in 0..threads {
            ready_workers.recv().expect("every worker initializes before timing");
        }
        Self {
            starters,
            finished: finished_workers,
            workers,
            workload,
        }
    }

    fn time(&self, iterations: u64) -> Duration {
        let threads = self.starters.len();
        #[cfg(target_os = "linux")]
        let before = pipeline_usage();
        let start = Instant::now();
        for starter in &self.starters {
            starter.send(Some(iterations)).expect("ready worker waits for start");
        }
        let mut checksum = 0_usize;
        for _ in 0..threads {
            checksum = checksum.wrapping_add(self.finished.recv().expect("every worker completes"));
        }
        let elapsed = start.elapsed();
        #[cfg(target_os = "linux")]
        if let Some(before) = before
            && let MediumWorkload::Pipeline { remote, mixed } = self.workload
        {
            let after = pipeline_usage().expect("metrics stay enabled throughout a sample");
            eprintln!(
                "pipeline_metrics threads={threads} remote={remote} mixed={mixed} iterations={iterations} minor_faults={} major_faults={} peak_rss_kib={}",
                after.ru_minflt - before.ru_minflt,
                after.ru_majflt - before.ru_majflt,
                after.ru_maxrss
            );
        }
        #[cfg(not(target_os = "linux"))]
        let _ = self.workload;
        black_box(checksum);
        elapsed
    }
}

impl Drop for MediumWorkers {
    fn drop(&mut self) {
        // This happens only after Criterion finishes the entire case, never
        // during a timed sample or between calibration iterations.
        for starter in &self.starters {
            starter.send(None).expect("worker waits for the shutdown command");
        }
        for worker in self.workers.drain(..) {
            worker.join().expect("benchmark worker completes without panicking");
        }
    }
}

fn consume_pipeline_batch(batch: &mut Vec<OwnedAllocation>) -> usize {
    let checksum = batch.iter().map(OwnedAllocation::read_pages).sum();
    batch.clear();
    checksum
}

#[cfg(target_os = "linux")]
fn pipeline_usage() -> Option<libc::rusage> {
    std::env::var_os("RALLOCATOR_PIPELINE_METRICS")?;
    // SAFETY: an all-zero rusage is valid storage for the OS to initialize.
    let mut usage = unsafe { std::mem::zeroed() };
    // SAFETY: getrusage receives writable storage for this process's counters.
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) }, 0);
    Some(usage)
}

#[cfg(target_os = "linux")]
fn node_interleaved_processors(allowed: &[usize]) -> Option<Vec<usize>> {
    let mut nodes = std::fs::read_dir("/sys/devices/system/node")
        .ok()?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?;
            let node = name.strip_prefix("node")?.parse::<usize>().ok()?;
            let cpu_ranges = std::fs::read_to_string(path.join("cpulist")).ok()?;
            let mut processors = Vec::new();
            for part in cpu_ranges.trim().split(',') {
                let (first, last) = part.split_once('-').unwrap_or((part, part));
                let first = first.parse::<usize>().ok()?;
                let last = last.parse::<usize>().ok()?;
                processors.extend((first..=last).filter(|cpu| allowed.contains(cpu)));
            }
            (!processors.is_empty()).then_some((node, processors))
        })
        .collect::<Vec<_>>();
    nodes.sort_unstable_by_key(|(node, _)| *node);
    if nodes.len() < 2 {
        return None;
    }
    let mut interleaved = Vec::with_capacity(allowed.len());
    for index in 0..allowed.len() {
        for (_, processors) in &nodes {
            if let Some(&processor) = processors.get(index) {
                interleaved.push(processor);
            }
        }
    }
    (interleaved.len() == allowed.len()).then_some(interleaved)
}

fn retained_medium(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/retained_medium"));
    let available = thread::available_parallelism()
        .expect("benchmark requires available CPU count")
        .get();
    for threads in RETAINED_THREAD_COUNTS.into_iter().filter(|&threads| threads <= available) {
        group.throughput(Throughput::Elements((threads * RETAINED_MEDIUM_BLOCKS) as u64));
        benchmark_medium(
            &mut group,
            BenchmarkId::new("unpinned", threads),
            threads,
            &[],
            MediumWorkload::Retained,
        );
        #[cfg(target_os = "linux")]
        {
            let processors = allowed_processors();
            if threads <= processors.len() {
                benchmark_medium(
                    &mut group,
                    BenchmarkId::new("pinned", threads),
                    threads,
                    &processors[..threads],
                    MediumWorkload::Retained,
                );
            }
        }
    }
    group.finish();
}

#[cfg(target_os = "linux")]
fn allowed_processors() -> Vec<usize> {
    // SAFETY: An all-zero cpu_set_t is a valid empty CPU set.
    let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    // SAFETY: `mask` is writable storage with the exact supplied size.
    let result = unsafe { libc::sched_getaffinity(0, size_of::<libc::cpu_set_t>(), &raw mut mask) };
    assert_eq!(result, 0, "cannot read CPU affinity: {}", std::io::Error::last_os_error());
    (0..libc::CPU_SETSIZE as usize)
        .filter(|&processor| {
            // SAFETY: `processor` is within CPU_SETSIZE and `mask` is initialized.
            unsafe { libc::CPU_ISSET(processor, &mask) }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn pin_worker(processor: usize) {
    // SAFETY: An all-zero cpu_set_t is a valid empty CPU set.
    let mut mask: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    assert!(processor < libc::CPU_SETSIZE as usize);
    // SAFETY: The processor index is bounded and the initialized set is writable.
    unsafe { libc::CPU_SET(processor, &mut mask) };
    // SAFETY: The initialized mask is valid for its supplied size; pid 0 selects this thread.
    let result = unsafe { libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &raw const mask) };
    assert_eq!(result, 0, "cannot pin benchmark worker: {}", std::io::Error::last_os_error());
}

fn cross_thread_free(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/cross_thread_free"));
    group.throughput(Throughput::Elements(CROSS_THREAD_BATCH_SIZE as u64));

    for size in CROSS_THREAD_SIZES {
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, &size| {
            bencher.iter_custom(|iterations| time_cross_thread_free(iterations, size));
        });
    }

    group.finish();
}

fn time_cross_thread_free(iterations: u64, size: usize) -> Duration {
    let layout = Layout::from_size_align(size, 16).expect("benchmark sizes and alignment are valid layouts");
    let (to_consumer, from_producer) = mpsc::sync_channel::<Vec<OwnedAllocation>>(1);
    let (to_producer, from_consumer) = mpsc::sync_channel::<Vec<OwnedAllocation>>(1);
    let (ready, consumer_ready) = mpsc::sync_channel(0);
    let release = Barrier::new(2);

    thread::scope(|scope| {
        let release = &release;
        scope.spawn(move || consume_remote_allocations(from_producer, to_producer, ready, iterations, release));
        consumer_ready
            .recv()
            .expect("consumer thread must signal readiness before processing allocations");

        let mut allocations = Vec::with_capacity(CROSS_THREAD_BATCH_SIZE);
        let mut checksum = 0_usize;
        let start = Instant::now();

        for _ in 0..iterations {
            for _ in 0..CROSS_THREAD_BATCH_SIZE {
                let mut allocation = OwnedAllocation::new(layout);
                allocation.touch_edges();
                checksum ^= allocation.address();
                allocations.push(allocation);
            }

            to_consumer
                .send(allocations)
                .expect("consumer thread must remain alive for the benchmark duration");
            allocations = from_consumer
                .recv()
                .expect("consumer thread must return the reusable transfer buffer");
        }

        // The final owner-thread allocation drains remote frees queued by the
        // last consumer batch, just as the next batch does for earlier rounds.
        let mut drain = OwnedAllocation::new(layout);
        drain.touch_edges();
        checksum ^= drain.address();
        drop(drain);

        let elapsed = start.elapsed();
        black_box(checksum);
        release.wait();
        elapsed
    })
}

fn consume_remote_allocations(
    from_producer: mpsc::Receiver<Vec<OwnedAllocation>>,
    to_producer: SyncSender<Vec<OwnedAllocation>>,
    ready: SyncSender<()>,
    iterations: u64,
    release: &Barrier,
) {
    ready.send(()).expect("producer thread must wait for the consumer readiness signal");

    for _ in 0..iterations {
        let mut allocations = from_producer
            .recv()
            .expect("producer thread must send one allocation batch per iteration");
        allocations.clear();
        to_producer
            .send(allocations)
            .expect("producer thread must receive the reusable transfer buffer");
    }
    release.wait();
}

fn mixed_scale_throughput(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/mixed_scale_throughput"));

    for threads in THREAD_COUNTS {
        group.throughput(Throughput::Elements((threads * MIXED_OPERATIONS_PER_THREAD) as u64));
        group.bench_with_input(BenchmarkId::new("threads", threads), &threads, |bencher, &threads| {
            bencher.iter_custom(|iterations| time_mixed_scale(iterations, threads));
        });
    }

    group.finish();
}

fn time_mixed_scale(iterations: u64, thread_count: usize) -> Duration {
    let (ready, workers_ready) = mpsc::sync_channel(0);
    let (result_sender, results) = mpsc::sync_channel(thread_count);
    let release = Barrier::new(thread_count + 1);

    thread::scope(|scope| {
        let mut starters = Vec::with_capacity(thread_count);
        for thread_index in 0..thread_count {
            let (start_sender, start_receiver) = mpsc::sync_channel(0);
            starters.push(start_sender);
            let ready = ready.clone();
            let result_sender = result_sender.clone();
            let release = &release;
            scope.spawn(move || {
                let mut live = Vec::with_capacity(MIXED_LIVE_ALLOCATIONS);
                ready.send(()).expect("benchmark coordinator must wait for every worker");
                start_receiver.recv().expect("benchmark coordinator must start every worker");

                let mut checksum = 0_usize;
                let mut random = 0xD1B5_4A32_D192_ED03_u64 ^ thread_index as u64;
                for _ in 0..iterations {
                    let result = run_mixed_scale_burst(&mut live, random);
                    random = result.random;
                    checksum ^= result.checksum;
                }
                result_sender
                    .send(checksum)
                    .expect("benchmark coordinator must collect every worker result");
                release.wait();
            });
        }
        drop(ready);
        drop(result_sender);

        for _ in 0..thread_count {
            workers_ready
                .recv()
                .expect("every worker must signal readiness before timing starts");
        }

        let start = Instant::now();
        for starter in starters {
            starter
                .send(())
                .expect("worker thread must remain alive until the benchmark starts");
        }

        let mut checksum = 0_usize;
        for _ in 0..thread_count {
            checksum ^= results.recv().expect("every worker must report a result before the sample ends");
        }
        let elapsed = start.elapsed();
        black_box(checksum);
        release.wait();
        elapsed
    })
}

struct MixedResult {
    random: u64,
    checksum: usize,
}

fn run_mixed_scale_burst(live: &mut Vec<OwnedAllocation>, mut random: u64) -> MixedResult {
    let mut checksum = 0_usize;

    for operation in 0..MIXED_OPERATIONS_PER_THREAD {
        random = xorshift64(random);

        if operation % 8_192 == 8_191 {
            let size = MIXED_LARGE_SIZES[random as usize % MIXED_LARGE_SIZES.len()];
            let layout = Layout::from_size_align(size, 64 * 1_024).expect("large benchmark sizes use valid alignment");
            let mut allocation = OwnedAllocation::new(layout);
            allocation.touch_edges();
            checksum ^= allocation.address().rotate_left((operation % usize::BITS as usize) as u32);
            continue;
        }

        if !live.is_empty() && (live.len() == live.capacity() || random.trailing_zeros() >= 2) {
            let index = random as usize % live.len();
            let allocation = live.swap_remove(index);
            checksum ^= allocation.address();
            drop(allocation);
            continue;
        }

        let selector = (random >> 16) as usize % 1_000;
        let size = if selector < 930 {
            MIXED_SMALL_SIZES[random as usize % MIXED_SMALL_SIZES.len()]
        } else {
            MIXED_MEDIUM_SIZES[random as usize % MIXED_MEDIUM_SIZES.len()]
        };
        let alignment = match (random >> 48) & 7 {
            0 => 64,
            1 => 4_096,
            2 if size >= 64 * 1_024 => 64 * 1_024,
            _ => 16,
        };
        let layout = Layout::from_size_align(size, alignment).expect("mixed benchmark sizes and alignments are valid layouts");
        let mut allocation = OwnedAllocation::new(layout);
        allocation.touch_edges();
        checksum = checksum.wrapping_add(allocation.address() ^ size);
        live.push(allocation);
    }

    for allocation in live.drain(..) {
        checksum ^= allocation.address();
    }

    MixedResult { random, checksum }
}

fn xorshift64(mut value: u64) -> u64 {
    value ^= value << 13;
    value ^= value >> 7;
    value ^ (value << 17)
}
