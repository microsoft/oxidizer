// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Whole-process allocator, ownership, payload, and external size-class regressions.

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Barrier, mpsc};

use rallocator::Rallocator;

rallocator::rallocator!();

#[test]
fn sizes_alignments_and_payloads() {
    for shift in 0..=20 {
        for size in [1, 15, 16, 17, 31, 48, 63, 64, 65, 513, 4095, 16385, 65535, 65536, 65537] {
            let layout = Layout::from_size_align(size, 1 << shift).unwrap();
            {
                // SAFETY: The validated layout is nonzero.
                let a = unsafe { Rallocator.alloc(layout) };
                // SAFETY: A second independent allocation uses the same valid layout.
                let b = unsafe { Rallocator.alloc(layout) };
                assert!(!a.is_null() && !b.is_null());
                assert_ne!(a, b);
                assert_eq!(a as usize % layout.align(), 0);
                // SAFETY: The layout provides size writable bytes.
                unsafe { a.write_bytes(0x5a, size) };
                // SAFETY: b is a distinct checked allocation with the same extent.
                unsafe { b.write_bytes(0xa5, size) };
                // SAFETY: All bytes in a were initialized above.
                assert!(unsafe { std::slice::from_raw_parts(a, size) }.iter().all(|&x| x == 0x5a));
                // SAFETY: All bytes in b were initialized above.
                assert!(unsafe { std::slice::from_raw_parts(b, size) }.iter().all(|&x| x == 0xa5));
                // SAFETY: The slice borrow ended; a is retired once.
                unsafe { Rallocator.dealloc(a, layout) };
                // SAFETY: b likewise has no remaining borrow.
                unsafe { Rallocator.dealloc(b, layout) };
            }
        }
    }
}

#[test]
fn all_small_sizes_and_class_reuse() {
    for size in 1..=65537 {
        let layout = Layout::from_size_align(size, 16).unwrap();
        // SAFETY: Every layout has a positive size and valid alignment.
        let ptr = unsafe { Rallocator.alloc(layout) };
        assert!(!ptr.is_null());
        // SAFETY: The checked allocation contains its first byte.
        unsafe { ptr.write(37) };
        // SAFETY: size-1 is within the checked allocation.
        let last = unsafe { ptr.add(size - 1) };
        // SAFETY: The last byte is exclusively writable.
        unsafe { last.write(37) };
        // SAFETY: The allocation is retired once using its exact layout.
        unsafe { Rallocator.dealloc(ptr, layout) };
    }
}

#[test]
fn zero_realloc_and_oom_preserve_input() {
    for align in [1, 16, 64, 4096, 131_072] {
        let mut layout = Layout::from_size_align(1031, align).unwrap();
        {
            // SAFETY: The initial layout is valid and nonzero.
            let mut ptr = unsafe { Rallocator.alloc_zeroed(layout) };
            assert!(!ptr.is_null());
            // SAFETY: The checked zeroed allocation initializes the full layout.
            assert!(unsafe { std::slice::from_raw_parts(ptr, layout.size()) }.iter().all(|&b| b == 0));
            // SAFETY: This live allocation is exclusively writable.
            unsafe { ptr.write_bytes(0x5b, layout.size()) };
            for size in [1040, 2049, 65536, 131_073, 7] {
                // SAFETY: ptr is live with this original layout and size is nonzero.
                let replacement = unsafe { Rallocator.realloc(ptr, layout, size) };
                assert!(!replacement.is_null());
                assert_eq!(replacement as usize % align, 0);
                assert!(
                    // SAFETY: Successful realloc preserves the overlapping initialized prefix.
                    unsafe { std::slice::from_raw_parts(replacement, size.min(layout.size())) }
                        .iter()
                        .all(|&b| b == 0x5b)
                );
                // SAFETY: The checked replacement owns size writable bytes.
                unsafe { replacement.write_bytes(0x5b, size) };
                ptr = replacement;
                layout = Layout::from_size_align(size, align).unwrap();
            }
            // SAFETY: ptr remains live; the impossible replacement is expected to fail.
            let failure = unsafe { Rallocator.realloc(ptr, layout, usize::MAX) };
            assert!(failure.is_null());
            // SAFETY: Failed realloc preserves this initialized live allocation.
            assert_eq!(unsafe { *ptr }, 0x5b);
            // SAFETY: The final live allocation is retired exactly once.
            unsafe { Rallocator.dealloc(ptr, layout) };
        }
    }
}

