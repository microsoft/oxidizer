// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Independently owned, unobserved storage for snapshot collection.
//!
//! A source can retain allocations, return an allocated error, or unwind with an
//! owned panic payload. A stack-scoped arena cannot own those allocations.
//! System provides ordinary allocation lifetimes without a dedicated OS mapping
//! per temporary object. A separate System-backed address registry preserves that
//! ownership across scope exit, reallocation and transfer to another thread.
//! The payload is allocated with its exact layout, without a shared header whose
//! backing release could conflict with a protected client allocation.

use std::alloc::{GlobalAlloc, Layout};
use std::hash::{BuildHasherDefault, DefaultHasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{mem, ptr};

use allocator_api2_02::alloc::System;
#[cfg(not(test))]
use allocator_api2_02::alloc::System as MetadataAllocator;
use hashbrown::HashSet;
#[cfg(test)]
use tests::MetadataAllocator;

use super::SpinLock;

type Addresses = HashSet<usize, BuildHasherDefault<DefaultHasher>, MetadataAllocator>;

// std::collections allocates through GlobalAlloc and would recurse here.
// hashbrown's System allocator keeps metadata out of both payloads and telemetry.
// Sharding also avoids a single lock when captures or escaped frees overlap.
static ADDRESSES: [Shard; 32] = [const { Shard::new() }; 32];

// Retain small tables for repeated capture, but release large transient indexes.
const RETAINED_CAPACITY: usize = 256;

fn addresses(address: usize) -> &'static Shard {
    // Mix page and slot bits so over-aligned buffers do not all share a shard.
    &ADDRESSES[((address >> 4) ^ (address >> 16)) & (ADDRESSES.len() - 1)]
}

const fn empty_addresses() -> Addresses {
    Addresses::with_hasher_in(BuildHasherDefault::new(), MetadataAllocator)
}

fn reserve_addresses(capacity: usize) -> Option<Addresses> {
    let mut entries = empty_addresses();
    entries.try_reserve(capacity).ok()?;
    Some(entries)
}

struct Shard {
    lower: AtomicUsize,
    upper: AtomicUsize,
    // Hashbrown's reported capacity can decrease as removals leave tombstones.
    // Track the original reservation for retention decisions, under the lock.
    allocation_capacity: AtomicUsize,
    entries: SpinLock<Addresses>,
    #[cfg(test)]
    locks: AtomicUsize,
}

impl Shard {
    const fn new() -> Self {
        Self {
            lower: AtomicUsize::new(usize::MAX),
            upper: AtomicUsize::new(0),
            allocation_capacity: AtomicUsize::new(0),
            entries: SpinLock::new(empty_addresses()),
            #[cfg(test)]
            locks: AtomicUsize::new(0),
        }
    }

    fn with_entries<R>(&self, operation: impl FnOnce(&mut Addresses) -> R) -> R {
        #[cfg(test)]
        self.locks.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.entries.lock();
        #[cfg(test)]
        let _locked = tests::Locked::enter();
        operation(&mut entries)
    }

    fn publish_bounds(&self, lower: usize, upper: usize) {
        // Writers hold the shard lock, and each bound includes every live member.
        // Registration precedes pointer publication. A valid ownership handoff
        // therefore makes these stores (or later inclusive bounds) visible to
        // removal, even with relaxed loads. A coherent pair is not required.
        self.lower.store(lower, Ordering::Relaxed);
        self.upper.store(upper, Ordering::Relaxed);
    }

    fn insert(&self, address: usize, mut reserve: impl FnMut(usize) -> Option<Addresses>) -> bool {
        let mut candidate = empty_addresses();
        loop {
            let result = self.with_entries(|entries| {
                if entries.len() == entries.capacity() {
                    let needed = entries
                        .len()
                        .checked_add(1)
                        .expect("a valid address table cannot contain usize::MAX elements");
                    if candidate.capacity() < needed {
                        return Err(needed);
                    }
                    // The reservation fits all existing entries plus this one.
                    // Drain/swap cannot allocate; retired storage stays in the
                    // outer local until after this lock has been released.
                    candidate.extend(entries.drain());
                    mem::swap(entries, &mut candidate);
                    self.allocation_capacity.store(entries.capacity(), Ordering::Relaxed);
                }
                let inserted = entries.insert(address);
                if inserted {
                    self.publish_bounds(
                        self.lower.load(Ordering::Relaxed).min(address),
                        self.upper.load(Ordering::Relaxed).max(address),
                    );
                }
                Ok(inserted)
            });
            match result {
                Ok(inserted) => return inserted,
                Err(capacity) => {
                    drop(candidate);
                    let Some(replacement) = reserve(capacity) else {
                        return false;
                    };
                    assert!(replacement.is_empty(), "a fresh address-table reservation must be empty");
                    candidate = replacement;
                }
            }
        }
    }

