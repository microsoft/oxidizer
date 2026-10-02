// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena and bump lifecycle hardening.
//!
//! These integration tests drive the public bump heap through
//! [`allocation_hints`] against a direct, non-global [`Rallocator`]. Keeping
//! `System` as the process allocator means libtest and test bookkeeping never
//! touch the rallocator counters, so payload verification and the telemetry
//! snapshots stay exact. Every scenario paints and re-reads *all* payload bytes
//! of *live* allocations to surface corruption or aliasing rather than probing
//! only the first and last byte.
#![expect(
    clippy::unwrap_used,
    reason = "Tests fail immediately on invalid layouts, poisoned locks, or violated invariants"
)]

use std::alloc::{GlobalAlloc, Layout};
use std::sync::Mutex;

use allocation_hints::heaps::{Heap, bump};
use allocation_hints::with_hint;
use common::{Block, check_disjoint};
use rallocator::Rallocator;
use support::stats;

mod common;
mod support;

// SAFETY: This is the only rallocator configuration used in this test binary.
// `System` remains the global allocator, so test bookkeeping does not
// contaminate the rallocator counters or consume active allocation hints.
static ALLOCATOR: Rallocator = unsafe { Rallocator::new() };
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// One live allocation from the shared static [`ALLOCATOR`], backed by the
/// shared [`Block`] payload fixture. A block uniquely owns its allocation, is
/// neither `Clone` nor `Copy`, and is `Send` because `Rallocator` is `Sync`, so
/// an allocation may outlive its [`Heap`] handle and the thread that produced it
/// and still be released exactly once on drop from any thread.
type Live = Block<'static, Rallocator>;

/// The documented bump chunk size (one 64 KiB slice); see the crate layout guide.
const BUMP_CHUNK_BYTES: usize = 64 * 1024;
/// The documented bump segment size (half a chunk); see the crate layout guide.
const BUMP_SEGMENT_BYTES: usize = BUMP_CHUNK_BYTES / 2;

/// Serializes tests so the shared static allocator's process-global telemetry
/// counters and backing pools stay meaningful, and touches the allocator once
/// so the current thread's heap exists before any measurement.
fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let layout = Layout::new::<u64>();
    // SAFETY: A single warm-up allocation is released with its exact layout.
    let address = unsafe { ALLOCATOR.alloc(layout) };
    assert!(!address.is_null());
    // SAFETY: `address`/`layout` describe the allocation made just above.
    unsafe { ALLOCATOR.dealloc(address, layout) };
    guard
}

/// Allocates one live block per spec while `heap` is the active hint, letting
/// the allocations escape the hint scope.
fn allocate_live(heap: &Heap, specs: &[(usize, usize, u64)]) -> Vec<Live> {
    with_hint(heap, || {
        specs
            .iter()
            .map(|&(size, align, tag)| Block::new(&ALLOCATOR, Layout::from_size_align(size, align).unwrap(), tag, false))
            .collect()
    })
}

#[test]
fn bump_payload_survives_many_live_allocations_and_segment_chunk_transitions() {
    let _guard = test_guard();
    let baseline = stats().unwrap();
    let count = if cfg!(miri) { 40 } else { 256 };
    // Sizes deliberately span sub-header, mid-segment, and near-full-segment
    // requests so the batch repeatedly crosses segment and chunk boundaries
    // while every allocation remains simultaneously live.
    let sizes = [16, 64, 4_096, 24, 8_192, 512, 2_048, 128];
    let aligns = [1, 16, 64, 8, 4_096, 32, 256, 8];
    let heap = Heap::bump(bump::Options::new());

    let specs: Vec<_> = (0..count)
        .map(|index| {
            (
                sizes[index % sizes.len()],
                aligns[index % aligns.len()],
                u64::try_from(index).unwrap() ^ 0xA5A5,
            )
        })
        .collect();
    let mut owners = allocate_live(&heap, &specs);
    check_disjoint(&owners);

    // Free in a deterministic, non-sequential order and re-verify the affected
    // allocations after each removal, so a rewind or metadata update that
    // corrupts a neighbour is caught immediately. Native runs re-sweep the whole
    // live set each step; Miri keeps this O(n) while still checking the victim
    // and the neighbour that moves into its slot.
    let full_scan = !cfg!(miri);
    let mut rng = fastrand::Rng::with_seed(0x5EED_1234);
    while !owners.is_empty() {
        let index = rng.usize(..owners.len());
        let victim = owners.swap_remove(index);
        victim.check();
        drop(victim);
        if full_scan {
            check_disjoint(&owners);
        } else if let Some(neighbour) = owners.get(index) {
            neighbour.check();
        }
    }

    drop(heap);
    let after = stats().unwrap();
    assert_eq!(after.live_bytes, baseline.live_bytes, "bump allocations leaked live bytes");
    assert_eq!(
        after.allocations - baseline.allocations,
        after.deallocations - baseline.deallocations,
        "bump allocation and deallocation counts diverged"
    );
}

