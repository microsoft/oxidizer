// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Address-indexed sparse metadata. Entries double as out-of-line buddy nodes.
//! Unlike a map collection this needs no allocations, locks, or hashing for
//! lookups, and its nodes remain accessible while object memory is decommitted.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::classes::CHUNK_BITS;
use crate::hal;

const ADDRESS_BITS: usize = 48;
const MAP_BYTES: usize = (1usize << (ADDRESS_BITS - CHUNK_BITS)) * 16;
pub(crate) const RESERVED_BYTES: usize = MAP_BYTES;
pub(crate) const BACKEND: usize = 128;
const BOUNDARY: usize = 1;
const RED: usize = 256;

#[repr(C)]
struct Entry {
    meta: AtomicUsize,
    owner: AtomicUsize,
}

/// An immutable handle, not ownership of the mapped entries.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq, Eq))]
pub(crate) struct Map(usize);

impl Map {
    pub(crate) fn reserve() -> Option<Self> {
        let ptr = hal::reserve(MAP_BYTES, hal::RESERVE_MIN);
        (!ptr.is_null()).then_some(Self(ptr as usize))
    }

    fn entry(self, address: usize) -> *mut Entry {
        (self.0 + (address >> CHUNK_BITS) * 16) as *mut Entry
    }

    /// # Safety
    /// A new OS reservation; no previous entry in its range may be in use.
    pub(crate) unsafe fn register(self, address: usize, size: usize) -> bool {
        if address.checked_add(size).is_none_or(|end| end > 1usize << ADDRESS_BITS) {
            return false;
        }
        let first = self.entry(address) as usize & !(hal::PAGE - 1);
        let last = (self.entry(address + size - 1) as usize + 16 + hal::PAGE - 1) & !(hal::PAGE - 1);
        // SAFETY: All entries lie within this map's single reservation.
        if !unsafe { hal::commit(first as *mut u8, last - first) } {
            return false;
        }
        // SAFETY: This reservation's first entry is committed and exclusively
        // backend-owned. Neighbouring entries in the same page are untouched.
        unsafe { (*self.entry(address)).meta.store(BOUNDARY, Ordering::Relaxed) };
        // SAFETY: The same committed entry is still exclusively backend-owned.
        unsafe { (*self.entry(address)).owner.store(BACKEND, Ordering::Release) };
        true
    }

    /// # Safety
    /// The range has registered entries exclusively owned by the caller.
    pub(crate) unsafe fn assign(self, address: usize, size: usize, meta: usize, owner: usize) {
        for offset in (0..size).step_by(1 << CHUNK_BITS) {
            let entry = self.entry(address + offset);
            // SAFETY: Each registered entry belongs to the caller's range.
            let boundary = unsafe { (*entry).meta.load(Ordering::Relaxed) & BOUNDARY };
            // SAFETY: Assignment preserves this owned entry's reservation boundary.
            unsafe { (*entry).meta.store(meta | boundary, Ordering::Relaxed) };
            // SAFETY: The owner word belongs to the same exclusively owned entry.
            // Its release ordering is retained as local publication discipline;
            // cross-thread free-only handoff relies on the allocation-list
            // release fence and the pointer consumer's acquire fence.
            unsafe { (*entry).owner.store(owner, Ordering::Release) };
        }
    }

    /// # Safety
    /// Address belongs to a live frontend allocation. Its entry is immutable
    /// until all objects and remote messages from its slab have been returned.
    pub(crate) unsafe fn lookup(self, address: usize) -> (usize, usize) {
        let entry = self.entry(address);
        // SAFETY: Live frontend entries are committed and immutable. Pointer
        // consumers acquire ownership before lookup, while same-owner callers
        // are sequenced after publication.
        let owner = unsafe { (*entry).owner.load(Ordering::Relaxed) };
        // SAFETY: Ownership acquisition or same-owner sequencing makes the
        // immutable metadata publication visible.
        let meta = unsafe { (*entry).meta.load(Ordering::Relaxed) & !BOUNDARY };
        (meta, owner)
    }

