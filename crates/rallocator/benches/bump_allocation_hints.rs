// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump workloads using passive prospective allocation hints.

mod bump_workloads;

use std::hint::black_box;

use allocation_hints::heaps::{Heap, bump};
use allocation_hints::with_hint;

rallocator::rallocator!();

fn main() {
    bump_workloads::run(
        "bump_allocation_hints",
        bump_workloads::Workloads {
            vectors,
            hash_maps,
            arcs_4: arcs::<4>,
            arcs_256: arcs::<256>,
            mixed_lifecycle,
        },
    );
}

fn vectors(count: usize, length: usize) {
    let heap = Heap::bump(bump::Options::new());
    with_hint(&heap, || {
        let mut vectors = Vec::with_capacity(count);
        for seed in 0..count {
            let mut values = Vec::with_capacity(length);
            values.extend((0..length).map(|index| (seed ^ index) as u64));
            vectors.push(values);
        }
        black_box(&vectors);
    });
}

fn hash_maps(count: usize, entries: usize) {
    let heap = Heap::bump(bump::Options::new());
    with_hint(&heap, || {
        let mut maps = Vec::with_capacity(count);
        for seed in 0..count {
            let mut map = hashbrown::HashMap::with_capacity(entries);
            map.extend((0..entries).map(|index| {
                let key = ((seed as u64) << 32) | index as u64;
                (key, key.rotate_left(17))
            }));
            maps.push(map);
        }
        black_box(&maps);
    });
}

fn arcs<const N: usize>(count: usize) {
    let heap = Heap::bump(bump::Options::new());
    with_hint(&heap, || {
        let mut values = Vec::with_capacity(count);
        for seed in 0..count {
            values.push(std::sync::Arc::new([seed as u64; N]));
        }
        black_box(&values);
    });
}

fn mixed_lifecycle(rounds: usize, noise_allocations: usize) {
    let mut noise = bump_workloads::AllocationNoise::new();

    for round in 0..rounds {
        noise.run(noise_allocations);
        let heap = Heap::bump(bump::Options::new());
        with_hint(&heap, || mixed_critical_section(round));
        noise.run(noise_allocations);
    }
}

fn mixed_critical_section(round: usize) {
    let mut vectors = Vec::with_capacity(bump_workloads::MIXED_VECTOR_COUNT);
    for index in 0..bump_workloads::MIXED_VECTOR_COUNT {
        let length = bump_workloads::mixed_vector_length(round, index);
        let value = bump_workloads::mixed_value(round, index);
        vectors.push(vec![value; length]);
    }

    let mut map = hashbrown::HashMap::with_capacity(bump_workloads::MIXED_MAP_ENTRIES);
    for index in 0..bump_workloads::MIXED_MAP_ENTRIES {
        let key = bump_workloads::mixed_value(round, index);
        map.insert(key, key.rotate_left(17));
    }

    let mut arcs = Vec::with_capacity(bump_workloads::MIXED_ARC_COUNT);
    for index in 0..bump_workloads::MIXED_ARC_COUNT {
        let value = bump_workloads::mixed_value(round, index);
        arcs.push(std::sync::Arc::new([value; 16]));
    }

    black_box((&vectors, &map, &arcs));
}
