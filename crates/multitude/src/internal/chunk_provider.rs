// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-arena chunk cache and allocation source.
//!
//! [`ChunkProvider`] owns the arena's allocator clone, enforces a byte
//! budget, and maintains a freed-chunk cache at the current class floor.
//!
//! The cache is a lock-free Treiber stack: any thread can push (an escaped
//! `Arc`/`Box` dropping the last reference on another thread), only the
//! owner pops. Below-floor stragglers are destroyed by
//! [`ChunkProvider::pop`]. The class floor ratchets upward as the arena
//! needs larger chunks; below-floor chunks are evicted or destroyed.

// These `unsafe fn`s have item-level safety contracts; inner unsafe blocks
// would not add a boundary here.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use alloc::sync::{Arc, Weak};
use core::mem;
use core::ptr::{self, NonNull};
#[cfg(feature = "stats")]
use core::sync::atomic::AtomicU64;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};

use allocator_api2::alloc::Allocator;

use super::chunk::Chunk;
use super::constants::{MAX_CHUNK_BYTES, MAX_NORMAL_ALLOC, MIN_CHUNK_BYTES, SizeClass};
use crate::AllocError;

/// Tunable knobs for a [`ChunkProvider`].
#[derive(Clone, Copy)]
pub(crate) struct ChunkProviderConfig {
    byte_budget: usize,
    max_normal_alloc: usize,
}

impl ChunkProviderConfig {
    /// Construct a configuration with the given limits.
    ///
    /// - `byte_budget`: maximum total bytes (header + payload) the provider
    ///   may have outstanding at any time. Allocations that would exceed
    ///   this fail.
    /// - `max_normal_alloc`: largest single allocation routed through normal
    ///   cache size classes; requests above this bypass the cache as
    ///   one-shot oversized chunks.
    #[inline]
    pub(crate) fn new(byte_budget: usize, max_normal_alloc: usize) -> Self {
        Self {
            byte_budget,
            max_normal_alloc,
        }
    }

    /// Largest single allocation routed through normal cache size classes.
    #[inline]
    pub(crate) fn max_normal_alloc(&self) -> usize {
        self.max_normal_alloc
    }
}

impl Default for ChunkProviderConfig {
    fn default() -> Self {
        Self::new(usize::MAX, MAX_NORMAL_ALLOC)
    }
}

/// Snapshot of a provider's lifetime chunk-allocation counters.
#[cfg(feature = "stats")]
#[derive(Clone, Copy)]
pub(crate) struct ChunkAllocStats {
    normal: u64,
    oversized: u64,
}

#[cfg(feature = "stats")]
impl ChunkAllocStats {
    /// Lifetime count of normal-class chunks allocated.
    #[inline]
    pub(crate) fn normal(&self) -> u64 {
        self.normal
    }

    /// Lifetime count of oversized one-shot chunks allocated.
    #[inline]
    pub(crate) fn oversized(&self) -> u64 {
        self.oversized
    }
}

/// Allocates and caches chunks for one arena.
pub(crate) struct ChunkProvider<A: Allocator + Clone> {
    allocator: A,
    config: ChunkProviderConfig,
    weak_self: Weak<Self>,
    /// Bytes currently outstanding (allocated, not yet freed). Updated via
    /// `AcqRel` speculative-add.
    bytes_outstanding: AtomicUsize,
    /// Lock-free chunk cache: single Treiber-stack head for the current class
    /// floor ([`Self::cache_class`]). Any thread may push (an escaped handle
    /// dropped elsewhere); only the owning thread pops.
    cache: AtomicPtr<u8>,
    /// Current class floor for the cache; below-floor chunks are evicted.
    cache_class: AtomicU8,

    /// Lifetime count of normal (cacheable) chunks allocated from the backing
    /// allocator (cache hits are not counted).
    #[cfg(feature = "stats")]
    normal_chunks_allocated: AtomicU64,
    /// Lifetime count of oversized one-shot chunks allocated.
    #[cfg(feature = "stats")]
    oversized_chunks_allocated: AtomicU64,
    /// Unused tail bytes in retired chunks not yet cached or freed. Retire
    /// increments; cache/destroy decrements.
    #[cfg(feature = "stats")]
    wasted_tail_bytes: AtomicU64,
}