#[test]
fn bump_varied_alignments_and_size_limits_fall_back_and_stay_disjoint() {
    let _guard = test_guard();
    let baseline = stats().unwrap();
    // Default bump limits: 32 KiB max allocation, 4 KiB max alignment. Requests
    // beyond either limit must fall back to the bump heap's general heap while
    // remaining disjoint from bump allocations painted the same way.
    let heap = Heap::bump(bump::Options::new());
    let mut specs = Vec::new();
    let mut tag = 1;
    for &size in &[1_usize, 24, 512, 4_096, BUMP_SEGMENT_BYTES] {
        for &align in &[1_usize, 16, 64, 4_096] {
            specs.push((size, align, tag));
            tag += 1;
        }
    }
    // Size-limited fallbacks (exceed the 32 KiB bump maximum).
    for &size in &[BUMP_SEGMENT_BYTES + 1, BUMP_CHUNK_BYTES] {
        specs.push((size, 64, tag));
        tag += 1;
    }
    // Alignment-limited fallbacks (exceed the 4 KiB bump maximum).
    for &align in &[8_192_usize, 64 * 1024] {
        specs.push((64, align, tag));
        tag += 1;
    }

    let owners = allocate_live(&heap, &specs);
    check_disjoint(&owners);
    drop(owners);
    drop(heap);

    let after = stats().unwrap();
    assert_eq!(after.live_bytes, baseline.live_bytes, "mixed bump/fallback allocations leaked");
}

#[test]
fn bump_local_tail_frees_rewind_and_middle_frees_preserve_neighbours() {
    let _guard = test_guard();
    let heap = Heap::bump(bump::Options::new());
    let layout = Layout::from_size_align(48, 16).unwrap();

    with_hint(&heap, || {
        // Deallocating the most recent allocation while the same bump heap is
        // active rewinds the cursor, so an identical follow-up request reuses
        // the exact address.
        let first = Block::new(&ALLOCATOR, layout, 0x11, false);
        let second = Block::new(&ALLOCATOR, layout, 0x22, false);
        let third = Block::new(&ALLOCATOR, layout, 0x33, false);
        let third_addr = third.address();
        third.check();
        drop(third);

        let reused = Block::new(&ALLOCATOR, layout, 0x44, false);
        assert_eq!(reused.address(), third_addr, "tail free did not rewind the bump cursor");

        // Freeing a non-tail allocation must not rewind or disturb its live
        // neighbours' payloads or addresses.
        let first_addr = first.address();
        let reused_addr = reused.address();
        drop(second);
        assert_eq!(first.address(), first_addr);
        assert_eq!(reused.address(), reused_addr);
        first.check();
        reused.check();

        drop(reused);
        drop(first);
    });

    drop(heap);
}

#[test]
fn bump_escaped_allocations_outlive_dropped_heap_handle() {
    let _guard = test_guard();
    let baseline = stats().unwrap();
    let count = if cfg!(miri) { 24 } else { 96 };

    // Allocate, then drop the logical heap handle while the allocations remain
    // live. The realized bump backing must persist and keep payloads valid.
    let owners = {
        let heap = Heap::bump(bump::Options::new());
        let specs: Vec<_> = (0..count).map(|index| (128, 16, u64::try_from(index).unwrap() + 0x300)).collect();
        let owners = allocate_live(&heap, &specs);
        drop(heap);
        owners
    };

    check_disjoint(&owners);
    for owner in owners.iter().rev() {
        owner.check();
    }
    drop(owners);

    let after = stats().unwrap();
    assert_eq!(
        after.live_bytes, baseline.live_bytes,
        "escaped bump allocations leaked after handle drop"
    );
}

