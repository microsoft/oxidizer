// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Payload, alignment, reallocation, and cross-thread allocation invariants.

#![expect(clippy::unwrap_used, reason = "Tests fail immediately on invalid layouts or violated invariants")]

use std::alloc::Layout;
use std::sync::{Mutex, mpsc};

use allocation_hints::heaps::{Heap, bump, thread_heap};
use allocation_hints::with_hint;
use common::{Block, check_disjoint};
use rallocator::config::{SizeClassLayout, StandardSizeClasses};
use scenarios::Recording;

mod common;
mod scenarios;
mod support;

// SAFETY: This is the only rallocator configuration used in this test binary.
// System remains the global allocator so test bookkeeping does not contaminate
// the rallocator counters or inadvertently consume active allocation hints.
static ALLOCATOR: rallocator::Rallocator = unsafe { rallocator::Rallocator::new() };
static TEST_LOCK: Mutex<()> = Mutex::new(());
const MAX_SCENARIO_LIVE_BLOCKS: usize = if cfg!(miri) { 4 } else { 16 };
const SCENARIO_SLAB_BYTES: usize = 32 * 1024;
const SCENARIO_SLICE_BYTES: usize = 64 * 1024;
const DEFAULT_LOCALITY_BYTES: usize = 4 * 1024 * 1024;
const PASSIVE_ATTACHMENT_BOUND: usize = 8;
// One boundary allocation is served by the general heap at a time and released
// before the next, so only the medium cache and its slices are retained. The
// unfixed retry loop mapped hundreds of megabytes here.
const BUMP_BOUNDARY_BACKING_BOUND: usize = 16 * 1024 * 1024;

// Derived from the fixed scenario topology, not from a warmed measurement:
// default heap small slabs, default/preserved bump medium caches, eight cached
// passive attachments, bump states pinned by escaped live blocks, and the three
// medium global bins reachable from the 131_073-byte request ceiling.
fn scenario_backing_retention_bound() -> usize {
    let transient_live = MAX_SCENARIO_LIVE_BLOCKS + 1;
    let default_small_slabs = 2 * StandardSizeClasses::SIZES.len() * transient_live;
    let default_small_bytes = default_small_slabs.div_ceil(DEFAULT_LOCALITY_BYTES / SCENARIO_SLAB_BYTES) * DEFAULT_LOCALITY_BYTES;
    let local_medium_cache_bytes = 3 * SCENARIO_SLICE_BYTES;
    let per_cached_attachment_bytes = scenarios::MAX_OPERATIONS * SCENARIO_SLICE_BYTES + local_medium_cache_bytes;
    let pinned_bump_pool_bytes = transient_live * (SCENARIO_SLICE_BYTES + local_medium_cache_bytes);
    let global_medium_bins_bytes = transient_live * (1 + 2 + 3) * SCENARIO_SLICE_BYTES;

    default_small_bytes
        + local_medium_cache_bytes
        + PASSIVE_ATTACHMENT_BOUND * per_cached_attachment_bytes
        + pinned_bump_pool_bytes
        + global_medium_bins_bytes
}

#[test]
fn bump_chunks_serve_both_segments_before_mapping_another_chunk() {
    // A chunk holds two segments. If the second were unreachable, every
    // segment-sized allocation would strand half of each 64 KiB chunk, doubling
    // bump backing. Three segment-sized live blocks must therefore occupy two
    // chunks, not three.
    let _test = TEST_LOCK.lock().unwrap();
    let heap = Heap::bump(bump::Options::new());
    let layout = Layout::from_size_align(20_000, 16).unwrap();
    let blocks: Vec<_> = (0..3)
        .map(|index| with_hint(&heap, || Block::new(&ALLOCATOR, layout, index, false)))
        .collect();
    check_disjoint(&blocks);
    let mut chunks: Vec<_> = blocks.iter().map(|block| block.address() & !(64 * 1024 - 1)).collect();
    chunks.sort_unstable();
    chunks.dedup();
    assert_eq!(
        chunks.len(),
        2,
        "three segment-sized bump allocations occupied {} chunks; both segments of a chunk must be used",
        chunks.len()
    );
}

