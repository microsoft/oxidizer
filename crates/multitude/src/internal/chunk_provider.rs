// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! `ChunkProvider`: factory + cache for chunks.
//!
//! The local cache is a singly-linked list mutated only by the arena's
//! owning thread, so its bookkeeping lives in [`LocalSlot`]s. The
//! shared cache is a lock-free Treiber stack: multi-producer (any
//! thread that drops the last `Arc<T>` to refcount zero), single-
//! consumer (only the arena's owning thread pops, via
//! `acquire_shared`). The single-consumer property eliminates the
//! classic Treiber field-read-before-ownership UAF and ABA hazards:
//! no other thread can pop, so the head node observed by the popper
//! cannot be removed (and therefore cannot be freed) between the
//! `head.load` and the popper's CAS.
//!
//! When the head's `capacity` is below the requested `min_bytes` the
//! popper pops anyway, frees the too-small chunk, and tries again.
//! This trades a (rare) too-small cached chunk for the simplicity of
//! head-only pops; in practice the high-water ratchet keeps cached
//! chunks at the current class, so misfits are uncommon.

use alloc::sync::Arc;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::constants::{LARGE, MAX_CHUNK_BYTES, NUM_CHUNK_CLASSES, class_to_bytes, min_class_for_bytes};
use super::local_chunk::{LocalChunk, header_size as local_header_size};
use super::local_slot::LocalSlot;
use super::shared_chunk::{SharedChunk, header_size as shared_header_size};
use super::sync::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};
#[cfg(feature = "stats")]
use crate::arena_stats::bump_stat;

/// Per-arena chunk factory + cache.
///
/// Held as `Arc<ChunkProvider>` by exactly one [`Arena`](crate::Arena)
/// and as `Weak<ChunkProvider>` by every chunk produced by the
/// provider so a chunk can route itself back to the cache when its
/// refcount reaches zero — *if* the provider is still alive at that
/// time.
pub(crate) struct ChunkProvider<A: Allocator + Clone> {
    /// Backing allocator the provider uses for fresh chunks. Cloned
    /// into every chunk it creates so the chunk can self-free even
    /// after the provider has been dropped.
    pub(crate) allocator: A,

    /// Per-arena `max_normal_alloc` knob: requests strictly larger
    /// than this take the oversized-chunk path.
    pub(crate) max_normal_alloc: usize,

    /// Optional lifetime cap on total chunk bytes outstanding (live +
    /// cached). When set, the provider returns [`AllocError`] from
    /// `acquire_*` rather than allocate past the budget.
    ///
    /// **Counting convention:** the budget tracks each chunk's
    /// **total allocation size** (`header_size + capacity` =
    /// `class_to_bytes(class)` for cached chunks, or
    /// `header_size + round_payload(user_request)` for one-shot
    /// oversized chunks). Reservation and release are symmetric.
    /// The underlying VM allocation matches this exactly (up to a
    /// small structural-alignment rounding for oversized shared
    /// chunks, at most 63 bytes).
    pub(crate) byte_budget: Option<usize>,

    /// Total bytes of chunk capacity currently outstanding (live +
    /// cached). Compared against `byte_budget` on every fresh
    /// allocation.
    pub(crate) total_chunk_bytes: AtomicUsize,

    /// Local-cache list head. Touched only by the arena's owning
    /// thread (enforced structurally by `LocalChunk: !Send`). The
    /// cache is unbounded — every chunk that passes the high-water
    /// filter is retained.
    pub(crate) local_cache: LocalSlot<Option<NonNull<LocalChunk<A>>>>,

    /// Shared-cache list head, as a thin pointer to the top
    /// [`SharedChunk`] (or null if empty). Lock-free Treiber stack:
    /// pushes (from any thread) use a CAS loop; pops (only by the
    /// arena's owning thread — `acquire_shared` is reached from
    /// owner-thread code paths) are single-consumer and therefore
    /// don't need ABA defenses or hazard pointers (no other thread
    /// can pop the head between our load and CAS).
    pub(crate) shared_cache_head: AtomicPtr<u8>,

