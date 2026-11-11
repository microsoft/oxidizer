// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
// Derived from mimalloc (39448a96), Copyright (c) Microsoft Research, Daan Leijen.
// See LICENSE-MIMALLOC.

//! Shared slice arenas, atomic ownership, commit tracking, and delayed purge.
//!
//! Published arenas and their metadata persist until process exit.
//! Allocation and release claim slices atomically without the publication lock.
//! Purge uses the same ownership protocol, so it cannot change live backing.
//!
//! Each 256-MiB region starts with a table of page metadata. These slices never
//! enter the free bitmap or the purge bitmap. The first region also stores the
//! arena and its bitmaps. Metadata uses the Rust page size, not a C layout.
//! Arenas stay NUMA-neutral, as with the reference default `arena_is_numa_local=0`.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::hal::bitmap::{Bitmap, CHUNK_SLICES, Claim, MAX_SLICES};
use super::hal::{self, Policy, Reservation, SLICE, VirtualMemory};
use super::heap::PAGE_META_SIZE;

// types.h: eight 512-slice chunks share one aligned page-metadata table.
// The Rust Page size determines the table size, as sizeof(mi_page_t) does in C.
pub(super) const META_SLICES: usize = 8 * CHUNK_SLICES;
pub(super) const META_ALIGNMENT: usize = META_SLICES * SLICE;
const META_SIZE: usize = (META_SLICES * PAGE_META_SIZE).div_ceil(SLICE) * SLICE;
// options.c defaults to a 1-GiB reserve and a 2-GiB object limit. Aligned
// metadata imposes the smaller fixed limit in arena.c.
const DEFAULT_ARENA: usize = 1024 * 1024 * 1024;
const MAX_OBJECT: usize = META_ALIGNMENT - META_SIZE;
const CHUNK: usize = CHUNK_SLICES * SLICE;
const MAX_ARENA: usize = MAX_SLICES * SLICE;
const SMALL_ARENA: usize = 4 * CHUNK;
// options.c: purge_delay (1000 ms) times arena_purge_mult (4).
const PURGE_DELAY_MS: u64 = 4000;
// types.h allows 160 arenas; arena.c rejects counts above 160 - 4 before
// creating another arena, so automatic creation stops at 157.
const MAX_ARENAS: usize = 157;

/// Owns one disjoint, fully committed range of arena slices or OS memory.
pub(super) struct Memory {
    address: *mut u8,
    size: usize,
    is_zero: bool,
    backing: Backing,
}

enum Backing {
    Arena { metadata: usize, slice: usize },
    Os(Reservation),
}

// SAFETY: Memory owns a disjoint range, has no thread-affine data, and moving
// ownership does not access its contents. Free bits coordinate cross-thread
// release and reuse. Raw pointer fields intentionally prevent automatic Sync.
unsafe impl Send for Memory {}

struct Arena {
    reservation: Reservation,
    bitmap: Bitmap,
    expires: AtomicU64,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PurgeState {
    Empty,
    Scheduled,
    Purged,
}

// SAFETY: Publication freezes the reservation and bitmap dimensions. All later
// metadata mutations are atomic. Reservation operations require a disjoint
// bitmap claim, which excludes every allocation and purge of the same slices.
// Shared references expose neither mutable references nor safe backing access.
unsafe impl Sync for Arena {}

struct Pool {
    arenas: [AtomicUsize; MAX_ARENAS],
    count: AtomicUsize,
    publication: Mutex<()>,
    purging: AtomicBool,
    expires: AtomicU64,
    epoch: Instant,
    policy: Policy,
    vm: VirtualMemory,
}

// A process-shared pool is deliberate and is not exposed across DLL boundaries.
// OnceLock, scalar TLS, and the publication lock require no Rust allocations.
static POOL: OnceLock<Pool> = OnceLock::new();
static NEXT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
// Direct-OS mappings bypass arena bitmaps. These slow-path counters track live
// reservations and data commitment without changing Memory or Page layout.
static OS_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static OS_RESERVED: AtomicUsize = AtomicUsize::new(0);
static OS_COMMITTED_DATA: AtomicUsize = AtomicUsize::new(0);
std::thread_local! {
    static SEQUENCE: Cell<Option<usize>> = const { Cell::new(None) };
}

fn pool() -> &'static Pool {
    POOL.get_or_init(|| Pool::new(hal::policy(), VirtualMemory::native()))
}

pub(super) fn sequence() -> usize {
    // Scalar TLS has no destructor. The fallback also permits calls during
    // late TLS teardown on platforms that reject access to a destroyed key.
    let cached = SEQUENCE.try_with(Cell::get).unwrap_or_default();
    let sequence = sequence_value(cached, &NEXT_SEQUENCE);
    if cached.is_none() {
        // Caching is optional. An unavailable key still gets a unique sequence.
        let _cached = SEQUENCE.try_with(|slot| slot.set(Some(sequence))).is_ok();
    }
    sequence
}

fn sequence_value(cached: Option<usize>, counter: &AtomicUsize) -> usize {
    cached.unwrap_or_else(|| counter.fetch_add(1, Ordering::Relaxed))
}

impl Memory {
    pub(super) fn allocate(size: usize, alignment: usize) -> Option<Self> {
        let slices = size.checked_add(SLICE - 1)? / SLICE;
        let metadata_slots = if alignment > SLICE || slices > 64 { slices.min(2) } else { slices };
        Self::allocate_page(size, alignment, metadata_slots)
    }

    /// Allocates backing with the page's native metadata count. Ordinary pages
    /// need one slot per slice. Singletons need at most two slots.
    pub(super) fn allocate_page(size: usize, alignment: usize, metadata_slots: usize) -> Option<Self> {
        if size == 0 || alignment < SLICE || !alignment.is_power_of_two() {
            return None;
        }
        let size = size.checked_add(SLICE - 1)? & !(SLICE - 1);
        if size > isize::MAX as usize || metadata_slots == 0 || metadata_slots > size / SLICE {
            return None;
        }
        Self::allocate_checked(size, alignment, metadata_slots, pool())
    }

