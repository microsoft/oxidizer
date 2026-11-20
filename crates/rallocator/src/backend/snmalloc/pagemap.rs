// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Address-indexed sparse metadata. Entries double as out-of-line buddy nodes.
//! Unlike a map collection this needs no allocations, locks, or hashing for
//! lookups, and its nodes remain accessible while object memory is decommitted.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::backend::snmalloc::classes::CHUNK_BITS;
use crate::backend::snmalloc::hal;

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

    fn commit_range(self, address: usize, size: usize) -> (usize, usize) {
        let first = self.entry(
            // #[gamma::skip(expr.increment, reason = "The registered range begins at a chunk boundary, so address plus one indexes the same entry")]
            address,
        ) as usize
            & !(hal::PAGE -
                // #[gamma::skip(literal.int_increment, reason = "Entry addresses are sixteen-byte aligned, so keeping bit zero in the page mask cannot change the result")]
                1);
        let last = (self.entry(
            // #[gamma::skip(expr.decrement, literal.int_increment, reason = "A registered range ends at a chunk boundary, so its final byte and the preceding byte index the identical last entry")]
            address + size - 1,
        ) as usize
            +
            // #[gamma::skip(literal.int_decrement, literal.int_to_one, reason = "Entries are sixteen-byte aligned; any positive intra-entry extent through sixteen rounds up to the identical committed page end")]
            16
            + hal::PAGE -
            // #[gamma::skip(literal.int_increment, reason = "The exclusive entry end is sixteen-byte aligned, so reducing the page-rounding bias from PAGE minus one to PAGE minus two cannot cross a rounding boundary")]
            1)
            & !(hal::PAGE - 1);
        (first, last - first)
    }

    fn invalid_range(address: usize, size: usize) -> bool {
        address.checked_add(size).is_none_or(|end| end > 1usize << ADDRESS_BITS)
    }

    /// # Safety
    /// A new OS reservation; no previous entry in its range may be in use.
    pub(crate) unsafe fn register(self, address: usize, size: usize) -> bool {
        self.register_with(
            address,
            // #[gamma::skip(expr.decrement, reason = "The sole caller registers nonzero whole-chunk reservations; subtracting one preserves aligned address-limit admission, both metadata page endpoints, and initialization of the same first entry")]
            size,
            |first, length| {
                // SAFETY: The caller owns the map reservation and the computed pages lie within it.
                unsafe { hal::commit(first as *mut u8, length) }
            },
            |address| {
                // SAFETY: Committing succeeded before this exclusively owned entry is initialized.
                unsafe {
                    (*self.entry(address)).meta.store(BOUNDARY, Ordering::Relaxed);
                };
                // SAFETY: Publication follows metadata initialization in the same committed entry.
                unsafe {
                    (*self.entry(address)).owner.store(BACKEND, Ordering::Release);
                };
            },
        )
    }

    fn register_with(self, address: usize, size: usize, commit: impl FnOnce(usize, usize) -> bool, initialize: impl FnOnce(usize)) -> bool {
        if Self::invalid_range(
            // #[gamma::skip(expr.decrement, reason = "The sole production caller registers nonzero whole-chunk OS reservations; their ends and the map limit are chunk-aligned, so subtracting one cannot cross the strict greater-than rejection boundary")]
            address,
            // #[gamma::skip(expr.decrement, reason = "The sole production caller registers nonzero whole-chunk OS reservations; their ends and the map limit are chunk-aligned, so subtracting one cannot cross the strict greater-than rejection boundary")]
            size,
        ) {
            return false;
        }
        let (first, length) = self.commit_range(
            address,
            // #[gamma::skip(expr.decrement, reason = "A registered whole-chunk reservation contains at least one full chunk; reducing its size by one makes commit_range inspect the preceding final byte in the same last chunk, preserving both committed metadata-page endpoints")]
            size,
        );
        if !commit(first, length) {
            return false;
        }
        initialize(address);
        true
    }

    /// # Safety
    /// The range has registered entries exclusively owned by the caller.
    pub(crate) unsafe fn assign(self, address: usize, size: usize, meta: usize, owner: usize) {
        for offset in (
            // #[gamma::skip(literal.int_increment, reason = "Starting at byte one visits the same entries because the range starts at a chunk boundary and contains whole chunks")]
            0..
                // #[gamma::skip(expr.decrement, reason = "Reducing a whole-chunk exclusive byte end by one retains the identical final chunk start")]
                size
        )
            .step_by(1 << CHUNK_BITS)
        {
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
        let entry = self.entry(
            // #[gamma::skip(expr.increment, reason = "A live allocation start is at least sixteen-byte aligned and adding one remains in its pagemap chunk")]
            address,
        );
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
        unsafe {
            (*self.entry(
                // #[gamma::skip(expr.increment, reason = "The higher buddy starts on a chunk boundary, so adding one selects its identical boundary marker")]
                address.max(address ^ size),
            ))
            .meta
            .load(Ordering::Relaxed)
                & BOUNDARY
                == 0
        }
    }

    /// # Safety
    /// Address is a registered, backend-owned node, under its range's exclusion.
    pub(crate) unsafe fn children(self, address: usize) -> (usize, usize, bool) {
        let entry = self.entry(
            // #[gamma::skip(expr.increment, reason = "Backend tree node addresses are chunk-aligned, so adding one selects the same metadata entry")]
            address,
        );
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
        let entry = self.entry(
            // #[gamma::skip(expr.increment, reason = "Backend tree node addresses are chunk-aligned, so adding one selects the same metadata entry")]
            address,
        );
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
    #[test]
    fn nov6_register_commits_only_the_required_metadata_page_at_an_exact_page_boundary() {
        thread_local! {
            static EXPECTED: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
            static CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }
        let entries: Vec<_> = (0..hal::PAGE * 3 / size_of::<Entry>())
            .map(|_| Entry {
                meta: AtomicUsize::new(0),
                owner: AtomicUsize::new(0),
            })
            .collect();
        let base = (entries.as_ptr().addr() + hal::PAGE - 1) & !(hal::PAGE - 1);
        let map = Map(base);
        let address = (hal::PAGE / size_of::<Entry>() - 1) << CHUNK_BITS;
        EXPECTED.set((base, hal::PAGE));
        CALLS.set(0);
        hal::with_commit_probe(
            |address, length| {
                assert_eq!((address, length), EXPECTED.get());
                CALLS.set(CALLS.get() + 1);
                true
            },
            || {
                // SAFETY: The modeled map owns initialized aligned metadata; the scoped commit callback never reaches the OS.
                assert!(unsafe { map.register(address, 1 << CHUNK_BITS) });
            },
        );
        assert_eq!(CALLS.get(), 1);
        let index = (base - entries.as_ptr().addr()) / size_of::<Entry>() + hal::PAGE / size_of::<Entry>() - 1;
        assert_eq!(entries[index].meta.load(Ordering::Relaxed), BOUNDARY);
        assert_eq!(entries[index].owner.load(Ordering::Relaxed), BACKEND);
    }

    use std::alloc::{GlobalAlloc, Layout};
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicPtr};

    use super::*;
    use crate::backend::snmalloc::buddy::{Buddy, Nodes};
    use crate::backend::snmalloc::classes::CHUNK;

    #[test]
    fn gamma_nov4_registration_dispatches_exact_pages_before_initializing_and_short_circuits_failures() {
        let map = Map(0x0010_0000);
        for address in [255 * CHUNK, 256 * CHUNK, (1usize << ADDRESS_BITS) - CHUNK] {
            for committed in [false, true] {
                let events = std::cell::RefCell::new(Vec::new());
                let first = (0x0010_0000 + (address / CHUNK) * 16) / hal::PAGE * hal::PAGE;
                let end = (0x0010_0000 + (address.div_ceil(CHUNK) + 1) * 16).div_ceil(hal::PAGE) * hal::PAGE;
                let result = map.register_with(
                    address,
                    CHUNK,
                    |start, size| {
                        events.borrow_mut().push(("commit", start, size));
                        committed
                    },
                    |base| events.borrow_mut().push(("initialize", base, 0)),
                );
                assert_eq!(result, committed);
                let mut expected = vec![("commit", first, end - first)];
                if committed {
                    expected.push(("initialize", address, 0));
                }
                assert_eq!(*events.borrow(), expected);
            }
        }
        for address in [1usize << ADDRESS_BITS, usize::MAX - CHUNK + 1] {
            assert!(!map.register_with(
                address,
                CHUNK,
                |_, _| panic!("invalid ranges cannot commit"),
                |_| { panic!("invalid ranges cannot initialize") }
            ));
        }
    }

    #[test]
    fn gamma_round2_registration_limits_check_the_exclusive_end_without_native_calls() {
        let limit = 1usize << ADDRESS_BITS;
        for (address, size, expected) in [
            (limit - CHUNK, CHUNK, true),
            (limit - CHUNK - 1, CHUNK, true),
            (limit - CHUNK + 1, CHUNK, false),
            (limit - 1, 1, true),
            (limit, 1, false),
            (usize::MAX - 10, 16, false),
        ] {
            assert_eq!(!Map::invalid_range(address, size), expected, "{address:#x}, {size:#x}");
        }
    }

    #[test]
    fn gamma_round2_commit_range_covers_exact_metadata_pages_without_native_calls() {
        let base = 1usize << 32;
        let map = Map(base);
        let page_span = (hal::PAGE / size_of::<Entry>()) * CHUNK;
        let cases = [
            (page_span, page_span, base + hal::PAGE, hal::PAGE),
            (page_span + CHUNK, page_span / 2, base + hal::PAGE, hal::PAGE),
            (page_span - CHUNK, 2 * CHUNK, base, 2 * hal::PAGE),
            (page_span * 2, page_span, base + 2 * hal::PAGE, hal::PAGE),
            (CHUNK, CHUNK, base, hal::PAGE),
            (page_span, CHUNK, base + hal::PAGE, hal::PAGE),
            (
                (1usize << ADDRESS_BITS) - page_span,
                page_span,
                base + MAP_BYTES - hal::PAGE,
                hal::PAGE,
            ),
        ];
        for (address, size, first, length) in cases {
            assert_eq!(map.commit_range(address, size), (first, length), "{address:#x}, {size:#x}");
        }
    }

    #[test]
    fn gamma_survivor_registered_range_preserves_boundary_and_routing_words() {
        let map = Map::reserve().unwrap();
        let size = 1 << 22;
        let base = hal::reserve(size, size).addr();
        assert_ne!(base, 0);
        // SAFETY: The test exclusively owns this fresh complete reservation.
        assert!(unsafe { map.register(base, size) });
        let entry = map.entry(base);
        // SAFETY: Registration commits and initializes the reservation's first map entry.
        assert_eq!(unsafe { (*entry).meta.load(Ordering::Relaxed) }, BOUNDARY);
        // SAFETY: The same initialized backend entry is exclusively test-owned.
        assert_eq!(unsafe { (*entry).owner.load(Ordering::Relaxed) }, BACKEND);
        // SAFETY: The test owns every registered entry; assign publishes simulated immutable frontend routing.
        unsafe { map.assign(base, size, 0x2000, 0x4041) };
        for offset in (0..size).step_by(1 << CHUNK_BITS) {
            // SAFETY: The simulated frontend range is live and its published entries are immutable.
            assert_eq!(unsafe { map.lookup(base + offset) }, (0x2000, 0x4041));
            // SAFETY: The last byte of each frontend chunk has that same live immutable entry.
            assert_eq!(unsafe { map.lookup(base + offset + (1 << CHUNK_BITS) - 1) }, (0x2000, 0x4041));
        }
        // SAFETY: No frontend access remains; all entries transfer back to exclusive backend ownership.
        unsafe { map.assign(base, size, 0, BACKEND) };
        let sibling = base + size / 2;
        // SAFETY: Both naturally aligned buddies belong to this same registered backend reservation.
        assert!(unsafe { map.can_merge(base, size / 2) });
        // SAFETY: These disjoint registered nodes are exclusively backend-owned.
        unsafe { map.set_children(base, sibling, base + (1 << CHUNK_BITS), true) };
        // SAFETY: The initialized node and its child links remain exclusively owned.
        assert_eq!(unsafe { map.children(base) }, (sibling, base + (1 << CHUNK_BITS), true));
        // SAFETY: Rewriting the owned node changes only links and color.
        unsafe { map.set_children(base, 0, 0, false) };
        // SAFETY: The same backend node remains initialized.
        assert_eq!(unsafe { map.children(base) }, (0, 0, false));
        // SAFETY: Node rewrites must preserve the original reservation boundary.
        assert_eq!(unsafe { (*entry).meta.load(Ordering::Relaxed) }, BOUNDARY);
        let mut buddy = Buddy::<14, 23>::new();
        // SAFETY: Each disjoint registered backend half is transferred exactly once.
        assert_eq!(unsafe { buddy.add(Nodes::Map(map), sibling, 21) }, 0);
        // SAFETY: The lower half is still exclusively owned and its higher sibling is in this buddy.
        assert_eq!(unsafe { buddy.add(Nodes::Map(map), base, 21) }, 0);
        // SAFETY: Both halves must coalesce inside the one live reservation.
        assert_eq!(unsafe { buddy.take(Nodes::Map(map), 22) }, base);
        // SAFETY: No object or node access survives releasing the complete owned reservation.
        unsafe { hal::release(base as *mut u8, size) };
    }

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
        struct CancelOnPanic<'a>(&'a AtomicBool);

        impl Drop for CancelOnPanic<'_> {
            fn drop(&mut self) {
                self.0.fetch_or(std::thread::panicking(), Ordering::Relaxed);
            }
        }

        const ITERATIONS: usize = 10_000;
        // SAFETY: This test allocates and frees only through this linked backend.
        const ALLOCATOR: crate::Rallocator = unsafe { crate::Rallocator::new(crate::backend::Snmalloc) };
        let pointer = Arc::new(AtomicPtr::<u8>::new(ptr::null_mut()));
        // A failed worker must not leave its peer spinning while scope joins.
        // Relaxed cancellation does not synchronize the allocation handoff.
        let cancelled = AtomicBool::new(false);
        let cancelled = &cancelled;
        std::thread::scope(|scope| {
            let producer_pointer = Arc::clone(&pointer);
            scope.spawn(move || {
                let _cancel_on_panic = CancelOnPanic(cancelled);
                let layout = Layout::from_size_align(48, 16).unwrap();
                for _ in 0..ITERATIONS {
                    // SAFETY: The valid layout requests one fresh allocation.
                    let allocation = unsafe { ALLOCATOR.alloc(layout) };
                    assert!(!allocation.is_null());
                    while producer_pointer
                        .compare_exchange(ptr::null_mut(), allocation, Ordering::Relaxed, Ordering::Relaxed)
                        .is_err()
                    {
                        assert!(!cancelled.load(Ordering::Relaxed), "handoff consumer panicked");
                        std::hint::spin_loop();
                    }
                }
            });
            let consumer_pointer = Arc::clone(&pointer);
            scope.spawn(move || {
                let _cancel_on_panic = CancelOnPanic(cancelled);
                let layout = Layout::from_size_align(48, 16).unwrap();
                for _ in 0..ITERATIONS {
                    let allocation = loop {
                        let allocation = consumer_pointer.swap(ptr::null_mut(), Ordering::Relaxed);
                        if !allocation.is_null() {
                            break allocation;
                        }
                        assert!(!cancelled.load(Ordering::Relaxed), "handoff producer panicked");
                        std::hint::spin_loop();
                    };
                    // SAFETY: The producer transfers this exact live allocation
                    // once. No payload byte is read or accessed concurrently.
                    unsafe { ALLOCATOR.dealloc(allocation, layout) };
                }
            });
        });
        assert!(pointer.load(Ordering::Relaxed).is_null());
    }
}
