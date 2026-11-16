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
            .clamp(
                hal::RESERVE_MIN,
                // #[gamma::skip(expr.decrement, reason = "The maximum is a power of two greater than RESERVE_MIN; clamping to maximum minus one still rounds to the identical next power of two after max(size)")]
                GLOBAL_REFILL,
            )
            // #[gamma::skip(expr.decrement, reason = "The validated requested size is a power of two greater than one; size minus one rounds back to the same power")]
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
        // #[gamma::skip(arith.sub_to_add, arith.sub_to_div, literal.int_decrement, reason = "Increasing this logarithm bound cannot change min with cursor trailing zeros for an aligned power-of-two suffix")]
        let highest_bit = usize::BITS - 1;
        while cursor < end {
            let bits = (cursor.trailing_zeros() as usize).min(
                // #[gamma::skip(expr.increment, arith.sub_to_add, reason = "The aligned power-of-two reservation end makes cursor trailing zeros no greater than floor(log2(end-cursor)); increasing the other min operand cannot affect bits")]
                (
                    // #[gamma::skip(arith.sub_to_div, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                    highest_bit - (end - cursor).leading_zeros()
                ) as usize,
            );
            // SAFETY: Suffix blocks are registered, aligned and exclusively owned.
            let overflow = unsafe { global.ranges.add(Nodes::Map(map), cursor, bits) };
            debug_assert_eq!(overflow, 0);
            cursor += 1 << bits;
        }
    }
    address
}

fn global_alloc(size: usize) -> usize {
    global_alloc_with(
        size,
        |size| GLOBAL.with(|global| global_alloc_reserved(global, size)),
        |address, size| {
            // CommitRange is OUTSIDE GlobalRange, exactly as in StandardLocalState:
            // cached local blocks stay committed; global cached blocks stay decommitted.
            // SAFETY: The removed block is exclusively owned and within one reservation.
            unsafe { hal::commit(address as *mut u8, size) }
        },
        global_free,
    )
}

fn global_alloc_with(
    size: usize,
    reserve: impl FnOnce(usize) -> usize,
    commit: impl FnOnce(usize, usize) -> bool,
    release: impl FnOnce(usize, usize),
) -> usize {
    let address = reserve(size);
    if address == 0 {
        return 0;
    }
    if !commit(address, size) {
        // A failed MEM_COMMIT does not publish any objects.
        release(address, size);
        return 0;
    }
    address
}

fn global_free(address: usize, size: usize) {
    global_free_with(
        address,
        size,
        |address, size| {
            // SAFETY: Caller has retired all allocations and acquired exclusive ownership.
            unsafe { hal::decommit(address as *mut u8, size) }
        },
        |address, size| {
            GLOBAL.with(|global| {
                let map = initialized_map(global.map);
                // SAFETY: The global lock transfers this registered, aligned block to its buddy.
                let overflow = unsafe { global.ranges.add(Nodes::Map(map), address, size.trailing_zeros() as usize) };
                debug_assert_eq!(overflow, 0);
            });
        },
    );
}