#[test]
fn bump_escaped_allocation_reallocates_across_the_fallback_boundary() {
    let _guard = test_guard();
    let baseline = stats().unwrap();

    // A bump-backed allocation escapes its dropped heap handle and is then
    // reallocated. Growing past the 32 KiB bump maximum moves it out of the
    // arena into the general heap and back; the preserved prefix must survive
    // every move. `Block::reallocate` verifies the preserved prefix before
    // initializing any extension.
    let mut block = {
        let heap = Heap::bump(bump::Options::new());
        let block = with_hint(&heap, || {
            Block::new(&ALLOCATOR, Layout::from_size_align(128, 64).unwrap(), 0xB10C_5EED, false)
        });
        drop(heap);
        block
    };
    block.check();
    for new_size in [BUMP_CHUNK_BYTES, 96, BUMP_SEGMENT_BYTES + 1, 64] {
        block.reallocate(new_size);
        block.check();
    }
    drop(block);

    let after = stats().unwrap();
    assert_eq!(after.live_bytes, baseline.live_bytes, "reallocated escaped allocation leaked");
}

#[test]
fn bump_attachment_cache_eviction_preserves_live_escaped_allocations() {
    let _guard = test_guard();
    let baseline = stats().unwrap();
    // The passive attachment cache retains 8 retired hints. Cycling through more
    // than 8 distinct bump heaps evicts the oldest, which releases that heap's
    // handle. Allocations still live at eviction must survive and stay valid.
    let heaps_count = if cfg!(miri) { 10 } else { 20 };
    let heaps: Vec<Heap> = (0..heaps_count).map(|_| Heap::bump(bump::Options::new())).collect();

    let mut owners = Vec::new();
    for (index, heap) in heaps.iter().enumerate() {
        // Each new hint's first allocation retires the previous hint; once the
        // cache is full, retiring evicts (and releases the handle of) the
        // oldest, whose escaped owner is still held here.
        owners.push(
            allocate_live(heap, &[(96, 16, u64::try_from(index).unwrap() + 0x1_000)])
                .pop()
                .unwrap(),
        );
    }
    // Force the final still-active hint to retire behind a fresh flush hint.
    let flush = Heap::bump(bump::Options::new());
    owners.push(allocate_live(&flush, &[(96, 16, 0x2_000)]).pop().unwrap());

    check_disjoint(&owners);
    drop(owners);
    drop(heaps);
    drop(flush);

    let after = stats().unwrap();
    assert_eq!(
        after.live_bytes, baseline.live_bytes,
        "eviction released memory with live allocations"
    );
}

#[test]
fn bump_reattachment_reuses_the_same_backing_across_hint_scopes() {
    let _guard = test_guard();
    let heap = Heap::bump(bump::Options::new());
    let layout = Layout::from_size_align(64, 16).unwrap();

    // First scope realizes and then retires the bump heap into the cache.
    let first = allocate_live(&heap, &[(layout.size(), layout.align(), 0x51)]).pop().unwrap();
    // Interpose a different hint so the first heap is genuinely retired.
    let other = Heap::bump(bump::Options::new());
    let interposed = allocate_live(&other, &[(layout.size(), layout.align(), 0x52)]).pop().unwrap();
    // Re-entering the first hint must restore the cached attachment rather than
    // realize a fresh bump, so the new allocation lands in the same chunk.
    let second = allocate_live(&heap, &[(layout.size(), layout.align(), 0x53)]).pop().unwrap();

    let first_chunk = first.address() & !(BUMP_CHUNK_BYTES - 1);
    let second_chunk = second.address() & !(BUMP_CHUNK_BYTES - 1);
    assert_eq!(
        first_chunk, second_chunk,
        "reattachment realized a fresh bump instead of reusing the cache"
    );
    assert_ne!(
        first_chunk,
        interposed.address() & !(BUMP_CHUNK_BYTES - 1),
        "the interposed heap must have distinct backing"
    );

    check_disjoint(&[first, interposed, second]);
    drop(heap);
    drop(other);
}