// `non_send_fields_in_send_ty`: the `Weak<Self>` back-pointer is the flagged
// field; it is sound because every owning chunk reaches the provider through it
// and the provider is single-owner per arena.
#[allow(
    clippy::non_send_fields_in_send_ty,
    reason = "Weak<Self> back-pointer is sound; provider is single-owner per arena"
)]
// SAFETY: `cache` is composed of `AtomicPtr`s, which are `Send + Sync`;
// `allocator` is `A: Allocator + Clone` (callers must use `Send + Sync`-capable
// allocators when sharing the provider across threads). Only the owning thread
// pops the cache (single-popper Treiber-stack invariant).
unsafe impl<A: Allocator + Clone + Send> Send for ChunkProvider<A> {}
// SAFETY: `cache` is composed of `AtomicPtr`s (`Send + Sync`) and `allocator`
// is `A: Sync`; sharing `&ChunkProvider` across threads only exposes those, and
// the single-popper Treiber-stack invariant is unaffected by shared `&`-access.
unsafe impl<A: Allocator + Clone + Sync> Sync for ChunkProvider<A> {}

impl<A: Allocator + Clone> ChunkProvider<A> {
    /// Builds a new provider returning an `Arc` that owning chunks will
    /// reference weakly.
    pub(crate) fn new(allocator: A, config: ChunkProviderConfig) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            allocator,
            config,
            weak_self: Weak::clone(weak),
            bytes_outstanding: AtomicUsize::new(0),
            cache: AtomicPtr::new(ptr::null_mut()),
            cache_class: AtomicU8::new(0),
            #[cfg(feature = "stats")]
            normal_chunks_allocated: AtomicU64::new(0),
            #[cfg(feature = "stats")]
            oversized_chunks_allocated: AtomicU64::new(0),
            #[cfg(feature = "stats")]
            wasted_tail_bytes: AtomicU64::new(0),
        })
    }

    /// Snapshot of the lifetime chunk-allocation counters.
    #[cfg(feature = "stats")]
    pub(crate) fn chunk_alloc_stats(&self) -> ChunkAllocStats {
        ChunkAllocStats {
            normal: self.normal_chunks_allocated.load(Ordering::Relaxed),
            oversized: self.oversized_chunks_allocated.load(Ordering::Relaxed),
        }
    }

    /// Total bytes currently outstanding from the underlying allocator: the
    /// sum of every chunk (header + payload) that has been allocated and not
    /// yet freed. Chunks released back to the size-class cache stay counted;
    /// only chunks returned to the underlying allocator (cache evictions,
    /// oversized one-shots dropped, drain-on-provider-drop) decrement.
    #[cfg(feature = "stats")]
    pub(crate) fn bytes_outstanding(&self) -> u64 {
        self.bytes_outstanding.load(Ordering::Relaxed) as u64
    }

    /// Currently "wasted" tail bytes (free region between bump cursor and
    /// payload end) across chunks that have been retired from a current
    /// `ChunkMutator` slot but have not yet been returned to the cache or
    /// freed back to the underlying allocator.
    #[cfg(feature = "stats")]
    pub(crate) fn wasted_tail_bytes(&self) -> u64 {
        self.wasted_tail_bytes.load(Ordering::Relaxed)
    }

    /// Adds `n` to the wasted-tail-bytes counter. Called when a chunk is
    /// retired from a current `ChunkMutator` slot.
    #[cfg(feature = "stats")]
    pub(in crate::internal) fn record_wasted_tail(&self, n: u64) {
        self.wasted_tail_bytes.fetch_add(n, Ordering::Relaxed);
    }

    /// Subtracts `n` from the wasted-tail-bytes counter. Called when a
    /// retired chunk is later cached or destroyed.
    #[cfg(feature = "stats")]
    fn release_wasted_tail(&self, n: u64) {
        self.wasted_tail_bytes.fetch_sub(n, Ordering::Relaxed);
    }

    /// Returns the provider's configuration.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Default::default mutation observably equivalent for reachable inputs
    pub(crate) fn config(&self) -> ChunkProviderConfig {
        self.config
    }

    /// Returns a borrowed handle to the provider's allocator.
    pub(crate) fn allocator(&self) -> &A {
        &self.allocator
    }

    /// Acquires a normal-class chunk with at least `min_payload` bytes.
    /// Caller must route oversized requests to [`Self::acquire_oversized`].
    pub(crate) fn acquire(&self, min_payload: usize, ratchet_class: SizeClass) -> Result<NonNull<Chunk<A>>, AllocError> {
        let header = Chunk::<A>::header_size();
        let needed_total = header.checked_add(min_payload).ok_or(AllocError::CAPACITY_OVERFLOW)?;
        debug_assert!(
            min_payload <= self.config.max_normal_alloc && !exceeds_max_chunk_bytes(needed_total),
            "acquire invoked with oversized request — caller must route to acquire_oversized",
        );
        self.acquire_normal(SizeClass::min_for_bytes(needed_total).max(ratchet_class))
    }

    /// Acquires a cacheable chunk in `class`, bumping the floor first
    /// when needed.
    //
    // Mutation testing is suppressed on the `class > floor` branch: `>` with
    // `<` / `==` only changes when the floor advances (cache memory pressure,
    // not a correctness bug, and exercised by the stats-driven cache-class
    // tests), and `>` with `>=` is a redundant no-op floor advance.
    #[cfg_attr(test, mutants::skip)]
    fn acquire_normal(&self, class: SizeClass) -> Result<NonNull<Chunk<A>>, AllocError> {
        // SAFETY: only the owning thread bumps the floor / pops (single-
        // popper Treiber-stack invariant); a popped chunk is uniquely
        // owned, so we can re-init its refcount/drop count in the same
        // scope.
        unsafe {
            if class.raw() > self.cache_class.load(Ordering::Relaxed) {
                self.advance_cache_floor(class);
            }
            if let Some(chunk) = self.pop() {
                Chunk::reinit_for_acquire(chunk);
                return Ok(chunk);
            }
        }

        self.allocate_fresh(class)
    }

    /// Sets the cache floor and destroys detached chunks below it.
    /// Racing below-floor pushes are handled by [`Self::pop`].
    ///
    /// # Safety
    ///
    /// Must be called from the cache's owning thread (single-popper
    /// invariant).
    #[cold]
    #[inline(never)]
    unsafe fn advance_cache_floor(&self, new_class: SizeClass) {
        // Publish the new floor with Release so concurrent pushers'
        // subsequent Acquire load sees it.
        self.cache_class.store(new_class.raw(), Ordering::Release);
        let new_min_total = new_class.bytes();
        // Detach the freelist; racing pushers target the empty head.
        let mut cur = self.cache.swap(ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: each linked chunk is a refcount-zero, uniquely-owned
        // chunk we just detached; we walk the list, re-push survivors,
        // and destroy below-floor stragglers.
        unsafe {
            while !cur.is_null() {
                let fat = Chunk::<A>::header_to_fat(cur);
                let chunk_nn = NonNull::new_unchecked(fat);
                let link = Chunk::cache_link(chunk_nn);
                let next = (*link).load(Ordering::Acquire);
                let total =
                    Chunk::<A>::footprint((*chunk_nn.as_ptr()).capacity()).expect("evicted chunk's layout was valid when it was allocated");
                if total >= new_min_total {
                    self.push(chunk_nn);
                } else {
                    Chunk::destroy(chunk_nn);
                    self.release_bytes(total);
                }
                cur = next;
            }
        }
    }

    /// Allocates a fresh normal chunk, bypassing the cache.
    #[cfg_attr(test, mutants::skip)] // `total - header → total / header` ⇒ runaway allocations
    fn allocate_fresh(&self, class: SizeClass) -> Result<NonNull<Chunk<A>>, AllocError> {
        let header = Chunk::<A>::header_size();
        let total = class.bytes();
        let payload_size = total - header;
        self.reserve_bytes(total)?;
        match Chunk::<A>::allocate(self.allocator.clone(), Weak::clone(&self.weak_self), payload_size) {
            Ok(chunk) => {
                #[cfg(feature = "stats")]
                self.normal_chunks_allocated.fetch_add(1, Ordering::Relaxed);
                Ok(chunk)
            }
            Err(e) => {
                self.release_bytes(total);
                Err(e)
            }
        }
    }

    /// Routes a refcount-zero chunk back to the cache or deallocates.
    ///
    /// # Safety
    ///
    /// `chunk` must have refcount zero and the caller must hold the unique
    /// remaining reference.
    pub(in crate::internal) unsafe fn release(&self, chunk: NonNull<Chunk<A>>) {
        // SAFETY: chunk is live and uniquely owned by caller.
        let capacity = (*chunk.as_ptr()).capacity();
        let total = Chunk::<A>::footprint(capacity).expect("released chunk's layout was valid when it was allocated");
        #[cfg(feature = "stats")]
        {
            // Acquire load pairs with retire on another thread.
            let wasted = u64::from((*chunk.as_ptr()).wasted_at_retire());
            if wasted != 0 {
                self.release_wasted_tail(wasted);
            }
        }
        // Bypass the cache for oversized / non-class totals and below-floor chunks.
        if !is_cacheable_size(total) || total < SizeClass::new(self.cache_class.load(Ordering::Acquire)).bytes() {
            Chunk::destroy(chunk);
            self.release_bytes(total);
            return;
        }
        self.push(chunk);
    }

    /// Pre-warms the cache with one chunk in the given size class. Always
    /// uses the fresh-allocate path, even when the payload exceeds
    /// `max_normal_alloc`.
    pub(crate) fn preallocate(&self, class: SizeClass) -> Result<(), AllocError> {
        let chunk = self.allocate_fresh(class)?;
        // SAFETY: we own the +1 from `allocate_fresh`; refcount-to-zero routes
        // it straight into the cache (the chunk is a valid class size).
        unsafe { Chunk::<A>::destroy_or_cache_just_acquired(self, chunk) };
        Ok(())
    }

    /// Speculative-add reservation against the byte budget.
    fn reserve_bytes(&self, n: usize) -> Result<(), AllocError> {
        // `fetch_update` hides the CAS retry loop, so the contention
        // path doesn't surface as an explicit uncoverable `Err` arm in
        // single-threaded test runs.
        if self
            .bytes_outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |cur| {
                let new = cur.checked_add(n)?;
                if new > self.config.byte_budget {
                    return None;
                }
                Some(new)
            })
            .is_ok()
        {
            Ok(())
        } else {
            Err(AllocError::ALLOCATOR_FAILED)
        }
    }

    fn release_bytes(&self, n: usize) {
        self.bytes_outstanding.fetch_sub(n, Ordering::AcqRel);
    }

    /// Allocates a one-shot oversized chunk sized to fit `min_payload` bytes.
    /// The caller uses a temporary [`ChunkMutator`](super::chunk_mutator::ChunkMutator),
    /// so the current chunk remains available for later small allocations.
    pub(crate) fn acquire_oversized(&self, min_payload: usize) -> Result<NonNull<Chunk<A>>, AllocError> {
        // Add worst-case payload-start alignment skew; round to the rounded
        // allocation size we then reserve.
        let slack = oversized_payload_align_slack();
        let payload = round_up_to_word_align(min_payload.checked_add(slack).ok_or(AllocError::CAPACITY_OVERFLOW)?)?;
        let total = Chunk::<A>::footprint(payload)?;
        self.reserve_bytes(total)?;
        match Chunk::<A>::allocate(self.allocator.clone(), Weak::clone(&self.weak_self), payload) {
            Ok(chunk) => {
                #[cfg(feature = "stats")]
                self.oversized_chunks_allocated.fetch_add(1, Ordering::Relaxed);
                Ok(chunk)
            }
            Err(e) => {
                self.release_bytes(total);
                Err(e)
            }
        }
    }

    /// Pops a cached chunk at or above the current class floor,
    /// destroying below-floor stragglers.
    ///
    /// # Safety
    ///
    /// Called only from the provider's owning thread (single popper
    /// invariant).
    unsafe fn pop(&self) -> Option<NonNull<Chunk<A>>> {
        let floor_min_total = SizeClass::new(self.cache_class.load(Ordering::Relaxed)).bytes();
        loop {
            // SAFETY: each observed non-null `cur` is a live, uniquely-
            // owned chunk (single popper); we read its cache-link via
            // `Chunk::cache_link` and on success the resulting
            // pointer is exclusively ours.
            let updated = self.cache.fetch_update(Ordering::AcqRel, Ordering::Acquire, |cur| {
                if cur.is_null() {
                    return None;
                }
                let fat = Chunk::<A>::header_to_fat(cur);
                let link = Chunk::cache_link(NonNull::new_unchecked(fat));
                Some((*link).load(Ordering::Acquire))
            });
            let Ok(popped) = updated else { return None };
            let fat = Chunk::<A>::header_to_fat(popped);
            let chunk_nn = NonNull::new_unchecked(fat);
            let total =
                Chunk::<A>::footprint((*chunk_nn.as_ptr()).capacity()).expect("popped chunk's layout was valid when it was allocated");
            if total >= floor_min_total {
                return Some(chunk_nn);
            }
            // Below-floor straggler from a concurrent push that raced the
            // floor bump; destroy and try the next entry.
            Chunk::destroy(chunk_nn);
            self.release_bytes(total);
        }
    }

    /// Pushes `chunk` onto the cache freelist.
    ///
    /// # Safety
    ///
    /// `chunk` must be a refcount-zero, uniquely-owned chunk.
    unsafe fn push(&self, chunk: NonNull<Chunk<A>>) {
        let head = &self.cache;
        let link = Chunk::cache_link(chunk);
        let new = chunk.cast::<u8>().as_ptr();
        // Exclusive ownership permits non-atomic link initialization before
        // the publishing CAS; later link changes use atomics.
        let mut cur = head.load(Ordering::Acquire);
        loop {
            ptr::write((*link).as_ptr(), cur);
            #[cfg(test)]
            tests::maybe_inject_push_race::<A>(head, cur);
            match head.compare_exchange_weak(cur, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return,
                Err(actual) => {
                    #[cfg(test)]
                    tests::note_push_retry();
                    cur = actual;
                }
            }
        }
    }

    /// Drains cached chunks and deallocates their backing memory.
    fn drain_all(&self) {
        // SAFETY: drain runs in Drop with no outstanding mutators; the
        // provider is single-owner at this point, so the Treiber stack is
        // quiescent. Every cached chunk is uniquely owned by us once popped.
        unsafe {
            let mut cur = self.cache.swap(ptr::null_mut(), Ordering::AcqRel);
            while !cur.is_null() {
                let fat = Chunk::<A>::header_to_fat(cur);
                let chunk_nn = NonNull::new_unchecked(fat);
                let link = Chunk::cache_link(chunk_nn);
                let next = (*link).load(Ordering::Acquire);
                Chunk::destroy(chunk_nn);
                cur = next;
            }
        }
    }
}

