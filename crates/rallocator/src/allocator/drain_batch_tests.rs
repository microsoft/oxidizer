// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;
use std::ptr;
use std::sync::Barrier;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

use super::{
    drain_detached_remote_blocks, read_free_next, read_free_requested, release_free_metadata, tracking, write_free_next,
    write_free_requested,
};

fn expected_claim(pending: usize, drained: usize, count: usize) -> [usize; 2] {
    [pending.wrapping_sub(count), drained.wrapping_add(count)]
}

struct Chain {
    _storage: Vec<[usize; 2]>,
    nodes: Vec<*mut u8>,
}

impl Chain {
    fn new(count: usize) -> Self {
        // Fully size the backing buffer before deriving pointers; never resize
        // it afterward, so these nonoverlapping nodes stay live and stable.
        let mut storage = vec![[0; 2]; count];
        let nodes = storage.iter_mut().map(|block| block.as_mut_ptr().cast::<u8>()).collect::<Vec<_>>();
        for (index, &node) in nodes.iter().enumerate() {
            let next = nodes.get(index + 1).copied().unwrap_or(ptr::null_mut());
            // SAFETY: each distinct nonoverlapping two-word node has room for both
            // native metadata words; the Miri HAL registers its side metadata.
            unsafe {
                write_free_next(node, next);
                write_free_requested(node, index + 1);
            }
        }
        Self { _storage: storage, nodes }
    }
}

// The real claim still updates cumulative D, now in process-wide shards.
// Assert the private pending cell at every release; exact D arithmetic/collision
// oracles live in telemetry::remote_counts and never sample racing global totals.
fn pending_value(pending: &AtomicUsize) -> usize {
    pending.load(Ordering::Relaxed)
}

#[test]
fn empty_detached_list_neither_checks_gates_nor_processes_nodes() {
    let mut gates = 0;
    let mut processed = 0;
    // SAFETY: the empty list owns no metadata; callbacks touch only local counters.
    unsafe {
        drain_detached_remote_blocks(
            ptr::null_mut(),
            0,
            || {
                gates += 1;
                None
            },
            |_| processed += 1,
        );
    };
    assert_eq!((gates, processed), (0, 0));
}

#[test]
fn singleton_claim_precedes_every_metadata_release() {
    exact_claim_precedes_every_metadata_release(1);
}

#[test]
fn two_node_claim_precedes_every_metadata_release() {
    exact_claim_precedes_every_metadata_release(2);
}

#[test]
fn multi_node_claim_precedes_every_metadata_release() {
    exact_claim_precedes_every_metadata_release(7);
}

fn exact_claim_precedes_every_metadata_release(count: usize) {
    let chain = Chain::new(count);
    let available = AtomicBool::new(true);
    let pending = AtomicUsize::new(count);
    let gates = Cell::new(0);
    let mut processed = 0;
    // SAFETY: this fixture owns the complete immutable chain. Processing releases
    // only its current HAL entry; backing allocations remain live until return.
    unsafe {
        drain_detached_remote_blocks(
            chain.nodes[0],
            count,
            || {
                gates.set(gates.get() + 1);
                tracking::RemoteDrainClaim::if_available(&available, &pending)
            },
            |node| {
                assert_eq!(pending_value(&pending), expected_claim(count, 0, count)[0]);
                assert_eq!(node, chain.nodes[processed]);
                // All not-yet-processed HAL entries survived the entire count
                // pass, including their padding metadata on the Miri path.
                for (index, &remaining) in chain.nodes.iter().enumerate().skip(processed) {
                    assert_eq!(read_free_requested(remaining), index + 1);
                    assert_eq!(
                        read_free_next(remaining),
                        chain.nodes.get(index + 1).copied().unwrap_or(ptr::null_mut())
                    );
                }
                release_free_metadata(node);
                processed += 1;
            },
        );
    };
    assert_eq!(
        (processed, gates.get(), pending_value(&pending)),
        (count, 1, expected_claim(count, 0, count)[0])
    );
}