#[test]
#[expect(clippy::needless_collect, reason = "All producers must be spawned before any is joined")]
fn bump_shared_heap_handle_realizes_independently_per_thread() {
    let _guard = test_guard();
    let threads = if cfg!(miri) { 2 } else { 4 };
    let per_thread = if cfg!(miri) { 8 } else { 48 };
    let heap = Heap::bump(bump::Options::new());

    // A cloned handle shares one logical identity across threads. Each thread
    // realizes its own physical bump, so the payloads must be globally disjoint.
    let escaped: Vec<Live> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|thread| {
                let heap = heap.clone();
                scope.spawn(move || {
                    let specs: Vec<_> = (0..per_thread)
                        .map(|index| (256, 64, u64::try_from(thread * per_thread + index).unwrap() + 0x9_000))
                        .collect();
                    allocate_live(&heap, &specs)
                })
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().unwrap()).collect()
    });

    check_disjoint(&escaped);
    // Releasing on the main thread frees allocations produced by other threads.
    drop(escaped);
    drop(heap);
}

#[test]
fn bump_allocations_survive_producer_exit_and_remote_final_frees() {
    let _guard = test_guard();
    let count = if cfg!(miri) { 8 } else { 40 };
    let heap = Heap::bump(bump::Options::new());
    let producer_heap = heap.clone();

    // The producer thread fully exits (join) before any allocation is freed,
    // tearing down its thread state and releasing its cached bump handle while
    // the escaped allocations remain live.
    let escaped: Vec<Live> = std::thread::spawn(move || {
        let specs: Vec<_> = (0..count).map(|index| (512, 64, u64::try_from(index).unwrap() + 0xF_000)).collect();
        allocate_live(&producer_heap, &specs)
    })
    .join()
    .unwrap();

    check_disjoint(&escaped);

    // A third, unrelated thread performs the final frees remotely, driving the
    // bump reference count to zero away from both the producer and this thread.
    std::thread::spawn(move || {
        for owner in escaped.into_iter().rev() {
            owner.check();
            drop(owner);
        }
    })
    .join()
    .unwrap();

    drop(heap);
}

#[test]
fn bump_pool_reuse_keeps_backing_growth_bounded_across_lifecycles() {
    let _guard = test_guard();
    let baseline = stats().unwrap();
    // Each lifecycle realizes a distinct bump heap, forces the same multi-chunk
    // workload, then frees it. Distinct ids fill the bounded passive attachment
    // cache; once it saturates, every further lifecycle evicts and pools the
    // oldest state, whose retained chunks back the next realization. Committed
    // backing therefore plateaus instead of growing with the lifecycle count.
    let chunks = if cfg!(miri) { 1 } else { 4 };
    let live_per_lifecycle = chunks * 2;
    // Just over half a segment so two allocations never share a segment,
    // forcing one chunk per pair while all are simultaneously live.
    let size = BUMP_SEGMENT_BYTES / 2 + 1_024;

    let run_lifecycle = || {
        let heap = Heap::bump(bump::Options::new());
        let specs: Vec<_> = (0..live_per_lifecycle)
            .map(|index| (size, 64, u64::try_from(index).unwrap() + 0x7_000))
            .collect();
        let owners = allocate_live(&heap, &specs);
        check_disjoint(&owners);
        drop(owners);
        drop(heap);
    };

    // Saturate the passive cache so backing reaches its steady state, then take
    // a settled measurement past the point where new backing is committed.
    let saturate_cycles = if cfg!(miri) { 9 } else { 14 };
    let extra_cycles = if cfg!(miri) { 2 } else { 12 };
    for _ in 0..saturate_cycles {
        run_lifecycle();
    }
    let settled = stats().unwrap();
    for _ in 0..extra_cycles {
        run_lifecycle();
    }
    let after = stats().unwrap();

    assert_eq!(after.live_bytes, baseline.live_bytes, "bump pool lifecycles leaked live bytes");
    // Past saturation, additional lifecycles must not scale committed backing.
    // Allow only a small, cycle-count-independent slack for pool/fallback churn.
    let slack = (chunks + 4) * BUMP_CHUNK_BYTES;
    assert!(
        after.mapped_bytes <= settled.mapped_bytes + slack,
        "bump backing grew after saturation: settled={}, after={}, slack={slack}",
        settled.mapped_bytes,
        after.mapped_bytes
    );
}
