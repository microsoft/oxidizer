// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for concurrent allocation and cross-thread frees.
#![expect(
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "allocator integration tests group direct allocation operations into compact fixtures"
)]

use std::alloc::{GlobalAlloc, Layout};
use std::sync::{Mutex, mpsc};

use support::stats;

mod support;

#[cfg(not(miri))]
const THREADS: usize = 8;
#[cfg(miri)]
const THREADS: usize = 2;
#[cfg(not(miri))]
const ITERATIONS: usize = 10_000;
#[cfg(miri)]
const ITERATIONS: usize = 8;

rallocator::rallocator!();
static TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
struct SendAddress(*mut u8);

// SAFETY: the wrapper only transports an address; tests deallocate it through the
// same process-global allocator after ownership has moved to the receiving thread.
unsafe impl Send for SendAddress {}

impl SendAddress {
    unsafe fn deallocate(self, allocator: &impl GlobalAlloc, layout: Layout) {
        unsafe { allocator.dealloc(self.0, layout) };
    }
}

#[test]
fn concurrent_threads_allocate_mixed_sizes() {
    let _test = test_lock();
    let before = stats().unwrap();
    let mut threads = Vec::new();

    for thread_index in 0..THREADS {
        let allocator = &GLOBAL;
        threads.push(std::thread::spawn(move || {
            let sizes: &[usize] = if cfg!(miri) {
                &[8, 64, 257, 4_096, 12_000, 64 * 1024, 512 * 1024]
            } else {
                &[
                    8,
                    64,
                    257,
                    4_096,
                    12_000,
                    64 * 1024,
                    1024 * 1024,
                    16 * 1024 * 1024,
                    64 * 1024 * 1024,
                ]
            };
            for iteration in 0..ITERATIONS {
                let size = sizes[(iteration + thread_index) % sizes.len()];
                let layout = Layout::from_size_align(size, 8).unwrap();
                let address = unsafe { allocator.alloc(layout) };
                assert!(!address.is_null());
                unsafe {
                    address.write(u8::try_from(iteration & 0xFF).unwrap());
                    address.add(size - 1).write(u8::try_from(thread_index & 0xFF).unwrap());
                    allocator.dealloc(address, layout);
                }
            }
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }

    let after = stats().unwrap();
    assert!(after.allocations - before.allocations >= THREADS * ITERATIONS);
    assert!(after.deallocations - before.deallocations >= THREADS * ITERATIONS);
}

#[test]
fn allocations_can_be_freed_on_another_thread() {
    let _test = test_lock();
    let allocator = &GLOBAL;
    let before = stats().unwrap();
    let layout = Layout::from_size_align(128 * 1024, 64 * 1024).unwrap();
    let allocation_count = if cfg!(miri) { 32 } else { 2_048 };
    let mut addresses = Vec::with_capacity(allocation_count);

    for _ in 0..addresses.capacity() {
        let address = unsafe { allocator.alloc(layout) };
        assert!(!address.is_null());
        addresses.push(SendAddress(address));
    }

    let freeing_allocator = &GLOBAL;
    std::thread::spawn(move || {
        for address in addresses {
            unsafe { address.deallocate(freeing_allocator, layout) };
        }
    })
    .join()
    .unwrap();

    let after = stats().unwrap();
    assert!(after.allocations - before.allocations >= allocation_count);
    assert!(after.deallocations - before.deallocations >= allocation_count);
}

#[test]
fn remotely_freed_small_blocks_return_to_the_owning_slab() {
    let _test = test_lock();
    let allocator = &GLOBAL;
    let layout = Layout::from_size_align(16 * 1024, 16).unwrap();
    let (addresses_tx, addresses_rx) = mpsc::channel();
    let (freed_tx, freed_rx) = mpsc::channel();
    let allocating = &GLOBAL;

    let thread = std::thread::spawn(move || {
        let addresses: Vec<_> = (0..3).map(|_| unsafe { allocating.alloc(layout) }).collect();
        assert!(addresses.iter().all(|address| !address.is_null()));
        addresses_tx.send(SendAddress(addresses[0])).unwrap();
        freed_rx.recv().unwrap();

        let reused = unsafe { allocating.alloc(layout) };
        assert_eq!(reused, addresses[0]);
        unsafe {
            allocating.dealloc(reused, layout);
            allocating.dealloc(addresses[1], layout);
            allocating.dealloc(addresses[2], layout);
        }
    });

    let address = addresses_rx.recv().unwrap();
    unsafe { address.deallocate(allocator, layout) };
    freed_tx.send(()).unwrap();
    thread.join().unwrap();
}

#[test]
fn one_remote_inbox_entry_drains_many_blocks_from_the_same_slab() {
    let _test = test_lock();
    let blocks = if cfg!(miri) { 7 } else { 31 };

    let allocator = &GLOBAL;
    let layout = Layout::from_size_align(1_024, 16).unwrap();
    let (addresses_tx, addresses_rx) = mpsc::channel();
    let (freed_tx, freed_rx) = mpsc::channel();
    let allocating = &GLOBAL;

    let thread = std::thread::spawn(move || {
        let addresses: Vec<_> = (0..blocks).map(|_| unsafe { allocating.alloc(layout) }).collect();
        let original: std::collections::HashSet<_> = addresses.iter().map(|address| address.addr()).collect();
        addresses_tx
            .send(addresses.iter().map(|address| SendAddress(*address)).collect::<Vec<_>>())
            .unwrap();
        freed_rx.recv().unwrap();

        let reused: Vec<_> = (0..blocks * 2).map(|_| unsafe { allocating.alloc(layout) }).collect();
        assert!(original.iter().all(|address| reused.iter().any(|reused| reused.addr() == *address)));
        for address in reused {
            unsafe { allocating.dealloc(address, layout) };
        }
    });

    for address in addresses_rx.recv().unwrap() {
        unsafe { address.deallocate(allocator, layout) };
    }
    freed_tx.send(()).unwrap();
    thread.join().unwrap();
}

#[test]
fn small_blocks_can_be_freed_after_the_owner_thread_exits() {
    let _test = test_lock();
    let allocator = &GLOBAL;
    let before = stats().unwrap();
    let layout = Layout::from_size_align(128, 16).unwrap();
    let allocating = &GLOBAL;
    let allocation_count = if cfg!(miri) { 16 } else { 128 };
    let addresses = std::thread::spawn(move || {
        (0..allocation_count)
            .map(|_| SendAddress(unsafe { allocating.alloc(layout) }))
            .collect::<Vec<_>>()
    })
    .join()
    .unwrap();

    for address in addresses {
        unsafe { address.deallocate(allocator, layout) };
    }
    assert!(stats().unwrap().deallocations - before.deallocations >= allocation_count);
}

#[test]
fn concurrent_overaligned_allocations_preserve_alignment() {
    let _test = test_lock();
    let mut threads = Vec::new();

    let thread_count = if cfg!(miri) { 2 } else { 4 };
    let iterations = if cfg!(miri) { 4 } else { 100 };
    for _ in 0..thread_count {
        let allocator = &GLOBAL;
        threads.push(std::thread::spawn(move || {
            for alignment in [16, 64, 4_096, 65_536] {
                let layout = Layout::from_size_align(257, alignment).unwrap();
                for _ in 0..iterations {
                    let address = unsafe { allocator.alloc(layout) };
                    assert!(!address.is_null());
                    assert_eq!(address as usize % alignment, 0);
                    unsafe { allocator.dealloc(address, layout) };
                }
            }
        }));
    }

    for thread in threads {
        thread.join().unwrap();
    }
}

fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
