// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Medium backing is shared by logical domain, not by heap lifetime.
//!
//! Sixteen fixed shards group heaps into four NUMA buckets with four
//! contention lanes each. A domain-local ticket balances initial heap assignment
//! within the sampled NUMA bucket instead of depending on startup CPU numbering.
//! Selection is a best-effort placement hint, never a
//! binding policy. A heap selects once; migrated threads remain safe, and frees
//! route through immutable region identity rather than the freeing processor.
//! Metadata does not grow with transient threads.
//!
//! Heap-local batches hold fresh **and** recycled spans, with geometrically
//! increasing refill sizes (1..16) and a combined 1 MiB committed-byte ceiling.
//! As in the local-cache policy, only power-of-two slice counts are batched.
//! A refill reserves one contiguous fresh extent, not N shared transactions.
//! A bounded shared mailbox accepts remote frees through four independently
//! locked, cache-line-separated publication lanes. They divide the original
//! 16-entry / 1 MiB budgets: each packet flushes at four spans or 256 KiB.
//! Full packets, consumers, and maintenance move spans into fixed-class bins
//! with one demand/budget transaction per packet.
//! No mailbox lock is held during that transaction or OS work. Pending packets
//! deliberately do not point at the allocation's heap:
//! existing retirement guards cover unregistering, after which the shared inbox
//! owns the span. Thus an idle or exited allocating worker cannot strand remote
//! frees. Publication takes one short mailbox lock per remote free, rather than
//! introducing an ABA-sensitive lock-free reclamation scheme.
//! Local cache overflow evicts the bounded cache as one mixed-class packet and
//! keeps the incoming span locally. Thus local bursts amortize shared locking in
//! both directions without enlarging the 1 MiB heap budget. Remote publication
//! remains immediate and individually synchronized.
//!
//! Free-slice regions have an intrusive availability list. Searches inspect at
//! most eight candidates and skip occupied bitmap words. Fragmentation may cause
//! a new reservation before every older region has been searched. Bounded scans
//! trade some virtual address space for predictable critical sections.
//!
//! Used bits also represent in-flight OS operations. Reserve and commit occur
//! without a spin lock; failed commits return the reservation. Purge detaches
//! spans under the lock, decommits outside it, then releases bits. On decommit
//! failure an inaccessible span is quarantined unless recommit restores it.
//! Shared caches retain recent demand through the purge-delay window, rather
//! than eagerly decommitting each free above a fixed small cap. Demand peaks
//! decay over quiet windows; inactive caches target a 16 MiB floor. Shards obtain
//! demand-sized credits from one allocator-wide pool, in 8 MiB units. Grants
//! survive short reuse cycles and return to the pool when demand and cached bytes
//! shrink. The pool reserves half the sampled available headroom and never grants
//! more than half of effective memory; it does not preallocate that capacity.
//! Low available headroom targets zero. These are policy targets, not strict RSS
//! ceilings: bounded work, concurrent returns, and OS failures permit overshoot.
//!
//! Maintenance visits up to eight nonempty classes and eight regions, detaching
//! up to 64 spans / 4 MiB (one larger span is permitted). Adjacent spans within
//! the same region are decommitted together outside locks. Original descriptors
//! remain available for rollback. Local heap retirement uses the same coalescing.
//! Every 256 medium allocation attempts rotates maintenance across domain shards,
//! including cache misses and spans above the local byte budget. Thus another
//! active worker can trim an inactive owner's cache and return its unused credits;
//! a heap visits all sixteen shards within 4096 medium attempts. A completely idle process
//! retains bounded cached memory until subsequent medium activity; no background
//! timer or allocation-reentrant topology/memory library is introduced.

use super::*;
#[cfg(all(test, not(miri)))]
mod policy_tests;
mod retention;
use retention::{CreditLease, Demand, MemoryBudget};

#[derive(Clone, Copy)]
enum BudgetSource {
    Shared(MemoryBudget),
    #[cfg(test)]
    Fixed(MemoryBudget),
}

impl BudgetSource {
    fn resolve(self, state: &mut MediumState, now: u64) -> MemoryBudget {
        match self {
            Self::Shared(global) => {
                let desired = if state.retained_bytes == 0 && state.demand.quiet(now) {
                    0
                } else {
                    state.demand.target(
                        now,
                        MemoryBudget {
                            limit: usize::MAX,
                            pressured: global.pressured,
                        },
                    )
                };
                state.credit.budget(desired, state.retained_bytes, now, global)
            }
            #[cfg(test)]
            Self::Fixed(budget) => budget,
        }
    }
}

pub(super) const SHARD_COUNT: usize = 16;
const REGION_SCAN_LIMIT: usize = 8;
pub(super) const BATCH_CAPACITY: usize = 16;
pub(super) const LOCAL_CACHE_BYTES: usize = 1024 * 1024;
pub(super) const SHARED_CACHE_BYTES: usize = 16 * 1024 * 1024;
const PURGE_WORK: usize = 64;
const PURGE_BYTES: usize = 4 * 1024 * 1024;
const PURGE_SCAN: usize = 8;
const MAINTENANCE_INTERVAL: usize = 256;
const REMOTE_STRIPES: usize = 4;
const REMOTE_CAPACITY: usize = BATCH_CAPACITY / REMOTE_STRIPES;
const REMOTE_BYTES: usize = LOCAL_CACHE_BYTES / REMOTE_STRIPES;

/// Publication concurrency is independent of backing-shard count. Quotas are
/// divided, not multiplied: all lanes together keep the original 16-entry and
/// 1 MiB flush budgets. One arriving larger span can exceed its byte threshold,
/// but is detached immediately, just as with the original single mailbox.
pub(super) struct RemoteMailbox {
    lanes: [RemoteLane; REMOTE_STRIPES],
}

#[repr(align(64))]
struct RemoteLane {
    pending: SpinLock<RemoteSpans>,
    ready: AtomicBool,
}

impl RemoteMailbox {
    pub(super) const fn new() -> Self {
        Self {
            lanes: [const {
                RemoteLane {
                    pending: SpinLock::new(RemoteSpans::new()),
                    ready: AtomicBool::new(false),
                }
            }; REMOTE_STRIPES],
        }
    }

    fn lane(address: *mut u8) -> usize {
        let slices = address.addr() / MEDIUM_SLICE_SIZE;
        // Fold slice-index digits so power-of-two span strides do not all
        // collide on the low address bits. No freeing-thread state is needed.
        let folded = slices ^ (slices >> 8);
        let folded = folded ^ (folded >> 4);
        (folded ^ (folded >> 2)) & (REMOTE_STRIPES - 1)
    }

    #[cfg(test)]
    fn pending(&self) -> (usize, usize) {
        self.lanes.iter().fold((0, 0), |(count, bytes), lane| {
            let pending = lane.pending.lock();
            (count + pending.count, bytes + pending.bytes)
        })
    }
}

/// A bounded, shared transfer packet, not a cache owned by the freeing thread.
/// Its short lock protects only publication; demand and retention accounting
/// happen once per packet under the normal shard lock, after releasing this one.
pub(super) struct RemoteSpans {
    spans: [(*mut u8, usize); REMOTE_CAPACITY],
    count: usize,
    bytes: usize,
    delay: u64,
}

// SAFETY: a packet exclusively owns its still-reserved committed spans. Moving
// it transfers ownership; it contains no references to an allocating heap.
unsafe impl Send for RemoteSpans {}

impl RemoteSpans {
    pub(super) const fn new() -> Self {
        Self {
            spans: [(ptr::null_mut(), 0); REMOTE_CAPACITY],
            count: 0,
            bytes: 0,
            delay: 0,
        }
    }
}

pub(super) struct MediumState {
    pub(super) regions: *mut RegionState,
    pub(super) last_region: *mut RegionState,
    available: *mut RegionState,
    pub(super) bins: [MediumBin; MEDIUM_MAX_SLICES],
    nonempty_bins: [u64; MEDIUM_MAX_SLICES.div_ceil(64)],
    retained_bytes: usize,
    purge_class: usize,
    purge_region: *mut RegionState,
    demand: Demand,
    credit: CreditLease,
    maintaining: bool,
    returns_since_maintenance: usize,
    #[cfg(test)]
    return_batches: usize,
}