    fn remove(&self, address: usize, mut reserve: impl FnMut(usize) -> Option<Addresses>) -> bool {
        if address < self.lower.load(Ordering::Relaxed) || address > self.upper.load(Ordering::Relaxed) {
            return false;
        }
        let mut retired = empty_addresses();
        let (removed, shrink) = self.with_entries(|entries| {
            if !entries.remove(&address) {
                return (false, None);
            }
            if entries.is_empty() {
                self.publish_bounds(usize::MAX, 0);
                if self.allocation_capacity.load(Ordering::Relaxed) > RETAINED_CAPACITY {
                    retired = mem::replace(entries, empty_addresses());
                    self.allocation_capacity.store(0, Ordering::Relaxed);
                }
                return (true, None);
            }
            (true, shrink_capacity(entries, self.allocation_capacity.load(Ordering::Relaxed)))
        });
        drop(retired);
        if let Some(capacity) = shrink {
            // Removal has already succeeded. Failed optional maintenance keeps
            // the surviving table intact; it must not misroute a System free.
            if let Some(mut candidate) = reserve(capacity) {
                assert!(candidate.is_empty(), "a fresh address-table reservation must be empty");
                self.with_entries(|entries| {
                    if shrink_capacity(entries, self.allocation_capacity.load(Ordering::Relaxed)) == Some(capacity)
                        && candidate.capacity() >= entries.len()
                        && candidate.capacity() < self.allocation_capacity.load(Ordering::Relaxed)
                    {
                        let mut lower = usize::MAX;
                        let mut upper = 0;
                        for address in entries.drain() {
                            lower = lower.min(address);
                            upper = upper.max(address);
                            candidate.insert(address);
                        }
                        mem::swap(entries, &mut candidate);
                        self.allocation_capacity.store(entries.capacity(), Ordering::Relaxed);
                        self.publish_bounds(lower, upper);
                    }
                });
            }
        }
        removed
    }
}

fn shrink_capacity(entries: &Addresses, allocation_capacity: usize) -> Option<usize> {
    // Hysteresis leaves spare capacity after shrinking. The minimum request
    // allows hashbrown's bucket rounding to stay below the retention limit.
    (allocation_capacity > RETAINED_CAPACITY && !entries.is_empty() && entries.len() <= allocation_capacity / 8)
        .then(|| (entries.len() * 2).max(RETAINED_CAPACITY / 2))
}

/// # Safety
///
/// `layout` must have nonzero size, as required by `GlobalAlloc::alloc`.
pub(super) unsafe fn allocate(layout: Layout) -> *mut u8 {
    // SAFETY: GlobalAlloc's caller supplies a nonzero, valid layout.
    let address = unsafe { System.alloc(layout) };
    if !address.is_null() && !addresses(address.addr()).insert(address.addr(), reserve_addresses) {
        // SAFETY: the allocation has not escaped and has its original layout.
        unsafe { System.dealloc(address, layout) };
        return ptr::null_mut();
    }
    address
}