    fn allocate_checked(size: usize, alignment: usize, metadata_slots: usize, pool: &Pool) -> Option<Self> {
        if alignment == SLICE
            && size <= MAX_OBJECT
            && let Some(memory) = pool.allocate(size, sequence())
        {
            return Some(memory);
        }
        let layout = OsLayout::new(size, alignment, metadata_slots)?;
        let reservation = pool.vm.reserve(layout.reservation_size, META_ALIGNMENT, false)?;
        // SAFETY: The new reservation is exclusive. Metadata and data are
        // disjoint, and both ranges are inside this reservation.
        if !unsafe { pool.vm.commit(&reservation, 0, layout.metadata_size) } {
            return None;
        }
        // SAFETY: The metadata commit cannot overlap the data range. No data
        // access occurs until this commit succeeds.
        if !unsafe { pool.vm.commit(&reservation, layout.data_offset, size) } {
            return None;
        }
        // SAFETY: The layout bounds the data range by the owned reservation.
        let address = unsafe { reservation.as_ptr().add(layout.data_offset) };
        OS_RESERVED.fetch_add(reservation.len(), Ordering::Relaxed);
        OS_COMMITTED_DATA.fetch_add(size, Ordering::Relaxed);
        OS_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        Some(Self {
            address,
            size,
            is_zero: true,
            backing: Backing::Os(reservation),
        })
    }

    pub(super) const fn as_ptr(&self) -> *mut u8 {
        self.address
    }

    pub(super) const fn len(&self) -> usize {
        self.size
    }

    pub(super) const fn is_zero(&self) -> bool {
        self.is_zero
    }

    /// Returns the committed metadata slot for the first data slice.
    ///
    /// Arena metadata stays committed until process exit. Direct OS backing
    /// commits the slot count from `allocate_page`, as in the reference
    /// `mi_arenas_page_alloc_fresh_area`.
    pub(super) fn metadata_ptr(&self) -> *mut u8 {
        let address = self.address.addr();
        let base = address & !(META_ALIGNMENT - 1);
        let slice = (address - base) / SLICE;
        self.address.with_addr(base + slice * PAGE_META_SIZE)
    }

    pub(super) fn arena_location(&self) -> Option<(usize, usize, usize)> {
        let Backing::Arena { metadata, slice } = self.backing else {
            return None;
        };
        // SAFETY: Each arena in a Memory owner is published permanently.
        let arena = unsafe { &*std::ptr::with_exposed_provenance::<Arena>(metadata) };
        Some((arena.index, slice, arena.reservation.len() / SLICE))
    }

    /// Collects scheduled backing purges. Forced collection visits all arenas
    /// and ignores their deadlines, but still preserves transparent huge pages.
    pub(super) fn collect(force: bool) {
        Self::collect_pool(POOL.get(), force);
    }

    fn collect_pool(pool: Option<&Pool>, force: bool) {
        if let Some(pool) = pool {
            pool.try_purge(pool.now(), sequence(), force);
        }
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        match &self.backing {
            Backing::Arena { metadata, slice } => {
                // SAFETY: Memory's private constructor records a published,
                // process-lifetime arena. Only shared references are created.
                let arena = unsafe { &*std::ptr::with_exposed_provenance::<Arena>(*metadata) };
                // SAFETY: This Memory is the unique owner of these claimed
                // slices. Its caller retired all block accesses before drop.
                let claim = unsafe { arena.bitmap.reclaim(*slice, self.size / SLICE) };
                arena.schedule_purge(&claim, pool());
                // The free-bit release publishes all preceding commit changes.
                drop(claim);
            }
            // The owned reservation releases itself after this method returns.
            Backing::Os(reservation) => {
                debug_assert!(self.address.addr() >= reservation.as_ptr().addr());
                debug_assert!(self.address.addr() - reservation.as_ptr().addr() + self.size <= reservation.len());
                OS_ALLOCATIONS.fetch_sub(1, Ordering::Relaxed);
                OS_RESERVED.fetch_sub(reservation.len(), Ordering::Relaxed);
                OS_COMMITTED_DATA.fetch_sub(self.size, Ordering::Relaxed);
            }
        }
    }
}

pub(super) fn observe(budget: usize) -> seismograph_rallocator::mimalloc::GlobalState {
    observe_pool(POOL.get(), budget)
}

fn observe_pool(pool: Option<&Pool>, mut budget: usize) -> seismograph_rallocator::mimalloc::GlobalState {
    let mut result = seismograph_rallocator::mimalloc::GlobalState {
        os_allocations: OS_ALLOCATIONS.load(Ordering::Relaxed) as u64,
        os_reserved_bytes: OS_RESERVED.load(Ordering::Relaxed) as u64,
        os_committed_bytes: OS_COMMITTED_DATA.load(Ordering::Relaxed) as u64,
        abandoned_pages: super::abandoned::observe(),
        ..seismograph_rallocator::mimalloc::GlobalState::EMPTY
    };
    // Inspection must not initialize a pool, allocate backing, or purge memory.
    let Some(pool) = pool else { return result };
    let count = pool.count.load(Ordering::Acquire);
    for index in 0..count {
        if budget == 0 {
            result.arenas_complete = false;
            break;
        }
        let arena = pool.arena(index);
        result.arena_count += 1;
        let size = arena.reservation.len();
        result.reserved_bytes += size as u64;
        // The first table also holds Arena; each later region reserves one table.
        let metadata =
            arena_info_size().unwrap_or_else(|| crate::backend::abort::abort()) + (size.div_ceil(META_ALIGNMENT) - 1) * META_SIZE;
        result.metadata_bytes += metadata as u64;
        let (free, committed, pending, complete) = arena.bitmap.observe(&mut budget);
        result.free_slices += free;
        result.committed_bytes += committed * SLICE as u64;
        result.pending_purge_slices += pending;
        result.arenas_complete &= complete;
    }
    result
}

impl Pool {
    fn new(policy: Policy, vm: VirtualMemory) -> Self {
        Self {
            arenas: [const { AtomicUsize::new(0) }; MAX_ARENAS],
            count: AtomicUsize::new(0),
            publication: Mutex::new(()),
            purging: AtomicBool::new(false),
            expires: AtomicU64::new(0),
            epoch: Instant::now(),
            policy,
            vm,
        }
    }

    fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn arena(&self, index: usize) -> &Arena {
        let address = self.arenas[index].load(Ordering::Acquire);
        // SAFETY: Callers bound index by an acquired published count. Each slot
        // is stored before that count and never changed or unmapped afterward.
        unsafe { &*std::ptr::with_exposed_provenance::<Arena>(address) }
    }

    fn find(&self, size: usize, sequence: usize, count: usize) -> Option<Memory> {
        // The default single heap rotates all but the last arena, then tries
        // that last arena. Explicit heaps are outside this backing interface.
        for index in arena_order(count, sequence) {
            let arena = self.arena(index);
            if let Some(memory) = arena.allocate(self.arenas[index].load(Ordering::Relaxed), size, sequence, self) {
                return Some(memory);
            }
        }
        None
    }

    fn allocate(&self, size: usize, sequence: usize) -> Option<Memory> {
        let count = self.count.load(Ordering::Acquire);
        if let Some(memory) = self.find(size, sequence, count) {
            return Some(memory);
        }
        if self.expand(size, count) {
            debug_assert!(self.count.load(Ordering::Acquire) > count);
        }
        self.find(size, sequence, self.count.load(Ordering::Acquire))
    }

