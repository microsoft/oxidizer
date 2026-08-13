// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::cast_possible_truncation,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    clippy::unwrap_used,
    reason = "Bounded benchmark generators favor direct arithmetic and grouped raw allocation operations"
)]

use std::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use std::hint::black_box;
use std::ptr;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};

const BURST_SIZE: usize = 128;
const MIXED_OPERATIONS: usize = 4_000_000;
const MIXED_LIVE_ALLOCATIONS: usize = 256;
const SMALL_SIZES: [usize; 12] = [8, 16, 24, 42, 64, 96, 256, 1_003, 4_096, 8_192, 12_288, 16_384];
const MEDIUM_SIZES: [usize; 8] = [
    64 * 1024,
    96 * 1024,
    256 * 1024,
    768 * 1024,
    2 * 1024 * 1024,
    4 * 1024 * 1024,
    8 * 1024 * 1024,
    16 * 1024 * 1024,
];
const LARGE_MEDIUM_SPAN_SIZES: [usize; 3] = [128 * 1024 * 1024, 256 * 1024 * 1024, 384 * 1024 * 1024];

#[derive(Clone, Copy)]
struct LiveAllocation {
    address: *mut u8,
    layout: Layout,
}

pub(crate) fn run(file_basename: &str) {
    let mut criterion = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .sample_size(20)
        .configure_from_args();

    single_allocations(&mut criterion, file_basename);
    odd_sized_allocations(&mut criterion, file_basename);
    medium_span_allocations(&mut criterion, file_basename);
    aligned_allocations(&mut criterion, file_basename);
    allocation_bursts(&mut criterion, file_basename);
    mixed_scale_bursts(&mut criterion, file_basename);
    criterion.final_summary();
}

fn single_allocations(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/single_allocation"));

    for size in [
        16, 4_096,  // Retain the 4 KiB page boundary.
        65_536, // Retain the 64 KiB OS-granularity boundary.
        1_048_576,
    ] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, &size| {
            let layout = Layout::from_size_align(size, 8).unwrap();
            bencher.iter(|| allocate_then_deallocate(black_box(layout)));
        });
    }

    group.finish();
}

fn odd_sized_allocations(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/odd_sized_allocation"));

    for size in [
        17, 4_097,  // Just over the 4 KiB page boundary.
        65_537, // Just over the 64 KiB OS-granularity boundary.
        3_145_745,
    ] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, &size| {
            let layout = Layout::from_size_align(size, 8).unwrap();
            bencher.iter(|| allocate_then_deallocate(black_box(layout)));
        });
    }

    group.finish();
}

fn medium_span_allocations(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/medium_span_allocation"));

    for size in [16 * 1024 * 1024, 128 * 1024 * 1024] {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, &size| {
            let layout = Layout::from_size_align(size, 8).unwrap();
            bencher.iter(|| allocate_then_deallocate(black_box(layout)));
        });
    }

    group.finish();
}

fn aligned_allocations(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/aligned_allocation"));

    for alignment in [
        8, 4_096, // Retain the page-aligned case.
        65_536,
    ] {
        group.bench_with_input(BenchmarkId::from_parameter(alignment), &alignment, |bencher, &alignment| {
            let layout = Layout::from_size_align(256, alignment).unwrap();
            bencher.iter(|| allocate_then_deallocate(black_box(layout)));
        });
    }

    group.finish();
}

fn allocation_bursts(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/allocation_burst"));

    for size in [
        32, 4_096,  // Retain the 4 KiB page boundary.
        65_536, // Retain the 64 KiB OS-granularity boundary.
        524_288,
    ] {
        group.throughput(Throughput::Elements(BURST_SIZE as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |bencher, &size| {
            let layout = Layout::from_size_align(size, 8).unwrap();
            bencher.iter(|| allocate_burst(black_box(layout)));
        });
    }

    group.finish();
}

fn mixed_scale_bursts(criterion: &mut Criterion, file_basename: &str) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/mixed_scale_burst"));
    group.throughput(Throughput::Elements(MIXED_OPERATIONS as u64));
    group.bench_function("4m_operations", |bencher| bencher.iter(|| black_box(run_mixed_scale_burst())));
    group.finish();
}

fn run_mixed_scale_burst() -> usize {
    let empty = LiveAllocation {
        address: ptr::null_mut(),
        layout: Layout::new::<u8>(),
    };
    let mut live = [empty; MIXED_LIVE_ALLOCATIONS];
    let mut live_len = 0;
    let mut random = 0xD1B5_4A32_D192_ED03_u64;
    let mut checksum = 0_usize;

    for operation in 0..MIXED_OPERATIONS {
        random = xorshift64(random);

        if operation % 1_000_000 == 999_999 {
            let size = LARGE_MEDIUM_SPAN_SIZES[(random as usize) % LARGE_MEDIUM_SPAN_SIZES.len()];
            let layout = Layout::from_size_align(size, 64 * 1024).unwrap();
            let allocation = allocate(layout);
            checksum ^= allocation.address.addr().rotate_left((operation % usize::BITS as usize) as u32);
            touch_edges(allocation);
            unsafe { dealloc(allocation.address, allocation.layout) };
            continue;
        }

        if live_len != 0 && (live_len == live.len() || random.trailing_zeros() >= 2) {
            let index = (random as usize) % live_len;
            let allocation = live[index];
            live_len -= 1;
            live[index] = live[live_len];
            checksum ^= allocation.address.addr();
            unsafe { dealloc(allocation.address, allocation.layout) };
            continue;
        }

        let selector = (random >> 16) as usize % 1_000;
        let size = if selector < 930 {
            SMALL_SIZES[(random as usize) % SMALL_SIZES.len()]
        } else {
            MEDIUM_SIZES[(random as usize) % MEDIUM_SIZES.len()]
        };
        let alignment = match (random >> 48) & 7 {
            0 => 64,
            1 => 4_096,
            2 if size >= 64 * 1024 => 64 * 1024,
            _ => 16,
        };
        let layout = Layout::from_size_align(size, alignment).unwrap();
        let allocation = allocate(layout);
        touch_edges(allocation);
        checksum = checksum.wrapping_add(allocation.address.addr() ^ size);
        live[live_len] = allocation;
        live_len += 1;
    }

    for allocation in live.into_iter().take(live_len) {
        checksum ^= allocation.address.addr();
        unsafe { dealloc(allocation.address, allocation.layout) };
    }
    checksum
}

fn allocate(layout: Layout) -> LiveAllocation {
    let address = unsafe { alloc(layout) };
    if address.is_null() {
        handle_alloc_error(layout);
    }
    LiveAllocation { address, layout }
}

fn touch_edges(allocation: LiveAllocation) {
    unsafe {
        allocation.address.write_volatile(0xA5);
        allocation.address.add(allocation.layout.size() - 1).write_volatile(0x5A);
    }
}

fn xorshift64(mut value: u64) -> u64 {
    value ^= value << 13;
    value ^= value >> 7;
    value ^ (value << 17)
}

fn allocate_then_deallocate(layout: Layout) {
    let address = unsafe { alloc(layout) };
    if address.is_null() {
        handle_alloc_error(layout);
    }

    black_box(address);
    unsafe { dealloc(address, layout) };
}

fn allocate_burst(layout: Layout) {
    let mut addresses = [ptr::null_mut(); BURST_SIZE];

    for address in &mut addresses {
        *address = unsafe { alloc(layout) };
        if address.is_null() {
            handle_alloc_error(layout);
        }
        black_box(*address);
    }

    for address in addresses.into_iter().rev() {
        unsafe { dealloc(address, layout) };
    }
}