    /// Largest local class ever produced; monotonic. Touched only by
    /// the arena's owning thread.
    pub(crate) local_high_water: LocalSlot<u8>,

    /// Largest shared class ever produced; monotonic. Updated with
    /// `fetch_max(Relaxed)` because shared-flavor acquires/releases
    /// can come from any thread.
    pub(crate) shared_high_water: AtomicU8,

    /// Runtime statistics counters. Only present when the `stats`
    /// Cargo feature is enabled.
    #[cfg(feature = "stats")]
    pub(crate) stats: crate::arena_stats::StatsStorage,
}

impl<A: Allocator + Clone> ChunkProvider<A> {
    pub(crate) fn new(
        allocator: A,
        max_normal_alloc: usize,
        byte_budget: Option<usize>,
        initial_local_class: u8,
        initial_shared_class: u8,
    ) -> Arc<Self> {
        Arc::new(Self {
            allocator,
            max_normal_alloc,
            byte_budget,
            total_chunk_bytes: AtomicUsize::new(0),
            local_cache: LocalSlot::new(None),
            shared_cache_head: AtomicPtr::new(core::ptr::null_mut()),
            local_high_water: LocalSlot::new(initial_local_class),
            shared_high_water: AtomicU8::new(initial_shared_class),
            #[cfg(feature = "stats")]
            stats: crate::arena_stats::StatsStorage::default(),
        })
    }

    /// Try to reserve `bytes` against the lifetime byte budget.
    /// Returns `Ok(())` if the budget allows it (or if no budget is
    /// configured); `Err(AllocError)` otherwise. Adds `bytes` to
    /// `total_chunk_bytes` on success.
    fn reserve_budget(&self, bytes: usize) -> Result<(), AllocError> {
        // Single-writer per provider: `reserve_budget` only runs on
        // the arena's owning thread. `release_budget` runs on any
        // thread (cross-thread `Arc::drop`). Pair the cross-thread
        // sub's Release with our Acquire load so a budget release on
        // a remote core is promptly visible here, avoiding spurious
        // `AllocError`s on weakly-ordered targets.
        if let Some(budget) = self.byte_budget {
            let current = self.total_chunk_bytes.load(Ordering::Acquire);
            let next = current.checked_add(bytes).ok_or(AllocError)?;
            if next > budget {
                return Err(AllocError);
            }
        }
        self.total_chunk_bytes.fetch_add(bytes, Ordering::Relaxed);
        Ok(())
    }

    /// Release `bytes` from the lifetime byte budget tracker.
    fn release_budget(&self, bytes: usize) {
        // Release so a subsequent owner-thread `reserve_budget`
        // Acquire load sees the decrement.
        self.total_chunk_bytes.fetch_sub(bytes, Ordering::Release);
    }