    fn expand(&self, size: usize, observed: usize) -> bool {
        // Only arena creation and directory publication use this lock.
        // Existing-arena claims, frees, commits, and purges do not acquire it.
        let publication = self.publication.lock();
        // Poisoning is a programming invariant failure, not recoverable OOM.
        // Check before extracting the guard, so into_inner never recovers it.
        crate::backend::abort::require(publication.is_ok());
        let _publication = publication.unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.count.load(Ordering::Acquire);
        if !can_expand(observed, current) {
            return false;
        }
        let Some(desired) = arena_size(current, size) else {
            return false;
        };
        if self.create_arena(desired, current).is_some() {
            return true;
        }
        match required_reservation(size) {
            Some(required) if desired > SMALL_ARENA && SMALL_ARENA > required => self.create_arena(SMALL_ARENA, current).is_some(),
            _ => false,
        }
    }

    /// Called only while the directory publication lock is held.
    fn create_arena(&self, size: usize, index: usize) -> Option<()> {
        let info_size = arena_info_size()?;
        if info_size >= CHUNK || info_size >= size {
            return None;
        }
        let reservation = self.vm.reserve(size, META_ALIGNMENT, self.policy.eager_commit)?;
        // SAFETY: The fresh mapping has no users. Initial metadata includes the
        // first page table and the arena, and stays reserved permanently.
        if !self.policy.eager_commit && !unsafe { self.vm.commit(&reservation, 0, info_size) } {
            return None;
        }
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "OS metadata reservations have 64-KiB alignment, exceeding Arena alignment"
        )]
        // SAFETY: The first page table precedes the arena within committed
        // metadata. META_SIZE is slice-aligned and info_size covers Arena.
        let pointer = unsafe { reservation.as_ptr().add(META_SIZE) }.cast::<Arena>();
        // SAFETY: Fresh anonymous committed OS pages are zero-initialized.
        // All Arena fields accept zero bits, including the atomic fields.
        // Write the owning reservation before creating any reference.
        let field = unsafe { std::ptr::addr_of_mut!((*pointer).reservation) };
        // SAFETY: The field address is aligned and writable in this owned mapping.
        unsafe { field.write(reservation) };
        // SAFETY: The fresh, committed metadata contains this aligned field.
        let field = unsafe { std::ptr::addr_of_mut!((*pointer).index) };
        // SAFETY: This writable field is inside the owned metadata mapping.
        unsafe { field.write(index) };
        // SAFETY: Arena is initialized and aligned within committed metadata.
        // No other reference exists before its directory slot is published.
        let arena = unsafe { &mut *pointer };
        arena.bitmap.initialize(size / SLICE, self.policy.eager_commit);
        for region in (0..size / SLICE).step_by(META_SLICES) {
            let reserved = if region == 0 { info_size / SLICE } else { META_SIZE / SLICE };
            arena.bitmap.reserve_metadata(region, reserved);
        }
        arena.bitmap.set_metadata_committed(0, info_size / SLICE);
        let address = pointer.expose_provenance();
        // The reservation is now owned by the embedded, process-lifetime Arena.
        self.arenas[index].store(address, Ordering::Release);
        self.count.store(index + 1, Ordering::Release);
        Some(())
    }

    fn try_purge(&self, now: u64, sequence: usize, force: bool) {
        let expires = self.expires.load(Ordering::Acquire);
        if !force && (expires == 0 || expires > now) {
            return;
        }
        let count = self.count.load(Ordering::Acquire);
        if count == 0 {
            return;
        }
        // arena.c allows only one purger, but allocation and release continue.
        if self
            .purging
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let _purging = PurgeGuard(&self.purging);
        // New notifications can now install their own deadline. Never clear
        // this global deadline again at the end of the scan.
        self.expires.swap(0, Ordering::AcqRel);
        let mut budget = purge_budget(count, force);
        let mut retain_deadline = false;
        for offset in 0..count {
            let arena = self.arena((sequence % count + offset) % count);
            let state = arena.purge(now, self.policy.purge_slices, force);
            retain_deadline |= state != PurgeState::Empty;
            if state == PurgeState::Purged {
                budget -= 1;
                if budget == 0 {
                    break;
                }
            }
        }
        // arena.c retains the old global deadline while any arena was purged
        // or is still scheduled. A forced early scan shortens it to delay/10.
        // Concurrent notifications survive because we only merge, never clear,
        // their newly published deadline here.
        if retain_deadline && expires != 0 {
            record_expiration(&self.expires, retained_deadline(expires, now));
        }
    }
}

impl Arena {
    fn schedule_purge(&self, claim: &Claim<'_>, pool: &Pool) {
        let expires = pool.now().saturating_add(PURGE_DELAY_MS);
        // Publish pending bits before their deadline. A purger that clears the
        // old deadline before visiting the bitmap cannot lose a later notice.
        claim.schedule_purge();
        record_expiration(&self.expires, expires);
        record_expiration(&pool.expires, expires);
    }

    fn allocate(&self, metadata: usize, size: usize, sequence: usize, pool: &Pool) -> Option<Memory> {
        let claim = self.bitmap.claim(size / SLICE, sequence)?;
        // A commit can cover both dirty and untouched slices, so the native
        // commit primitives do not promise zeros. Initial OS reservations do.
        let is_zero = claim.mark_dirty();
        if claim.committed() {
            // SAFETY: The claim excludes every data access during reuse.
            unsafe { self.reservation.reuse(claim.start() * SLICE, size) };
        } else {
            // SAFETY: The atomic claim excludes allocations and purge from this
            // disjoint range until commit succeeds or the guard rolls back.
            if !unsafe { pool.vm.commit(&self.reservation, claim.start() * SLICE, size) } {
                return None;
            }
            claim.set_committed(true);
        }
        let meta_start = (claim.start() / META_SLICES) * META_SLICES;
        let meta_count = META_SIZE / SLICE;
        if !self.bitmap.metadata_committed(meta_start, meta_count) {
            // SAFETY: These permanently reserved slices never contain data
            // allocations or purge claims. Concurrent commits preserve bytes.
            if !unsafe { pool.vm.commit(&self.reservation, meta_start * SLICE, META_SIZE) } {
                // Data commitment already succeeded. Like _mi_arenas_free on
                // a metadata failure, let backing collection reclaim it.
                self.schedule_purge(&claim, pool);
                return None;
            }
            self.bitmap.set_metadata_committed(meta_start, meta_count);
        }
        // SAFETY: The claim is entirely within this live reservation.
        let address = unsafe { self.reservation.as_ptr().add(claim.start() * SLICE) };
        Some(Memory {
            address,
            size,
            is_zero,
            backing: Backing::Arena {
                metadata,
                slice: claim.into_index(),
            },
        })
    }