impl MediumState {
    fn detach_bins(&mut self, pending: &mut [(*mut u8, usize); PURGE_WORK], force: bool, ignore_age: bool, now: u64) -> (usize, usize) {
        let mut count = 0;
        let mut bytes = 0;
        let visits = if force { MEDIUM_MAX_SLICES } else { PURGE_SCAN };
        for _ in 0..visits {
            let Some(class) = self.next_bin(self.purge_class) else { break };
            self.purge_class = (class + 1) % MEDIUM_MAX_SLICES;
            let bin = &mut self.bins[class];
            if !force && !ignore_age && (bin.purge_after == 0 || bin.purge_after > now) {
                continue;
            }
            while !bin.free_list.is_null() && count < PURGE_WORK && bytes < PURGE_BYTES {
                if count != 0 && bytes + (class + 1) * MEDIUM_SLICE_SIZE > PURGE_BYTES {
                    break;
                }
                let block = bin.free_list;
                // SAFETY: caller's shard lock exclusively owns this committed list.
                bin.free_list = unsafe { (*block).next };
                pending[count] = (block.cast(), class + 1);
                count += 1;
                bytes += (class + 1) * MEDIUM_SLICE_SIZE;
            }
            if bin.free_list.is_null() {
                bin.purge_after = 0;
                self.nonempty_bins[class / 64] &= !(1 << (class % 64));
            }
            if count == PURGE_WORK || bytes >= PURGE_BYTES {
                break;
            }
        }
        (count, bytes)
    }

    fn next_bin(&self, start: usize) -> Option<usize> {
        let search = |first: usize, end: usize| {
            for word_index in first / 64..end.div_ceil(64) {
                let low = first.saturating_sub(word_index * 64);
                let high = (end - word_index * 64).min(64);
                let mask = (u64::MAX << low) & (u64::MAX >> (64 - high));
                let bits = self.nonempty_bins[word_index] & mask;
                if bits != 0 {
                    return Some(word_index * 64 + bits.trailing_zeros() as usize);
                }
            }
            None
        };
        search(start, MEDIUM_MAX_SLICES).or_else(|| search(0, start))
    }

    pub(super) const fn new() -> Self {
        Self {
            regions: ptr::null_mut(),
            last_region: ptr::null_mut(),
            available: ptr::null_mut(),
            bins: [const { MediumBin::new() }; MEDIUM_MAX_SLICES],
            nonempty_bins: [0; MEDIUM_MAX_SLICES.div_ceil(64)],
            retained_bytes: 0,
            purge_class: 0,
            purge_region: ptr::null_mut(),
            demand: Demand::new(),
            credit: CreditLease::new(),
            maintaining: false,
            returns_since_maintenance: 0,
            #[cfg(test)]
            return_batches: 0,
        }
    }
}

impl Drop for MediumState {
    fn drop(&mut self) {
        self.credit.release_unused(0);
    }
}

pub(super) struct MediumCache {
    blocks: [[*mut u8; BATCH_CAPACITY]; LOCAL_MEDIUM_CLASSES],
    lengths: [usize; LOCAL_MEDIUM_CLASSES],
    targets: [usize; LOCAL_MEDIUM_CLASSES],
    bytes: usize,
    maintenance: usize,
}

impl MediumCache {
    pub(super) const fn new() -> Self {
        Self {
            blocks: [[ptr::null_mut(); BATCH_CAPACITY]; LOCAL_MEDIUM_CLASSES],
            lengths: [0; LOCAL_MEDIUM_CLASSES],
            targets: [1; LOCAL_MEDIUM_CLASSES],
            bytes: 0,
            maintenance: 0,
        }
    }

    pub(super) fn pop(&mut self, class: usize) -> Option<*mut u8> {
        let length = &mut self.lengths[class];
        if *length == 0 {
            return None;
        }

        *length -= 1;
        self.bytes -= MEDIUM_SLICE_SIZE << class;
        Some(mem::replace(&mut self.blocks[class][*length], ptr::null_mut()))
    }

    pub(super) fn drain(&mut self, output: &mut [(*mut u8, usize); BATCH_CAPACITY]) -> usize {
        let mut count = 0;
        for class in 0..LOCAL_MEDIUM_CLASSES {
            while let Some(address) = self.pop(class) {
                // The combined 1 MiB budget and 64 KiB minimum span bound the
                // total entry count, including mixed-class eviction packets.
                output[count] = (address, 1 << class);
                count += 1;
            }
        }
        count
    }

    pub(super) fn maintenance_shard(&mut self) -> Option<usize> {
        self.maintenance = self.maintenance.wrapping_add(1);
        self.maintenance
            .is_multiple_of(MAINTENANCE_INTERVAL)
            .then_some((self.maintenance / MAINTENANCE_INTERVAL) % SHARD_COUNT)
    }

    pub(super) fn push(&mut self, class: usize, address: *mut u8) -> bool {
        let bytes = MEDIUM_SLICE_SIZE << class;
        let length = &mut self.lengths[class];
        if *length == BATCH_CAPACITY || self.bytes + bytes > LOCAL_CACHE_BYTES {
            self.targets[class] = 1;
            return false;
        }
        self.blocks[class][*length] = address;
        *length += 1;
        self.bytes += bytes;
        true
    }

    pub(super) fn refill_count(&mut self, class: usize) -> usize {
        let bytes = MEDIUM_SLICE_SIZE << class;
        // The first result is immediately live, so only the remainder consumes
        // cache budget. A class larger than the total budget is never prefetched.
        let count = self.targets[class].min(1 + (LOCAL_CACHE_BYTES - self.bytes) / bytes);
        self.targets[class] = (self.targets[class] * 2).min(BATCH_CAPACITY);
        count
    }
}

pub(super) fn shard_index(ticket: usize, node: usize) -> usize {
    (node % (SHARD_COUNT / 4)) * 4 + ticket % 4
}

unsafe fn balanced_shard(domain: *mut DomainState, node: usize) -> usize {
    // SAFETY: the caller retains the domain. One cold-path atomic per heap
    // distributes startup bursts even when the scheduler initially bunches CPUs.
    let ticket = unsafe { (*domain).medium_lanes[node % (SHARD_COUNT / 4)].fetch_add(1, Ordering::Relaxed) };
    shard_index(ticket, node)
}

pub(super) fn heap_regions(heap: &mut ReusableHeapState) -> &'static MediumRegion {
    if heap.medium_shard.is_null() {
        let (_, node) = hal::current_processor_location();
        // Non-default unit-test domains use their primary shard so synthetic
        // region fixtures remain deterministic; explicit shard tests bypass this.
        #[cfg(test)]
        let index = if unsafe { (*heap.domain).is_default.load(Ordering::Relaxed) } {
            unsafe { balanced_shard(heap.domain, node) }
        } else {
            0
        };
        #[cfg(not(test))]
        let index = unsafe { balanced_shard(heap.domain, node) };
        // SAFETY: heaps retain their logical domain for their whole lifetime.
        heap.medium_shard = unsafe { domain_shard(heap.domain, index) };
    }
    // SAFETY: all shards are embedded in process-retained domain storage.
    unsafe { &*heap.medium_shard }
}

pub(super) unsafe fn domain_shard(domain: *mut DomainState, index: usize) -> &'static MediumRegion {
    // SAFETY: caller supplies a live domain and bounded index.
    unsafe {
        if index == 0 {
            &(*domain).regions
        } else {
            &(*domain).medium_shards[index - 1]
        }
    }
}

pub(super) unsafe fn region_backing(region: *mut RegionState) -> &'static MediumRegion {
    // SAFETY: region publication initializes this immutable, process-lived pointer.
    unsafe { &*(*region).backing }
}