#[test]
fn false_first_gate_is_not_rechecked_after_it_publishes_true() {
    let chain = Chain::new(3);
    let available = AtomicBool::new(false);
    let pending = AtomicUsize::new(3);
    let gates = Cell::new(0);
    let mut observations = Vec::new();
    // SAFETY: one consumer owns these three live, immutable HAL entries.
    unsafe {
        drain_detached_remote_blocks(
            chain.nodes[0],
            3,
            || {
                let observed = tracking::RemoteDrainClaim::if_available(&available, &pending);
                gates.set(gates.get() + 1);
                if gates.get() == 1 {
                    // Publish after the actual first false observation, before
                    // that result is consumed by the drain's fallback.
                    available.store(true, Ordering::Release);
                }
                observed
            },
            |node| {
                observations.push(pending_value(&pending));
                release_free_metadata(node);
            },
        );
    };
    assert_eq!(
        (gates.get(), observations),
        (
            3,
            vec![expected_claim(3, 0, 0)[0], expected_claim(3, 0, 1)[0], expected_claim(3, 0, 2)[0]]
        )
    );
}

#[test]
fn disabled_gate_keeps_all_nodes_unitwise_unreported() {
    let chain = Chain::new(3);
    let gates = Cell::new(0);
    let mut processed = 0;
    // This injected None models unavailable aggregate counters, without resetting
    // process-lifetime state. The pre-count exists only inside the Some branch.
    // SAFETY: this fixture owns the complete chain until each entry is released.
    unsafe {
        drain_detached_remote_blocks(
            chain.nodes[0],
            3,
            || {
                gates.set(gates.get() + 1);
                None
            },
            |node| {
                assert_eq!(gates.get(), processed + 1);
                release_free_metadata(node);
                processed += 1;
            },
        );
    };
    assert_eq!((gates.get(), processed), (3, 3));
}

#[test]
fn acquire_detached_count_excludes_a_concurrent_new_publication() {
    let chain = Chain::new(3);
    // SAFETY: links are changed before publication; node2 is the separate
    // producer's allocation, and nodes0/1 form the original detached list.
    unsafe { write_free_next(chain.nodes[1], ptr::null_mut()) };
    let remote = AtomicPtr::new(ptr::null_mut());
    remote.store(chain.nodes[0], Ordering::Release);
    let detached = remote.swap(ptr::null_mut(), Ordering::Acquire);
    let producer_node = AtomicPtr::new(chain.nodes[2]);
    let pending = AtomicUsize::new(2);
    let available = AtomicBool::new(true);
    let ready = Barrier::new(2);
    let published = Barrier::new(2);
    let gates = Cell::new(0);
    let mut processed = 0;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            ready.wait();
            let node = producer_node.load(Ordering::Relaxed);
            pending.fetch_add(1, Ordering::Relaxed);
            let mut head = remote.load(Ordering::Relaxed);
            loop {
                // SAFETY: only this producer writes node2 until successful CAS;
                // it does not dereference any old head or touch detached nodes.
                unsafe { write_free_next(node, head) };
                match remote.compare_exchange_weak(head, node, Ordering::Release, Ordering::Relaxed) {
                    Ok(_) => break,
                    Err(actual) => head = actual,
                }
            }
            published.wait();
        });
        // SAFETY: Acquire detached nodes0/1; node2 is not in their chain.
        unsafe {
            drain_detached_remote_blocks(
                detached,
                3,
                || {
                    assert_eq!(
                        gates.replace(gates.get() + 1),
                        0,
                        "a positive whole-list gate must not be rechecked"
                    );
                    ready.wait();
                    published.wait();
                    tracking::RemoteDrainClaim::if_available(&available, &pending)
                },
                |node| {
                    assert_eq!(pending_value(&pending), expected_claim(3, 0, 2)[0]);
                    release_free_metadata(node);
                    processed += 1;
                },
            );
        }
    });
    assert_eq!((processed, remote.load(Ordering::Acquire)), (2, chain.nodes[2]));
    let remaining = remote.swap(ptr::null_mut(), Ordering::Acquire);
    // SAFETY: the producer joined; node2's singleton metadata remains live.
    unsafe {
        drain_detached_remote_blocks(
            remaining,
            3,
            || tracking::RemoteDrainClaim::if_available(&available, &pending),
            |node| release_free_metadata(node),
        );
    };
    assert_eq!(pending_value(&pending), expected_claim(3, 0, 3)[0]);
}