    fn purge(&self, now: u64, minimum: usize, force: bool) -> PurgeState {
        let expires = self.expires.load(Ordering::Acquire);
        if expires == 0 {
            return PurgeState::Empty;
        }
        if !purge_due(expires, now, force) {
            return PurgeState::Scheduled;
        }
        self.expires.swap(0, Ordering::AcqRel);
        let purged = self.bitmap.purge_pending(minimum, |claim| {
            // SAFETY: The purge visitor acquired the exact free slices it passes
            // here. Their claim remains held through the OS operation and the
            // commit-bitmap update, before release permits another allocation.
            unsafe { self.reservation.purge(claim.start() * SLICE, claim.count() * SLICE) }
        });
        if purged { PurgeState::Purged } else { PurgeState::Empty }
    }
}

struct PurgeGuard<'a>(&'a AtomicBool);

impl Drop for PurgeGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[expect(
    clippy::allow_attributes,
    reason = "The minimum compiler does not deprecate this API, so expect(deprecated) cannot cover every supported compiler"
)]
#[allow(deprecated, reason = "AtomicU64::try_update is newer than the Rust 1.95 minimum")]
fn record_expiration(expiration: &AtomicU64, deadline: u64) -> bool {
    expiration
        .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |previous| {
            (previous == 0 || deadline < previous).then_some(deadline)
        })
        .is_ok()
}

#[derive(Debug, PartialEq, Eq)]
struct OsLayout {
    data_offset: usize,
    metadata_size: usize,
    reservation_size: usize,
}

impl OsLayout {
    fn new(size: usize, alignment: usize, metadata_slots: usize) -> Option<Self> {
        // arena.c rejects alignments at or above the metadata-region size.
        // Otherwise a pointer exactly on a region boundary decodes to the
        // next table, rather than to this reservation's table.
        if alignment >= META_ALIGNMENT {
            return None;
        }
        let data_offset = alignment.max(SLICE);
        let metadata_size = (data_offset / SLICE + 1).checked_add(metadata_slots)?.checked_mul(PAGE_META_SIZE)?;
        if metadata_size >= data_offset {
            return None;
        }
        Some(Self {
            data_offset,
            metadata_size,
            reservation_size: size.checked_add(data_offset)?,
        })
    }
}

fn arena_info_size() -> Option<usize> {
    Some(META_SIZE.checked_add(std::mem::size_of::<Arena>())?.checked_add(SLICE - 1)? & !(SLICE - 1))
}

fn purge_budget(count: usize, force: bool) -> usize {
    if force { count } else { count / 4 + 1 }
}

fn purge_due(expires: u64, now: u64, force: bool) -> bool {
    expires != 0 && (force || expires <= now)
}

fn retained_deadline(expires: u64, now: u64) -> u64 {
    if expires > now {
        now.saturating_add(PURGE_DELAY_MS / 10)
    } else {
        expires
    }
}

pub(super) fn arena_count() -> usize {
    pool().count.load(Ordering::Acquire)
}

pub(super) fn arena_order(count: usize, sequence: usize) -> impl Iterator<Item = usize> {
    let cycle = count.saturating_sub(1);
    (0..count).map(move |index| if index < cycle { (index + sequence % cycle) % cycle } else { index })
}

fn arena_size(count: usize, size: usize) -> Option<usize> {
    let shift = if (1..=128).contains(&count) { (count / 8).min(16) } else { 0 };
    let reserve = DEFAULT_ARENA.checked_shl(u32::try_from(shift).ok()?)?;
    let required = required_reservation(size)?;
    let reserve = reserve.max(required).clamp(CHUNK, MAX_ARENA);
    (reserve >= required).then_some(reserve)
}

fn can_expand(observed: usize, current: usize) -> bool {
    observed == current && current < MAX_ARENAS
}