    /// # Safety
    /// Both buddies exist in the caller's backend range.
    pub(crate) unsafe fn can_merge(self, address: usize, size: usize) -> bool {
        // SAFETY: The higher buddy's entry is registered, exclusively owned.
        unsafe { (*self.entry(address.max(address ^ size))).meta.load(Ordering::Relaxed) & BOUNDARY == 0 }
    }

    /// # Safety
    /// Address is a registered, backend-owned node, under its range's exclusion.
    pub(crate) unsafe fn children(self, address: usize) -> (usize, usize, bool) {
        let entry = self.entry(address);
        // SAFETY: The backend exclusively owns this registered node.
        let left = unsafe { (*entry).meta.load(Ordering::Relaxed) & !(RED | BOUNDARY) };
        // SAFETY: The owner word contains this node's right link.
        let right = unsafe { (*entry).owner.load(Ordering::Relaxed) & !BACKEND };
        // SAFETY: The same metadata word contains the node's color.
        let red = unsafe { (*entry).meta.load(Ordering::Relaxed) & RED != 0 };
        (left, right, red)
    }

    /// # Safety
    /// Address is a registered, exclusively backend-owned node. Child addresses
    /// must be chunk-aligned registered nodes belonging to this same range.
    pub(crate) unsafe fn set_children(self, address: usize, left: usize, right: usize, red: bool) {
        let entry = self.entry(address);
        // SAFETY: The entry words belong exclusively to the backend range.
        let boundary = unsafe { (*entry).meta.load(Ordering::Relaxed) & BOUNDARY };
        // SAFETY: The aligned left link leaves the boundary and color bits free.
        unsafe { (*entry).meta.store(left | boundary | if red { RED } else { 0 }, Ordering::Relaxed) };
        // SAFETY: The aligned right link leaves the backend tag bit free.
        unsafe { (*entry).owner.store(right | BACKEND, Ordering::Relaxed) };
    }
}

#[cfg(test)]
mod tests {
    use std::alloc::{GlobalAlloc, Layout};
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::AtomicPtr;

    use super::*;
    use crate::buddy::{Buddy, Nodes};

    #[test]
    fn registration_rejects_addresses_outside_the_map_without_touching_entries() {
        let map = Map::reserve().unwrap();
        for address in [usize::MAX, (1usize << ADDRESS_BITS) - hal::PAGE / 2] {
            // SAFETY: These rejected ranges never refer to entries or existing reservations.
            assert!(!unsafe { map.register(address, hal::PAGE) });
        }
    }

    #[test]
    fn boundary_bit_survives_frontend_and_tree_states() {
        let map = Map::reserve().unwrap();
        let size = 1 << 17;
        let base = hal::reserve(size, size) as usize;
        assert_ne!(base, 0);
        // SAFETY: Test exclusively owns and registers the complete reservation.
        assert!(unsafe { map.register(base, size) });
        // SAFETY: Registration committed this exclusively owned entry.
        assert_eq!(unsafe { (*map.entry(base)).meta.load(Ordering::Relaxed) & BOUNDARY }, BOUNDARY);
        // SAFETY: The test owns the registered range.
        unsafe { map.assign(base, size, 0x1000, 0x2040) };
        // SAFETY: The simulated frontend entry remains immutable.
        assert_eq!(unsafe { map.lookup(base) }, (0x1000, 0x2040));
        // SAFETY: No frontend reference remains.
        unsafe { map.assign(base, size, 0, BACKEND) };
        // SAFETY: These aligned children lie in the registered owned range.
        unsafe { map.set_children(base, base + (1 << 14), base + (1 << 15), true) };
        // SAFETY: The test exclusively owns the committed entry.
        assert_eq!(unsafe { (*map.entry(base)).meta.load(Ordering::Relaxed) & BOUNDARY }, BOUNDARY);
        // SAFETY: The entry is an initialized backend node.
        assert_eq!(unsafe { map.children(base) }, (base + (1 << 14), base + (1 << 15), true));
        // SAFETY: Both buddies are registered and owned.
        assert!(unsafe { map.can_merge(base, 1 << 16) });
        // Exercise boundaries independently of OS reservation placement.
        // SAFETY: The test exclusively owns this committed entry.
        unsafe { (*map.entry(base + (1 << 16))).meta.fetch_or(BOUNDARY, Ordering::Relaxed) };
        // SAFETY: Both simulated reservations remain registered and owned.
        assert!(!unsafe { map.can_merge(base, 1 << 16) });
        // SAFETY: No frontend references remain.
        unsafe { map.assign(base, size, 0, BACKEND) };
        let mut buddy = Buddy::<14, 18>::new();
        // SAFETY: Each aligned half is transferred exactly once.
        assert_eq!(unsafe { buddy.add(Nodes::Map(map), base, 16) }, 0);
        // SAFETY: The disjoint second half is still exclusively owned.
        assert_eq!(unsafe { buddy.add(Nodes::Map(map), base + (1 << 16), 16) }, 0);
        // SAFETY: All buddy nodes are registered and exclusively owned.
        assert_eq!(unsafe { buddy.take(Nodes::Map(map), 17) }, 0);
        // SAFETY: The cached halves remain exclusively owned.
        assert_eq!(unsafe { buddy.take(Nodes::Map(map), 16) }, base + (1 << 16));
        // SAFETY: The last cached half remains exclusively owned.
        assert_eq!(unsafe { buddy.take(Nodes::Map(map), 16) }, base);
        // SAFETY: The buddy is empty and no object reference survives.
        unsafe { hal::release(base as *mut u8, size) };
    }

