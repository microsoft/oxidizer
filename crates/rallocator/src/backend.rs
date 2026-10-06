// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `StandardLocalState` range composition, without the optional `StatsRange` or
//! logging adapter. The combining lock is a slow-path `GlobalRange` boundary;
//! owner-local range and metadata buddies have no locks or atomics.

use crate::buddy::{Buddy, Nodes};
use crate::classes::{CHUNK, CHUNK_BITS};
use crate::combining::Combining;
use crate::hal;
use crate::pagemap::Map;

const LOCAL_BITS: usize = 21;
const GLOBAL_REFILL: usize = 1 << 24;

struct Global {
    map: Option<Map>,
    ranges: Buddy<CHUNK_BITS, 63>,
    requested: usize,
}

static GLOBAL: Combining<Global> = Combining::new(Global {
    map: None,
    ranges: Buddy::new(),
    requested: 0,
});

pub(crate) fn map() -> Option<Map> {
    GLOBAL.with(|global| {
        if global.map.is_none() {
            global.map = Some(Map::reserve()?);
        }
        global.map
    })
}

pub(crate) fn initialized_map(map: Option<Map>) -> Map {
    map.unwrap_or_else(|| crate::abort::abort())
}

pub(crate) fn observe() -> seismograph_rallocator::native::GlobalState {
    GLOBAL.with(|global| observe_global(global))
}

fn observe_global(global: &Global) -> seismograph_rallocator::native::GlobalState {
    let mut budget = crate::observation::WALK_BUDGET;
    let ranges = if let Some(map) = global.map {
        // SAFETY: The combining lock or exclusive test ownership protects the buddy and its nodes.
        unsafe { global.ranges.observe(Nodes::Map(map), &mut budget) }
    } else {
        seismograph_rallocator::native::Ranges::EMPTY
    };
    seismograph_rallocator::native::GlobalState {
        reserved_bytes: global.requested as u64,
        ranges,
        pagemap_reserved_bytes: if global.map.is_some() {
            crate::pagemap::RESERVED_BYTES as u64
        } else {
            0
        },
        local_limit_bytes: 1u64 << LOCAL_BITS,
        global_refill_bytes: GLOBAL_REFILL as u64,
    }
}

fn global_alloc_reserved(global: &mut Global, size: usize) -> usize {
    let Some(map) = global.map else {
        return 0;
    };
    let bits = size.trailing_zeros() as usize;
    // SAFETY: The lock uniquely owns every global buddy block; map is initialized.
    let mut address = unsafe { global.ranges.take(Nodes::Map(map), bits) };
    if address == 0 {
        let refill = global
            .requested
            .clamp(hal::RESERVE_MIN, GLOBAL_REFILL)
            .max(size)
            .next_power_of_two();
        address = hal::reserve(refill, refill) as usize;
        if address == 0 {
            return 0;
        }
        // SAFETY: This is a fresh, nonoverlapping OS reservation.
        if !unsafe { map.register(address, refill) } {
            // SAFETY: No allocation or metadata entry was published.
            unsafe { hal::release(address as *mut u8, refill) };
            return 0;
        }
        global.requested = global.requested.saturating_add(refill);
        // Split the unused, uncommitted suffix into naturally aligned powers.
        let mut cursor = address + size;
        let end = address + refill;
        while cursor < end {
            let bits = (cursor.trailing_zeros() as usize).min((usize::BITS - 1 - (end - cursor).leading_zeros()) as usize);
            // SAFETY: Suffix blocks are registered, aligned and exclusively owned.
            let overflow = unsafe { global.ranges.add(Nodes::Map(map), cursor, bits) };
            debug_assert_eq!(overflow, 0);
            cursor += 1 << bits;
        }
    }
    address
}

fn global_alloc(size: usize) -> usize {
    let address = GLOBAL.with(|global| global_alloc_reserved(global, size));
    if address == 0 {
        return 0;
    }
    // CommitRange is OUTSIDE GlobalRange, exactly as in StandardLocalState:
    // cached local blocks stay committed; global cached blocks stay decommitted.
    // SAFETY: The removed block is exclusively owned and within one reservation.
    if !unsafe { hal::commit(address as *mut u8, size) } {
        // A failed MEM_COMMIT does not publish any objects.
        global_free(address, size);
        return 0;
    }
    address
}