#[test]
fn many_live_objects_survive_shuffled_returns() {
    let mut pointers = Vec::new();
    for n in 0..24000usize {
        let size = [16, 48, 80, 224, 896, 3072, 14336, 57344][n % 8];
        let layout = Layout::from_size_align(size, 16).unwrap();
        // SAFETY: The validated nonzero layout requests 16-byte alignment.
        let ptr = unsafe { Rallocator.alloc(layout) };
        assert!(!ptr.is_null());
        #[expect(clippy::cast_ptr_alignment, reason = "The checked allocation was requested with 16-byte alignment")]
        let first = ptr.cast::<usize>();
        // SAFETY: The aligned first word lies inside the live allocation.
        unsafe { first.write(n) };
        // SAFETY: Every selected size is at least 16 and a multiple of 16.
        let last = unsafe { ptr.add(size - 8) };
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "The final 8-byte word is aligned within the 16-byte-aligned allocation"
        )]
        let last = last.cast::<usize>();
        // SAFETY: The final aligned word lies within the same live allocation.
        unsafe { last.write(!n) };
        pointers.push((ptr as usize, layout, n));
    }
    let mut seed = 123_456_789_u64;
    for i in (1..pointers.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        pointers.swap(i, usize::try_from(seed).unwrap() % (i + 1));
    }
    for (address, layout, n) in pointers {
        let ptr = address as *mut u8;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "Saved pointers retain their original 16-byte-aligned layouts"
        )]
        let first = ptr.cast::<usize>();
        // SAFETY: Every saved pointer is live with an initialized first word.
        assert_eq!(unsafe { first.read() }, n);
        // SAFETY: The selected layouts all contain an aligned final word.
        let last = unsafe { ptr.add(layout.size() - 8) };
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "The layout size is a multiple of 16, making its final word 8-byte aligned"
        )]
        let last = last.cast::<usize>();
        // SAFETY: The last word was initialized before shuffling.
        assert_eq!(unsafe { last.read() }, !n);
        // SAFETY: The uniquely saved allocation is retired exactly once.
        unsafe { Rallocator.dealloc(ptr, layout) };
    }
}

struct LiveAllocation {
    pointer: *mut u8,
    layout: Layout,
    marker: u8,
}

impl LiveAllocation {
    fn new(size: usize, alignment: usize, marker: u8) -> Self {
        #[expect(
            clippy::unwrap_used,
            reason = "the stress generator uses nonzero sizes and power-of-two alignments"
        )]
        let layout = Layout::from_size_align(size, alignment).unwrap();
        // SAFETY: The validated layout is nonzero.
        let pointer = unsafe { Rallocator.alloc(layout) };
        assert!(!pointer.is_null());
        assert_eq!(pointer as usize % alignment, 0);
        let allocation = Self { pointer, layout, marker };
        allocation.write_markers();
        allocation
    }

    fn verify(&self) {
        // SAFETY: This object uniquely owns the first byte of its live allocation.
        assert_eq!(unsafe { self.pointer.read() }, self.marker);
        if self.layout.size() > 1 {
            // SAFETY: The final byte lies within this uniquely owned live layout.
            let last = unsafe { self.pointer.add(self.layout.size() - 1) };
            // SAFETY: The final byte was initialized by write_markers.
            assert_eq!(unsafe { last.read() }, !self.marker);
        }
    }

    fn write_markers(&self) {
        // SAFETY: This object uniquely owns the first byte of its live allocation.
        unsafe { self.pointer.write(self.marker) };
        if self.layout.size() > 1 {
            // SAFETY: The final byte lies within this uniquely owned live layout.
            let last = unsafe { self.pointer.add(self.layout.size() - 1) };
            // SAFETY: The final byte is exclusively writable.
            unsafe { last.write(!self.marker) };
        }
    }

    fn resize(&mut self, size: usize) {
        self.verify();
        // SAFETY: The pointer is live with the exact stored layout, and the
        // replacement size is nonzero and valid for the retained alignment.
        let replacement = unsafe { Rallocator.realloc(self.pointer, self.layout, size) };
        assert!(!replacement.is_null());
        assert_eq!(replacement as usize % self.layout.align(), 0);
        // SAFETY: Every successful realloc preserves the initialized first byte.
        assert_eq!(unsafe { replacement.read() }, self.marker);
        self.pointer = replacement;
        #[expect(clippy::unwrap_used, reason = "the nonzero generated size retains an already valid alignment")]
        let layout = Layout::from_size_align(size, self.layout.align()).unwrap();
        self.layout = layout;
        self.write_markers();
    }
}