pub(super) unsafe fn add_available(state: &mut MediumState, region: *mut RegionState) {
    // SAFETY: caller owns the shard lock and region is not already linked.
    unsafe {
        (*region).available_previous = ptr::null_mut();
        (*region).available_next = state.available;
        if !state.available.is_null() {
            (*state.available).available_previous = region;
        }
        state.available = region;
    }
}

unsafe fn remove_available(state: &mut MediumState, region: *mut RegionState) {
    // SAFETY: caller owns the shard lock and region is linked.
    unsafe {
        let previous = (*region).available_previous;
        let next = (*region).available_next;
        if previous.is_null() {
            state.available = next;
        } else {
            (*previous).available_next = next;
        }
        if !next.is_null() {
            (*next).available_previous = previous;
        }
        (*region).available_next = ptr::null_mut();
        (*region).available_previous = ptr::null_mut();
    }
}

pub(super) unsafe fn release_availability(state: &mut MediumState, region: *mut RegionState, count: usize) {
    // SAFETY: caller owns the shard lock, and has released exactly count used bits.
    unsafe {
        if (*region).free_slices == 0 {
            add_available(state, region);
        }
        (*region).free_slices += count;
        debug_assert!((*region).free_slices <= MEDIUM_REGION_SLICE_COUNT);
    }
}

pub(super) unsafe fn reserve_existing(state: &mut MediumState, count: usize) -> Option<(*mut u8, *mut RegionState)> {
    let mut region = state.available;
    for _ in 0..REGION_SCAN_LIMIT {
        if region.is_null() {
            break;
        }
        // SAFETY: caller owns the shard lock; only linked live regions are visited.
        unsafe {
            if (*region).free_slices >= count
                && let Some(index) = find_free_slices(&(*region).used, (*region).next_slice, count)
            {
                mark_slices(&mut (*region).used, index, count, true);
                (*region).next_slice = (index + count) % MEDIUM_REGION_SLICE_COUNT;
                (*region).free_slices -= count;
                if (*region).free_slices == 0 {
                    remove_available(state, region);
                }
                return Some(((*region).base.add(index * MEDIUM_SLICE_SIZE), region));
            }
            region = (*region).available_next;
        }
    }
    None
}

impl MediumRegion {
    pub(super) fn record_fresh(&self, bytes: usize) {
        let now = hal::monotonic_millis();
        self.state.lock().demand.acquire(bytes, now);
    }

    pub(super) fn reserve_slices(&self, domain: *mut DomainState, count: usize) -> Option<(*mut u8, *mut RegionState)> {
        if count == 0 || count > MEDIUM_REGION_SLICE_COUNT {
            return None;
        }
        {
            let mut state = self.state.lock();
            // SAFETY: exclusive shard metadata access.
            if let Some(reserved) = unsafe { reserve_existing(&mut state, count) } {
                return Some(reserved);
            }
        }
        // No spin lock is held while the OS reserves address space or maps metadata.
        let region = unsafe { create_region(domain, self) }?;
        let mut state = self.state.lock();
        // Another creator may have supplied capacity while the OS call ran.
        // Do not process-retain one mostly empty region per racing worker.
        if let Some(reserved) = unsafe { reserve_existing(&mut state, count) } {
            drop(state);
            // SAFETY: region was never published and remains exclusively owned.
            unsafe {
                hal::unmap((*region).base, MEDIUM_REGION_SIZE);
                hal::unmap(region.cast(), size_of::<RegionState>());
            }
            return Some(reserved);
        }
        // SAFETY: exclusively owned unpublished region becomes shard-owned here.
        unsafe {
            publish_region(&mut state, &self.regions, region);
            reserve_existing(&mut state, count)
        }
    }

    pub(super) fn take_batch(&self, slices: usize, result: &mut [*mut u8]) -> usize {
        self.flush_remote();
        let now = hal::monotonic_millis();
        let mut state = self.state.lock();
        if slices <= MEDIUM_MAX_SLICES {
            let bin = &mut state.bins[slices - 1];
            let mut count = 0;
            for output in result {
                let block = bin.free_list;
                if block.is_null() {
                    break;
                }
                // SAFETY: bin owns committed free blocks while the shard lock is held.
                bin.free_list = unsafe { (*block).next };
                *output = block.cast();
                count += 1;
            }
            if bin.free_list.is_null() {
                bin.purge_after = 0;
                state.nonempty_bins[(slices - 1) / 64] &= !(1 << ((slices - 1) % 64));
            }
            let bytes = count * slices * MEDIUM_SLICE_SIZE;
            state.retained_bytes -= bytes;
            state.demand.acquire(bytes, now);
            count
        } else {
            let mut region = state.last_region;
            for _ in 0..REGION_SCAN_LIMIT {
                if region.is_null() {
                    break;
                }
                // SAFETY: shared extent lists are protected by this shard lock.
                if let Some(address) = unsafe { take_large_extent(region, slices) } {
                    result[0] = address;
                    state.retained_bytes -= slices * MEDIUM_SLICE_SIZE;
                    state.demand.acquire(slices * MEDIUM_SLICE_SIZE, now);
                    return 1;
                }
                region = if ptr::eq(region, state.last_region) {
                    state.regions
                } else {
                    unsafe { (*region).next.load(Ordering::Relaxed) }
                };
            }
            0
        }
    }

    pub(super) unsafe fn return_span(&self, address: *mut u8, slices: usize, delay: u64) {
        // SAFETY: the caller transfers one committed, unregistered span.
        unsafe { self.return_batch(&[(address, slices)], delay) };
    }

    pub(super) unsafe fn return_remote_span(&self, address: *mut u8, slices: usize, delay: u64) {
        let lane = &self.remote.lanes[RemoteMailbox::lane(address)];
        let packet = {
            let mut remote = lane.pending.lock();
            let index = remote.count;
            remote.spans[index] = (address, slices);
            remote.count += 1;
            remote.bytes += slices * MEDIUM_SLICE_SIZE;
            remote.delay = remote.delay.max(delay);
            if remote.count == REMOTE_CAPACITY || remote.bytes >= REMOTE_BYTES {
                lane.ready.store(false, Ordering::Release);
                Some(mem::replace(&mut *remote, RemoteSpans::new()))
            } else {
                if remote.count == 1 {
                    lane.ready.store(true, Ordering::Release);
                }
                None
            }
        };
        if let Some(packet) = packet {
            // SAFETY: the mailbox transferred unique ownership of every entry.
            // The mailbox lock is no longer held during accounting or OS work.
            unsafe { self.return_batch(&packet.spans[..packet.count], packet.delay) };
        }
    }

    fn flush_remote(&self) {
        for lane in &self.remote.lanes {
            if !lane.ready.load(Ordering::Acquire) {
                continue;
            }
            let packet = {
                let mut remote = lane.pending.lock();
                if remote.count == 0 {
                    continue;
                }
                lane.ready.store(false, Ordering::Release);
                mem::replace(&mut *remote, RemoteSpans::new())
            };
            // SAFETY: detached entries remain committed and reserved. No lane
            // lock is held during backing accounting or OS work. Shared-domain
            // storage outlives all heaps, permitting independent helper drains.
            unsafe { self.return_batch(&packet.spans[..packet.count], packet.delay) };
        }
    }

    pub(super) unsafe fn return_batch(&self, spans: &[(*mut u8, usize)], delay: u64) {
        let now = hal::monotonic_millis();
        let budget = MemoryBudget::sample(now);
        // SAFETY: caller transfers exclusively owned spans from this shard.
        unsafe { self.return_batch_with_policy(spans, delay, now, BudgetSource::Shared(budget)) };
    }

    #[cfg(all(test, not(miri)))]
    unsafe fn return_span_with_budget(&self, address: *mut u8, slices: usize, delay: u64, now: u64, budget: MemoryBudget) {
        unsafe { self.return_batch_with_policy(&[(address, slices)], delay, now, BudgetSource::Fixed(budget)) };
    }