impl<A: Allocator + Clone> Drop for ChunkProvider<A> {
    fn drop(&mut self) {
        self.drain_all();
    }
}

/// Convenience: cache lookup by total allocation size.
#[inline]
fn is_cacheable_size(total: usize) -> bool {
    (MIN_CHUNK_BYTES..=MAX_CHUNK_BYTES).contains(&total) && total.is_power_of_two()
}

/// Rounds an oversized chunk's payload up to a multiple of the machine word
/// alignment (`align_of::<usize>()`). Returns `Err(AllocError)` on overflow.
/// Keeps the usable capacity from falling below `min_payload` after the bump
/// cursor pays any payload-start alignment skew.
#[cfg_attr(test, mutants::skip)] // mask mutations underfit payload → OOM spin
#[inline]
fn round_up_to_word_align(min_payload: usize) -> Result<usize, AllocError> {
    let mask = mem::align_of::<usize>() - 1;
    min_payload
        .checked_add(mask)
        .map(|v| v & !mask)
        .ok_or(AllocError::CAPACITY_OVERFLOW)
}

/// Worst-case alignment skew the bump cursor pays at the start of an
/// oversized chunk's (possibly unaligned) payload. Added to oversized
/// requests so the first allocation always fits after alignment.
#[inline]
// Mutation testing is suppressed: `align - 1` is the exact maximum skew.
// The `-`→`+` / `-`→`/` mutants only ever *over*-reserve by a few bytes
// (never under-allocate), so they are equivalent for correctness and
// invisible through any public API contract.
#[cfg_attr(test, mutants::skip)]
fn oversized_payload_align_slack() -> usize {
    mem::align_of::<usize>() - 1
}