#[test]
fn normal_slab_drain_recycles_and_clears_padding() {
    actual_normal_drain_recycles_and_clears_padding(false);
}

#[test]
fn context_slab_drain_recycles_and_clears_padding() {
    actual_normal_drain_recycles_and_clears_padding(true);
}

#[test]
fn real_producer_and_owner_drain_allow_concurrent_aggregate_observation() {
    use super::{
        DIRECT_SLAB_SEGMENT, DomainState, GeneralOptions, Rallocator, ReusableHeapState, SLAB_MARKER, SLAB_SIZE, SlabAllocation,
        SlabHeader, drain_remote_blocks, hal, push_remote_block, record_small_allocation, take_local_slab_block,
    };
    use crate::config::Standard;

    // SAFETY: a private mapped slab, never installed as a global heap, stays
    // alive until the scoped publisher and observer have both joined.
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let mut domain = DomainState::new();
    let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
    let storage = hal::map(SLAB_SIZE);
    assert!(!storage.is_null());
    let first = allocator.initialize_slab(
        SlabAllocation {
            address: storage,
            segment_slices: DIRECT_SLAB_SEGMENT,
            committed_bytes: SLAB_SIZE,
        },
        0,
        &mut heap,
        SLAB_MARKER,
    );
    let slab = storage.cast::<SlabHeader>();
    // SAFETY: the fixture exclusively owns all allocated slots until published.
    let nodes = unsafe {
        let mut nodes = vec![AtomicPtr::new(first)];
        for _ in 1..8 {
            nodes.push(AtomicPtr::new(take_local_slab_block::<Standard>(slab, 0)));
        }
        for node in &nodes {
            let node = node.load(Ordering::Relaxed);
            assert!(!node.is_null());
            record_small_allocation::<Standard>(node, 0, 1, ptr::null_mut());
        }
        nodes
    };
    let shared_slab = AtomicPtr::new(slab);
    let done = AtomicBool::new(false);
    let ready = Barrier::new(3);
    tracking::record_allocation(1);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            ready.wait();
            for node in &nodes {
                // SAFETY: this publisher owns each distinct allocated slot.
                // Only the consumer touches recycled metadata after publication.
                unsafe {
                    push_remote_block(shared_slab.load(Ordering::Relaxed), node.load(Ordering::Relaxed), 0, 1);
                }
                std::thread::yield_now();
            }
            done.store(true, Ordering::Release);
        });
        scope.spawn(|| {
            ready.wait();
            for _ in 0..16 {
                // No common-instant relation among F/P/I/D is asserted.
                assert!(tracking::stats().is_some());
                std::thread::yield_now();
            }
        });
        ready.wait();
        while !done.load(Ordering::Acquire) {
            // SAFETY: the sole owner drains only Acquire-detached nodes.
            unsafe { drain_remote_blocks::<Standard>(slab, 0) };
            std::thread::yield_now();
        }
    });
    // SAFETY: all publishers joined, the fixture remains the sole owner, and no
    // node or inbox link will be used after clearing the inbox and unmapping.
    unsafe {
        drain_remote_blocks::<Standard>(slab, 0);
        assert_eq!(((*slab).free_count, (*slab).requested_bytes), ((*slab).usable_blocks as usize, 0));
        (*heap.owner).remote_slabs.store(ptr::null_mut(), Ordering::Relaxed);
        hal::unmap(storage, SLAB_SIZE);
    }
    tracking::record_deallocation_stats(1);
}