#[test]
fn bump_sizes_spanning_the_segment_prefix_terminate_with_bounded_backing() {
    // A bump heap admits requests up to its configured maximum allocation size,
    // but every chunk segment starts after an allocator-owned prefix, so the
    // largest admissible sizes cannot be served from a segment and must fall
    // back to the general heap. Regression: sizes 32_705..=32_713 (align 16)
    // were admitted yet fit neither a chunk's first nor its second segment, so
    // each retry mapped a fresh 64 KiB chunk until the process exhausted
    // memory. That window is nine bytes wide, so it is enumerated explicitly
    // rather than sampled. Reaching the end of this test at all is the primary
    // assertion; the bound additionally catches a partial regression that still
    // terminates.
    let _test = TEST_LOCK.lock().unwrap();
    let heap = Heap::bump(bump::Options::new());
    let sizes = if cfg!(miri) {
        vec![32_688, 32_704, 32_705, 32_713, 32_714]
    } else {
        (32_600..=32_768).collect::<Vec<_>>()
    };
    let alignments = if cfg!(miri) { &[16][..] } else { &[1, 16, 64, 4_096][..] };
    drop(with_hint(&heap, || Block::new(&ALLOCATOR, Layout::new::<u64>(), 0, false)));
    let before = support::stats().unwrap();
    for &alignment in alignments {
        for &size in &sizes {
            let layout = Layout::from_size_align(size, alignment).unwrap();
            let block = with_hint(&heap, || Block::new(&ALLOCATOR, layout, u64::try_from(size).unwrap(), false));
            block.check();
        }
    }
    // `mapped_bytes` is current committed backing, not a monotonic counter:
    // medium purging can release backing retained by an earlier test on this
    // shared allocator, so a subtraction here could underflow on a run that
    // actually improved retention. Compare absolute values instead.
    let after = support::stats().unwrap().mapped_bytes;
    let ceiling = before.mapped_bytes.saturating_add(BUMP_BOUNDARY_BACKING_BOUND);
    assert!(
        after <= ceiling,
        "bump boundary sizes left {after} bytes of backing, above the {ceiling} byte ceiling (baseline {})",
        before.mapped_bytes
    );
}

#[test]
fn size_class_boundaries_preserve_disjoint_live_payloads() {
    let _test = TEST_LOCK.lock().unwrap();
    let sizes = if cfg!(miri) {
        &[16, 48, 256, 4_096, 16_384][..]
    } else {
        StandardSizeClasses::SIZES
    };
    let alignment_shifts = if cfg!(miri) {
        &[0, 4, 6, 12][..]
    } else {
        &(0..=21).collect::<Vec<_>>()
    };
    for recording in [false, true] {
        let _recording = Recording::new(recording);
        for &boundary in sizes {
            let mut blocks = Vec::with_capacity(3 * alignment_shifts.len());
            for size in [boundary - 1, boundary, boundary + 1] {
                for &shift in alignment_shifts {
                    let tag = u64::try_from(blocks.len() + 1).unwrap();
                    blocks.push(Block::new(
                        &ALLOCATOR,
                        Layout::from_size_align(size, 1 << shift).unwrap(),
                        tag,
                        false,
                    ));
                }
            }
            check_disjoint(&blocks);
            // Leave live neighbours on both sides of holes, then force dirty
            // free-list entries through alloc_zeroed instead of only fresh pages.
            for index in (0..blocks.len()).step_by(2) {
                let layout = blocks[index].layout();
                let old = blocks.swap_remove(index);
                old.check();
                drop(old);
                let tag = u64::try_from(index).unwrap() + 1_000;
                blocks.push(Block::new(&ALLOCATOR, layout, tag, true));
                check_disjoint(&blocks);
            }
        }
    }
}