impl Drop for LiveAllocation {
    fn drop(&mut self) {
        self.verify();
        // SAFETY: Drop retires this uniquely owned allocation exactly once.
        unsafe { Rallocator.dealloc(self.pointer, self.layout) };
    }
}

fn random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn randomized_allocate_resize_and_free_trace_preserves_payloads() {
    const SIZES: [usize; 24] = [
        1, 15, 16, 17, 31, 32, 48, 63, 64, 65, 95, 96, 129, 257, 513, 1025, 4097, 8193, 16_385, 32_769, 49_153, 65_536, 65_537, 131_073,
    ];
    let mut seed = 0xd1b5_4a32_d192_ed03;
    let mut live = Vec::with_capacity(512);
    for operation in 0..100_000 {
        let choice = random(&mut seed);
        if live.is_empty() || live.len() < 512 && choice % 100 < 45 {
            let size = SIZES[usize::try_from(random(&mut seed)).unwrap() % SIZES.len()];
            let alignment = 1usize << (usize::try_from(random(&mut seed)).unwrap() % 13);
            let marker = u8::try_from(operation % 251).unwrap();
            live.push(LiveAllocation::new(size, alignment, marker));
        } else {
            let index = usize::try_from(random(&mut seed)).unwrap() % live.len();
            if choice % 100 < 72 {
                let size = SIZES[usize::try_from(random(&mut seed)).unwrap() % SIZES.len()];
                live[index].resize(size);
            } else {
                drop(live.swap_remove(index));
            }
        }
        if operation % 4096 == 0 {
            for allocation in &live {
                allocation.verify();
            }
        }
    }
    for allocation in &live {
        allocation.verify();
    }
    drop(live);
}

#[test]
fn relaxed_pointer_handoffs_support_concurrent_free_only_ownership_transfer() {
    const PAIRS: usize = 8;
    const PER_PAIR: usize = 4096;
    std::thread::scope(|scope| {
        for pair in 0..PAIRS {
            let slots = Arc::new((0..PER_PAIR).map(|_| AtomicPtr::new(std::ptr::null_mut())).collect::<Vec<_>>());
            let producer_slots = Arc::clone(&slots);
            scope.spawn(move || {
                for (index, slot) in producer_slots.iter().enumerate() {
                    let size = 16 + (index * 131 + pair * 17) % 65521;
                    let layout = Layout::from_size_align(size, 16).unwrap();
                    // SAFETY: The validated layout is nonzero and ownership is
                    // transferred to the consumer through this unique slot.
                    let pointer = unsafe { Rallocator.alloc(layout) };
                    assert!(!pointer.is_null());
                    slot.store(pointer, Ordering::Relaxed);
                }
            });
            scope.spawn(move || {
                for (index, slot) in slots.iter().enumerate() {
                    let pointer = loop {
                        let pointer = slot.swap(std::ptr::null_mut(), Ordering::Relaxed);
                        if !pointer.is_null() {
                            break pointer;
                        }
                        std::hint::spin_loop();
                    };
                    let size = 16 + (index * 131 + pair * 17) % 65521;
                    let layout = Layout::from_size_align(size, 16).unwrap();
                    // SAFETY: The relaxed swap uniquely acquires this pointer
                    // solely for deallocation; no application payload is read.
                    unsafe { Rallocator.dealloc(pointer, layout) };
                }
            });
        }
    });
}