fn global_free_with(address: usize, size: usize, decommit: impl FnOnce(usize, usize) -> bool, cache: impl FnOnce(usize, usize)) {
    // A failed decommit leaves the block committed, which is still valid cache
    // storage; the next commit is idempotent and retries normal OS treatment.
    let _decommitted = decommit(address, size);
    cache(address, size);
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

    fn invalid_alloc_size(size: usize) -> bool {
        !size.is_power_of_two() || !(CHUNK..=(1usize << 46)).contains(&size)
    }

    fn refill_size(requested: usize, size: usize) -> usize {
        requested
            .min(1 << LOCAL_BITS)
            .max(
                // #[gamma::skip(expr.decrement, reason = "For validated power-of-two allocation sizes above one, size minus one rounds back to the identical refill")]
                size,
            )
            .next_power_of_two()
    }

    /// Transfers a committed power-of-two object range to the caller.
    pub(crate) fn alloc(&mut self, size: usize) -> usize {
        if Self::invalid_alloc_size(size) {
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
        let refill = Self::refill_size(
            self.requested,
            // #[gamma::skip(expr.decrement, reason = "refill_size rounds size minus one back to the same validated power-of-two allocation size")]
            size,
        );
        let address = global_alloc(refill);
        if address != 0 {
            self.requested = self.requested.saturating_add(refill);
            let mut cursor = address + size;
            let end = address + refill;
            // #[gamma::skip(arith.sub_to_add, arith.sub_to_div, literal.int_decrement, reason = "Increasing this logarithm bound cannot change min with cursor trailing zeros for an aligned power-of-two suffix")]
            let highest_bit = usize::BITS - 1;
            while cursor < end {
                let bits = (cursor.trailing_zeros() as usize).min(
                    // #[gamma::skip(expr.increment, arith.sub_to_add, reason = "The aligned power-of-two reservation end makes cursor trailing zeros no greater than floor(log2(end-cursor)); increasing the other min operand cannot affect bits")]
                    (
                        // #[gamma::skip(arith.sub_to_div, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                        highest_bit
                            - (
                                // #[gamma::skip(arith.sub_to_div, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
                                end - cursor
                            )
                                .leading_zeros()
                    ) as usize,
                );
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
        // #[gamma::skip(relational.ge_to_gt, literal.int_increment, reason = "At the local limit the buddy cannot retain the block and returns it unchanged for global_free, giving the same global return")]
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
            let bits = Self::metadata_seed_bits(
                address,
                // #[gamma::skip(expr.increment, reason = "Donated sizes are multiples of sixteen bounded by a registered reservation; adding one cannot cross a power-of-two boundary or overflow")]
                size,
            );
            let block =
            // #[gamma::skip(literal.int_decrement, shift.shl_to_shr, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
            1usize << bits;
            // SAFETY: Each aligned power-of-two block is a disjoint subrange
            // of the exclusively owned spare metadata supplied by the caller.
            unsafe { self.free_meta(address, block) };
            address += block;
            size -= block;
        }
    }

    fn metadata_seed_bits(address: usize, size: usize) -> usize {
        (address.trailing_zeros() as usize).min((usize::BITS - 1 - size.leading_zeros()) as usize)
    }

    pub(crate) fn alloc_meta(&mut self, size: usize) -> usize {
        self.alloc_meta_with(size, Self::alloc)
    }

    fn alloc_meta_with(&mut self, size: usize, mut allocate: impl FnMut(&mut Self, usize) -> usize) -> usize {
        if
        // #[gamma::skip(cond.always_false, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        // #[gamma::skip(literal.int_decrement, reason = "There is no power of two between fifteen and sixteen, and non-powers are rejected by the other disjunct")]
        !size.is_power_of_two() || size < 16 {
            return 0;
        }
        // #[gamma::skip(relational.ge_to_gt, reason = "At exactly CHUNK the metadata buddy take returns zero at its exclusive upper bound, then requests the same full chunk with no suffix to split")]
        if size >= CHUNK {
            return allocate(self, size);
        }
        // SAFETY: Metadata buddy blocks remain committed and exclusively owned.
        let address = unsafe { self.metadata.take(Nodes::Inline, size.trailing_zeros() as usize) };
        if address != 0 {
            return address;
        }
        let address = allocate(self, CHUNK);
        split_metadata_with(address, size, |address, size| {
            // SAFETY: Each suffix is disjoint committed metadata from this allocation.
            unsafe { self.free_meta(address, size) };
        })
    }

    /// # Safety
    /// The aligned power-of-two range was allocated with `alloc_meta`, or is a
    /// disjoint subdivision of registered backend-owned metadata permanently
    /// transferred to this local state. It contains no live metadata.
    pub(crate) unsafe fn free_meta(&mut self, address: usize, size: usize) {
        // #[gamma::skip(relational.ge_to_gt, reason = "At exactly CHUNK the metadata buddy add immediately returns the complete block as overflow and free receives the same range")]
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

// #[gamma::skip(parameter.default_shadow, tag = "timeout", reason = "Defaulting size to zero makes the metadata donation loop repeat forever because doubling zero never advances it")]
fn split_metadata_with(address: usize, size: usize, mut donate: impl FnMut(usize, usize)) -> usize {
    if address == 0 {
        return 0;
    }
    let mut offset = size;
    while offset < CHUNK {
        donate(address + offset, offset);
        offset *= 2;
    }
    address
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_nov4_metadata_split_failure_has_no_donations_and_success_is_exact() {
        for size in [16, CHUNK / 2] {
            let mut donations = Vec::new();
            assert_eq!(split_metadata_with(0, size, |a, s| donations.push((a, s))), 0);
            assert!(donations.is_empty());
            assert_eq!(split_metadata_with(0x10000, size, |a, s| donations.push((a, s))), 0x10000);
            let expected: Vec<_> = (size.trailing_zeros()..CHUNK.trailing_zeros())
                .map(|bits| (0x10000 + (1 << bits), 1 << bits))
                .collect();
            assert_eq!(donations, expected);
        }
    }

    #[test]
    fn gamma_round2_global_free_decommits_exact_range_before_caching_even_on_failure() {
        for result in [false, true] {
            let calls = std::cell::RefCell::new(Vec::new());
            global_free_with(
                0x10000,
                CHUNK,
                |address, size| {
                    calls.borrow_mut().push(("decommit", address, size));
                    result
                },
                |address, size| calls.borrow_mut().push(("cache", address, size)),
            );
            assert_eq!(*calls.borrow(), [("decommit", 0x10000, CHUNK), ("cache", 0x10000, CHUNK)]);
        }
    }

    #[test]
    fn gamma_round2_metadata_donation_selects_the_largest_aligned_fitting_block() {
        for (address, size, expected) in [(0x10000, 64, 6), (0x10010, 64, 4), (0x10000, 48, 5), (0x10020, 16, 4)] {
            assert_eq!(Local::metadata_seed_bits(address, size), expected);
        }
    }

    #[test]
    fn gamma_round2_commit_plan_releases_exactly_the_failed_reservation() {
        use std::cell::RefCell;

        for (address, committed, expected) in [(0, true, 0), (0x40000, true, 0x40000), (0x40000, false, 0)] {
            let calls = RefCell::new(Vec::new());
            let actual = global_alloc_with(
                CHUNK,
                |size| {
                    calls.borrow_mut().push(("reserve", 0, size));
                    address
                },
                |pointer, size| {
                    calls.borrow_mut().push(("commit", pointer, size));
                    committed
                },
                |pointer, size| calls.borrow_mut().push(("release", pointer, size)),
            );
            assert_eq!(actual, expected);
            let mut expected = vec![("reserve", 0, CHUNK)];
            if address != 0 {
                expected.push(("commit", address, CHUNK));
                if !committed {
                    expected.push(("release", address, CHUNK));
                }
            }
            assert_eq!(calls.into_inner(), expected);
        }
    }

    #[test]
    fn gamma_round2_refill_plan_caps_accumulated_requests_at_the_local_limit() {
        for (requested, size, expected) in [
            (0, CHUNK, CHUNK),
            (CHUNK, CHUNK, CHUNK),
            (3 * CHUNK, CHUNK, 4 * CHUNK),
            (3 << LOCAL_BITS, CHUNK, 1 << LOCAL_BITS),
        ] {
            assert_eq!(Local::refill_size(requested, size), expected);
        }
    }

    #[test]
    fn gamma_round2_failed_metadata_refill_leaves_every_metadata_class_empty() {
        let map = map().unwrap();
        // SAFETY: This test creates a new local state using the backend's unique map.
        let mut local = unsafe { Local::new(map) };
        let mut sizes = Vec::new();
        assert_eq!(
            local.alloc_meta_with(16, |_, size| {
                sizes.push(size);
                0
            }),
            0
        );
        assert_eq!(sizes, [CHUNK]);
        // SAFETY: The local metadata buddy has never received any backing.
        assert!(unsafe { local.metadata.validate(Nodes::Inline) }.is_empty());
        assert_eq!(local.requested, 0);
    }

    #[test]
    fn gamma_round2_size_limit_decision_is_inclusive_without_native_calls() {
        for (size, accepted) in [
            (0, false),
            (CHUNK - 1, false),
            (CHUNK, true),
            (CHUNK + 1, false),
            ((1_usize << 46) - 1, false),
            (1_usize << 46, true),
            ((1_usize << 46) + 1, false),
            (1_usize << 47, false),
            (usize::MAX, false),
        ] {
            assert_eq!(!Local::invalid_alloc_size(size), accepted, "size {size}");
        }
    }

    #[test]
    fn gamma_survivor_local_refill_partition_and_metadata_donation() {
        let map = map().unwrap();
        // SAFETY: The test holds the backend's unique map handle and owns this independent Local.
        let mut local = unsafe { Local::new(map) };
        for size in [0, 1, 15, 17, CHUNK - 1, CHUNK + 1, 1 << 47] {
            assert_eq!(local.alloc(size), 0);
        }
        for size in [0, 1, 8, 15, 17, CHUNK + 1] {
            assert_eq!(local.alloc_meta(size), 0);
        }
        let direct = local.alloc(1 << LOCAL_BITS);
        assert_ne!(direct, 0);
        assert_eq!(local.requested, 0);
        // SAFETY: The complete unpublished large block is returned exactly once.
        unsafe { local.free(direct, 1 << LOCAL_BITS) };
        let mut budget = usize::MAX;
        assert_eq!(local.observe(&mut budget).ranges.observed_bytes(), 0);
        for size in [CHUNK, CHUNK * 2] {
            let address = local.alloc_meta(size);
            assert_ne!(address, 0);
            assert_eq!(address % size, 0);
            let before = local.observe(&mut budget).ranges.observed_bytes();
            // SAFETY: Return the exact complete metadata allocation once, without subdividing it.
            unsafe { local.free_meta(address, size) };
            assert_eq!(local.observe(&mut budget).ranges.observed_bytes(), before + size as u128);
            local.validate();
        }
        // Exhaust these small cached blocks so the next refill has a known size.
        let cached_small = local.alloc(CHUNK);
        let cached_large = local.alloc(CHUNK * 2);
        assert_ne!(cached_small, 0);
        assert_ne!(cached_large, 0);
        local.requested = 1 << LOCAL_BITS;
        let base = local.alloc(CHUNK);
        assert_ne!(base, 0);
        let mut remaining = (1 << LOCAL_BITS) - CHUNK;
        let mut sizes = vec![(base, CHUNK)];
        for bits in CHUNK_BITS..LOCAL_BITS {
            let size = 1 << bits;
            let mut budget = usize::MAX;
            assert_eq!(local.observe(&mut budget).ranges.observed_bytes(), remaining as u128);
            let address = local.alloc(size);
            assert_ne!(address, 0);
            assert_eq!(address % size, 0);
            assert!(
                sizes
                    .iter()
                    .all(|&(other, length)| address + size <= other || other + length <= address)
            );
            remaining -= size;
            sizes.push((address, size));
            local.validate();
        }
        assert_eq!(remaining, 0);
        for (address, size) in sizes {
            // SAFETY: Each unpublished block is returned exactly once; all are mutually disjoint.
            unsafe { local.free(address, size) };
        }
        // SAFETY: This separately withheld cached allocation is still live and unpublished.
        unsafe { local.free(cached_small, CHUNK) };
        // SAFETY: This other withheld allocation remains live, disjoint, and unpublished.
        unsafe { local.free(cached_large, CHUNK * 2) };
        let donor = local.alloc(CHUNK);
        assert_ne!(donor, 0);
        // SAFETY: Permanently transfer this registered, committed, disjoint suffix once.
        // The withheld prefix is not in a buddy and the containing block is never separately returned.
        unsafe { local.seed_metadata(donor + 16, CHUNK - 16) };
        let mut budget = usize::MAX;
        assert_eq!(local.observe(&mut budget).metadata.observed_bytes(), (CHUNK - 16) as u128);
        let metadata: Vec<_> = (0..CHUNK / 16 - 1).map(|_| local.alloc_meta(16)).collect();
        assert_eq!(
            metadata.iter().copied().collect::<std::collections::BTreeSet<_>>().len(),
            metadata.len()
        );
        assert!(
            metadata
                .iter()
                .all(|&address| (donor + 16..donor + CHUNK).contains(&address) && address % 16 == 0)
        );
        for address in metadata {
            // SAFETY: Each donated subdivision was removed from the buddy and is retired exactly once.
            unsafe { local.free_meta(address, 16) };
        }
        // SAFETY: The withheld disjoint prefix is permanently donated now; no owner references the containing block.
        unsafe { local.free_meta(donor, 16) };
        local.validate();
        assert_eq!(local.observe(&mut budget).metadata.observed_bytes(), 0);
        let returned = local.alloc_meta(CHUNK);
        assert_ne!(returned, 0);
        // SAFETY: Return the complete, unpublished allocation from alloc_meta once.
        unsafe { local.free_meta(returned, CHUNK) };
        // SAFETY: A separate Local uses the same unique backend map without sharing range ownership.
        let mut partial = unsafe { Local::new(map) };
        let donor = partial.alloc(CHUNK);
        assert_ne!(donor, 0);
        // SAFETY: Permanently donate exactly the first 48 committed, aligned bytes once.
        // The remainder stays withheld; the containing allocation is never separately returned.
        unsafe { partial.seed_metadata(donor, 48) };
        assert_eq!(partial.observe(&mut budget).metadata.observed_bytes(), 48);
        let nodes: Vec<_> = (0..3).map(|_| partial.alloc_meta(16)).collect();
        assert_eq!(
            nodes.iter().copied().collect::<std::collections::BTreeSet<_>>(),
            [donor, donor + 16, donor + 32].into_iter().collect()
        );
        for node in nodes {
            // SAFETY: Each node came from one disjoint donated subdivision and is retired once.
            unsafe { partial.free_meta(node, 16) };
        }
        partial.validate();
        assert_eq!(partial.observe(&mut budget).metadata.observed_bytes(), 48);
    }

    #[test]
    fn gamma_survivor_isolated_global_refill_cap_and_budget() {
        for requested in [0, GLOBAL_REFILL - 1, GLOBAL_REFILL, GLOBAL_REFILL + 1] {
            let mut global = Global {
                map: Some(Map::reserve().unwrap()),
                ranges: Buddy::new(),
                requested,
            };
            let refill = if requested == 0 { hal::RESERVE_MIN } else { GLOBAL_REFILL };
            let first = global_alloc_reserved(&mut global, CHUNK);
            assert_ne!(first, 0);
            assert_eq!(first % refill, 0);
            let observed = observe_global(&global);
            assert_eq!(observed.reserved_bytes, (requested + refill) as u64);
            assert_eq!(observed.pagemap_reserved_bytes, crate::pagemap::RESERVED_BYTES as u64);
            assert_eq!(observed.global_refill_bytes, 1 << 24);
            assert_eq!(observed.local_limit_bytes, 1 << 21);
            assert_eq!(observed.ranges.observed_bytes(), (refill - CHUNK) as u128);
            for bits in CHUNK_BITS..refill.trailing_zeros() as usize {
                assert_eq!(global_alloc_reserved(&mut global, 1 << bits), first + (1 << bits));
            }
            assert_eq!(observe_global(&global).ranges.observed_bytes(), 0);
            // SAFETY: The initial block and exhausted disjoint suffix cover the entire unpublished reservation.
            unsafe { hal::release(first as *mut u8, refill) };
        }
    }

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
    fn reserved_refill_partitions_the_entire_reservation_without_overlap() {
        let mut global = Global {
            map: Some(Map::reserve().unwrap()),
            ranges: Buddy::new(),
            requested: 0,
        };
        let base = global_alloc_reserved(&mut global, CHUNK);
        assert_ne!(base, 0);
        assert_eq!(base % hal::RESERVE_MIN, 0);
        let mut addresses = vec![base];
        for allocated in 1..=hal::RESERVE_MIN / CHUNK {
            let state = observe_global(&global);
            assert!(state.ranges.complete);
            assert_eq!(state.reserved_bytes, hal::RESERVE_MIN as u64);
            assert_eq!(state.ranges.observed_bytes(), (hal::RESERVE_MIN - allocated * CHUNK) as u128);
            if allocated < hal::RESERVE_MIN / CHUNK {
                let address = global_alloc_reserved(&mut global, CHUNK);
                assert!((base..base + hal::RESERVE_MIN).contains(&address));
                assert_eq!(address % CHUNK, 0);
                assert!(!addresses.contains(&address));
                addresses.push(address);
            }
        }
        assert_eq!(addresses.len() * CHUNK, hal::RESERVE_MIN);
        // SAFETY: The isolated reservation was exhausted without publishing objects;
        // its exclusive blocks are all retired together, with no cached nodes remaining.
        unsafe { hal::release(base as *mut u8, hal::RESERVE_MIN) };
    }

    #[test]
    fn reserved_suffix_contains_one_aligned_block_per_size() {
        for refill in [hal::RESERVE_MIN, 2 * hal::RESERVE_MIN, GLOBAL_REFILL] {
            let mut global = Global {
                map: Some(Map::reserve().unwrap()),
                ranges: Buddy::new(),
                requested: refill,
            };
            let base = global_alloc_reserved(&mut global, CHUNK);
            assert_ne!(base, 0);
            assert_eq!(base % refill, 0);
            assert_eq!(global.requested, 2 * refill);
            let state = observe_global(&global);
            assert!(state.ranges.complete);
            for (bits, count) in state.ranges.counts.iter().enumerate() {
                assert_eq!(*count, u64::from((CHUNK..refill).contains(&(1usize << bits))));
            }
            let mut size = CHUNK;
            while size < refill {
                assert_eq!(global_alloc_reserved(&mut global, size), base + size);
                assert_eq!(global.requested, 2 * refill);
                size *= 2;
            }
            assert_eq!(observe_global(&global).ranges, seismograph_rallocator::native::Ranges::EMPTY);
            // SAFETY: All disjoint suffix blocks and the initial chunk are
            // exclusively test-owned, unpublished, and removed from the buddy.
            unsafe { hal::release(base as *mut u8, refill) };
        }
    }

    #[test]
    fn globally_retired_block_is_observable_and_reused_without_reserving() {
        const CHILD: &str = "RALLOCATOR_GLOBAL_REUSE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "backend::tests::globally_retired_block_is_observable_and_reused_without_reserving",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }
        // A fresh process prevents other allocator tests from changing global accounting.
        map().unwrap();
        let size = 1 << LOCAL_BITS;
        let address = global_alloc(size);
        assert_ne!(address, 0);
        let reserved = observe().reserved_bytes;
        assert_eq!(observe().ranges.observed_bytes(), 0);
        global_free(address, size);
        let returned = observe();
        assert!(returned.ranges.complete);
        assert_eq!(returned.reserved_bytes, reserved);
        assert_eq!(returned.ranges.observed_bytes(), size as u128);
        assert_eq!(global_alloc(size), address);
        assert_eq!(observe().reserved_bytes, reserved);
        assert_eq!(observe().ranges.observed_bytes(), 0);
        global_free(address, size);
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
        if run_with_fresh_backend("metadata_refill_commit_failure_can_be_retried") {
            return;
        }
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
            assert_eq!(address % CHUNK, 0);
            assert!(!addresses.contains(&address));
            addresses.push(address);
            let mut budget = usize::MAX;
            let state = local.observe(&mut budget);
            assert!(state.ranges.complete);
            assert_eq!(state.ranges.observed_bytes(), (expected - addresses.len() * CHUNK) as u128);
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
    fn local_suffix_exhausts_the_refill_without_crossing_its_boundary() {
        if run_with_fresh_backend("local_suffix_exhausts_the_refill_without_crossing_its_boundary") {
            return;
        }
        let map = map().unwrap();
        // SAFETY: The handle comes from this backend's unique global map.
        let mut local = unsafe { Local::new(map) };
        let refill = 1 << LOCAL_BITS;
        local.requested = refill;
        let base = local.alloc(CHUNK);
        assert_ne!(base, 0);
        assert_eq!(base % refill, 0);
        let reserved = observe().reserved_bytes;
        let mut budget = usize::MAX;
        let state = local.observe(&mut budget);
        assert!(state.ranges.complete);
        for (bits, count) in state.ranges.counts.iter().enumerate() {
            assert_eq!(*count, u64::from((CHUNK..refill).contains(&(1usize << bits))));
        }
        let mut size = CHUNK;
        while size < refill {
            assert_eq!(local.alloc(size), base + size);
            assert_eq!(local.requested, 2 * refill);
            assert_eq!(observe().reserved_bytes, reserved);
            size *= 2;
        }
        assert_eq!(local.observe(&mut budget).ranges, seismograph_rallocator::native::Ranges::EMPTY);
        // SAFETY: The initial chunk and every disjoint suffix block are
        // unpublished and exclusively owned; together they exhaust this refill.
        unsafe { local.free(base, refill) };
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
        if run_with_fresh_backend("commit_and_decommit_failures_leave_backend_reusable") {
            return;
        }
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

    #[cfg_attr(coverage_nightly, coverage(off))] // Test subprocess orchestration; allocator behavior remains measured inside the child.
    fn run_with_fresh_backend(name: &str) -> bool {
        let name = format!("backend::tests::{name}");
        if std::env::var("RALLOCATOR_BACKEND_FAILURE_TEST").as_deref() == Ok(name.as_str()) {
            return false;
        }
        // A cached global range can bypass a commit and leave the injected
        // failure pending. These tests require an empty backend, not test order.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("RALLOCATOR_BACKEND_FAILURE_TEST", &name)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }
}
