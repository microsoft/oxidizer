// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::cast_possible_truncation,
    reason = "Bounded deterministic benchmark generators use compact integer arithmetic"
)]

use std::hint::black_box;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};

#[derive(Clone, Copy)]
pub(crate) struct Workloads {
    pub vectors: fn(usize, usize),
    pub hash_maps: fn(usize, usize),
    pub arcs_4: fn(usize),
    pub arcs_256: fn(usize),
    pub mixed_lifecycle: fn(usize, usize),
}

pub(crate) fn run(file_basename: &str, workloads: Workloads) {
    let mut criterion = Criterion::default()
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3))
        .sample_size(20)
        .configure_from_args();

    vector_workloads(&mut criterion, file_basename, workloads.vectors);
    hash_map_workloads(&mut criterion, file_basename, workloads.hash_maps);
    arc_workloads(&mut criterion, file_basename, workloads);
    mixed_lifecycle_workloads(&mut criterion, file_basename, workloads.mixed_lifecycle);
    criterion.final_summary();
}

fn vector_workloads(criterion: &mut Criterion, file_basename: &str, workload: fn(usize, usize)) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/container_vec"));
    for (count, length) in [(256, 8), (32, 1_024)] {
        group.throughput(Throughput::Elements((count * length) as u64));
        group.bench_with_input(
            BenchmarkId::new("vectors_x_elements", format!("{count}x{length}")),
            &(count, length),
            |bencher, &(count, length)| {
                bencher.iter(|| workload(black_box(count), black_box(length)));
            },
        );
    }
    group.finish();
}

fn hash_map_workloads(criterion: &mut Criterion, file_basename: &str, workload: fn(usize, usize)) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/container_hash_map"));
    for (count, entries) in [(128, 8), (32, 512)] {
        group.throughput(Throughput::Elements((count * entries) as u64));
        group.bench_with_input(
            BenchmarkId::new("maps_x_entries", format!("{count}x{entries}")),
            &(count, entries),
            |bencher, &(count, entries)| {
                bencher.iter(|| workload(black_box(count), black_box(entries)));
            },
        );
    }
    group.finish();
}

fn arc_workloads(criterion: &mut Criterion, file_basename: &str, workloads: Workloads) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/container_arc"));
    for (count, elements, workload) in [(1_024, 4, workloads.arcs_4), (128, 256, workloads.arcs_256)] {
        group.throughput(Throughput::Elements((count * elements) as u64));
        group.bench_with_input(
            BenchmarkId::new("arcs_x_elements", format!("{count}x{elements}")),
            &count,
            |bencher, &count| {
                bencher.iter(|| workload(black_box(count)));
            },
        );
    }
    group.finish();
}

fn mixed_lifecycle_workloads(criterion: &mut Criterion, file_basename: &str, workload: fn(usize, usize)) {
    let mut group = criterion.benchmark_group(format!("{file_basename}/mixed_lifecycle"));
    for (rounds, noise_allocations) in [(200, 64), (500, 32)] {
        group.throughput(Throughput::Elements(rounds as u64));
        group.bench_with_input(
            BenchmarkId::new("rounds_x_noise_allocations", format!("{rounds}x{noise_allocations}")),
            &(rounds, noise_allocations),
            |bencher, &(rounds, noise_allocations)| {
                bencher.iter(|| {
                    workload(black_box(rounds), black_box(noise_allocations));
                });
            },
        );
    }
    group.finish();
}

pub(crate) const MIXED_VECTOR_COUNT: usize = 24;
pub(crate) const MIXED_MAP_ENTRIES: usize = 64;
pub(crate) const MIXED_ARC_COUNT: usize = 24;

pub(crate) struct AllocationNoise {
    state: u64,
}

impl AllocationNoise {
    pub(crate) fn new() -> Self {
        Self {
            state: 0x7a15_4f29_d6e8_b3c1,
        }
    }

    pub(crate) fn run(&mut self, operations: usize) {
        const LIVE_SLOTS: usize = 32;
        const SIZES: [usize; 16] = [8, 16, 24, 32, 48, 64, 96, 128, 192, 256, 384, 512, 768, 1_024, 2_048, 4_096];

        let mut live = Vec::with_capacity(LIVE_SLOTS);
        live.resize_with(LIVE_SLOTS, || None::<Vec<u8>>);

        for _ in 0..operations {
            let random = self.next();
            let base_size = SIZES[random as usize % SIZES.len()];
            let size = base_size + self.next() as usize % (base_size / 2 + 1);
            let mut allocation = vec![random as u8; size];

            if random.trailing_zeros() >= 2 {
                allocation.reserve_exact(base_size / 2 + 1);
            }

            let slot = self.next() as usize % LIVE_SLOTS;
            live[slot] = Some(allocation);

            if random.trailing_zeros() >= 3 {
                let dropped_slot = self.next() as usize % LIVE_SLOTS;
                live[dropped_slot] = None;
            }
        }

        black_box(&live);
    }

    fn next(&mut self) -> u64 {
        let mut value = self.state;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.state = value;
        value
    }
}

pub(crate) fn mixed_vector_length(round: usize, index: usize) -> usize {
    8 + (mix64(((round as u64) << 32) | index as u64) % 249) as usize
}

pub(crate) fn mixed_value(round: usize, index: usize) -> u64 {
    mix64(((round as u64) << 32) | index as u64)
}

fn mix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