    /// Produce a chunk whose payload is at least `min_payload` bytes.
    ///
    /// The returned [`NonNull`] holds a `+1` on the chunk's refcount;
    /// the caller owns that hold.
    ///
    /// Requests strictly larger than [`MAX_NORMAL_ALLOC`] get a
    /// one-shot oversized chunk and leave the high-water mark untouched.
    #[cfg_attr(test, mutants::skip)] // Chunk-class clamp mutations still choose a class that satisfies the request.
    pub(crate) fn acquire_local(self: &Arc<Self>, min_payload: usize) -> Result<NonNull<LocalChunk<A>>, AllocError> {
        if min_payload > self.max_normal_alloc {
            // Oversized: allocation sized exactly to the request,
            // plus header and drop-list rounding. Bypasses the cache.
            let rounded_payload = super::drop_list::round_payload(min_payload, local_header_size::<A>()).ok_or(AllocError)?;
            let total_bytes = local_header_size::<A>().checked_add(rounded_payload).ok_or(AllocError)?;
            self.reserve_budget(total_bytes)?;
            #[cfg(feature = "stats")]
            bump_stat!(self, oversized_local_chunks_allocated, 1);
            match LocalChunk::allocate(self.allocator.clone(), Arc::downgrade(self), total_bytes) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    self.release_budget(total_bytes);
                    return Err(e);
                }
            }
        }

        // Smallest class whose payload covers the request.
        // `class_to_bytes` is total, so add the header.
        let req_class = min_class_for_bytes(min_payload + local_header_size::<A>());
        // SAFETY: single-thread-local — `LocalChunk: !Send`.
        let high_water = unsafe { self.local_high_water.with_mut(|h| *h) };
        let max_class = NUM_CHUNK_CLASSES - 1;
        let target_class = req_class.max(high_water).min(max_class);
        debug_assert!(target_class < NUM_CHUNK_CLASSES);
        let target_total = class_to_bytes(target_class);

        // Pop the first cached chunk whose payload fits.
        // SAFETY: single-thread-local invariant.
        let popped = unsafe {
            self.local_cache.with_mut(|head| -> Option<NonNull<LocalChunk<A>>> {
                use core::cell::Cell;
                // Re-interpret `&mut Option` as `&Cell<Option>` so the
                // walk can rewrite the predecessor's `next` link
                // without juggling overlapping borrows.
                let mut prev_link: *const Cell<Option<NonNull<LocalChunk<A>>>> = {
                    let head_cell: &Cell<Option<NonNull<LocalChunk<A>>>> = Cell::from_mut(head);
                    core::ptr::from_ref(head_cell)
                };
                let mut cur = (*prev_link).get();
                while let Some(chunk) = cur {
                    // SAFETY: chunk lives in the cache list.
                    let c = chunk.as_ref();
                    let cap = c.capacity;
                    let next = c.next.get();
                    if cap >= min_payload {
                        (*prev_link).set(next);
                        c.next.set(None);
                        return Some(chunk);
                    }
                    prev_link = &raw const c.next;
                    cur = next;
                }
                None
            })
        };

        if let Some(chunk) = popped {
            // SAFETY: chunk just left the cache; we own it exclusively.
            unsafe { chunk.as_ref().revive_for_reuse() };
            return Ok(chunk);
        }

        let total_bytes = target_total;
        self.reserve_budget(total_bytes)?;
        #[cfg(feature = "stats")]
        bump_stat!(self, normal_local_chunks_allocated, 1);
        let chunk = match LocalChunk::allocate(self.allocator.clone(), Arc::downgrade(self), total_bytes) {
            Ok(c) => c,
            Err(e) => {
                self.release_budget(total_bytes);
                return Err(e);
            }
        };
        // Ratchet high-water so future chunks grow with the workload.
        let next_high_water = target_class.saturating_add(1).min(NUM_CHUNK_CLASSES - 1).min(max_class);
        // SAFETY: single-thread-local invariant.
        unsafe {
            self.local_high_water.with_mut(|h| {
                if next_high_water > *h {
                    *h = next_high_water;
                }
            });
        }
        Ok(chunk)
    }

    /// Allocate a fresh local chunk, set its refcount to 0 (cache
    /// state), and push it onto the local cache. Used by
    /// [`ArenaBuilder::with_capacity_local`](crate::ArenaBuilder::with_capacity_local).
    pub(crate) fn preallocate_local(self: &Arc<Self>) -> Result<(), AllocError> {
        // SAFETY: single-thread-local invariant.
        let high_water = unsafe { self.local_high_water.with_mut(|h| *h) };
        let target_class = high_water;
        let total_bytes = class_to_bytes(target_class);
        self.reserve_budget(total_bytes)?;
        #[cfg(feature = "stats")]
        bump_stat!(self, normal_local_chunks_allocated, 1);
        let chunk = match LocalChunk::allocate(self.allocator.clone(), Arc::downgrade(self), total_bytes) {
            Ok(c) => c,
            Err(e) => {
                self.release_budget(total_bytes);
                return Err(e);
            }
        };
        // SAFETY: just allocated; the chunk's refcount is inflated
        // to `LARGE`. Drop it now to 0 so the chunk sits in the
        // cache; `revive_for_reuse` will restore the inflation when
        // the chunk is later popped.
        let chunk_ref = unsafe { chunk.as_ref() };
        let prev = chunk_ref.refcount.replace(0);
        debug_assert_eq!(prev, LARGE);
        // SAFETY: single-thread-local invariant.
        unsafe {
            self.local_cache.with_mut(|head| {
                chunk_ref.next.set(*head);
                *head = Some(chunk);
            });
        }
        // `target_class == high_water` by construction, so the high-water mark
        // would not change here; skip the redundant read-modify-write.
        Ok(())
    }

    /// Allocate a fresh shared chunk, set its refcount to 0 (cache
    /// state), and push it onto the shared cache Treiber stack. Used
    /// by [`ArenaBuilder::with_capacity_shared`](crate::ArenaBuilder::with_capacity_shared).
    pub(crate) fn preallocate_shared(self: &Arc<Self>) -> Result<(), AllocError> {
        let high_water = self.shared_high_water.load(Ordering::Relaxed);
        let target_class = high_water;
        let total_bytes = class_to_bytes(target_class);
        self.reserve_budget(total_bytes)?;
        #[cfg(feature = "stats")]
        bump_stat!(self, normal_shared_chunks_allocated, 1);
        let chunk = match SharedChunk::allocate(self.allocator.clone(), Arc::downgrade(self), total_bytes) {
            Ok(c) => c,
            Err(e) => {
                self.release_budget(total_bytes);
                return Err(e);
            }
        };
        // SAFETY: just allocated; refcount is inflated to LARGE.
        // Drop to 0 so the chunk sits in the cache;
        // `revive_for_reuse` will restore the inflation on pop.
        unsafe { chunk.as_ref() }.refcount.0.store(0, Ordering::Relaxed);
        // SAFETY: refcount-zero — exclusive access; publish on the
        // lock-free shared cache.
        unsafe { self.push_shared_cache(chunk) };
        let _ = self.shared_high_water.fetch_max(target_class, Ordering::Relaxed);
        Ok(())
    }

    /// Get a snapshot of the current statistics counters.
    #[cfg(feature = "stats")]
    #[inline]
    pub fn stats_snapshot(&self) -> crate::arena_stats::ArenaStats {
        self.stats.snapshot()
    }

    /// Release a chunk whose refcount has just reached zero.
    ///
    /// Replays the chunk's drop list, then routes the chunk to the
    /// local cache (if it passes the high-water + budget filter) or
    /// frees its backing allocation directly.
    ///
    /// # Safety
    ///
    /// `chunk` must have refcount zero and the caller must own that
    /// last `+1` (i.e., the chunk must not be reachable from any other
    /// strand).
    pub(crate) unsafe fn release_local(&self, chunk: NonNull<LocalChunk<A>>) {
        // SAFETY: caller owns the last +1; no other strand observes the payload.
        unsafe { LocalChunk::replay_drops(chunk) };

        // SAFETY: caller owns the last +1.
        let cap = unsafe { (*chunk.as_ptr()).capacity };
        let header = local_header_size::<A>();
        // Oversized iff the chunk's total exceeds the largest cached class.
        let oversized = header.saturating_add(cap) > MAX_CHUNK_BYTES;

        // Eligible for caching iff not oversized and the payload covers
        // the high-water class (never re-issue a chunk smaller than
        // what current workloads expect).
        let eligible = if oversized {
            false
        } else {
            // SAFETY: single-thread-local invariant.
            let high_water = unsafe { self.local_high_water.with_mut(|h| *h) };
            cap >= class_to_bytes(high_water).saturating_sub(header)
        };

        if eligible {
            // SAFETY: only the owning thread mutates the cache. Use raw
            // access to keep the SharedReadOnly borrow tag off the chunk
            // (uniform with paths that do reach `free_backing`).
            unsafe {
                self.local_cache.with_mut(|head| {
                    (*chunk.as_ptr()).next.set(*head);
                    *head = Some(chunk);
                });
            }
        } else {
            self.release_budget(header + cap);
            // SAFETY: caller owns the last +1; drops just replayed.
            unsafe { LocalChunk::free_backing(chunk) };
        }
    }

    /// Produce a [`SharedChunk`] whose payload is at least `min_payload`
    /// bytes. The returned pointer holds the chunk's inflated refcount
    /// ([`LARGE`]) for the new tenant arena.
    #[cfg_attr(test, mutants::skip)] // Chunk-class clamp mutations still choose a class that satisfies the request.
    pub(crate) fn acquire_shared(self: &Arc<Self>, min_payload: usize) -> Result<NonNull<SharedChunk<A>>, AllocError> {
        if min_payload > self.max_normal_alloc {
            // Oversized: see `acquire_local`.
            let rounded_payload = super::drop_list::round_payload(min_payload, shared_header_size::<A>()).ok_or(AllocError)?;
            let total_bytes = shared_header_size::<A>().checked_add(rounded_payload).ok_or(AllocError)?;
            self.reserve_budget(total_bytes)?;
            #[cfg(feature = "stats")]
            bump_stat!(self, oversized_shared_chunks_allocated, 1);
            match SharedChunk::allocate(self.allocator.clone(), Arc::downgrade(self), total_bytes) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    self.release_budget(total_bytes);
                    return Err(e);
                }
            }
        }

        let req_class = min_class_for_bytes(min_payload + shared_header_size::<A>());
        let high_water = self.shared_high_water.load(Ordering::Relaxed);
        let max_class = NUM_CHUNK_CLASSES - 1;
        let target_class = req_class.max(high_water).min(max_class);
        debug_assert!(target_class < NUM_CHUNK_CLASSES);
        let target_total = class_to_bytes(target_class);

        if let Some(chunk) = self.try_pop_shared_at_least(min_payload) {
            // SAFETY: chunk just left the cache; sole owner until handoff.
            unsafe { chunk.as_ref().revive_for_reuse() };
            return Ok(chunk);
        }

        let total_bytes = target_total;
        self.reserve_budget(total_bytes)?;
        #[cfg(feature = "stats")]
        bump_stat!(self, normal_shared_chunks_allocated, 1);
        let chunk = match SharedChunk::allocate(self.allocator.clone(), Arc::downgrade(self), total_bytes) {
            Ok(c) => c,
            Err(e) => {
                self.release_budget(total_bytes);
                return Err(e);
            }
        };
        let next_high_water = target_class.saturating_add(1).min(NUM_CHUNK_CLASSES - 1).min(max_class);
        let _ = self.shared_high_water.fetch_max(next_high_water, Ordering::Relaxed);
        Ok(chunk)
    }

    /// Push a refcount-zero shared chunk onto the lock-free Treiber
    /// stack. Callable from any thread.
    ///
    /// # Safety
    ///
    /// `chunk` must be at refcount zero with its drop list replayed,
    /// and the caller must own the chunk exclusively.
    #[cfg_attr(coverage_nightly, coverage(off))]
    // Treiber-stack push from any thread that drops the last reference of a shared
    // chunk. The CAS retry arm (`Err(observed)`) only fires when two threads
    // attempt to push concurrently — deterministically exercising this requires
    // precise interleaving that isn't feasible in unit tests.
    unsafe fn push_shared_cache(&self, chunk: NonNull<SharedChunk<A>>) {
        let chunk_thin = SharedChunk::to_thin_ptr(chunk);
        // SAFETY: caller owns `chunk` exclusively; safe to borrow.
        let chunk_ref = unsafe { chunk.as_ref() };
        // Relaxed initial load: the pusher has no read-side dependency
        // on the head. If the load is stale, the failure-ordering
        // `Acquire` of `compare_exchange_weak` returns the fresh
        // value on retry.
        let mut current = self.shared_cache_head.load(Ordering::Relaxed);
        loop {
            // Store `next` BEFORE the CAS so that any concurrent pop
            // that succeeds in taking `chunk` sees the settled `next`
            // link.
            chunk_ref.next.store(current, Ordering::Relaxed);
            match self
                .shared_cache_head
                .compare_exchange_weak(current, chunk_thin, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return,
                Err(observed) => current = observed,
            }
        }
    }

    /// Pop a cached chunk whose capacity is at least `min_bytes`,
    /// freeing the backing memory of any cached chunks that are too
    /// small for the request along the way.
    ///
    /// Single-consumer: callable only from the arena's owning thread
    /// (enforced by routing through `acquire_shared`). The pop reads
    /// `head.next` before the CAS that takes ownership — this is
    /// sound because no other thread can pop, so `head` remains in
    /// the list (and therefore alive) for the duration of the CAS.
    /// Concurrent pushes from other threads only add nodes above
    /// `head`; they do not mutate `head.next` of the existing top
    /// node, so the read is stable.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // CAS-race and cache-release arithmetic mutations are not deterministic through public APIs.
    fn try_pop_shared_at_least(&self, min_bytes: usize) -> Option<NonNull<SharedChunk<A>>> {
        loop {
            let head_thin = self.shared_cache_head.load(Ordering::Acquire);
            if head_thin.is_null() {
                return None;
            }
            // SAFETY: `head_thin` is the thin-pointer base of a live
            // cached chunk. Single-consumer: no other thread pops, so
            // the chunk cannot be freed between this load and the CAS
            // below.
            let head = unsafe { SharedChunk::<A>::from_thin_ptr(head_thin) };
            // Read `next` and `capacity` via raw pointer (no `&Self`
            // borrow) so we don't leave a SharedReadOnly tag that
            // would conflict with `free_backing` further down.
            // SAFETY: `head` is live until we either CAS it off the
            // list or another push appears above it. Relaxed is
            // sufficient: the `Acquire` on `shared_cache_head` above
            // already synchronizes-with the pusher's AcqRel CAS that
            // installed `head` (which is sequenced-after the pusher's
            // Relaxed store to `head.next`).
            let next_thin = unsafe { (*head.as_ptr()).next.load(Ordering::Relaxed) };
            if self
                .shared_cache_head
                .compare_exchange_weak(head_thin, next_thin, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                // CAS Err: lost the race to a concurrent push; loop will
                // retry with the new head observed on the next iteration.
                continue;
            }
            // We now own `head` exclusively.
            // Clear our `next` link so the popped chunk is in no list.
            // SAFETY: caller-owned chunk.
            unsafe { (*head.as_ptr()).next.store(core::ptr::null_mut(), Ordering::Relaxed) };
            // SAFETY: caller-owned chunk.
            let cap = unsafe { (*head.as_ptr()).capacity };
            if cap >= min_bytes {
                return Some(head);
            }
            // Too small: free its backing and retry. We still
            // hold the budget reservation made when the chunk was
            // originally allocated, so release it now.
            self.release_budget(shared_header_size::<A>() + cap);
            // SAFETY: refcount-zero with drops already replayed
            // at cache-push time; we own the chunk.
            unsafe { SharedChunk::free_backing(head) };
        }
    }

    /// Release a [`SharedChunk`] whose refcount has just reached zero.
    ///
    /// Replays the drop list, then routes the chunk to the shared
    /// cache (if it passes the high-water + budget filter) or frees
    /// its backing allocation directly.
    ///
    /// # Safety
    ///
    /// `chunk` must have refcount zero (with Acquire-fence already
    /// taken by the caller); caller owns the last reference.
    pub(crate) unsafe fn release_shared(&self, chunk: NonNull<SharedChunk<A>>) {
        // SAFETY: refcount-zero — caller owns the last reference.
        unsafe { SharedChunk::replay_drops(chunk) };
        // SAFETY: refcount-zero — caller owns the last reference.
        let cap = unsafe { (*chunk.as_ptr()).capacity };
        let header = shared_header_size::<A>();
        let oversized = header.saturating_add(cap) > MAX_CHUNK_BYTES;

        let eligible = if oversized {
            false
        } else {
            let high_water = self.shared_high_water.load(Ordering::Relaxed);
            cap >= class_to_bytes(high_water).saturating_sub(header)
        };

        if eligible {
            // SAFETY: refcount-zero — exclusive access until pushed.
            // `push_shared_cache` publishes with Release so any popper
            // sees the chunk's settled fields.
            unsafe { self.push_shared_cache(chunk) };
        } else {
            self.release_budget(header + cap);
            // SAFETY: refcount-zero — exclusive access; drops replayed.
            unsafe { SharedChunk::free_backing(chunk) };
        }
    }
}