    #[test]
    fn backend_tree_does_not_touch_uncommitted_objects() {
        let map = Map::reserve().unwrap();
        let base = hal::reserve(1 << 20, 1 << 20) as usize;
        assert_ne!(base, 0);
        // SAFETY: Only pagemap pages are committed. Any accidental in-object
        // buddy link access will fault, making this a strict regression test.
        assert!(unsafe { map.register(base, 1 << 20) });
        let mut buddy = Buddy::<14, 21>::new();
        for offset in (0..(1 << 20)).step_by(1 << 15) {
            // SAFETY: Each disjoint registered block is transferred once.
            assert_eq!(unsafe { buddy.add(Nodes::Map(map), base + offset, 14) }, 0);
        }
        for offset in (3..32).chain((0..3).rev()).map(|i| i << 15) {
            // SAFETY: The tree uses only committed map entries.
            assert_eq!(unsafe { buddy.take(Nodes::Map(map), 14) }, base + offset);
        }
        // SAFETY: The empty buddy remains exclusively owned.
        assert_eq!(unsafe { buddy.take(Nodes::Map(map), 14) }, 0);
        // SAFETY: No object or tree reference remains.
        unsafe { hal::release(base as *mut u8, 1 << 20) };
    }

    #[test]
    fn relaxed_free_only_handoff_keeps_frontend_metadata_race_free() {
        const ITERATIONS: usize = 10_000;
        let pointer = Arc::new(AtomicPtr::<u8>::new(ptr::null_mut()));
        std::thread::scope(|scope| {
            let producer_pointer = Arc::clone(&pointer);
            scope.spawn(move || {
                let layout = Layout::from_size_align(48, 16).unwrap();
                for _ in 0..ITERATIONS {
                    // SAFETY: The valid layout requests one fresh allocation.
                    let allocation = unsafe { crate::Rallocator.alloc(layout) };
                    assert!(!allocation.is_null());
                    while producer_pointer
                        .compare_exchange(ptr::null_mut(), allocation, Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                    {
                        std::hint::spin_loop();
                    }
                }
            });
            let consumer_pointer = Arc::clone(&pointer);
            scope.spawn(move || {
                let layout = Layout::from_size_align(48, 16).unwrap();
                for _ in 0..ITERATIONS {
                    let allocation = loop {
                        let allocation = consumer_pointer.swap(ptr::null_mut(), Ordering::Relaxed);
                        if !allocation.is_null() {
                            break allocation;
                        }
                        std::hint::spin_loop();
                    };
                    // SAFETY: The producer transfers this exact live allocation
                    // once. No payload byte is read or accessed concurrently.
                    unsafe { crate::Rallocator.dealloc(allocation, layout) };
                }
            });
        });
        assert!(pointer.load(Ordering::Relaxed).is_null());
    }
}