fn global_free(address: usize, size: usize) {
    // SAFETY: Caller has retired all allocations and acquired exclusive ownership.
    // A failed decommit leaves the block committed, which is still valid cache
    // storage; the next commit is idempotent and retries normal OS treatment.
    let _decommitted = unsafe { hal::decommit(address as *mut u8, size) };
    GLOBAL.with(|global| {
        let map = initialized_map(global.map);
        // SAFETY: The global lock transfers this registered, aligned block to its buddy.
        let overflow = unsafe { global.ranges.add(Nodes::Map(map), address, size.trailing_zeros() as usize) };
        debug_assert_eq!(overflow, 0);
    });
}

pub(crate) struct Local {
    map: Map,
    ranges: Buddy<CHUNK_BITS, LOCAL_BITS>,
    metadata: Buddy<4, CHUNK_BITS>,
    requested: usize,
}

impl Local {
    pub(crate) fn observe(&self, budget: &mut usize) -> seismograph_rallocator::native::LocalState {
        // SAFETY: The local backend and both buddies remain immutable under the current owner lease.
        let ranges = unsafe { self.ranges.observe(Nodes::Map(self.map), budget) };
        // SAFETY: Metadata buddy nodes are committed and protected by that same owner lease.
        let metadata = unsafe { self.metadata.observe(Nodes::Inline, budget) };
        seismograph_rallocator::native::LocalState {
            ranges,
            metadata,
            requested_bytes: self.requested as u64,
        }
    }

    /// # Safety
    /// map must be the handle returned by this module's `map()`, not an
    /// independently reserved map: global refills register in that exact map.
    pub(crate) const unsafe fn new(map: Map) -> Self {
        Self {
            map,
            ranges: Buddy::new(),
            metadata: Buddy::new(),
            requested: 0,
        }
    }

    /// Transfers a committed power-of-two object range to the caller.
    pub(crate) fn alloc(&mut self, size: usize) -> usize {
        if !size.is_power_of_two() || !(CHUNK..=(1usize << 46)).contains(&size) {
            return 0;
        }
        if size >= 1 << LOCAL_BITS {
            return global_alloc(size);
        }
        let bits = size.trailing_zeros() as usize;
        // SAFETY: The local range is exclusively owned by this owner lease.
        let cached = unsafe { self.ranges.take(Nodes::Map(self.map), bits) };
        if cached != 0 {
            return cached;
        }
        let refill = self.requested.min(1 << LOCAL_BITS).max(size).next_power_of_two();
        let address = global_alloc(refill);
        if address != 0 {
            self.requested = self.requested.saturating_add(refill);
            let mut cursor = address + size;
            let end = address + refill;
            while cursor < end {
                let bits = (cursor.trailing_zeros() as usize).min((usize::BITS - 1 - (end - cursor).leading_zeros()) as usize);
                // SAFETY: Unused suffix is committed, registered and exclusively owned.
                unsafe { self.free(cursor, 1 << bits) };
                cursor += 1 << bits;
            }
        }
        address
    }

    /// # Safety
    /// The complete range came from `alloc()`, is no longer in use, and its
    /// frontend pagemap entries have been changed back to backend-owned.
    pub(crate) unsafe fn free(&mut self, address: usize, size: usize) {
        if size >= 1 << LOCAL_BITS {
            global_free(address, size);
            return;
        }
        // SAFETY: Caller transfers exclusive ownership of this registered block.
        let overflow = unsafe { self.ranges.add(Nodes::Map(self.map), address, size.trailing_zeros() as usize) };
        if overflow != 0 {
            global_free(overflow, 1 << LOCAL_BITS);
        }
    }

    /// # Safety
    /// The 16-byte-aligned range is committed backend-owned metadata registered
    /// in this local state's map, with size a multiple of 16. It contains no
    /// live objects or block already present in a buddy. Ownership is transferred
    /// permanently: a donor must not subsequently return its containing backing.
    pub(crate) unsafe fn seed_metadata(&mut self, mut address: usize, mut size: usize) {
        while size != 0 {
            let bits = (address.trailing_zeros() as usize).min((usize::BITS - 1 - size.leading_zeros()) as usize);
            let block = 1usize << bits;
            // SAFETY: Each aligned power-of-two block is a disjoint subrange
            // of the exclusively owned spare metadata supplied by the caller.
            unsafe { self.free_meta(address, block) };
            address += block;
            size -= block;
        }
    }