#[test]
fn reallocation_preserves_prefix_across_sizes_and_alignments() {
    let _test = TEST_LOCK.lock().unwrap();
    let sizes = if cfg!(miri) {
        &[1, 17, 257][..]
    } else {
        &[1, 17, 257, 4_097, 16_384, 16_385, 65_536, 524_289][..]
    };
    let alignments = if cfg!(miri) {
        &[1, 64][..]
    } else {
        &[1, 16, 64, 4_096, 65_536, 131_072][..]
    };
    for recording in [false, true] {
        let _recording = Recording::new(recording);
        for &alignment in alignments {
            for &old_size in sizes {
                for &new_size in sizes {
                    let mut blocks = [
                        Block::new(&ALLOCATOR, Layout::from_size_align(73, alignment).unwrap(), 0xCAFE, false),
                        Block::new(&ALLOCATOR, Layout::from_size_align(old_size, alignment).unwrap(), 0xDEAD_BEEF, true),
                        Block::new(&ALLOCATOR, Layout::from_size_align(119, alignment).unwrap(), 0xFACE, false),
                    ];
                    blocks[1].reallocate(new_size);
                    check_disjoint(&blocks);
                    blocks[1].reallocate(old_size);
                    check_disjoint(&blocks);
                }
            }
        }
    }
}

#[test]
fn reallocation_crosses_the_medium_direct_boundary() {
    let _test = TEST_LOCK.lock().unwrap();
    let boundary = if cfg!(miri) { 512 * 1024 } else { 32 * 1024 * 1024 };
    let mut block = Block::new(&ALLOCATOR, Layout::from_size_align(boundary - 1, 16).unwrap(), 0x1234_5678, false);
    for size in [boundary, boundary + 1, 16_385, 1] {
        block.reallocate(size);
        block.check();
    }
}

#[test]
fn deterministic_mixed_heap_sequences() {
    let _test = TEST_LOCK.lock().unwrap();
    for seed in [0, 1, 42, 0xA5A5_5A5A, u64::MAX] {
        let mut input = [0; scenarios::INPUT_BYTES];
        fastrand::Rng::with_seed(seed).fill(&mut input);
        scenarios::run(&ALLOCATOR, &input);
    }
}

#[test]
fn remote_frees_preserve_live_neighbours_during_refill() {
    let _test = TEST_LOCK.lock().unwrap();
    let threads = if cfg!(miri) { 2 } else { 4 };
    let rounds = if cfg!(miri) { 2 } else { 64 };
    let batch_size = if cfg!(miri) { 4 } else { 64 };
    for recording in [false, true] {
        let _recording = Recording::new(recording);
        std::thread::scope(|scope| {
            let mut senders = Vec::with_capacity(threads);
            for _ in 0..threads {
                let (sender, receiver) = mpsc::sync_channel::<Vec<Block<'_, rallocator::Rallocator>>>(1);
                senders.push(sender);
                scope.spawn(move || {
                    for mut blocks in receiver {
                        check_disjoint(&blocks);
                        while let Some(mut block) = blocks.pop() {
                            block.check();
                            block.paint(0xBAD0_F00D);
                        }
                    }
                });
            }
            let mut neighbours = Vec::with_capacity(batch_size * 2);
            for index in 0..batch_size {
                neighbours.push(Block::new(
                    &ALLOCATOR,
                    Layout::from_size_align(257, 16).unwrap(),
                    u64::try_from(index).unwrap(),
                    false,
                ));
            }
            for round in 0..rounds {
                for index in 0..batch_size {
                    let size = [257, 1_024, 4_097, 16_385][index % 4];
                    neighbours.push(Block::new(
                        &ALLOCATOR,
                        Layout::from_size_align(size, 16).unwrap(),
                        u64::try_from(round * batch_size + index).unwrap() + 1_000,
                        round % 2 == 0,
                    ));
                }
                check_disjoint(&neighbours);
                senders[round % threads].send(neighbours.split_off(batch_size)).unwrap();
            }
            drop(senders);
            check_disjoint(&neighbours);
        });
    }
}