#[test]
fn many_exited_owners_feed_concurrent_remote_consumers() {
    const PRODUCERS: usize = 16;
    const CONSUMERS: usize = 8;
    const OBJECTS_PER_DESTINATION: usize = 64;
    let start = Arc::new(Barrier::new(PRODUCERS + CONSUMERS));
    let consumers = (0..CONSUMERS)
        .map(|consumer| {
            let (sender, receiver) = mpsc::channel::<Vec<Vec<u8>>>();
            let start = Arc::clone(&start);
            let handle = std::thread::spawn(move || {
                start.wait();
                for batch in receiver {
                    for (index, object) in batch.into_iter().enumerate() {
                        let marker = u8::try_from((consumer + index) % 251).unwrap();
                        assert!(object.iter().all(|&byte| byte == marker));
                    }
                }
            });
            (sender, handle)
        })
        .collect::<Vec<_>>();
    let producers = (0..PRODUCERS)
        .map(|producer| {
            let destinations = consumers.iter().map(|(sender, _)| sender.clone()).collect::<Vec<_>>();
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                for (consumer, destination) in destinations.into_iter().enumerate() {
                    let batch = (0..OBJECTS_PER_DESTINATION)
                        .map(|index| {
                            let marker = u8::try_from((consumer + index) % 251).unwrap();
                            vec![marker; 1 + (producer * 97 + consumer * 53 + index * 31) % 65536]
                        })
                        .collect();
                    destination.send(batch).unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for producer in producers {
        producer.join().unwrap();
    }
    let handles = consumers
        .into_iter()
        .map(|(sender, handle)| {
            drop(sender);
            handle
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn owner_exit_remote_free_and_address_reuse_soak() {
    const WAVES: usize = 24;
    const PRODUCERS: usize = 8;
    const CONSUMERS: usize = 4;
    const OBJECTS_PER_PRODUCER: usize = 128;
    let sizes = [16, 48, 80, 224, 896, 3072, 4097, 14_336, 57_344, 65_537];
    let mut seen = std::collections::BTreeSet::new();
    let mut reused = 0;

    for wave in 0..WAVES {
        let start = Arc::new(Barrier::new(PRODUCERS));
        #[expect(
            clippy::needless_collect,
            reason = "all producers must reach the barrier before any handle is joined"
        )]
        let producers = (0..PRODUCERS)
            .map(|producer| {
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    (0..OBJECTS_PER_PRODUCER)
                        .map(|index| {
                            let size = sizes[(wave + producer * 3 + index * 7) % sizes.len()];
                            let layout = Layout::from_size_align(size, 16).unwrap();
                            // SAFETY: The validated layout is nonzero.
                            let pointer = unsafe { Rallocator.alloc(layout) };
                            assert!(!pointer.is_null());
                            let marker = u8::try_from((wave + producer + index) % 251).unwrap();
                            // SAFETY: The allocation contains its first byte.
                            unsafe { pointer.write(marker) };
                            // SAFETY: The final byte lies within this live allocation.
                            let last = unsafe { pointer.add(size - 1) };
                            // SAFETY: The final byte is exclusively writable.
                            unsafe { last.write(!marker) };
                            (pointer as usize, layout, marker)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let allocations = producers
            .into_iter()
            .flat_map(|producer| producer.join().unwrap())
            .collect::<Vec<_>>();
        for &(pointer, _, _) in &allocations {
            reused += usize::from(!seen.insert(pointer));
        }

        let mut destinations = (0..CONSUMERS).map(|_| Vec::new()).collect::<Vec<_>>();
        for (index, allocation) in allocations.into_iter().enumerate() {
            destinations[index % CONSUMERS].push(allocation);
        }
        let consumers = destinations
            .into_iter()
            .map(|allocations| {
                std::thread::spawn(move || {
                    for (address, layout, marker) in allocations {
                        let pointer = address as *mut u8;
                        // SAFETY: Ownership moved from its exited producer to
                        // this consumer, which reads only initialized bytes.
                        assert_eq!(unsafe { pointer.read() }, marker);
                        // SAFETY: The final initialized byte lies in this live allocation.
                        let last = unsafe { pointer.add(layout.size() - 1) };
                        // SAFETY: The producer initialized this byte before ownership transfer.
                        assert_eq!(unsafe { last.read() }, !marker);
                        // SAFETY: This consumer retires the allocation exactly once.
                        unsafe { Rallocator.dealloc(pointer, layout) };
                    }
                })
            })
            .collect::<Vec<_>>();
        for consumer in consumers {
            consumer.join().unwrap();
        }
    }
    assert!(reused > 0, "owner recycling never reused an allocation address");
}

#[test]
fn repeated_oversized_failures_leave_allocator_operational() {
    for alignment in [1, 16, 4096, 1 << 20] {
        let oversized = Layout::from_size_align(1 << 47, alignment).unwrap();
        for _ in 0..128 {
            // SAFETY: The layout is valid; allocation failure is represented by null.
            assert!(unsafe { Rallocator.alloc(oversized) }.is_null());
            // SAFETY: The same valid oversized request may fail without side effects.
            assert!(unsafe { Rallocator.alloc_zeroed(oversized) }.is_null());
        }
        let ordinary = Layout::from_size_align(4097, alignment).unwrap();
        // SAFETY: This valid ordinary request verifies recovery after failures.
        let pointer = unsafe { Rallocator.alloc(ordinary) };
        assert!(!pointer.is_null());
        // SAFETY: The allocation contains its complete requested extent.
        unsafe { pointer.write_bytes(0x6d, ordinary.size()) };
        // SAFETY: The live allocation is retired with its exact layout.
        unsafe { Rallocator.dealloc(pointer, ordinary) };
    }
}

#[test]
fn owner_exits_before_remote_free() {
    for _ in 0..40 {
        let allocations = std::thread::spawn(|| {
            (0..2000)
                .map(|i| vec![u8::try_from(i % 251).unwrap(); 16 + i % 200])
                .collect::<Vec<_>>()
        })
        .join()
        .unwrap();
        let consumer = std::thread::spawn(move || {
            for (i, allocation) in allocations.into_iter().enumerate() {
                assert!(allocation.iter().all(|&b| b == u8::try_from(i % 251).unwrap()));
            }
        });
        consumer.join().unwrap();
    }
}

#[test]
fn concurrent_publishers_while_owner_drains() {
    let barrier = Arc::new(Barrier::new(9));
    let (tx, rx) = mpsc::channel();
    let consumers = (0..8)
        .map(|_| {
            let (send, receive) = mpsc::channel::<Vec<Vec<u8>>>();
            let barrier = Arc::clone(&barrier);
            let done = tx.clone();
            let handle = std::thread::spawn(move || {
                barrier.wait();
                for batch in receive {
                    for object in batch {
                        assert_eq!(object[0], 0x35);
                    }
                }
                done.send(()).unwrap();
            });
            (send, handle)
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for _ in 0..100 {
        for (sender, _) in &consumers {
            sender.send((0..200).map(|i| vec![0x35; 16 + i * 16]).collect()).unwrap();
        }
        for i in 0..200 {
            std::hint::black_box(vec![0x33u8; 64 + i]);
        }
    }
    let handles = consumers
        .into_iter()
        .map(|(sender, handle)| {
            drop(sender);
            handle
        })
        .collect::<Vec<_>>();
    for _ in 0..8 {
        rx.recv().unwrap();
    }
    for handle in handles {
        handle.join().unwrap();
    }
}

std::thread_local! {
    static LATE: LateDrop = const { LateDrop };
}

struct LateDrop;
impl Drop for LateDrop {
    fn drop(&mut self) {
        // This registration predates allocator TLS, so its destructor runs
        // after the allocator's registration destructor in LIFO implementations.
        for _ in 0..32 {
            let x = vec![0x2au8; 32000];
            assert_eq!(x[31999], 0x2a);
            drop(x);
        }
    }
}

#[test]
fn allocations_during_late_tls_teardown() {
    for _ in 0..100 {
        std::thread::spawn(|| {
            LATE.with(|_| ());
            #[expect(
                clippy::large_stack_arrays,
                reason = "This regression intentionally exercises a large fixed-size Box after late TLS registration"
            )]
            let value = Box::new([0x11u8; 70000]);
            std::hint::black_box(value);
        })
        .join()
        .unwrap();
    }
}