    pub(crate) fn alloc_meta(&mut self, size: usize) -> usize {
        if !size.is_power_of_two() || size < 16 {
            return 0;
        }
        if size >= CHUNK {
            return self.alloc(size);
        }
        // SAFETY: Metadata buddy blocks remain committed and exclusively owned.
        let address = unsafe { self.metadata.take(Nodes::Inline, size.trailing_zeros() as usize) };
        if address != 0 {
            return address;
        }
        let address = self.alloc(CHUNK);
        if address == 0 {
            return 0;
        }
        let mut offset = size;
        while offset < CHUNK {
            // SAFETY: Each suffix is a separate aligned, exclusively owned block.
            unsafe { self.free_meta(address + offset, offset) };
            offset *= 2;
        }
        address
    }

    /// # Safety
    /// The aligned power-of-two range was allocated with `alloc_meta`, or is a
    /// disjoint subdivision of registered backend-owned metadata permanently
    /// transferred to this local state. It contains no live metadata.
    pub(crate) unsafe fn free_meta(&mut self, address: usize, size: usize) {
        if size >= CHUNK {
            // SAFETY: The caller transfers the complete retired metadata block.
            unsafe { self.free(address, size) };
        } else {
            // SAFETY: Inline tree nodes occupy retired, committed metadata.
            let overflow = unsafe { self.metadata.add(Nodes::Inline, address, size.trailing_zeros() as usize) };
            if overflow != 0 {
                // SAFETY: The entire metadata chunk has coalesced; no live entries.
                unsafe { self.free(overflow, CHUNK) };
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn validate(&self) {
        // SAFETY: A Core test holds the unique Local lease, so both intrusive
        // buddies and every node they own are stable during validation.
        let ranges = unsafe { self.ranges.validate(Nodes::Map(self.map)) };
        // SAFETY: Inline metadata nodes remain committed while cached.
        let metadata = unsafe { self.metadata.validate(Nodes::Inline) };
        for &(meta, meta_bits) in &metadata {
            let meta_end = meta + (1usize << meta_bits);
            assert!(
                ranges
                    .iter()
                    .all(|&(range, range_bits)| meta_end <= range || range + (1usize << range_bits) <= meta),
                "metadata and object-range buddies overlap at {meta:#x}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_initialization_aborts_before_reading_range_metadata() {
        crate::abort::assert_aborts(
            "backend::tests::missing_initialization_aborts_before_reading_range_metadata",
            || {
                initialized_map(None);
            },
        );
        let map = Map::reserve().unwrap();
        initialized_map(Some(map));
    }

    #[test]
    fn reserved_refill_failures_do_not_publish_ranges_or_accounting() {
        let mut global = Global {
            map: None,
            ranges: Buddy::new(),
            requested: 0,
        };
        let empty = observe_global(&global);
        assert_eq!(empty.reserved_bytes, 0);
        assert_eq!(empty.pagemap_reserved_bytes, 0);
        assert_eq!(empty.ranges, seismograph_rallocator::native::Ranges::EMPTY);
        assert_eq!(empty.local_limit_bytes, 1u64 << LOCAL_BITS);
        assert_eq!(empty.global_refill_bytes, GLOBAL_REFILL as u64);
        assert_eq!(global_alloc_reserved(&mut global, CHUNK), 0);
        global.map = Some(Map::reserve().unwrap());
        hal::fail_next(hal::Failure::Reserve);
        assert_eq!(global_alloc_reserved(&mut global, CHUNK), 0);
        assert_eq!(global.requested, 0);
        hal::fail_next(hal::Failure::Commit);
        assert_eq!(global_alloc_reserved(&mut global, CHUNK), 0);
        assert_eq!(global.requested, 0);
        let mut budget = usize::MAX;
        // SAFETY: This isolated global and all its map nodes are exclusively test-owned.
        let ranges = unsafe { global.ranges.observe(Nodes::Map(global.map.unwrap()), &mut budget) };
        assert_eq!(ranges.observed_bytes(), 0);
    }

    #[test]
    fn local_rejects_invalid_sizes_and_round_trips_chunk_sized_metadata() {
        let map = map().unwrap();
        // SAFETY: This handle comes from the backend's unique global map.
        let mut local = unsafe { Local::new(map) };
        for size in [0, 1, CHUNK - 1, CHUNK + 1, 1usize << 47] {
            assert_eq!(local.alloc(size), 0);
        }
        for size in [0, 1, 15, 17] {
            assert_eq!(local.alloc_meta(size), 0);
        }
        let address = local.alloc_meta(CHUNK);
        assert_ne!(address, 0);
        // SAFETY: The test returns the entire live, unpublished metadata allocation.
        unsafe { local.free_meta(address, CHUNK) };
        local.validate();
        assert_eq!(local.alloc_meta(CHUNK), address);
        // SAFETY: The reused chunk was not subdivided or published.
        unsafe { local.free_meta(address, CHUNK) };
        let mut budget = usize::MAX;
        let observed = local.observe(&mut budget);
        assert!(observed.ranges.complete);
        assert!(observed.metadata.complete);
        assert_eq!(observed.requested_bytes, CHUNK as u64);
    }

    #[test]
    fn metadata_refill_commit_failure_can_be_retried() {
        let map = map().unwrap();
        // SAFETY: This handle comes from the backend's unique global map.
        let mut local = unsafe { Local::new(map) };
        hal::fail_next(hal::Failure::Commit);
        assert_eq!(local.alloc_meta(16), 0);
        assert_eq!(local.requested, 0);
        let address = local.alloc_meta(16);
        assert_ne!(address, 0);
        // SAFETY: The unpublished metadata allocation is returned once.
        unsafe { local.free_meta(address, 16) };
        local.validate();
    }

    #[test]
    fn local_geometric_refill_and_committed_reuse() {
        let map = map().unwrap();
        // SAFETY: The handle comes from this backend's unique global map.
        let mut local = unsafe { Local::new(map) };
        let mut addresses = Vec::new();
        for expected in [CHUNK, CHUNK * 2, CHUNK * 4, CHUNK * 4, CHUNK * 8] {
            let address = local.alloc(CHUNK);
            assert_ne!(address, 0);
            assert_eq!(local.requested, expected);
            addresses.push(address);
        }
        let a = addresses.pop().unwrap();
        // SAFETY: Raw backend blocks were never published as frontend objects;
        // they remain registered and exclusively belong to this local range.
        unsafe { (a as *mut usize).write(0x1234_5678) };
        // SAFETY: This complete unpublished chunk is exclusively owned.
        unsafe { local.free(a, CHUNK) };
        // Metadata nodes are in the pagemap, not object bytes; a retained
        // local block is still committed and has not been zeroed/decommitted.
        // SAFETY: The isolated local cache retains this initialized word.
        assert_eq!(unsafe { (a as *const usize).read() }, 0x1234_5678);
        for address in addresses {
            // SAFETY: Each remaining unpublished chunk is retired exactly once.
            unsafe { local.free(address, CHUNK) };
        }
    }

    #[test]
    fn metadata_buddy_is_independent_and_recombines() {
        let map = map().unwrap();
        // SAFETY: The handle comes from this backend's unique global map.
        let mut local = unsafe { Local::new(map) };
        let addresses = (0..256).map(|_| local.alloc_meta(64)).collect::<Vec<_>>();
        assert!(addresses.iter().all(|&a| a != 0 && a % 64 == 0));
        let base = addresses[0] & !(CHUNK - 1);
        assert!(addresses.iter().all(|&a| a & !(CHUNK - 1) == base));
        for address in addresses {
            // SAFETY: Every metadata object from this chunk is retired once.
            unsafe { local.free_meta(address, 64) };
        }
        assert_eq!(local.alloc(CHUNK), base);
        // SAFETY: Complete object chunk is still owned by the test.
        unsafe { local.free(base, CHUNK) };
    }

    #[test]
    fn commit_and_decommit_failures_leave_backend_reusable() {
        let map = map().unwrap();
        // SAFETY: The handle comes from this backend's unique global map.
        let mut local = unsafe { Local::new(map) };
        let size = 1 << LOCAL_BITS;

        crate::hal::fail_next(crate::hal::Failure::Commit);
        assert_eq!(local.alloc(size), 0);
        let address = local.alloc(size);
        assert_ne!(address, 0);
        // SAFETY: The checked committed block is exclusively owned.
        unsafe { (address as *mut usize).write(0x1234_5678) };

        crate::hal::fail_next(crate::hal::Failure::Decommit);
        // SAFETY: The complete unpublished block is exclusively retired.
        unsafe { local.free(address, size) };
        let reused = local.alloc(size);
        assert_ne!(reused, 0);
        // SAFETY: The test retires its final unpublished block.
        unsafe { local.free(reused, size) };
    }
}