/// Wraps the `needed_total > MAX_CHUNK_BYTES` check used by the
/// `acquire_*` routing gates.
#[cfg_attr(test, mutants::skip)] // boundary unreachable: max_normal_alloc capped well below
#[inline]
fn exceeds_max_chunk_bytes(needed_total: usize) -> bool {
    needed_total > MAX_CHUNK_BYTES
}

// --- Helpers wired into the chunk type via an inherent impl -------------------

impl<A: Allocator + Clone> Chunk<A> {
    /// Routes a just-acquired refcount-1 chunk straight to the provider cache
    /// (used by preallocation, which warms the cache without handing the
    /// chunk to a mutator).
    ///
    /// # Safety
    ///
    /// `chunk` must be the result of a fresh `acquire`/`allocate_fresh` call
    /// on the same `provider`.
    unsafe fn destroy_or_cache_just_acquired(provider: &ChunkProvider<A>, chunk: NonNull<Self>) {
        // SAFETY: chunk is live and uniquely owned; dec_ref takes it to 0,
        // then `release` routes it to the cache (no drops were committed
        // since this is a fresh acquisition).
        unsafe {
            let last = chunk.as_ref().dec_ref();
            debug_assert!(last, "preallocate chunk refcount should reach zero");
            provider.release(chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use std::thread_local;

    use allocator_api2::alloc::Global;

    use super::*;

    thread_local! {
        /// Test-only: when non-null, the next `push` on this thread
        /// splices this chunk onto the stack head right before its CAS,
        /// deterministically forcing the contended retry (`Err`) arm.
        static INJECT_PUSH_RACE: Cell<*mut u8> = const { Cell::new(ptr::null_mut()) };
        /// Test-only: counts how many times `push`'s CAS retry arm
        /// ran on this thread.
        static PUSH_RETRY_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    /// Test hook that injects a competing cache push before the CAS.
    ///
    /// # Safety
    ///
    /// `cur` must be the value `push` loaded from `head`, and any
    /// armed injection pointer must be a refcount-zero, uniquely-owned
    /// chunk header owned by the test.
    pub(super) unsafe fn maybe_inject_push_race<A: Allocator + Clone>(head: &AtomicPtr<u8>, cur: *mut u8) {
        let inject = INJECT_PUSH_RACE.with(|slot| slot.replace(ptr::null_mut()));
        if inject.is_null() {
            return;
        }
        let fat = Chunk::<A>::header_to_fat(inject);
        let link = Chunk::cache_link(NonNull::new_unchecked(fat));
        ptr::write((*link).as_ptr(), cur);
        head.store(inject, Ordering::Release);
    }

    /// Test hook invoked by `push` whenever its CAS retry arm runs.
    pub(super) fn note_push_retry() {
        PUSH_RETRY_COUNT.with(|c| c.set(c.get() + 1));
    }

    /// Covers `Default for ChunkProviderConfig` (lines 58-63).
    #[test]
    fn chunk_provider_config_default_matches_constants() {
        let c = ChunkProviderConfig::default();
        assert_eq!(c.byte_budget, usize::MAX);
        assert_eq!(c.max_normal_alloc(), MAX_NORMAL_ALLOC);
    }

    // Kills `reserve_bytes`' `new > byte_budget` boundary mutations
    // (`> → >=` and `> → ==`): reserving exactly up to the budget must
    // succeed (rejected by both mutants), while exceeding it must fail.
    #[test]
    fn reserve_bytes_allows_exactly_budget_and_rejects_over() {
        let provider = ChunkProvider::<Global>::new(Global, ChunkProviderConfig::new(100, 4096));
        // Reaching exactly the budget is allowed (`new == budget` is not `> budget`).
        provider.reserve_bytes(100).expect("reaching exactly the budget must be allowed");
        // One more byte exceeds the budget and must be rejected.
        provider.reserve_bytes(1).expect_err("exceeding the budget must be rejected");
    }

    // Covers `pop`'s below-floor straggler arm by raising the floor,
    // then pushing a smaller chunk.
    #[test]
    fn pop_destroys_below_floor_straggler() {
        let provider = ChunkProvider::<Global>::new(Global, ChunkProviderConfig::default());
        // SAFETY: single-threaded test owns the cache; the floor is raised
        // on an empty freelist, then a below-floor straggler is injected
        // and popped, exactly mirroring the documented push/floor race.
        unsafe {
            // Raise the floor well above class 0 (512 B) — class 3 = 4 KiB.
            provider.advance_cache_floor(SizeClass::new(3));
            // Allocate a class-0 (512 B) chunk: below the new floor.
            let chunk = provider.allocate_fresh(SizeClass::ZERO).expect("fresh class-0 chunk");
            // `push` requires a refcount-zero, uniquely-owned chunk.
            assert!(chunk.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            provider.push(chunk);
            // The straggler is below the floor, so the pop destroys it and
            // finds the now-empty cache, returning `None`.
            assert!(provider.pop().is_none());
        }
    }

    /// `is_cacheable_size` checks the closed interval [MIN, MAX] **and**
    /// power-of-two. Pin both arms so `&&`/`||` mutations flip the
    /// result on probes that exercise either constraint independently.
    #[test]
    fn is_cacheable_size_requires_range_and_power_of_two() {
        // In range, power of two → true.
        assert!(is_cacheable_size(MIN_CHUNK_BYTES));
        assert!(is_cacheable_size(MAX_CHUNK_BYTES));
        // In range, NOT power of two → false (would be `true` under
        // `&& → ||` if the right arm dominated).
        assert!(!is_cacheable_size(MIN_CHUNK_BYTES + 1));
        // Out of range, power of two → false (would be `true` under
        // `&& → ||`).
        assert!(!is_cacheable_size(MAX_CHUNK_BYTES * 2));
        assert!(!is_cacheable_size(MIN_CHUNK_BYTES / 2));
        // Zero is below the lower bound (and not a power of two).
        assert!(!is_cacheable_size(0));
    }

    // Covers `push`'s contended CAS retry arm via deterministic
    // thread-local race injection.
    #[test]
    fn push_retries_on_contended_cas() {
        let provider = ChunkProvider::<Global>::new(Global, ChunkProviderConfig::default());
        PUSH_RETRY_COUNT.with(|c| c.set(0));
        // SAFETY: every chunk below is freshly allocated, uniquely owned,
        // and dropped to refcount 0 before being pushed/injected. The
        // injected chunk is spliced into the freelist by the hook, so the
        // stack stays valid and the provider's drain frees all three.
        unsafe {
            // Base chunk C establishes a non-null head for the race.
            let c = provider.allocate_fresh(SizeClass::ZERO).expect("chunk c");
            assert!(c.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            provider.push(c);

            // Chunk D is injected by the hook during the next push to model
            // a concurrent pusher mutating `head`.
            let d = provider.allocate_fresh(SizeClass::ZERO).expect("chunk d");
            assert!(d.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            INJECT_PUSH_RACE.with(|slot| slot.set(d.cast::<u8>().as_ptr()));

            // Pushing B loads head == C, but the hook publishes D before B's
            // CAS, forcing the retry arm before B finally settles on top.
            let b = provider.allocate_fresh(SizeClass::ZERO).expect("chunk b");
            assert!(b.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            provider.push(b);
        }
        // At least one retry must have run (CAS may also fail spuriously on
        // weakly-ordered targets, so we assert a lower bound, not equality).
        assert!(
            PUSH_RETRY_COUNT.with(Cell::get) >= 1,
            "the contended CAS retry arm must run at least once",
        );
    }
}