fn actual_normal_drain_recycles_and_clears_padding(context: bool) {
    use super::{
        CONTEXT_SLAB_MARKER, DIRECT_SLAB_SEGMENT, DomainState, GeneralOptions, Rallocator, ReusableHeapState, SLAB_MARKER, SLAB_SIZE,
        SlabAllocation, SlabHeader, drain_remote_blocks, hal, push_remote_block, record_small_allocation, take_local_slab_block,
    };
    use crate::config::Standard;

    // SAFETY: this is the same isolated, directly mapped slab setup used by the
    // existing publication-retry fixture; it is not installed as a global heap.
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let mut domain = DomainState::new();
    let mut heap = ReusableHeapState::new(GeneralOptions::new(), ptr::from_mut(&mut domain));
    let storage = hal::map(SLAB_SIZE);
    assert!(!storage.is_null());
    let first = allocator.initialize_slab(
        SlabAllocation {
            address: storage,
            segment_slices: DIRECT_SLAB_SEGMENT,
            committed_bytes: SLAB_SIZE,
        },
        0,
        &mut heap,
        if context { CONTEXT_SLAB_MARKER } else { SLAB_MARKER },
    );
    let slab = storage.cast::<SlabHeader>();
    // SAFETY: initialization gave this fixture exclusive live slab ownership.
    unsafe {
        let second = take_local_slab_block::<Standard>(slab, 0);
        assert!(!first.is_null() && !second.is_null());
        for node in [first, second] {
            record_small_allocation::<Standard>(node, 0, 1, ptr::null_mut());
            // Use the real producer so process-global pending increments are
            // paired with the real drain, without resetting/inspecting globals.
            push_remote_block(slab, node, 0, 1);
        }
        drain_remote_blocks::<Standard>(slab, 0);
        assert_eq!(
            (
                (*slab).free_count,
                (*slab).requested_bytes,
                (*slab).remote_free.load(Ordering::Acquire)
            ),
            ((*slab).usable_blocks as usize, 0, ptr::null_mut())
        );
        (*heap.owner).remote_slabs.store(ptr::null_mut(), Ordering::Relaxed);
        hal::unmap(storage, SLAB_SIZE);
    }
}

#[cfg(not(miri))]
#[test]
fn queued_retirement_drain_and_late_free_keep_separate_lifetimes() {
    use super::{
        DIRECT_SLAB_SEGMENT, DomainState, Rallocator, RetiredSliceState, SLAB_MARKER, SLAB_SIZE, SlabAllocation, SlabHeader,
        create_bump_fallback_heap, hal, push_remote_block, record_small_allocation, retire_general_heap, take_local_slab_block,
    };
    use crate::config::Standard;

    // SAFETY: the fixture retains its domain through the final escaped release,
    // following the existing non-Miri direct-slab retirement fixture.
    let allocator = unsafe { Rallocator::<Standard>::new() };
    let mut domain = DomainState::new();
    let heap = create_bump_fallback_heap(ptr::from_mut(&mut domain));
    assert!(!heap.is_null());
    let storage = hal::map(SLAB_SIZE);
    assert!(!storage.is_null());
    // SAFETY: the new heap and slab are exclusively owned, initialized below,
    // and retained until the final late free. No pointer is used after it.
    unsafe {
        let queued = allocator.initialize_slab(
            SlabAllocation {
                address: storage,
                segment_slices: DIRECT_SLAB_SEGMENT,
                committed_bytes: SLAB_SIZE,
            },
            0,
            &mut *heap,
            SLAB_MARKER,
        );
        let slab = storage.cast::<SlabHeader>();
        let escaped = take_local_slab_block::<Standard>(slab, 0);
        assert!(!queued.is_null() && !escaped.is_null());
        for node in [queued, escaped] {
            record_small_allocation::<Standard>(node, 0, 1, ptr::null_mut());
        }
        push_remote_block(slab, queued, 0, 1);
        retire_general_heap(heap);
        let retired = storage.cast::<RetiredSliceState>();
        assert_eq!(
            (
                (*retired).remaining.load(Ordering::Acquire),
                (*retired).ready.load(Ordering::Acquire)
            ),
            (1, true)
        );
        // Rejected operation admission takes the existing late F-only path;
        // the real remaining counter, not pending/drained, releases storage.
        push_remote_block(slab, escaped, 0, 1);
    }
}