fn required_reservation(size: usize) -> Option<usize> {
    Some(size.checked_add(CHUNK)?.checked_add(CHUNK - 1)? & !(CHUNK - 1))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::mem::ManuallyDrop;
    use std::ops::Deref;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use super::{
        Arena, Backing, Bitmap, CHUNK, DEFAULT_ARENA, MAX_ARENA, MAX_ARENAS, META_ALIGNMENT, META_SIZE, META_SLICES, Memory, OsLayout,
        PAGE_META_SIZE, Policy, Pool, PurgeState, SLICE, SMALL_ARENA, VirtualMemory, arena_info_size, arena_order, arena_size, can_expand,
        hal, purge_budget, purge_due, record_expiration, retained_deadline,
    };

    struct TestPool(Pool);

    struct TestMemory<'a> {
        memory: ManuallyDrop<Memory>,
        pool: &'a Pool,
    }

    impl TestPool {
        fn new(reserve_failures: usize, commit_failures: usize) -> Self {
            Self(Pool::new(
                Policy {
                    eager_commit: false,
                    purge_slices: 1,
                    page_size: 4096,
                    large_page_size: 2 * 1024 * 1024,
                    #[cfg(target_os = "linux")]
                    overcommit: false,
                },
                VirtualMemory::with_failures(reserve_failures, commit_failures),
            ))
        }

        fn own(&self, memory: Memory) -> TestMemory<'_> {
            TestMemory {
                memory: ManuallyDrop::new(memory),
                pool: &self.0,
            }
        }

        fn allocate(&self, size: usize, alignment: usize, metadata_slots: usize) -> Option<TestMemory<'_>> {
            Memory::allocate_checked(size, alignment, metadata_slots, &self.0).map(|memory| self.own(memory))
        }
    }

    impl Drop for TestPool {
        fn drop(&mut self) {
            for index in 0..self.0.count.load(Ordering::Acquire) {
                let address = self.0.arenas[index].load(Ordering::Acquire);
                let arena = std::ptr::with_exposed_provenance::<Arena>(address);
                // SAFETY: TestMemory borrows this pool, so every owner and every
                // test thread has finished before cleanup. Published fixtures
                // own exactly one mapping through this embedded reservation.
                let field = unsafe { std::ptr::addr_of!((*arena).reservation) };
                // SAFETY: Moving the owner out prevents a self-referential drop
                // from reading metadata after the mapping has been released.
                let reservation = unsafe { field.read() };
                drop(reservation);
            }
        }
    }

    impl Deref for TestMemory<'_> {
        type Target = Memory;

        fn deref(&self) -> &Self::Target {
            &self.memory
        }
    }

    impl Drop for TestMemory<'_> {
        fn drop(&mut self) {
            match self.memory.backing {
                Backing::Arena { metadata, slice } => {
                    // SAFETY: The pool borrow keeps this published fixture alive.
                    let arena = unsafe { &*std::ptr::with_exposed_provenance::<Arena>(metadata) };
                    // SAFETY: This fixture owns the sole claim for these slices.
                    let claim = unsafe { arena.bitmap.reclaim(slice, self.memory.size / SLICE) };
                    arena.schedule_purge(&claim, self.pool);
                    drop(claim);
                }
                Backing::Os(_) => {
                    // SAFETY: OS Memory does not use a pool during destruction.
                    // ManuallyDrop ensures its reservation is released once.
                    unsafe { ManuallyDrop::drop(&mut self.memory) };
                }
            }
        }
    }

    #[cfg_attr(
        miri,
        ignore = "Native arena backing runs natively and under cargo careful; atomic bitmap observation runs under Miri"
    )]
    #[test]
    fn bounded_global_observation_never_commits_claims_or_purges_memory() {
        let pool = TestPool::new(0, 0);
        let _memory = pool.allocate(SLICE, SLICE, 1).unwrap();
        let before = pool.0.vm.calls();

        let empty = super::observe_pool(None, 0);
        let omitted = super::observe_pool(Some(&pool.0), 0);
        let complete = super::observe_pool(Some(&pool.0), usize::MAX);

        assert_eq!((empty.arena_count, empty.reserved_bytes, empty.arenas_complete), (0, 0, true));
        assert_eq!((omitted.arena_count, omitted.arenas_complete), (0, false));
        assert_eq!(complete.arena_count, 1);
        assert!(complete.arenas_complete && complete.committed_bytes >= SLICE as u64);
        assert!(complete.metadata_bytes != 0 && complete.free_slices != 0);
        assert_eq!(pool.0.vm.calls(), before);
    }

    #[test]
    fn late_tls_cache_failures_still_produce_distinct_thread_sequences() {
        struct Probe {
            outcome: Arc<AtomicBool>,
            cached: usize,
        }

        std::thread_local! {
            static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
        }

        impl Drop for Probe {
            fn drop(&mut self) {
                let first = super::sequence_value(PROBE.try_with(probe_value).unwrap_or_default(), &super::NEXT_SEQUENCE);
                let second = super::sequence_value(PROBE.try_with(probe_value).unwrap_or_default(), &super::NEXT_SEQUENCE);
                self.outcome
                    .store(first != second && first != self.cached && second != self.cached, Ordering::Release);
            }
        }

        fn probe_value(slot: &RefCell<Option<Probe>>) -> Option<usize> {
            slot.borrow().as_ref().map(|probe| probe.cached)
        }

        let outcome = Arc::new(AtomicBool::new(false));
        let result = Arc::clone(&outcome);

        std::thread::spawn(move || {
            let cached = super::sequence();
            PROBE.with(|slot| *slot.borrow_mut() = Some(Probe { outcome: result, cached }));
            assert_eq!(PROBE.try_with(probe_value).unwrap(), Some(cached));
        })
        .join()
        .unwrap();

        assert!(outcome.load(Ordering::Acquire));
    }

    #[test]
    fn collection_before_arena_creation_does_not_allocate_or_consume_notifications() {
        let pool = TestPool::new(usize::MAX, usize::MAX);
        pool.0.expires.store(1, Ordering::Release);

        Memory::collect_pool(None, true);
        Memory::collect_pool(Some(&pool.0), true);

        assert_eq!((pool.0.expires.load(Ordering::Acquire), pool.0.vm.calls()), (1, (0, 0, [0; 4])));
    }

    #[test]
    fn public_collection_keeps_live_backing_and_its_directory_entry() {
        let memory = Memory::allocate_page(SLICE, SLICE, 1).unwrap();
        let location = memory.arena_location().unwrap();
        let count = super::arena_count();
        // SAFETY: This Memory uniquely owns the complete committed data range.
        unsafe { memory.as_ptr().write(0x3D) };
        // SAFETY: The final byte stays in the same live data range.
        let last = unsafe { memory.as_ptr().add(memory.len() - 1) };
        // SAFETY: This final byte is uniquely owned.
        unsafe { last.write(0x7B) };

        Memory::collect(false);
        Memory::collect(true);

        // SAFETY: Collection cannot purge the still-owned range.
        let first = unsafe { memory.as_ptr().read() };
        // SAFETY: The final byte remains in the same committed range.
        let last = unsafe { last.read() };
        assert_eq!(
            (memory.arena_location(), super::arena_count() >= count, first, last),
            (Some(location), true, 0x3D, 0x7B)
        );
    }

    #[test]
    fn a_full_arena_is_skipped_without_attempting_data_commitment() {
        let pool = TestPool::new(0, 0);
        pool.0.create_arena(CHUNK, 0).unwrap();
        let address = pool.0.arenas[0].load(Ordering::Acquire);
        // SAFETY: This fixture has no data owners or other readers. The test
        // controls the bitmap before beginning the allocation search.
        let arena = unsafe { &mut *std::ptr::with_exposed_provenance_mut::<Arena>(address) };
        arena.bitmap.reserve_metadata(0, CHUNK / SLICE);

        let failed = pool.0.find(SLICE, 0, 1).is_none();

        assert_eq!((failed, pool.0.vm.calls()), (true, (1, 1, [CHUNK, 0, 0, 0])));
    }

    #[test]
    fn default_bitmap_storage_claims_large_spans_after_the_metadata_chunk() {
        let pool = TestPool::new(0, 0);
        pool.0.create_arena(SMALL_ARENA, 0).unwrap();
        let size = CHUNK + 8 * 1024 * 1024;

        let memory = pool.allocate(size, SLICE, 2).unwrap();

        assert_eq!(
            (memory.len(), memory.arena_location(), memory.is_zero()),
            (size, Some((0, CHUNK / SLICE, SMALL_ARENA / SLICE)), true)
        );
    }

    #[test]
    fn cooperative_collection_stops_at_its_budget_and_force_finishes_the_rest() {
        let pool = TestPool::new(0, 0);
        for index in 0..4 {
            pool.0.create_arena(CHUNK, index).unwrap();
            let address = pool.0.arenas[index].load(Ordering::Acquire);
            let arena = pool.0.arena(index);
            drop(pool.own(arena.allocate(address, SLICE, 0, &pool.0).unwrap()));
            arena.expires.store(4000, Ordering::Release);
        }
        pool.0.expires.store(4000, Ordering::Release);

        pool.0.try_purge(4000, 0, false);
        let partial = std::array::from_fn::<_, 4, _>(|index| pool.0.arena(index).expires.load(Ordering::Acquire));
        pool.0.try_purge(0, 0, true);
        let forced = std::array::from_fn::<_, 4, _>(|index| pool.0.arena(index).expires.load(Ordering::Acquire));

        assert_eq!(
            (partial, forced, pool.0.expires.load(Ordering::Acquire)),
            ([0, 0, 4000, 4000], [0; 4], 400)
        );
    }

    #[test]
    fn collection_preserves_notifications_before_the_global_deadline() {
        let pool = TestPool::new(usize::MAX, usize::MAX);
        pool.0.expires.store(4000, Ordering::Release);

        pool.0.try_purge(3999, 0, false);

        assert_eq!((pool.0.expires.load(Ordering::Acquire), pool.0.vm.calls()), (4000, (0, 0, [0; 4])));
    }

    #[test]
    fn a_concurrent_purger_keeps_its_notifications_and_guard() {
        let pool = TestPool::new(0, 0);
        pool.0.create_arena(CHUNK, 0).unwrap();
        pool.0.expires.store(4000, Ordering::Release);
        pool.0.purging.store(true, Ordering::Release);

        pool.0.try_purge(4000, 0, false);

        assert_eq!(
            (
                pool.0.expires.load(Ordering::Acquire),
                pool.0.purging.load(Ordering::Acquire),
                pool.0.vm.calls()
            ),
            (4000, true, (1, 1, [CHUNK, 0, 0, 0]))
        );
    }

    #[test]
    fn a_followup_scan_clears_the_last_stale_global_notification() {
        let pool = TestPool::new(0, 0);
        pool.0.create_arena(CHUNK, 0).unwrap();
        let address = pool.0.arenas[0].load(Ordering::Acquire);
        let arena = pool.0.arena(0);
        drop(pool.own(arena.allocate(address, SLICE, 0, &pool.0).unwrap()));
        arena.expires.store(4000, Ordering::Release);
        pool.0.expires.store(4000, Ordering::Release);

        pool.0.try_purge(4000, 0, false);
        let after_work = pool.0.expires.load(Ordering::Acquire);
        pool.0.try_purge(4000, 0, false);

        assert_eq!(
            (
                after_work,
                pool.0.expires.load(Ordering::Acquire),
                arena.expires.load(Ordering::Acquire)
            ),
            (4000, 0, 0)
        );
    }

    #[test]
    fn memory_rejects_empty_overflowing_and_invalidly_aligned_requests() {
        let invalid = [
            Memory::allocate(0, SLICE).is_none(),
            Memory::allocate(usize::MAX, SLICE).is_none(),
            Memory::allocate(SLICE, SLICE / 2).is_none(),
            Memory::allocate(SLICE, 3 * SLICE).is_none(),
            Memory::allocate_page(isize::MAX as usize + 1, SLICE, 1).is_none(),
            Memory::allocate_page(SLICE, SLICE, 0).is_none(),
            Memory::allocate_page(SLICE, SLICE, 2).is_none(),
        ];

        assert_eq!(invalid, [true; 7]);
    }

    #[test]
    fn os_pages_and_singletons_have_distinct_metadata_counts() {
        let page = OsLayout::new(64 * SLICE, SLICE, 64).unwrap();
        let singleton = OsLayout::new(64 * SLICE, SLICE, 2).unwrap();

        assert_eq!(
            (page.metadata_size, singleton.metadata_size),
            (66 * PAGE_META_SIZE, 4 * PAGE_META_SIZE)
        );
    }

    #[test]
    fn os_metadata_cannot_overlap_data_or_overflow_the_prefix() {
        assert_eq!(
            (
                OsLayout::new(DEFAULT_ARENA, SLICE, DEFAULT_ARENA / SLICE),
                OsLayout::new(SLICE, SLICE, usize::MAX),
            ),
            (None, None)
        );
    }

    #[test]
    fn failed_arena_reservations_fall_back_to_direct_os_memory() {
        let pool = TestPool::new(0b11, 0);

        let memory = pool.allocate(SLICE, SLICE, 1).unwrap();

        assert_eq!(
            (memory.arena_location(), pool.0.count.load(Ordering::Acquire), pool.0.vm.calls()),
            (None, 0, (3, 2, [DEFAULT_ARENA, SMALL_ARENA, 2 * SLICE, 0]))
        );
    }

    #[test]
    fn exhausted_arena_and_os_reservations_return_no_memory() {
        let pool = TestPool::new(0b111, 0);

        let failed = pool.allocate(SLICE, SLICE, 1).is_none();

        assert_eq!(
            (failed, pool.0.vm.calls()),
            (true, (3, 0, [DEFAULT_ARENA, SMALL_ARENA, 2 * SLICE, 0]))
        );
    }

    #[test]
    fn an_os_metadata_commit_failure_releases_the_unpublished_mapping() {
        let pool = TestPool::new(0, 0b1);

        let failed = pool.allocate(SLICE, 2 * SLICE, 1).is_none();

        assert_eq!((failed, pool.0.vm.calls()), (true, (1, 1, [3 * SLICE, 0, 0, 0])));
    }

    #[test]
    fn an_os_data_commit_failure_releases_committed_metadata_too() {
        let pool = TestPool::new(0, 0b10);

        let failed = pool.allocate(SLICE, 2 * SLICE, 1).is_none();

        assert_eq!((failed, pool.0.vm.calls()), (true, (1, 2, [3 * SLICE, 0, 0, 0])));
    }

    #[test]
    fn a_failed_os_reservation_does_not_attempt_commitment() {
        let pool = TestPool::new(0b1, 0);

        let failed = pool.allocate(SLICE, 2 * SLICE, 1).is_none();

        assert_eq!((failed, pool.0.vm.calls()), (true, (1, 0, [3 * SLICE, 0, 0, 0])));
    }

    #[test]
    fn failed_initial_metadata_commitment_never_publishes_an_arena() {
        let pool = TestPool::new(0, 0b1);

        let failed = pool.0.create_arena(CHUNK, 0).is_none();

        assert_eq!(
            (
                failed,
                pool.0.count.load(Ordering::Acquire),
                pool.0.arenas[0].load(Ordering::Acquire),
                pool.0.vm.calls()
            ),
            (true, 0, 0, (1, 1, [CHUNK, 0, 0, 0]))
        );
    }

    #[test]
    fn an_arena_smaller_than_its_metadata_never_reaches_the_os() {
        let pool = TestPool::new(0, 0);

        let failed = pool.0.create_arena(SLICE, 0).is_none();

        assert_eq!((failed, pool.0.vm.calls()), (true, (0, 0, [0; 4])));
    }

    #[test]
    fn a_failed_large_reservation_retries_with_the_native_small_arena() {
        let pool = TestPool::new(0b1, 0);

        let memory = pool.allocate(SLICE, SLICE, 1).unwrap();

        assert_eq!(
            (memory.arena_location(), memory.is_zero(), pool.0.vm.calls()),
            (
                Some((0, arena_info_size().unwrap() / SLICE, SMALL_ARENA / SLICE)),
                true,
                (2, 2, [DEFAULT_ARENA, SMALL_ARENA, 0, 0])
            )
        );
    }

    #[test]
    fn a_small_arena_that_cannot_fit_the_request_is_not_attempted() {
        let pool = TestPool::new(usize::MAX, 0);

        let expanded = pool.0.expand(SMALL_ARENA, 0);

        assert_eq!((expanded, pool.0.vm.calls()), (false, (1, 0, [DEFAULT_ARENA, 0, 0, 0])));
    }

    #[test]
    fn oversized_arena_requests_do_not_reserve_or_publish() {
        let pool = TestPool::new(0, 0);

        let expanded = pool.0.expand(MAX_ARENA, 0);

        assert_eq!((expanded, pool.0.vm.calls()), (false, (0, 0, [0; 4])));
    }

    #[test]
    fn a_concurrent_publication_invalidates_the_old_reserve_snapshot() {
        let pool = TestPool::new(0, 0);
        pool.0.create_arena(CHUNK, 0).unwrap();

        let expanded = pool.0.expand(SLICE, 0);

        assert_eq!(
            (expanded, pool.0.count.load(Ordering::Acquire), pool.0.vm.calls()),
            (false, 1, (1, 1, [CHUNK, 0, 0, 0]))
        );
    }

    #[test]
    fn automatic_creation_keeps_the_native_arena_limit() {
        assert_eq!(
            (can_expand(MAX_ARENAS - 1, MAX_ARENAS - 1), can_expand(MAX_ARENAS, MAX_ARENAS)),
            (true, false)
        );
    }

    #[test]
    fn eager_arenas_do_not_recommit_their_data_on_first_use() {
        let mut pool = TestPool::new(0, 0);
        pool.0.policy.eager_commit = true;
        pool.0.create_arena(CHUNK, 0).unwrap();

        let memory = pool.own(pool.0.allocate(SLICE, 0).unwrap());

        assert_eq!((memory.is_zero(), pool.0.vm.calls()), (true, (1, 0, [CHUNK, 0, 0, 0])));
    }

    #[test]
    fn a_failed_data_commit_returns_the_claim_but_preserves_dirty_tracking() {
        let pool = TestPool::new(0, 0b10);
        pool.0.create_arena(CHUNK, 0).unwrap();
        let arena = pool.0.arena(0);
        let address = pool.0.arenas[0].load(Ordering::Acquire);

        let failed = arena.allocate(address, SLICE, 0, &pool.0).is_none();
        let memory = pool.own(arena.allocate(address, SLICE, 0, &pool.0).unwrap());

        assert_eq!(
            (failed, memory.is_zero(), pool.0.vm.calls()),
            (true, false, (1, 3, [CHUNK, 0, 0, 0]))
        );
    }

    #[test]
    fn failed_later_metadata_commit_schedules_successfully_committed_data() {
        let pool = TestPool::new(0, 0b100);
        pool.0.create_arena(META_ALIGNMENT + CHUNK, 0).unwrap();
        let address = pool.0.arenas[0].load(Ordering::Acquire);
        // SAFETY: This unpublished-to-workers fixture has no data owners.
        // The exclusive initialization step forces allocation into region two.
        let arena = unsafe { &mut *std::ptr::with_exposed_provenance_mut::<Arena>(address) };
        arena.bitmap.reserve_metadata(0, META_SLICES);

        let failed = arena.allocate(address, SLICE, 0, &pool.0).is_none();
        let scheduled = arena.expires.load(Ordering::Acquire) != 0 && pool.0.expires.load(Ordering::Acquire) != 0;
        let memory = pool.own(arena.allocate(address, SLICE, 0, &pool.0).unwrap());

        assert_eq!(
            (failed, scheduled, memory.is_zero(), pool.0.vm.calls()),
            (true, true, false, (1, 4, [META_ALIGNMENT + CHUNK, 0, 0, 0]))
        );
    }

    #[test]
    fn arena_growth_doubles_after_eight_entries_and_stops_at_maximum() {
        assert_eq!(
            (
                arena_size(0, 65536),
                arena_size(8, 65536),
                arena_size(128, 65536),
                arena_size(129, 65536)
            ),
            (Some(DEFAULT_ARENA), Some(2 * DEFAULT_ARENA), Some(MAX_ARENA), Some(DEFAULT_ARENA))
        );
    }

    #[test]
    fn singleton_reserve_leaves_upstream_chunk_slack() {
        assert_eq!(arena_size(0, DEFAULT_ARENA), Some(DEFAULT_ARENA + CHUNK));
    }

    #[test]
    fn arena_rotation_keeps_the_last_arena_as_fallback() {
        assert_eq!(arena_order(5, 2).collect::<Vec<_>>(), [2, 3, 0, 1, 4]);
    }

    #[test]
    fn single_arena_search_has_no_rotation_division() {
        assert_eq!(arena_order(1, 99).collect::<Vec<_>>(), [0]);
    }

    #[test]
    fn empty_arena_search_has_no_rotation_division() {
        assert_eq!(arena_order(0, usize::MAX).collect::<Vec<_>>(), []);
    }

    #[test]
    fn ordinary_os_pages_commit_every_metadata_slot() {
        let layout = OsLayout::new(64 * SLICE, SLICE, 64).unwrap();

        assert_eq!(
            layout,
            OsLayout {
                data_offset: SLICE,
                metadata_size: 66 * PAGE_META_SIZE,
                reservation_size: 65 * SLICE,
            }
        );
    }

    #[test]
    fn huge_os_pages_need_only_two_metadata_slots_after_the_prefix() {
        let layout = OsLayout::new(DEFAULT_ARENA, SLICE, 2).unwrap();

        assert_eq!(
            layout,
            OsLayout {
                data_offset: SLICE,
                metadata_size: 4 * PAGE_META_SIZE,
                reservation_size: DEFAULT_ARENA + SLICE,
            }
        );
    }

    #[test]
    fn os_alignment_moves_data_without_losing_its_metadata_table() {
        let layout = OsLayout::new(SLICE, 32 * SLICE, 1).unwrap();

        assert_eq!(
            layout,
            OsLayout {
                data_offset: 32 * SLICE,
                metadata_size: 34 * PAGE_META_SIZE,
                reservation_size: 33 * SLICE,
            }
        );
    }

    #[test]
    fn os_alignment_cannot_select_the_next_metadata_region() {
        assert_eq!(OsLayout::new(SLICE, META_ALIGNMENT, 1), None);
    }

    #[test]
    fn os_metadata_prefix_cannot_overflow_the_reservation() {
        assert_eq!(OsLayout::new(usize::MAX, SLICE, 2), None);
    }

    #[test]
    fn initial_arena_metadata_fits_before_the_first_data_chunk() {
        let info = arena_info_size().unwrap();

        assert_eq!(
            (info >= META_SIZE + std::mem::size_of::<super::Arena>(), info < CHUNK, info % SLICE),
            (true, true, 0)
        );
    }

    #[test]
    fn cooperative_collection_waits_until_the_exact_deadline() {
        assert_eq!((purge_due(4000, 3999, false), purge_due(4000, 4000, false)), (false, true));
    }

    #[test]
    fn forced_collection_ignores_only_scheduled_deadlines() {
        assert_eq!((purge_due(4000, 0, true), purge_due(0, 0, true)), (true, false));
    }

    #[test]
    fn cooperative_collection_limits_arenas_but_force_visits_all() {
        assert_eq!((purge_budget(32, false), purge_budget(32, true)), (9, 32));
    }

    #[test]
    fn an_early_forced_scan_shortens_the_next_global_deadline() {
        assert_eq!(retained_deadline(8000, 1000), 1400);
    }

    #[test]
    fn a_completed_scan_keeps_an_expired_global_deadline_for_the_next_pass() {
        assert_eq!(retained_deadline(4000, 5000), 4000);
    }

    #[test]
    fn os_backing_commits_both_metadata_and_the_aligned_data_range() {
        let memory = Memory::allocate_page(64 * SLICE, 2 * SLICE, 64).unwrap();
        let metadata = memory.metadata_ptr();
        // SAFETY: Ordinary OS pages commit every complete metadata slot.
        let last_metadata = unsafe { metadata.add(63 * PAGE_META_SIZE) };
        // SAFETY: This Memory uniquely owns the complete committed data range.
        let last_data = unsafe { memory.as_ptr().add(memory.len() - 1) };
        // SAFETY: The first byte is inside this fresh, committed reservation.
        let first_zero = unsafe { memory.as_ptr().read() };
        // SAFETY: The last byte is inside the same committed data range.
        let last_zero = unsafe { last_data.read() };
        // SAFETY: The first and last complete metadata slots are committed and
        // separate from data. No Page objects have been initialized in them.
        unsafe { last_metadata.write_bytes(0x5D, PAGE_META_SIZE) };
        // SAFETY: There are no concurrent users of the owned data range.
        unsafe { memory.as_ptr().write(0xA7) };
        // SAFETY: last_data is within the same uniquely owned data range.
        unsafe { last_data.write(0xC1) };
        // SAFETY: The final byte is within the last complete metadata slot.
        let last_metadata_byte = unsafe { last_metadata.add(PAGE_META_SIZE - 1) };
        // SAFETY: The final metadata byte remains in the committed slot.
        let last_metadata_byte = unsafe { last_metadata_byte.read() };
        // SAFETY: The earlier writes initialized these owned data bytes.
        let first_data = unsafe { memory.as_ptr().read() };
        // SAFETY: The last data byte was initialized above and remains owned.
        let last_data = unsafe { last_data.read() };

        assert_eq!(
            (
                memory.arena_location(),
                memory.as_ptr().addr() % (2 * SLICE),
                metadata.addr() - (memory.as_ptr().addr() & !(META_ALIGNMENT - 1)),
                memory.is_zero() && first_zero == 0 && last_zero == 0,
                last_metadata_byte,
                (first_data, last_data),
            ),
            (None, 0, 2 * PAGE_META_SIZE, true, 0x5D, (0xA7, 0xC1))
        );
    }

    #[test]
    fn later_region_metadata_stays_committed_through_data_purge_and_reuse() {
        let operations = TestPool::new(0, 0);
        let reservation = hal::reserve(META_ALIGNMENT + CHUNK, META_ALIGNMENT).unwrap();
        let slices = reservation.len() / SLICE;
        let mut bitmap = Bitmap::zeroed();
        bitmap.initialize(slices, false);
        // Simulate a full first region without committing or touching its data.
        bitmap.reserve_metadata(0, META_SLICES);
        bitmap.reserve_metadata(META_SLICES, META_SIZE / SLICE);
        let arena = Arena {
            reservation,
            bitmap,
            expires: AtomicU64::new(0),
            index: 0,
        };
        let arena_address = std::ptr::from_ref(&arena).expose_provenance();
        let memory = ManuallyDrop::new(arena.allocate(arena_address, SLICE, 0, &operations.0).unwrap());
        let location = memory.arena_location();
        let metadata = memory.metadata_ptr();
        // SAFETY: Allocation committed the complete later-region metadata slot.
        let last_metadata = unsafe { metadata.add(PAGE_META_SIZE - 1) };
        // SAFETY: No Page exists in this private, unshared test arena.
        unsafe { metadata.write(0xB3) };
        // SAFETY: This final metadata byte is in the same committed slot.
        unsafe { last_metadata.write(0xD5) };
        let slice = META_SLICES + META_SIZE / SLICE;
        // SAFETY: This transfers the ManuallyDrop Memory's exclusive data claim
        // back to the bitmap. Its destructor must not release that claim again.
        let claim = unsafe { arena.bitmap.reclaim(slice, 1) };
        claim.schedule_purge();
        drop(claim);
        arena.expires.store(4000, Ordering::Release);

        let early = arena.purge(3999, 1, false);
        let purged = arena.purge(4000, 1, true);
        let reused = ManuallyDrop::new(arena.allocate(arena_address, SLICE, 0, &operations.0).unwrap());
        // SAFETY: Metadata is permanently reserved and was not a purge target.
        let first = unsafe { metadata.read() };
        // SAFETY: The entire metadata slot stays committed through data reuse.
        let last = unsafe { last_metadata.read() };
        let reused_location = reused.arena_location();
        let reused_zero = reused.is_zero();
        // SAFETY: The reused ManuallyDrop Memory transfers its sole data claim.
        let claim = unsafe { arena.bitmap.reclaim(slice, 1) };
        drop(claim);

        assert_eq!(
            (location, early, purged, reused_location, reused_zero, first, last),
            (
                Some((0, slice, slices)),
                PurgeState::Scheduled,
                PurgeState::Purged,
                Some((0, slice, slices)),
                false,
                0xB3,
                0xD5,
            )
        );
    }

    #[test]
    fn concurrent_purge_notifications_keep_the_earliest_deadline() {
        let expiration = AtomicU64::new(0);
        std::thread::scope(|scope| {
            scope.spawn(|| record_expiration(&expiration, 8000));
            scope.spawn(|| record_expiration(&expiration, 4000));
        });

        assert_eq!(expiration.load(Ordering::Acquire), 4000);
    }
}