impl<A: Allocator + Clone> Drop for ChunkProvider<A> {
    fn drop(&mut self) {
        // Drain the local cache list, freeing each chunk's backing
        // allocation. The cache holds chunks at refcount zero with
        // empty drop ledgers; only the backing allocation needs
        // releasing here.
        //
        // SAFETY: `&mut self` proves no other strand can observe the
        // cache list, so we may drain without synchronization.
        let mut head = unsafe { self.local_cache.with_mut(Option::take) };
        while let Some(chunk) = head {
            // SAFETY: chunk is in the cache (refcount zero, drops
            // replayed); we hold exclusive access via `&mut self`.
            // Read `next` via raw pointer (no `&Self` borrow) so it
            // doesn't conflict with `free_backing`'s `drop_in_place`.
            let next = unsafe { (*chunk.as_ptr()).next.replace(None) };
            // SAFETY: see above; the cache is the sole owner.
            unsafe { LocalChunk::free_backing(chunk) };
            head = next;
        }

        // Drain the shared cache. `&mut self` proves no other strand
        // can observe the cache, so a plain load (no synchronization)
        // is sufficient.
        let mut cur_thin = self.shared_cache_head.swap(core::ptr::null_mut(), Ordering::Relaxed);
        while !cur_thin.is_null() {
            // SAFETY: `cur_thin` is the thin-pointer base of a cached
            // chunk; chunks in the cache are at refcount zero with
            // their drop ledgers already replayed.
            let chunk = unsafe { SharedChunk::<A>::from_thin_ptr(cur_thin) };
            // SAFETY: chunk is live and exclusively owned (raw access
            // to avoid SRO tag conflicting with `free_backing`).
            let next_thin = unsafe { (*chunk.as_ptr()).next.load(Ordering::Relaxed) };
            // SAFETY: refcount-zero with cleaned-up drop ledger.
            unsafe { SharedChunk::free_backing(chunk) };
            cur_thin = next_thin;
        }
    }
}

/// `ChunkProvider` is `Send + Sync`: shared-cache fields are atomic,
/// and local-cache fields are only touched by the arena's owning thread.
// SAFETY: `ChunkProvider` is `Send + Sync` even though it contains a
// `LocalSlot<Option<NonNull<LocalChunk<A>>>>` (whose `NonNull` content is
// `!Send`). This is sound only because of an additional runtime
// invariant: every code path that mutates `local_cache` originates
// from `LocalChunk::route_release`, and `LocalChunk` is `!Send` /
// `!Sync` (its header contains `Cell<u64>` and `Cell<Option<...>>`),
// so any release into the local cache is necessarily on the owning
// thread of the chunk. The provider can be shared across threads via
// `Arc<ChunkProvider>` for shared-chunk allocation (which uses
// atomic state) but the local-chunk cache is touched only by the
// owning thread. Removing `LocalChunk: !Send` would invalidate this
// reasoning — keep that constraint or revisit these impls.
unsafe impl<A: Allocator + Clone + Send + Sync> Send for ChunkProvider<A> {}
// SAFETY: see the `Send` impl above.
unsafe impl<A: Allocator + Clone + Send + Sync> Sync for ChunkProvider<A> {}
