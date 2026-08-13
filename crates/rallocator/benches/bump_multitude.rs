// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump workload benchmark using multitude.

mod bump_workloads;

use std::cell::RefCell;
use std::hint::black_box;
use std::thread::LocalKey;

thread_local! {
    static VECTOR_REGION: RefCell<multitude::Arena> = RefCell::new(multitude::Arena::new());
    static HASH_MAP_REGION: RefCell<multitude::Arena> = RefCell::new(multitude::Arena::new());
    static ARC_REGION: RefCell<multitude::Arena> = RefCell::new(multitude::Arena::new());
    static MIXED_REGION: RefCell<multitude::Arena> = RefCell::new(multitude::Arena::new());
}

fn main() {
    bump_workloads::run(
        "bump_multitude",
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
    with_reset_region(&VECTOR_REGION, |region| {
        let mut vectors = region.alloc_vec_with_capacity(count);
        for seed in 0..count {
            let mut values = region.alloc_vec_with_capacity(length);
            values.extend((0..length).map(|index| (seed ^ index) as u64));
            vectors.push(values);
        }
        black_box(&vectors);
    });
}

fn hash_maps(count: usize, entries: usize) {
    with_reset_region(&HASH_MAP_REGION, |region| {
        let mut maps = region.alloc_vec_with_capacity(count);
        for seed in 0..count {
            let mut map = region.alloc_hash_map_with_capacity(entries);
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
    with_reset_region(&ARC_REGION, |region| {
        let mut values = region.alloc_vec_with_capacity(count);
        for seed in 0..count {
            values.push(region.alloc_arc([seed as u64; N]));
        }
        black_box(&values);
    });
}

fn mixed_lifecycle(rounds: usize, noise_allocations: usize) {
    let mut noise = bump_workloads::AllocationNoise::new();

    for round in 0..rounds {
        noise.run(noise_allocations);
        with_reset_region(&MIXED_REGION, |region| mixed_critical_section(region, round));
        noise.run(noise_allocations);
    }
}

fn mixed_critical_section(region: &multitude::Arena, round: usize) {
    let mut vectors = region.alloc_vec_with_capacity(bump_workloads::MIXED_VECTOR_COUNT);
    for index in 0..bump_workloads::MIXED_VECTOR_COUNT {
        let length = bump_workloads::mixed_vector_length(round, index);
        let value = bump_workloads::mixed_value(round, index);
        let mut values = region.alloc_vec_with_capacity(length);
        values.extend(std::iter::repeat_n(value, length));
        vectors.push(values);
    }

    let mut map = region.alloc_hash_map_with_capacity(bump_workloads::MIXED_MAP_ENTRIES);
    for index in 0..bump_workloads::MIXED_MAP_ENTRIES {
        let key = bump_workloads::mixed_value(round, index);
        map.insert(key, key.rotate_left(17));
    }

    let mut arcs = region.alloc_vec_with_capacity(bump_workloads::MIXED_ARC_COUNT);
    for index in 0..bump_workloads::MIXED_ARC_COUNT {
        let value = bump_workloads::mixed_value(round, index);
        arcs.push(region.alloc_arc([value; 16]));
    }

    black_box((&vectors, &map, &arcs));
}

fn with_reset_region(local: &'static LocalKey<RefCell<multitude::Arena>>, workload: impl FnOnce(&multitude::Arena)) {
    local.with(|region| {
        let mut region = region.borrow_mut();
        workload(&region);
        region.reset();
    });
}