#[test]
#[expect(clippy::needless_collect, reason = "All producers must be spawned before any is joined")]
fn thread_heap_allocations_survive_concurrent_producers_and_owner_exit() {
    let _test = TEST_LOCK.lock().unwrap();
    let threads = if cfg!(miri) { 2 } else { 4 };
    let count = if cfg!(miri) { 2 } else { 32 };
    let (heap, escaped) = std::thread::spawn(move || {
        let heap = thread_heap();
        drop(Block::new(&ALLOCATOR, Layout::new::<u64>(), 1, false));
        let escaped = std::thread::scope(|scope| {
            let producers: Vec<_> = (0..threads)
                .map(|thread| {
                    let heap = &heap;
                    scope.spawn(move || {
                        with_hint(heap, || {
                            (0..count)
                                .map(|index| {
                                    Block::new(
                                        &ALLOCATOR,
                                        Layout::from_size_align(1_024 << (index % 3), 64).unwrap(),
                                        u64::try_from(thread * count + index).unwrap(),
                                        index % 2 == 0,
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                })
                .collect();
            producers
                .into_iter()
                .flat_map(|producer| producer.join().unwrap())
                .collect::<Vec<_>>()
        });
        (heap, escaped)
    })
    .join()
    .unwrap();
    check_disjoint(&escaped);
    let mut after_exit = with_hint(&heap, || {
        Block::new(&ALLOCATOR, Layout::from_size_align(2_048, 64).unwrap(), 0xABCD, true)
    });
    after_exit.reallocate(4_097);
    check_disjoint(&escaped);
    drop(heap);
    for block in escaped.into_iter().rev() {
        block.check();
    }
    after_exit.check();
}

#[test]
#[ignore = "Opt-in bounded campaign; configure RALLOCATOR_STRESS_SEEDS and RALLOCATOR_STRESS_START_SEED"]
fn seeded_soak() {
    let _test = TEST_LOCK.lock().unwrap();
    let count = environment_u64("RALLOCATOR_STRESS_SEEDS", 4_096);
    let start = environment_u64("RALLOCATOR_STRESS_START_SEED", 0);
    assert!(
        (1..=10_000_000).contains(&count),
        "stress seed count must be between 1 and 10,000,000"
    );
    let end = start.checked_add(count).unwrap();
    drop(Block::new(&ALLOCATOR, Layout::new::<u64>(), 0, false));
    let baseline = support::stats().unwrap();
    let mut peak_mapped = baseline.mapped_bytes;
    let mapped_limit = baseline.mapped_bytes.saturating_add(scenario_backing_retention_bound());
    for seed in start..end {
        if (seed - start).is_multiple_of(256) {
            eprintln!("rallocator seed {seed}, mapped high-water {peak_mapped} bytes, bound {mapped_limit} bytes");
        }
        let mut input = [0; scenarios::INPUT_BYTES];
        fastrand::Rng::with_seed(seed).fill(&mut input);
        // Corruption, overlap and alignment failures panic inside the scenario
        // with no seed in scope, and progress is only printed every 256 seeds.
        // Naming the seed here keeps every failure directly replayable through
        // RALLOCATOR_STRESS_START_SEED without altering the original panic.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scenarios::run(&ALLOCATOR, &input))).unwrap_or_else(|payload| {
            eprintln!(
                "rallocator scenario failed at seed {seed}; replay with RALLOCATOR_STRESS_START_SEED={seed} RALLOCATOR_STRESS_SEEDS=1"
            );
            std::panic::resume_unwind(payload);
        });
        let stats = support::stats().unwrap();
        assert_eq!(stats.live_bytes, baseline.live_bytes, "live allocations leaked at seed {seed}");
        assert!(
            stats.mapped_bytes <= mapped_limit,
            "retained backing exceeded derived fixed-workload bound at seed {seed}: baseline={} mapped={} limit={}",
            baseline.mapped_bytes,
            stats.mapped_bytes,
            mapped_limit
        );
        peak_mapped = peak_mapped.max(stats.mapped_bytes);
    }
    eprintln!("completed seeds {start}..{end}; mapped high-water {peak_mapped} bytes, bound {mapped_limit} bytes");
}

fn environment_u64(name: &str, default: u64) -> u64 {
    std::env::var_os(name).map_or(default, |value| value.into_string().unwrap().parse().unwrap())
}