    unsafe fn return_batch_with_policy(&self, spans: &[(*mut u8, usize)], delay: u64, now: u64, policy: BudgetSource) {
        let bytes = spans.iter().map(|(_, slices)| slices * MEDIUM_SLICE_SIZE).sum();
        let maintain = {
            let mut state = self.state.lock();
            state.demand.release(bytes, now, delay);
            for &(address, slices) in spans {
                // SAFETY: caller transfers exclusive ownership; all entries have
                // already been unregistered and belong to this shard.
                unsafe { insert_cached_span(&mut state, address, slices, now.saturating_add(delay)) };
            }
            state.returns_since_maintenance += spans.len();
            #[cfg(test)]
            {
                state.return_batches += 1;
            }
            let budget = policy.resolve(&mut state, now);
            let target = state.demand.target(now, budget);
            !state.maintaining
                && state.retained_bytes > target
                && (state.returns_since_maintenance >= PURGE_WORK || state.retained_bytes > target.saturating_add(PURGE_BYTES))
        };
        if maintain {
            self.purge_with_policy(false, now, policy);
        }
    }

    #[cfg(test)]
    pub(super) unsafe fn decommit_span(&self, address: *mut u8, slices: usize, retry_after: u64) -> bool {
        let bytes = slices * MEDIUM_SLICE_SIZE;
        self.state.lock().demand.retire(bytes);
        // SAFETY: the span has been removed from every cache; used bits prevent
        // allocation of overlapping storage while this OS operation is in flight.
        if unsafe { hal::decommit(address, bytes) } {
            unsafe { self.release_slices(address, slices) };
            tracking::record_unmapping(bytes);
            record_medium_event(MediumEventKind::PurgedSpan, 1);
            return true;
        } else if unsafe { hal::commit(address, bytes) } {
            // Restore access before writing links, including on platforms where
            // decommit can fail after changing page protection.
            let mut state = self.state.lock();
            unsafe { insert_cached_span(&mut state, address, slices, retry_after) };
        }
        // If both operations fail, retain the used bits and quarantine the span.
        // Reusing potentially inaccessible pages would be unsound.
        false
    }

    pub(super) fn purge(&self, force: bool, now: u64) {
        self.flush_remote();
        self.purge_with_policy(force, now, BudgetSource::Shared(MemoryBudget::sample(now)));
    }

    #[cfg(test)]
    fn purge_with_budget(&self, force: bool, now: u64, budget: MemoryBudget) {
        self.purge_with_policy(force, now, BudgetSource::Fixed(budget));
    }

    #[cfg(test)]
    pub(super) fn purge_with_unlimited_budget(&self, force: bool, now: u64) {
        self.purge_with_budget(
            force,
            now,
            MemoryBudget {
                limit: usize::MAX,
                pressured: false,
            },
        );
    }

    fn purge_with_policy(&self, force: bool, now: u64, policy: BudgetSource) {
        let mut pending = [(ptr::null_mut::<u8>(), 0_usize); PURGE_WORK];
        {
            let mut state = self.state.lock();
            if state.maintaining {
                return;
            }
            state.maintaining = true;
            state.returns_since_maintenance = 0;
        }
        let mut remaining = if force {
            self.state.lock().retained_bytes / MEDIUM_SLICE_SIZE
        } else {
            PURGE_WORK
        };
        let mut first_pass = true;
        loop {
            let mut count;
            let mut bytes;
            {
                let mut state = self.state.lock();
                let budget = policy.resolve(&mut state, now);
                let target = if force { 0 } else { state.demand.target(now, budget) };
                if state.retained_bytes <= target {
                    state.maintaining = false;
                    if force {
                        state.credit.release_unused(0);
                    }
                    return;
                }
                if force && first_pass {
                    state.purge_class = 0;
                    state.purge_region = state.regions;
                }
                first_pass = false;
                // A memory-budget limit can equal the idle floor. Excess backing
                // must still be reclaimed even when continuing returns refresh bin age.
                let ignore_age = budget.pressured || state.retained_bytes > budget.limit || target != SHARED_CACHE_BYTES;
                (count, bytes) = state.detach_bins(&mut pending, force, ignore_age, now);
                if state.purge_region.is_null() {
                    state.purge_region = state.regions;
                }
                let mut visits = 0;
                while !state.purge_region.is_null() && count < PURGE_WORK && bytes < PURGE_BYTES && (force || visits < PURGE_SCAN) {
                    let region = state.purge_region;
                    let mut byte_limited = false;
                    // SAFETY: extent links are accessed exclusively under the shard lock.
                    unsafe {
                        if force || ignore_age || ((*region).large_purge_after != 0 && (*region).large_purge_after <= now) {
                            while !(*region).large_free.is_null() && count < PURGE_WORK && bytes < PURGE_BYTES {
                                let block = (*region).large_free;
                                if count != 0 && bytes + (*block).slice_count * MEDIUM_SLICE_SIZE > PURGE_BYTES {
                                    byte_limited = true;
                                    break;
                                }
                                (*region).large_free = (*block).next;
                                pending[count] = (block.cast(), (*block).slice_count);
                                count += 1;
                                bytes += (*block).slice_count * MEDIUM_SLICE_SIZE;
                            }
                        }
                        if (*region).large_free.is_null() {
                            (*region).large_purge_after = 0;
                        }
                        if !byte_limited && count < PURGE_WORK && bytes < PURGE_BYTES {
                            state.purge_region = (*region).next.load(Ordering::Relaxed);
                        }
                    }
                    visits += 1;
                    if byte_limited {
                        break;
                    }
                }
                for &(_, slices) in &pending[..count] {
                    state.retained_bytes -= slices * MEDIUM_SLICE_SIZE;
                }
            }
            // SAFETY: detached blocks remain reserved and are exclusively owned.
            let failed = !unsafe { self.reclaim_packet(&mut pending[..count], now.saturating_add(1000)) };
            // Even a forced purge is a finite pass in the presence of decommit
            // failures; callers may retry later, not spin forever on OS failures.
            remaining = remaining.saturating_sub(count);
            if !force || failed || count == 0 || remaining == 0 {
                let mut state = self.state.lock();
                state.maintaining = false;
                if force {
                    let retained = state.retained_bytes;
                    state.credit.release_unused(retained);
                } else {
                    policy.resolve(&mut state, now);
                }
                return;
            }
            // A forced drain is intended for tests/explicit maintenance; stop if
            // retry spans are the only remaining work.
            let mut state = self.state.lock();
            if state.retained_bytes == 0 {
                state.maintaining = false;
                state.credit.release_unused(0);
                return;
            }
        }
    }
    pub(super) unsafe fn retire_packet(&self, pending: &mut [(*mut u8, usize)]) {
        let bytes = pending.iter().map(|(_, slices)| slices * MEDIUM_SLICE_SIZE).sum();
        self.state.lock().demand.retire(bytes);
        // SAFETY: the retired heap exclusively transfers its entire local cache.
        unsafe { self.reclaim_packet(pending, hal::monotonic_millis()) };
    }

    unsafe fn reclaim_packet(&self, pending: &mut [(*mut u8, usize)], retry_after: u64) -> bool {
        // In-place unstable sorting does not allocate or call the allocator.
        pending.sort_unstable_by_key(|(address, _)| address.addr());
        let mut first = 0;
        let mut succeeded = true;
        while first < pending.len() {
            let (address, mut slices) = pending[first];
            let region = region_containing(address).expect("detached spans retain immutable region identity");
            let mut end = first + 1;
            while end < pending.len()
                && address.addr() + slices * MEDIUM_SLICE_SIZE == pending[end].0.addr()
                && region_containing(pending[end].0) == Some(region)
            {
                slices += pending[end].1;
                end += 1;
            }
            let bytes = slices * MEDIUM_SLICE_SIZE;
            // SAFETY: only adjacent spans from the same reservation are merged.
            // Used bits exclude allocators until this OS transaction completes.
            if unsafe { hal::decommit(address, bytes) } {
                unsafe { self.release_slices(address, slices) };
                tracking::record_unmapping(bytes);
                record_medium_event(MediumEventKind::PurgedSpan, end - first);
            } else {
                succeeded = false;
                if unsafe { hal::commit(address, bytes) } {
                    let mut state = self.state.lock();
                    for &(original, original_slices) in &pending[first..end] {
                        // SAFETY: recommit restored accessibility; preserve the
                        // original span classes when returning packet ownership.
                        unsafe { insert_cached_span(&mut state, original, original_slices, retry_after) };
                    }
                }
                // Double failure quarantines the whole range with used bits set.
            }
            first = end;
        }
        succeeded
    }
}

