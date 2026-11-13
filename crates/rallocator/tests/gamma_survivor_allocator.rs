// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Tests allocation alignment, ownership, zeroing, and resizing.

#![cfg(test)]

use std::alloc::{GlobalAlloc, Layout};

use rallocator::Rallocator;

struct GammaSurvivorAllocation {
    pointer: *mut u8,
    layout: Layout,
}

impl GammaSurvivorAllocation {
    fn new(size: usize, alignment: usize, zeroed: bool) -> Self {
        let layout = Layout::from_size_align(size, alignment).expect("test callers provide nonzero sizes and power-of-two alignments");
        let pointer = if zeroed {
            // SAFETY: The layout is valid and nonzero; this handle owns the resulting allocation.
            unsafe { Rallocator.alloc_zeroed(layout) }
        } else {
            // SAFETY: The layout is valid and nonzero; this handle owns the resulting allocation.
            unsafe { Rallocator.alloc(layout) }
        };
        assert!(!pointer.is_null());
        assert_eq!(pointer.addr() % alignment, 0);
        Self { pointer, layout }
    }

    fn fill(&mut self, byte: u8) {
        // SAFETY: The handle uniquely owns layout.size() writable bytes.
        unsafe { self.pointer.write_bytes(byte, self.layout.size()) };
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: All callers initialize the requested bytes before reading, and this handle keeps them live.
        unsafe { std::slice::from_raw_parts(self.pointer, self.layout.size()) }
    }

    fn resize(&mut self, size: usize) {
        let layout = Layout::from_size_align(size, self.layout.align()).expect("test resize sizes fit the handle's valid alignment");
        // SAFETY: This handle owns the live pointer for its exact old layout; the new layout is valid and nonzero.
        let pointer = unsafe { Rallocator.realloc(self.pointer, self.layout, size) };
        assert!(!pointer.is_null());
        self.pointer = pointer;
        self.layout = layout;
    }
}

impl Drop for GammaSurvivorAllocation {
    fn drop(&mut self) {
        // SAFETY: The handle returns its unique live allocation once with its current exact layout.
        unsafe { Rallocator.dealloc(self.pointer, self.layout) };
    }
}

#[test]
fn gamma_survivor_alignment_disjointness_and_live_data_across_classes() {
    let mut allocations = Vec::new();
    for alignment in [1, 8, 16, 32, 64, 256, 4096, 65_536] {
        for size in [1, 15, 16, 17, 47, 48, 49, 127, 255, 1023, 4095, 16_384, 65_536, 65_537, 131_073] {
            let mut allocation = GammaSurvivorAllocation::new(size, alignment, false);
            allocation.fill(71);
            for other in &allocations {
                let other: &GammaSurvivorAllocation = other;
                assert!(
                    allocation.pointer.addr() + size <= other.pointer.addr()
                        || other.pointer.addr() + other.layout.size() <= allocation.pointer.addr()
                );
            }
            allocations.push(allocation);
        }
    }
    for allocation in &allocations {
        assert!(allocation.bytes().iter().all(|&byte| byte == 71));
    }
}

#[test]
fn gamma_survivor_zeroed_reuse_and_reallocation_prefix() {
    for size in [16, 48, 256, 4096, 65_536, 65_537, 262_144] {
        let mut dirty = GammaSurvivorAllocation::new(size, 16, false);
        dirty.fill(255);
        drop(dirty);
        let zeroed = GammaSurvivorAllocation::new(size, 16, true);
        assert!(zeroed.bytes().iter().all(|&byte| byte == 0));
    }
    for (old, new) in [(33, 47), (48, 49), (65_536, 65_537), (131_073, 17), (131_073, 262_145)] {
        let mut allocation = GammaSurvivorAllocation::new(old, 16, false);
        allocation.fill(53);
        let old_pointer = allocation.pointer;
        allocation.resize(new);
        assert_eq!(allocation.pointer.addr() % 16, 0);
        // SAFETY: Reallocation preserves min(old, new) initialized bytes.
        let prefix = unsafe { std::slice::from_raw_parts(allocation.pointer, old.min(new)) };
        assert!(prefix.iter().all(|&byte| byte == 53));
        if (old, new) == (33, 47) {
            assert_eq!(allocation.pointer, old_pointer);
        }
    }
}

#[test]
fn gamma_survivor_cross_thread_ownership_returns_keep_live_peers_intact() {
    let mut allocations = Vec::new();
    for index in 0..1300 {
        let mut allocation = GammaSurvivorAllocation::new(48, 16, false);
        allocation.fill(u8::try_from(index % 251).unwrap());
        allocations.push((allocation.pointer.addr(), allocation.layout, u8::try_from(index % 251).unwrap()));
        std::mem::forget(allocation);
    }
    let peers: Vec<_> = (0..16).map(|_| GammaSurvivorAllocation::new(48, 16, true)).collect();
    std::thread::scope(|scope| {
        for part in allocations.chunks(325) {
            scope.spawn(move || {
                for &(address, layout, expected) in part {
                    let pointer = address as *mut u8;
                    // SAFETY: Each pointer is transferred exactly once to one worker and remains live until its deallocation.
                    let bytes = unsafe { std::slice::from_raw_parts(pointer, layout.size()) };
                    assert!(bytes.iter().all(|&byte| byte == expected));
                    // SAFETY: This worker owns this unique live allocation and its exact layout.
                    unsafe { Rallocator.dealloc(pointer, layout) };
                }
            });
        }
    });
    for peer in peers {
        assert!(peer.bytes().iter().all(|&byte| byte == 0));
    }
    let replacement = GammaSurvivorAllocation::new(48, 16, true);
    assert!(replacement.bytes().iter().all(|&byte| byte == 0));
}