/// # Safety
///
/// If `address` belongs to this registry, it is a live allocation with `layout`,
/// all payload accesses have ended, and it is released exactly once.
/// Unknown addresses are not dereferenced or released.
pub(super) unsafe fn try_deallocate(address: *mut u8, layout: Layout) -> bool {
    let removed = addresses(address.addr()).remove(address.addr(), reserve_addresses);
    if removed {
        // SAFETY: registration identifies exact-layout System storage. Use the
        // current caller's pointer, not a parent or separately saved payload pointer.
        unsafe { System.dealloc(address, layout) };
    }
    removed
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::ptr::NonNull;
    use std::sync::{Arc, mpsc};

    use allocator_api2_02::alloc::{AllocError, Allocator};

    use super::*;
    use crate::allocator::{GlobalRallocator, Standard};
    use crate::telemetry;

    thread_local! {
        static LOCK_DEPTH: Cell<usize> = const { Cell::new(0) };
        static MEMORY_WHILE_LOCKED: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) struct Locked;

    impl Locked {
        pub(super) fn enter() -> Self {
            LOCK_DEPTH.set(LOCK_DEPTH.get() + 1);
            Self
        }
    }

    impl Drop for Locked {
        fn drop(&mut self) {
            LOCK_DEPTH.set(LOCK_DEPTH.get() - 1);
        }
    }

    #[derive(Clone, Copy)]
    pub(super) struct MetadataAllocator;

    fn record_memory_operation() {
        MEMORY_WHILE_LOCKED.set(MEMORY_WHILE_LOCKED.get() + usize::from(LOCK_DEPTH.get() != 0));
    }

    // SAFETY: allocation and deallocation retain System's exact pointer/layout
    // contracts; the thread-local counters neither own nor access its storage.
    unsafe impl Allocator for MetadataAllocator {
        fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
            record_memory_operation();
            Allocator::allocate(&System, layout)
        }

        unsafe fn deallocate(&self, address: NonNull<u8>, layout: Layout) {
            record_memory_operation();
            // SAFETY: the caller supplies the original System storage and layout.
            unsafe { Allocator::deallocate(&System, address, layout) };
        }
    }

    #[test]
    fn bounds_reject_nonmembers_without_locking_and_reset_after_removal() {
        let shard = Shard::new();
        assert!(shard.insert(10, reserve_addresses));
        assert!(shard.insert(30, reserve_addresses));
        let before = shard.locks.load(Ordering::Relaxed);
        assert!(!shard.remove(9, reserve_addresses));
        assert!(!shard.remove(31, reserve_addresses));
        assert_eq!(shard.locks.load(Ordering::Relaxed), before);
        assert!(!shard.remove(20, reserve_addresses));
        assert_eq!(shard.locks.load(Ordering::Relaxed), before + 1);
        assert!(shard.remove(10, reserve_addresses));
        assert!(shard.remove(30, reserve_addresses));
        assert_eq!(
            (shard.lower.load(Ordering::Relaxed), shard.upper.load(Ordering::Relaxed)),
            (usize::MAX, 0)
        );
        assert!(shard.insert(100, reserve_addresses));
        assert!(shard.remove(100, reserve_addresses));
    }

    #[test]
    fn reservation_failure_preserves_existing_members() {
        let shard = Shard::new();
        assert!(!shard.insert(1, |_| None));
        assert!(shard.with_entries(|entries| entries.is_empty()));
        assert!(shard.insert(1, reserve_addresses));
        let capacity = shard.with_entries(|entries| entries.capacity());
        for address in 2..=capacity {
            assert!(shard.insert(address, reserve_addresses));
        }
        assert!(!shard.insert(capacity + 1, |_| None));
        for address in 1..=capacity {
            assert!(shard.remove(address, reserve_addresses));
        }
        assert!(!shard.remove(capacity + 1, reserve_addresses));
    }

    #[test]
    fn stale_reservations_are_rechecked_without_memory_operations_under_lock() {
        MEMORY_WHILE_LOCKED.set(0);
        let shard = Shard::new();
        let mut reservations = 0;
        assert!(shard.insert(1000, |capacity| {
            let candidate = reserve_addresses(capacity);
            reservations += 1;
            if reservations == 1 {
                for address in 1..=100 {
                    assert!(shard.insert(address, reserve_addresses));
                }
                let capacity = shard.with_entries(|entries| entries.capacity());
                for address in 101..=capacity {
                    assert!(shard.insert(address, reserve_addresses));
                }
            }
            candidate
        }));
        assert_eq!(reservations, 2);
        assert!(shard.remove(1000, reserve_addresses));
        drop(shard);
        assert_eq!(MEMORY_WHILE_LOCKED.get(), 0);
    }

    #[test]
    fn shrinking_is_best_effort_and_retains_small_warm_tables() {
        MEMORY_WHILE_LOCKED.set(0);
        let shard = Shard::new();
        for address in 1..=1024 {
            assert!(shard.insert(address, reserve_addresses));
        }
        let capacity = shard.allocation_capacity.load(Ordering::Relaxed);
        for address in 1..=1000 {
            assert!(shard.remove(address, |_| None));
        }
        assert_eq!(shard.allocation_capacity.load(Ordering::Relaxed), capacity);
        assert!(shard.remove(1001, reserve_addresses));
        assert!(shard.with_entries(|entries| entries.capacity()) < capacity);
        for address in 1002..=1024 {
            assert!(shard.remove(address, reserve_addresses));
        }
        let retained = shard.with_entries(|entries| entries.capacity());
        assert!(retained > 0 && retained <= RETAINED_CAPACITY);
        assert!(shard.insert(2000, |_| panic!("the small empty table must retain its reservation")));
        assert!(shard.remove(2000, reserve_addresses));
        assert_eq!(shard.with_entries(|entries| entries.capacity()), retained);
        drop(shard);
        assert_eq!(MEMORY_WHILE_LOCKED.get(), 0);
    }

    #[test]
    fn empty_large_tables_release_storage_even_when_shrinking_fails() {
        MEMORY_WHILE_LOCKED.set(0);
        let shard = Shard::new();
        for address in 1..=1024 {
            assert!(shard.insert(address, reserve_addresses));
        }
        for address in 1..=1024 {
            assert!(shard.remove(address, |_| None));
        }
        assert_eq!(shard.with_entries(|entries| entries.capacity()), 0);
        assert_eq!(MEMORY_WHILE_LOCKED.get(), 0);
    }

    #[test]
    fn pending_shrink_preserves_a_reset_and_repopulated_table() {
        MEMORY_WHILE_LOCKED.set(0);
        let shard = Shard::new();
        for address in 1..=1024 {
            assert!(shard.insert(address, reserve_addresses));
        }
        for address in 1..=800 {
            assert!(shard.remove(address, |_| None));
        }
        let mut reservations = 0;
        assert!(shard.remove(801, |capacity| {
            reservations += 1;
            let candidate = reserve_addresses(capacity);
            for address in 802..=1024 {
                assert!(shard.remove(address, |_| None));
            }
            assert!(shard.insert(10_000, reserve_addresses));
            candidate
        }));
        assert_eq!(reservations, 1);
        assert_eq!(
            shard.with_entries(|entries| entries.iter().copied().collect::<Vec<_>>()),
            vec![10_000]
        );
        assert!(shard.allocation_capacity.load(Ordering::Relaxed) <= RETAINED_CAPACITY);
        assert!(shard.remove(10_000, reserve_addresses));
        drop(shard);
        assert_eq!(MEMORY_WHILE_LOCKED.get(), 0);
    }

    #[test]
    fn ownership_handoff_publishes_bounds_before_cross_thread_removal() {
        let shard = Arc::new(Shard::new());
        let receiver_shard = Arc::clone(&shard);
        let (sender, receiver) = mpsc::channel::<Box<u64>>();
        let thread = std::thread::spawn(move || {
            for value in receiver {
                assert!(receiver_shard.remove(ptr::from_ref(value.as_ref()).addr(), reserve_addresses));
            }
        });
        for value in 0..128 {
            let value = Box::new(value);
            assert!(shard.insert(ptr::from_ref(value.as_ref()).addr(), reserve_addresses));
            sender.send(value).unwrap();
        }
        drop(sender);
        thread.join().unwrap();
        assert!(shard.with_entries(|entries| entries.is_empty()));
    }

    #[test]
    fn escaped_storage_preserves_alignment_contents_and_resize() {
        // SAFETY: this fixture uses the standard allocator personality throughout.
        let allocator = unsafe { GlobalRallocator::<Standard>::new() };
        for (size, alignment) in [(1, 1), (49, 16), (32768, 4096), (131_073, 65536)] {
            let layout = Layout::from_size_align(size, alignment).unwrap();
            let address = telemetry::with_snapshot_arena(|| {
                // SAFETY: layout is nonzero and the returned allocation is owned below.
                unsafe { allocator.alloc(layout) }
            });
            assert!(!address.is_null());
            assert_eq!(address.addr() % alignment, 0);
            // SAFETY: the allocation outlives the capture scope and covers the layout.
            unsafe {
                address.write_bytes(0x5a, size);
                let resized = allocator.realloc(address, layout, size + 1);
                assert!(!resized.is_null());
                assert_eq!(resized.addr() % alignment, 0);
                assert!(std::slice::from_raw_parts(resized, size).iter().all(|byte| *byte == 0x5a));
                allocator.dealloc(resized, Layout::from_size_align(size + 1, alignment).unwrap());
            }
        }
    }

    #[test]
    fn unknown_address_is_not_released() {
        let mut value = 42_u8;
        // SAFETY: this live stack address is not a registered System allocation.
        assert!(!unsafe { try_deallocate(ptr::from_mut(&mut value), Layout::new::<u8>()) });
        assert_eq!(value, 42);
    }
}