unsafe fn insert_cached_span(state: &mut MediumState, address: *mut u8, slices: usize, deadline: u64) {
    // SAFETY: caller holds the correct shard lock and transfers a committed span.
    unsafe {
        if slices <= MEDIUM_MAX_SLICES {
            let bin = &mut state.bins[slices - 1];
            let block = address.cast::<MediumFreeBlock>();
            block.write(MediumFreeBlock { next: bin.free_list });
            bin.free_list = block;
            bin.purge_after = deadline;
            state.nonempty_bins[(slices - 1) / 64] |= 1 << ((slices - 1) % 64);
        } else {
            let region = region_containing(address).expect("returned medium span has immutable region identity");
            insert_large_extent(region, address, slices);
            (*region).large_purge_after = deadline;
        }
        state.retained_bytes += slices * MEDIUM_SLICE_SIZE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain() -> *mut DomainState {
        let domain = crate::domain::Domain::new().unwrap();
        crate::domain::state(domain)
    }

    fn allocator() -> Rallocator {
        // SAFETY: all tests use the standard configuration.
        unsafe { Rallocator::new() }
    }

    fn used_slices(backing: &MediumRegion) -> usize {
        let state = backing.state.lock();
        let mut region = state.regions;
        let mut used = 0;
        while !region.is_null() {
            // SAFETY: immutable links and locked region accounting.
            unsafe {
                used += MEDIUM_REGION_SLICE_COUNT - (*region).free_slices;
                region = (*region).next.load(Ordering::Relaxed);
            }
        }
        used
    }

    #[test]
    fn remote_packet_is_visible_to_consumers_before_it_fills() {
        let domain = domain();
        let allocator = allocator();
        let heap = create_bump_fallback_heap(domain);
        assert!(!heap.is_null());
        // SAFETY: this test exclusively owns the heap.
        unsafe { (*heap).medium_cache_max_bytes = 0 };
        let backing = unsafe { domain_shard(domain, 0) };
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let addresses: Vec<_> = (0..7)
            .map(|_| allocator.allocate_medium(layout, unsafe { &mut *heap }, None))
            .collect();
        assert!(addresses.iter().all(|address| !address.is_null()));
        for &address in &addresses {
            // SAFETY: each live allocation is freed exactly once by a non-owner.
            unsafe { allocator.deallocate_medium(address, layout, ptr::null_mut()) };
        }
        assert_eq!(backing.remote.pending(), (7, 7 * MEDIUM_SLICE_SIZE));
        assert_eq!(used_slices(backing), 7);
        assert_eq!(backing.state.lock().retained_bytes, 0);
        assert_eq!(backing.state.lock().return_batches, 0);
        let packets = backing.remote.lanes.iter().filter(|lane| lane.pending.lock().count != 0).count();
        let mut reused = [ptr::null_mut(); BATCH_CAPACITY];
        assert_eq!(backing.take_batch(1, &mut reused), 7);
        let mut expected = addresses.clone();
        expected.sort_unstable();
        reused[..7].sort_unstable();
        assert_eq!(&reused[..7], &expected);
        assert_eq!(backing.remote.pending(), (0, 0));
        assert_eq!(backing.state.lock().return_batches, packets);
        for address in addresses {
            // SAFETY: taking the batch transferred ownership back to this test.
            unsafe { backing.decommit_span(address, 1, 0) };
        }
        unsafe { retire_general_heap(heap) };
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn remote_packet_byte_limit_flushes_before_its_entry_limit() {
        let domain = domain();
        let allocator = allocator();
        let heap = create_bump_fallback_heap(domain);
        assert!(!heap.is_null());
        unsafe { (*heap).medium_cache_max_bytes = 0 };
        let backing = unsafe { domain_shard(domain, 0) };
        let layout = Layout::from_size_align(REMOTE_BYTES, 16).unwrap();
        let first = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
        let second = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
        assert!(!first.is_null() && !second.is_null());
        // SAFETY: both live spans are independently transferred exactly once.
        unsafe { allocator.deallocate_medium(first, layout, ptr::null_mut()) };
        assert_eq!(backing.remote.pending(), (0, 0));
        assert_eq!(backing.state.lock().return_batches, 1);
        unsafe { allocator.deallocate_medium(second, layout, ptr::null_mut()) };
        assert_eq!(backing.remote.pending(), (0, 0));
        assert_eq!(backing.state.lock().return_batches, 2);
        unsafe { retire_general_heap(heap) };
        backing.purge(true, hal::monotonic_millis());
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn partial_remote_packet_survives_owner_exit_and_is_purged_without_owner_drain() {
        let domain = domain();
        let allocator = allocator();
        let heap = create_bump_fallback_heap(domain);
        assert!(!heap.is_null());
        let backing = unsafe { domain_shard(domain, 0) };
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let address = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
        assert!(!address.is_null());
        // SAFETY: the outstanding allocation retains retirement coordination.
        unsafe { retire_general_heap(heap) };
        // SAFETY: owner exit did not invalidate this outstanding allocation.
        unsafe { allocator.deallocate_medium(address, layout, ptr::null_mut()) };
        assert_eq!(backing.remote.pending().0, 1);
        backing.purge(true, hal::monotonic_millis());
        assert_eq!(backing.remote.pending(), (0, 0));
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn concurrent_remote_publishers_transfer_each_span_once_in_bounded_packets() {
        let domain = domain();
        let source_allocator = allocator();
        let heap = create_bump_fallback_heap(domain);
        assert!(!heap.is_null());
        let backing = unsafe { domain_shard(domain, 0) };
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let groups: Vec<Vec<_>> = (0..4)
            .map(|_| {
                (0..BATCH_CAPACITY)
                    .map(|_| {
                        let address = source_allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
                        assert!(!address.is_null());
                        SendAddress(address)
                    })
                    .collect()
            })
            .collect();
        let mut expected = Vec::new();
        let mut per_lane = [0; REMOTE_STRIPES];
        for group in &groups {
            for SendAddress(address) in group {
                expected.push(*address);
                per_lane[RemoteMailbox::lane(*address)] += 1;
            }
        }
        std::thread::scope(|scope| {
            for group in groups {
                scope.spawn(move || {
                    let allocator = allocator();
                    for SendAddress(address) in group {
                        // SAFETY: the moved group uniquely owns these allocations.
                        unsafe { allocator.deallocate_medium(address, layout, ptr::null_mut()) };
                    }
                });
            }
        });
        let remaining: usize = per_lane.iter().map(|count| count % REMOTE_CAPACITY).sum();
        assert_eq!(backing.remote.pending(), (remaining, remaining * MEDIUM_SLICE_SIZE));
        assert_eq!(
            backing.state.lock().return_batches,
            per_lane.iter().map(|count| count / REMOTE_CAPACITY).sum::<usize>()
        );
        let mut recycled = Vec::new();
        loop {
            let mut batch = [ptr::null_mut(); BATCH_CAPACITY];
            let count = backing.take_batch(1, &mut batch);
            if count == 0 {
                break;
            }
            recycled.extend_from_slice(&batch[..count]);
        }
        expected.sort_unstable();
        recycled.sort_unstable();
        // Shared retention may already have decommitted some returned spans.
        // Every reusable entry must still belong to the original set exactly
        // once; final reservation accounting below also catches orphaned spans.
        assert!(recycled.windows(2).all(|pair| pair[0] != pair[1]));
        assert!(recycled.iter().all(|address| expected.binary_search(address).is_ok()));
        assert_eq!(backing.remote.pending(), (0, 0));
        for address in recycled {
            // SAFETY: the complete, duplicate-free set was reclaimed above.
            unsafe { backing.decommit_span(address, 1, 0) };
        }
        unsafe { retire_general_heap(heap) };
        backing.purge(true, hal::monotonic_millis());
        assert_eq!(used_slices(backing), 0);
    }

    #[cfg(not(miri))]
    #[test]
    fn remote_packet_purge_failure_restores_committed_reuse() {
        let domain = domain();
        let allocator = allocator();
        let heap = create_bump_fallback_heap(domain);
        assert!(!heap.is_null());
        let backing = unsafe { domain_shard(domain, 0) };
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let address = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
        assert!(!address.is_null());
        // SAFETY: the live allocation is transferred once; retirement can then
        // finish without invalidating the mailbox's domain-owned reservation.
        unsafe {
            allocator.deallocate_medium(address, layout, ptr::null_mut());
            retire_general_heap(heap);
        }
        hal::fail_next_decommit();
        backing.purge(true, hal::monotonic_millis());
        assert_eq!(backing.remote.pending(), (0, 0));
        assert_eq!(used_slices(backing), 1);
        let mut reused = [ptr::null_mut(); 1];
        assert_eq!(backing.take_batch(1, &mut reused), 1);
        assert_eq!(reused[0], address);
        // SAFETY: rollback recommitted the span, and the test now owns it.
        unsafe {
            address.write(42);
            assert_eq!(address.read(), 42);
            backing.decommit_span(address, 1, 0);
        }
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn local_overflow_publishes_batches_without_expanding_the_heap_cache() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let allocator = allocator();
        let heap = create_bump_fallback_heap(domain);
        assert!(!heap.is_null());
        let layout = Layout::from_size_align(32 * 1024, 16).unwrap();
        let count = if cfg!(miri) { 32 } else { 1024 };
        let mut addresses = Vec::with_capacity(count);
        for index in 0..count {
            let address = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
            assert!(!address.is_null());
            unsafe { address.write(index as u8) };
            addresses.push(address);
        }
        let before = backing.state.lock().return_batches;
        for (index, address) in addresses.into_iter().enumerate() {
            assert_eq!(unsafe { address.read() }, index as u8);
            unsafe { allocator.deallocate_medium(address, layout, heap) };
        }
        let publications = backing.state.lock().return_batches - before;
        assert_eq!(publications, count / BATCH_CAPACITY);
        assert!(unsafe { (*heap).medium_batch.bytes } <= LOCAL_CACHE_BYTES);
        unsafe { retire_general_heap(heap) };
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn publication_stripes_preserve_budgets_and_spread_power_of_two_strides() {
        assert_eq!(REMOTE_STRIPES * REMOTE_CAPACITY, BATCH_CAPACITY);
        assert_eq!(REMOTE_STRIPES * REMOTE_BYTES, LOCAL_CACHE_BYTES);
        for stride in [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
            let mut counts = [0; REMOTE_STRIPES];
            for index in 0..256 {
                let address = ptr::without_provenance_mut(index * stride * MEDIUM_SLICE_SIZE);
                counts[RemoteMailbox::lane(address)] += 1;
            }
            assert_eq!(counts, [256 / REMOTE_STRIPES; REMOTE_STRIPES]);
        }
    }

    #[test]
    fn mixed_class_eviction_keeps_each_original_span_size() {
        let mut cache = MediumCache::new();
        let addresses = [1, 2, 3, 4, 5].map(ptr::without_provenance_mut);
        for (class, address) in addresses[..4].iter().enumerate() {
            assert!(cache.push(class, *address));
        }
        assert!(cache.push(0, addresses[4]));
        assert_eq!(cache.bytes, LOCAL_CACHE_BYTES);
        let mut spans = [(ptr::null_mut(), 0); BATCH_CAPACITY];
        let count = cache.drain(&mut spans);
        assert_eq!(
            &spans[..count],
            &[
                (addresses[4], 1),
                (addresses[0], 1),
                (addresses[1], 2),
                (addresses[2], 4),
                (addresses[3], 8),
            ]
        );
        assert_eq!(cache.bytes, 0);
        assert!(cache.push(4, addresses[0]));
        assert_eq!(cache.bytes, LOCAL_CACHE_BYTES);
    }

    #[test]
    fn bitmap_word_boundaries_and_wrapped_runs_match_reference() {
        let mut used = [u64::MAX; MEDIUM_REGION_BITMAP_WORDS];
        let boundary = (MEDIUM_REGION_SLICE_COUNT / 2).min(64);
        mark_slices(&mut used, boundary - 3, 7, false);
        for start in 0..MEDIUM_REGION_SLICE_COUNT.min(130) {
            for count in 1..=9 {
                let reference = (start..MEDIUM_REGION_SLICE_COUNT).chain(0..start).find(|&first| {
                    first + count <= MEDIUM_REGION_SLICE_COUNT && (first..first + count).all(|bit| used[bit / 64] & (1 << (bit % 64)) == 0)
                });
                assert_eq!(find_free_slices(&used, start, count), reference, "start={start}, count={count}");
            }
        }
        mark_slices(&mut used, boundary - 3, 7, true);
        assert_eq!(used, [u64::MAX; MEDIUM_REGION_BITMAP_WORDS]);
    }

    #[test]
    fn batch_cache_has_a_combined_byte_ceiling_and_adapts_after_overflow() {
        let mut cache = MediumCache::new();
        assert_eq!(cache.refill_count(0), 1);
        assert_eq!(cache.refill_count(0), 2);
        assert_eq!(cache.refill_count(0), 4);
        for index in 0..BATCH_CAPACITY {
            assert!(cache.push(0, ptr::without_provenance_mut(index + 1)));
        }
        assert_eq!(cache.bytes, LOCAL_CACHE_BYTES);
        assert!(!cache.push(1, ptr::without_provenance_mut(100)));
        assert_eq!(cache.refill_count(1), 1);
        for index in (0..BATCH_CAPACITY).rev() {
            assert_eq!(cache.pop(0).unwrap().addr(), index + 1);
        }
        assert_eq!(cache.bytes, 0);
        assert_eq!(cache.pop(0), None);
    }

    #[test]
    fn full_regions_leave_availability_and_rejoin_after_release() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let (address, region) = backing.reserve_slices(domain, MEDIUM_REGION_SLICE_COUNT).unwrap();
        assert!(backing.state.lock().available.is_null());
        // SAFETY: uncommitted reservation belongs exclusively to this test.
        unsafe { backing.release_slices(address, 1) };
        assert_eq!(backing.state.lock().available, region);
        let (reused, _) = backing.reserve_slices(domain, 1).unwrap();
        assert_eq!(reused, address);
        assert!(backing.state.lock().available.is_null());
        unsafe { backing.release_slices(address, MEDIUM_REGION_SLICE_COUNT) };
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn unexpired_bins_remain_available_for_reuse() {
        let mut block = MediumFreeBlock { next: ptr::null_mut() };
        let mut state = MediumState::new();
        let mut bin = MediumBin::new();
        bin.free_list = ptr::from_mut(&mut block);
        bin.purge_after = 20;
        state.bins[0] = bin;
        state.nonempty_bins[0] = 1;
        let mut pending = [(ptr::null_mut(), 0); PURGE_WORK];
        assert_eq!(state.detach_bins(&mut pending, false, false, 10), (0, 0));
        assert_eq!(state.bins[0].free_list, ptr::from_mut(&mut block));
        assert_eq!(state.next_bin(0), Some(0));
    }

    #[cfg(not(miri))]
    #[test]
    fn availability_unlinks_middle_regions_and_large_search_visits_all_regions() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let mut result = [ptr::null_mut()];
        assert_eq!(backing.take_batch(MEDIUM_MAX_SLICES + 1, &mut result), 0);
        let mut state = backing.state.lock();
        // SAFETY: the private shard lock protects all three newly published regions.
        let regions = unsafe { std::array::from_fn::<_, 3, _>(|_| append_region(&mut state, &backing.regions, domain).unwrap()) };
        // SAFETY: all regions are linked and the test holds their shard lock.
        unsafe {
            remove_available(&mut state, regions[1]);
            assert_eq!(
                ((*regions[2]).available_next, (*regions[0]).available_previous),
                (regions[0], regions[2])
            );
            add_available(&mut state, regions[1]);
        }
        drop(state);
        assert_eq!(backing.take_batch(MEDIUM_MAX_SLICES + 1, &mut result), 0);
    }

    #[test]
    fn remote_flush_tolerates_an_already_detached_packet() {
        let backing = MediumRegion::new();
        // A competing consumer can detach the packet after this consumer has
        // observed readiness but before it obtains the packet lock.
        backing.remote.lanes[0].ready.store(true, Ordering::Relaxed);
        backing.flush_remote();
        assert_eq!(backing.remote.pending(), (0, 0));
        assert_eq!(backing.state.lock().retained_bytes, 0);
    }

    #[cfg(not(miri))]
    #[test]
    fn failed_batch_and_single_reservations_leave_no_cached_spans() {
        let domain = domain();
        let allocator = allocator();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain);
        heap.medium_batch.targets[0] = BATCH_CAPACITY;
        hal::fail_next_reserve();
        hal::fail_next_map();
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        assert!(allocator.allocate_medium(layout, &mut heap, None).is_null());
        assert_eq!((used_slices(heap_regions(&mut heap)), heap.medium_batch.bytes), (0, 0));
    }

    #[test]
    fn recycled_refill_takes_a_partial_batch_in_one_transaction() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let allocator = allocator();
        let mut heap = ReusableHeapState::new(GeneralOptions::new().with_medium_cache_max_bytes(0), domain);
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let first = allocator.allocate_medium(layout, &mut heap, None);
        let second = allocator.allocate_medium(layout, &mut heap, None);
        // SAFETY: both allocations are live and owned by this heap.
        unsafe {
            allocator.deallocate_medium(first, layout, ptr::from_mut(&mut heap));
            allocator.deallocate_medium(second, layout, ptr::from_mut(&mut heap));
        }
        let mut batch = [ptr::null_mut(); BATCH_CAPACITY];
        assert_eq!(backing.take_batch(1, &mut batch), 2);
        assert_eq!(&batch[..2], &[second, first]);
        assert_eq!(backing.state.lock().retained_bytes, 0);
        assert_eq!(backing.take_batch(1, &mut batch), 0);
        for address in [first, second] {
            unsafe { backing.decommit_span(address, 1, 0) };
        }
    }

    #[cfg(not(miri))]
    #[test]
    fn failed_fresh_batch_commit_rolls_back_every_reserved_slice() {
        let domain = domain();
        let allocator = allocator();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain);
        heap.medium_batch.targets[0] = BATCH_CAPACITY;
        hal::fail_next_commit();
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        assert!(allocator.allocate_medium(layout, &mut heap, None).is_null());
        let backing = heap_regions(&mut heap);
        assert_eq!(used_slices(backing), 0);
        assert_eq!(heap.medium_batch.bytes, 0);
        let address = allocator.allocate_medium(layout, &mut heap, None);
        assert!(!address.is_null());
        assert_eq!(used_slices(backing), BATCH_CAPACITY);
        assert_eq!(heap.medium_batch.bytes, (BATCH_CAPACITY - 1) * MEDIUM_SLICE_SIZE);
        unsafe { allocator.deallocate_medium(address, layout, ptr::from_mut(&mut heap)) };
        while let Some(cached) = heap.medium_batch.pop(0) {
            unsafe { backing.decommit_span(cached, 1, 0) };
        }
        assert_eq!(used_slices(backing), 0);
    }

    #[cfg(not(miri))]
    #[test]
    fn batch_reservation_failure_falls_back_to_an_existing_single_span() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let (occupied, _) = backing.reserve_slices(domain, MEDIUM_REGION_SLICE_COUNT - 1).unwrap();
        let allocator = allocator();
        let mut heap = ReusableHeapState::new(GeneralOptions::new(), domain);
        heap.medium_batch.targets[0] = BATCH_CAPACITY;
        hal::fail_next_reserve();
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let address = allocator.allocate_medium(layout, &mut heap, None);
        assert!(!address.is_null());
        assert_eq!(heap.medium_batch.bytes, 0);
        assert_eq!(used_slices(backing), MEDIUM_REGION_SLICE_COUNT);
        unsafe {
            allocator.deallocate_medium(address, layout, ptr::null_mut());
            backing.release_slices(occupied, MEDIUM_REGION_SLICE_COUNT - 1);
        }
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn shared_retention_and_opportunistic_purge_are_bounded() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        // Miri uses 2 MiB regions; use enough small spans to cross the shared
        // retention budget without requiring a larger-than-region allocation.
        for _ in 0..SHARED_CACHE_BYTES / MEDIUM_SLICE_SIZE + 2 {
            let (address, _) = backing.reserve_slices(domain, 1).unwrap();
            // SAFETY: this test owns the reserved slice until returning it.
            assert!(unsafe { hal::commit(address, MEDIUM_SLICE_SIZE) });
            // Inject cache entries independently of machine-specific pressure.
            unsafe { insert_cached_span(&mut backing.state.lock(), address, 1, 1) };
        }
        assert_eq!(backing.state.lock().retained_bytes, SHARED_CACHE_BYTES + 2 * MEDIUM_SLICE_SIZE);
        let before = used_slices(backing);
        backing.purge_with_budget(
            false,
            u64::MAX,
            MemoryBudget {
                limit: usize::MAX,
                pressured: false,
            },
        );
        assert_eq!(before - used_slices(backing), PURGE_WORK);
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), 0);
    }

    #[cfg(not(miri))]
    #[test]
    fn unexpired_large_extent_survives_opportunistic_purge() {
        let domain = domain();
        // SAFETY: the isolated domain owns its primary shard for this test.
        let backing = unsafe { domain_shard(domain, 0) };
        let slices = SHARED_CACHE_BYTES / MEDIUM_SLICE_SIZE + 1;
        let bytes = slices * MEDIUM_SLICE_SIZE;
        let (address, _) = backing.reserve_slices(domain, slices).unwrap();
        // SAFETY: the test exclusively owns this reserved span.
        assert!(unsafe { hal::commit(address, bytes) });
        // SAFETY: the committed span transfers into the shard's locked free list.
        unsafe { insert_cached_span(&mut backing.state.lock(), address, slices, 20) };
        backing.purge_with_budget(
            false,
            10,
            MemoryBudget {
                limit: usize::MAX,
                pressured: false,
            },
        );
        let retained = backing.state.lock().retained_bytes;
        let used = used_slices(backing);
        backing.purge(true, u64::MAX);
        assert_eq!((retained, used, used_slices(backing)), (bytes, slices, 0));
    }

    #[cfg(not(miri))]
    #[test]
    fn continuous_returns_honor_a_memory_budget_equal_to_the_idle_floor() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let budget = MemoryBudget {
            limit: SHARED_CACHE_BYTES,
            pressured: false,
        };
        for now in 1..=1024 {
            let (address, _) = backing.reserve_slices(domain, 1).unwrap();
            // SAFETY: the reserved span is exclusively owned until publication below.
            assert!(unsafe { hal::commit(address, MEDIUM_SLICE_SIZE) });
            backing.record_fresh(MEDIUM_SLICE_SIZE);
            // SAFETY: the committed span is returned exactly once to its backing shard.
            unsafe { backing.return_span_with_budget(address, 1, 1000, now, budget) };
            assert!(backing.state.lock().retained_bytes <= budget.limit + PURGE_BYTES);
        }
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), 0);
    }

    #[cfg(not(miri))]
    #[test]
    fn failed_purge_of_a_full_batch_makes_progress_without_retrying_forever() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        for _ in 0..PURGE_WORK {
            let (address, _) = backing.reserve_slices(domain, 1).unwrap();
            assert!(unsafe { hal::commit(address, MEDIUM_SLICE_SIZE) });
            unsafe { backing.return_span(address, 1, 0) };
        }
        hal::fail_next_decommit();
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), PURGE_WORK);
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), 0);
    }

    #[cfg(not(miri))]
    #[test]
    fn short_burst_reuse_avoids_os_calls_and_expired_packets_coalesce() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let (base, _) = backing.reserve_slices(domain, 1024).unwrap();
        assert!(unsafe { hal::commit(base, 1024 * MEDIUM_SLICE_SIZE) });
        backing.record_fresh(1024 * MEDIUM_SLICE_SIZE);
        let budget = MemoryBudget {
            limit: 128 * 1024 * 1024,
            pressured: false,
        };
        let before = hal::medium_os_counts();
        for index in 0..1024 {
            unsafe { backing.return_span_with_budget(base.add(index * MEDIUM_SLICE_SIZE), 1, 1000, 1, budget) };
        }
        assert_eq!(backing.state.lock().retained_bytes, 1024 * MEDIUM_SLICE_SIZE);
        // Take the entire burst before returning it, rather than repeatedly
        // recycling a tiny subset that would hide retained-workload regressions.
        let mut addresses = [ptr::null_mut(); 1024];
        for batch in addresses.chunks_mut(BATCH_CAPACITY) {
            assert_eq!(backing.take_batch(1, batch), batch.len());
        }
        for address in addresses {
            unsafe { backing.return_span_with_budget(address, 1, 1000, 2, budget) };
        }
        assert_eq!(hal::medium_os_counts(), before);
        while backing.state.lock().retained_bytes > SHARED_CACHE_BYTES {
            backing.purge_with_budget(false, 1003, budget);
        }
        // 48 MiB expired beyond the idle floor: twelve 4 MiB merged operations,
        // not 768 individual protection changes.
        assert_eq!(hal::medium_os_counts(), (before.0, before.1 + 12));
        assert_eq!(used_slices(backing), SHARED_CACHE_BYTES / MEDIUM_SLICE_SIZE);
        let pressured = MemoryBudget { pressured: true, ..budget };
        while backing.state.lock().retained_bytes != 0 {
            backing.purge_with_budget(false, 1003, pressured);
        }
        assert_eq!(used_slices(backing), 0);
    }

    #[cfg(not(miri))]
    #[test]
    fn failed_packet_restoration_quarantines_all_original_spans() {
        let domain = domain();
        let backing = unsafe { domain_shard(domain, 0) };
        let (base, _) = backing.reserve_slices(domain, PURGE_WORK).unwrap();
        assert!(unsafe { hal::commit(base, PURGE_WORK * MEDIUM_SLICE_SIZE) });
        for index in 0..PURGE_WORK {
            unsafe { insert_cached_span(&mut backing.state.lock(), base.add(index * MEDIUM_SLICE_SIZE), 1, 1) };
        }
        hal::fail_next_decommit();
        hal::fail_next_commit();
        backing.purge(true, 2);
        assert_eq!(used_slices(backing), PURGE_WORK);
        assert_eq!(backing.state.lock().retained_bytes, 0);
        let (other, _) = backing.reserve_slices(domain, PURGE_WORK).unwrap();
        assert_ne!(other, base);
        unsafe {
            backing.release_slices(other, PURGE_WORK);
            // Test fault injection left the pages accessible; production has
            // deliberately abandoned this possibly inaccessible reservation.
            assert!(hal::decommit(base, PURGE_WORK * MEDIUM_SLICE_SIZE));
            backing.release_slices(base, PURGE_WORK);
        }
    }

    #[test]
    fn startup_on_one_processor_still_balances_numa_local_lanes() {
        let domain = domain();
        let mut counts = [0; SHARD_COUNT];
        for _ in 0..32 {
            counts[unsafe { balanced_shard(domain, 1) }] += 1;
        }
        assert_eq!(&counts[4..8], &[8, 8, 8, 8]);
        assert_eq!(counts.iter().sum::<usize>(), 32);
    }

    #[test]
    fn shard_routing_is_bounded_and_node_local_when_buckets_do_not_collide() {
        for node in 0..1024 {
            for cpu in 0..128 {
                let shard = shard_index(cpu, node);
                assert!(shard < SHARD_COUNT);
                assert_eq!(shard / 4, node % 4);
            }
        }
        assert_ne!(shard_index(0, 0), shard_index(1, 0));
        assert_ne!(shard_index(0, 0), shard_index(0, 1));
    }

    struct SendAddress(*mut u8);
    // SAFETY: test channels transfer exclusive allocation ownership, not a borrow.
    unsafe impl Send for SendAddress {}

    #[test]
    fn remote_free_is_reusable_without_the_owning_worker_draining() {
        let domain = crate::domain::Domain::new().unwrap();
        let allocator = allocator();
        let heap = create_bump_fallback_heap(crate::domain::state(domain));
        assert!(!heap.is_null());
        let backing = unsafe { domain_shard(crate::domain::state(domain), 7) };
        // SAFETY: this test owns the newly created heap.
        unsafe { (*heap).medium_shard = backing };
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        let address = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
        let transfer = SendAddress(address);
        std::thread::spawn(move || {
            let transfer = transfer;
            // SAFETY: the channel-equivalent move transferred the live allocation.
            unsafe { allocator.deallocate_medium(transfer.0, layout, ptr::null_mut()) };
        })
        .join()
        .unwrap();
        // No allocating-worker drain or heap-local mutation occurred.
        let mut reused = [ptr::null_mut(); 2];
        assert_eq!(backing.take_batch(1, &mut reused), 1);
        assert_eq!(reused[0], address);
        assert_eq!(unsafe { (*region_containing(address).unwrap()).backing }, ptr::from_ref(backing));
        unsafe {
            backing.decommit_span(address, 1, 0);
            retire_general_heap(heap);
        }
    }

    #[test]
    fn outstanding_medium_allocations_survive_owner_exit_and_reclaim_on_another_thread() {
        let domain = crate::domain::Domain::new().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let allocator = allocator();
            let heap = create_bump_fallback_heap(crate::domain::state(domain));
            assert!(!heap.is_null());
            let backing = unsafe { domain_shard(crate::domain::state(domain), 11) };
            unsafe { (*heap).medium_shard = backing };
            let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
            for value in 1_u8..=20 {
                let address = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
                assert!(!address.is_null());
                unsafe { address.write(value) };
                sender.send(SendAddress(address)).unwrap();
            }
            unsafe { retire_general_heap(heap) };
        })
        .join()
        .unwrap();
        let allocator = allocator();
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        for (index, SendAddress(address)) in receiver.into_iter().enumerate() {
            assert_eq!(unsafe { address.read() }, (index + 1) as u8);
            unsafe { allocator.deallocate_medium(address, layout, ptr::null_mut()) };
        }
        let backing = unsafe { domain_shard(crate::domain::state(domain), 11) };
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), 0);
    }

    #[test]
    fn remote_free_races_owner_retirement_without_retaining_heap_storage() {
        let domain = crate::domain::Domain::new().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (retire_sender, retire_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let allocator = allocator();
            let heap = create_bump_fallback_heap(crate::domain::state(domain));
            assert!(!heap.is_null());
            let backing = unsafe { domain_shard(crate::domain::state(domain), 5) };
            unsafe { (*heap).medium_shard = backing };
            let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
            for _ in 0..32 {
                let address = allocator.allocate_medium(layout, unsafe { &mut *heap }, None);
                assert!(!address.is_null());
                sender.send(SendAddress(address)).unwrap();
            }
            retire_receiver.recv().unwrap();
            // SAFETY: the worker owns the heap; outstanding external allocations
            // keep retirement coordination alive while the consumer unregisters.
            unsafe { retire_general_heap(heap) };
        });
        let addresses = (0..32).map(|_| receiver.recv().unwrap()).collect::<Vec<_>>();
        retire_sender.send(()).unwrap();
        let allocator = allocator();
        let layout = Layout::from_size_align(MEDIUM_SLICE_SIZE, 16).unwrap();
        for SendAddress(address) in addresses {
            unsafe { allocator.deallocate_medium(address, layout, ptr::null_mut()) };
        }
        worker.join().unwrap();
        let backing = unsafe { domain_shard(crate::domain::state(domain), 5) };
        backing.purge(true, u64::MAX);
        assert_eq!(used_slices(backing), 0);
    }
}
