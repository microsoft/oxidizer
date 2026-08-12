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
use std::thread;
use std::time::{Duration, Instant};

use criterion::{BenchmarkId, Criterion, Throughput};

const CROSS_THREAD_BATCH_SIZE: usize = 1_024;
const CROSS_THREAD_SIZES: [usize; 3] = [
    16, 4_096, // Retain the 4 KiB page boundary for remote frees.
    16_384,
];
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

    cross_thread_free(&mut criterion, file_basename);
    mixed_scale_throughput(&mut criterion, file_basename);
    criterion.final_summary();
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

    thread::scope(|scope| {
        scope.spawn(move || consume_remote_allocations(from_producer, to_producer, ready, iterations));
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
        elapsed
    })
}

fn consume_remote_allocations(
    from_producer: mpsc::Receiver<Vec<OwnedAllocation>>,
    to_producer: SyncSender<Vec<OwnedAllocation>>,
    ready: SyncSender<()>,
    iterations: u64,
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

    thread::scope(|scope| {
        let mut starters = Vec::with_capacity(thread_count);
        for thread_index in 0..thread_count {
            let (start_sender, start_receiver) = mpsc::sync_channel(0);
            starters.push(start_sender);
            let ready = ready.clone();
            let result_sender = result_sender.clone();
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
