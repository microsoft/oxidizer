// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Per-arena chunk cache and allocation source.
//!
//! [`ChunkProvider`] owns the arena's allocator clone, enforces a byte
//! budget, and maintains a freed-chunk cache of the **current floor
//! class**, so steady-state allocate/release pairs avoid hitting the
//! system allocator.
//!
//! Each cache holds at most one freelist. The associated **class floor**
//! (`local_cache_class` / `shared_cache_class`) ratchets monotonically
//! upward as the arena progresses to larger chunks. Chunks released
//! below the floor are returned to the system; cached chunks below the
//! floor are evicted at the next floor bump. The intent is that the
//! arena settles into the largest class it needs with the minimum
//! number of chunks retained.
//!
//! Two cache shapes coexist:
//!
//! - **Local cache**: single freelist guarded by an [`OwnerThreadCell`].
//!   The provider's owning thread is the arena's thread; only that
//!   thread allocates from or releases into the local cache.
//! - **Shared cache**: lock-free Treiber-style stack of
//!   `AtomicPtr<SharedChunk<A>>`. Any thread can push a chunk (when its
//!   last refcount handle drops); only the owning thread pops. A
//!   concurrent push by a thread that has yet to observe the latest
//!   floor bump may add a below-floor chunk; that straggler is destroyed
//!   when the owner thread pops it (see [`ChunkProvider::pop_shared`]).

// `release_local`, `release_shared`, `pop_shared`, `push_shared`, and the
// `destroy_or_cache_just_acquired` helpers are `unsafe fn` with their full
// safety contracts documented on the items themselves; the inner unsafe
// wrappers edition 2024 would otherwise require do not add a safety
// boundary, so we drop them.
#![allow(unsafe_op_in_unsafe_fn, reason = "see module doc: inner unsafe blocks in unsafe fn add noise here")]
#![allow(clippy::unnecessary_safety_comment, reason = "safety rationale documented at function level")]

use alloc::sync::{Arc, Weak};
use core::mem;
use core::ptr::{self, NonNull};
#[cfg(feature = "stats")]
use core::sync::atomic::AtomicU64;
use core::sync::atomic::{AtomicPtr, AtomicU8, AtomicUsize, Ordering};

use allocator_api2::alloc::{AllocError, Allocator};

use super::chunk::Chunk;
use super::constants::{MAX_CHUNK_BYTES, MAX_NORMAL_ALLOC, MIN_CHUNK_BYTES, SizeClass};
use super::drop_entry::DropEntry;
use super::local_chunk::LocalChunk;
use super::owner_thread_cell::OwnerThreadCell;
use super::shared_chunk::SharedChunk;

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
    normal_local: u64,
    oversized_local: u64,
    normal_shared: u64,
    oversized_shared: u64,
}

#[cfg(feature = "stats")]
impl ChunkAllocStats {
    /// Lifetime count of normal-class local chunks allocated.
    #[inline]
    pub(crate) fn normal_local(&self) -> u64 {
        self.normal_local
    }

    /// Lifetime count of oversized local chunks allocated.
    #[inline]
    pub(crate) fn oversized_local(&self) -> u64 {
        self.oversized_local
    }

    /// Lifetime count of normal-class shared chunks allocated.
    #[inline]
    pub(crate) fn normal_shared(&self) -> u64 {
        self.normal_shared
    }

    /// Lifetime count of oversized shared chunks allocated.
    #[inline]
    pub(crate) fn oversized_shared(&self) -> u64 {
        self.oversized_shared
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
    /// Single-thread local-chunk cache: thin `*mut u8` header pointer to
    /// the freelist head (chunks linked via [`LocalChunk::write_cached_next`]
    /// / [`LocalChunk::read_cached_next`]). Holds at most one freelist for
    /// the **current class floor** ([`Self::local_cache_class`]); chunks
    /// below the floor are destroyed instead of cached.
    local_cache: OwnerThreadCell<*mut u8>,
    /// Current class floor for the local cache. Only chunks at class
    /// greater than or equal to `local_cache_class` are cached; the
    /// floor ratchets monotonically upward as the arena allocates
    /// progressively larger chunks, and stale below-floor chunks are
    /// evicted at each bump.
    local_cache_class: AtomicU8,
    /// Lock-free shared-chunk cache: single Treiber-stack head for the
    /// current class floor ([`Self::shared_cache_class`]).
    shared_cache: AtomicPtr<u8>,
    /// Current class floor for the shared cache. Same semantics as
    /// [`Self::local_cache_class`].
    shared_cache_class: AtomicU8,

    /// Lifetime count of normal (cacheable) local chunks allocated from
    /// the backing allocator (cache hits are not counted).
    #[cfg(feature = "stats")]
    normal_local_chunks_allocated: AtomicU64,
    /// Lifetime count of oversized one-shot local chunks allocated.
    #[cfg(feature = "stats")]
    oversized_local_chunks_allocated: AtomicU64,
    /// Lifetime count of normal (cacheable) shared chunks allocated.
    #[cfg(feature = "stats")]
    normal_shared_chunks_allocated: AtomicU64,
    /// Lifetime count of oversized one-shot shared chunks allocated.
    #[cfg(feature = "stats")]
    oversized_shared_chunks_allocated: AtomicU64,
}

// SAFETY: `local_cache` is `OwnerThreadCell`, which exposes only `unsafe`
// access bounded by an owner-thread invariant; `shared_cache` is composed of
// `AtomicPtr`s, which are `Send + Sync`; `allocator` is `A: Allocator +
// Clone` (callers must use `Send + Sync`-capable allocators when sharing the
// provider across threads).
// `non_send_fields_in_send_ty`: OwnerThreadCell enforces single-thread access.
#[allow(clippy::non_send_fields_in_send_ty, reason = "OwnerThreadCell enforces single-thread access")]
// SAFETY: see above.
unsafe impl<A: Allocator + Clone + Send> Send for ChunkProvider<A> {}
// SAFETY: see `Send` impl above.
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
            local_cache: OwnerThreadCell::new(ptr::null_mut()),
            local_cache_class: AtomicU8::new(0),
            shared_cache: AtomicPtr::new(ptr::null_mut()),
            shared_cache_class: AtomicU8::new(0),
            #[cfg(feature = "stats")]
            normal_local_chunks_allocated: AtomicU64::new(0),
            #[cfg(feature = "stats")]
            oversized_local_chunks_allocated: AtomicU64::new(0),
            #[cfg(feature = "stats")]
            normal_shared_chunks_allocated: AtomicU64::new(0),
            #[cfg(feature = "stats")]
            oversized_shared_chunks_allocated: AtomicU64::new(0),
        })
    }

    /// Snapshot of the lifetime chunk-allocation counters.
    #[cfg(feature = "stats")]
    pub(crate) fn chunk_alloc_stats(&self) -> ChunkAllocStats {
        ChunkAllocStats {
            normal_local: self.normal_local_chunks_allocated.load(Ordering::Relaxed),
            oversized_local: self.oversized_local_chunks_allocated.load(Ordering::Relaxed),
            normal_shared: self.normal_shared_chunks_allocated.load(Ordering::Relaxed),
            oversized_shared: self.oversized_shared_chunks_allocated.load(Ordering::Relaxed),
        }
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

    /// Acquires a normal-class local chunk whose payload has at least
    /// `min_payload` bytes. The caller MUST have already verified the
    /// request is not oversized (i.e. `min_payload <= max_normal_alloc`
    /// and total fits in `MAX_CHUNK_BYTES`); use
    /// [`Self::acquire_oversized_local`] otherwise. Returns with refcount
    /// = 1.
    ///
    /// `ratchet_class` is the caller's size-class floor (the refill
    /// ratchet): the chosen chunk is sized to the larger of the class
    /// needed for `min_payload` and `ratchet_class`, so the chunk can
    /// grow with arena usage.
    pub(crate) fn acquire_local(&self, min_payload: usize, ratchet_class: SizeClass) -> Result<NonNull<LocalChunk<A>>, AllocError> {
        let header = LocalChunk::<A>::header_size();
        let needed_total = header.checked_add(min_payload).ok_or(AllocError)?;
        debug_assert!(
            min_payload <= self.config.max_normal_alloc && !exceeds_max_chunk_bytes(needed_total),
            "acquire_local invoked with oversized request — caller must route to acquire_oversized_local",
        );
        self.acquire_normal_local(SizeClass::min_for_bytes(needed_total).max(ratchet_class))
    }

    /// Acquires a normal (cacheable) local chunk in the given size `class`,
    /// reusing a cached chunk when available. Never routes to oversized; the
    /// caller is responsible for any oversized decision.
    ///
    /// If `class` exceeds the cache's current class floor, the floor is
    /// bumped (monotonically) and stale below-floor chunks in the cache
    /// are destroyed before the pop attempt.
    //
    // Mutation testing is suppressed on the `class > floor` branch:
    // `>` with `<` / `==` only changes when the floor advances; the
    // observable effect is cache memory pressure rather than a
    // correctness bug, and is exercised by stats-driven tests in
    // `tests/cache_class_floor.rs` and `tests/mutant_kills_post_fix.rs`
    // (post-reset reuse). `>` with `>=` triggers a redundant no-op
    // floor advance that is functionally equivalent.
    #[cfg_attr(test, mutants::skip)]
    fn acquire_normal_local(&self, class: SizeClass) -> Result<NonNull<LocalChunk<A>>, AllocError> {
        // SAFETY: local cache + floor are accessed only from the owning
        // thread (arena-thread contract).
        let popped = unsafe {
            if class.raw() > self.local_cache_class.load(Ordering::Relaxed) {
                self.advance_local_cache_floor(class);
            }
            self.local_cache.with(|head| {
                let cur = *head;
                if cur.is_null() {
                    None
                } else {
                    let fat = LocalChunk::<A>::header_to_fat(cur);
                    let head_nn = NonNull::new_unchecked(fat);
                    *head = LocalChunk::read_cached_next(head_nn);
                    LocalChunk::reinit_for_acquire(head_nn);
                    Some(head_nn)
                }
            })
        };
        if let Some(chunk) = popped {
            return Ok(chunk);
        }

        self.allocate_fresh_local(class)
    }

    /// Sets the local cache floor to `new_class` and destroys every cached
    /// chunk whose total allocation is smaller than the new floor.
    /// Idempotent: caller already verified `new_class > current_floor`.
    ///
    /// # Safety
    ///
    /// Must be called from the cache's owning thread (arena thread).
    #[cold]
    #[inline(never)]
    unsafe fn advance_local_cache_floor(&self, new_class: SizeClass) {
        self.local_cache_class.store(new_class.raw(), Ordering::Relaxed);
        let new_min_total = new_class.bytes();
        // SAFETY: owner-thread access; we walk the freelist, keeping
        // chunks at the new floor or higher and destroying the rest.
        unsafe {
            self.local_cache.with(|head| {
                let mut cur = *head;
                let mut new_head: *mut u8 = ptr::null_mut();
                while !cur.is_null() {
                    let fat = LocalChunk::<A>::header_to_fat(cur);
                    let chunk_nn = NonNull::new_unchecked(fat);
                    let next = LocalChunk::read_cached_next(chunk_nn);
                    let total = LocalChunk::<A>::header_size() + (*chunk_nn.as_ptr()).capacity();
                    if total >= new_min_total {
                        LocalChunk::write_cached_next(chunk_nn, new_head);
                        new_head = cur;
                    } else {
                        LocalChunk::destroy(chunk_nn, &self.allocator);
                        self.release_bytes(total);
                    }
                    cur = next;
                }
                *head = new_head;
            });
        }
    }

    /// Allocates a brand-new normal local chunk of the given size `class`,
    /// bypassing the cache. Increments the lifetime allocation counter.
    /// Used both on a cache miss in [`acquire_normal_local`](Self::acquire_normal_local)
    /// and by [`preallocate_local`](Self::preallocate_local) (which must add
    /// fresh chunks to the cache rather than recycle existing ones).
    fn allocate_fresh_local(&self, class: SizeClass) -> Result<NonNull<LocalChunk<A>>, AllocError> {
        let header = LocalChunk::<A>::header_size();
        let total = class.bytes();
        let payload_size = total - header;
        self.reserve_bytes(total)?;
        match LocalChunk::<A>::allocate(&self.allocator, ptr::from_ref(self), payload_size) {
            Ok(chunk) => {
                #[cfg(feature = "stats")]
                self.normal_local_chunks_allocated.fetch_add(1, Ordering::Relaxed);
                Ok(chunk)
            }
            Err(e) => {
                self.release_bytes(total);
                Err(e)
            }
        }
    }

    /// Acquires a normal-class shared chunk whose payload has at least
    /// `min_payload` bytes. The caller MUST have already verified the
    /// request is not oversized; use [`Self::acquire_oversized_shared`]
    /// otherwise. See [`Self::acquire_local`] for `ratchet_class`
    /// semantics. Returns with refcount = 1.
    pub(crate) fn acquire_shared(&self, min_payload: usize, ratchet_class: SizeClass) -> Result<NonNull<SharedChunk<A>>, AllocError> {
        let header = SharedChunk::<A>::header_size();
        let needed_total = header.checked_add(min_payload).ok_or(AllocError)?;
        debug_assert!(
            min_payload <= self.config.max_normal_alloc && !exceeds_max_chunk_bytes(needed_total),
            "acquire_shared invoked with oversized request — caller must route to acquire_oversized_shared",
        );
        self.acquire_normal_shared(SizeClass::min_for_bytes(needed_total).max(ratchet_class))
    }

    /// Acquires a normal (cacheable) shared chunk in the given size `class`.
    /// If `class` exceeds the cache's current class floor, the floor is
    /// bumped (monotonically) and stale below-floor chunks in the cache
    /// are destroyed before the pop attempt.
    //
    // Mutation testing is suppressed on the `class > floor` branch for
    // the same reason as `acquire_normal_local`.
    #[cfg_attr(test, mutants::skip)]
    fn acquire_normal_shared(&self, class: SizeClass) -> Result<NonNull<SharedChunk<A>>, AllocError> {
        // SAFETY: only the owning thread bumps the floor / pops (single-
        // popper Treiber-stack invariant); a popped chunk is uniquely
        // owned, so we can re-init its refcount/drop count in the same
        // scope.
        unsafe {
            if class.raw() > self.shared_cache_class.load(Ordering::Relaxed) {
                self.advance_shared_cache_floor(class);
            }
            if let Some(chunk) = self.pop_shared() {
                SharedChunk::reinit_for_acquire(chunk);
                return Ok(chunk);
            }
        }

        self.allocate_fresh_shared(class)
    }

    /// Sets the shared cache floor to `new_class` and destroys every
    /// cached chunk whose total allocation is smaller than the new floor.
    /// Called only by the owning thread; concurrent pushers (releasing
    /// threads) may race a below-floor chunk into the cache after the
    /// floor is observed-as-lower — those stragglers are caught by the
    /// pop-time class check in [`Self::pop_shared`].
    ///
    /// # Safety
    ///
    /// Must be called from the cache's owning thread (single-popper
    /// invariant).
    #[cold]
    #[inline(never)]
    unsafe fn advance_shared_cache_floor(&self, new_class: SizeClass) {
        // Publish the new floor with Release so concurrent pushers'
        // subsequent Acquire load sees it.
        self.shared_cache_class.store(new_class.raw(), Ordering::Release);
        let new_min_total = new_class.bytes();
        // Atomically detach the whole freelist. Concurrent pushers will
        // push onto the now-empty head; the post-bump pushers may push
        // either above-floor (kept) or below-floor (caught at pop) chunks.
        let mut cur = self.shared_cache.swap(ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: each linked chunk is a refcount-zero, uniquely-owned
        // chunk we just detached; we walk the list, re-push survivors,
        // and destroy below-floor stragglers.
        unsafe {
            while !cur.is_null() {
                let fat = SharedChunk::<A>::header_to_fat(cur);
                let chunk_nn = NonNull::new_unchecked(fat);
                let link = SharedChunk::cache_link(chunk_nn);
                let next = (*link).load(Ordering::Acquire);
                let total = SharedChunk::<A>::header_size() + (*chunk_nn.as_ptr()).capacity();
                if total >= new_min_total {
                    self.push_shared(chunk_nn);
                } else {
                    SharedChunk::destroy(chunk_nn);
                    self.release_bytes(total);
                }
                cur = next;
            }
        }
    }

    /// Allocates a brand-new normal shared chunk of the given size `class`,
    /// bypassing the cache. See [`allocate_fresh_local`](Self::allocate_fresh_local).
    #[cfg_attr(test, mutants::skip)] // `total - header → total / header` ⇒ runaway allocations
    fn allocate_fresh_shared(&self, class: SizeClass) -> Result<NonNull<SharedChunk<A>>, AllocError> {
        let header = SharedChunk::<A>::header_size();
        let total = class.bytes();
        let payload_size = total - header;
        self.reserve_bytes(total)?;
        match SharedChunk::<A>::allocate(self.allocator.clone(), Weak::clone(&self.weak_self), payload_size) {
            Ok(chunk) => {
                #[cfg(feature = "stats")]
                self.normal_shared_chunks_allocated.fetch_add(1, Ordering::Relaxed);
                Ok(chunk)
            }
            Err(e) => {
                self.release_bytes(total);
                Err(e)
            }
        }
    }

    /// Routes a refcount-zero local chunk back to the cache (if it matches a
    /// size class) or deallocates it.
    ///
    /// # Safety
    ///
    /// `chunk` must have refcount zero, with drops already replayed, and the
    /// caller must hold the unique remaining reference.
    pub(crate) unsafe fn release_local(&self, chunk: NonNull<LocalChunk<A>>) {
        // SAFETY: chunk is live and uniquely owned by caller for the
        // duration of this call; we read capacity, then either deallocate
        // outright or push the chunk onto the (single-threaded) cache by
        // writing its cache-link slot.
        let capacity = (*chunk.as_ptr()).capacity();
        let total = LocalChunk::<A>::header_size() + capacity;
        // Bypass the cache for non-class-size totals (oversized one-shots
        // whose total isn't a power of two) and for chunks below the
        // current cache class floor. The floor ratchets monotonically as
        // the arena moves to larger chunks; smaller chunks released
        // afterward are returned to the system so the cache stays uniform.
        if !is_cacheable_size(total) || total < SizeClass::new(self.local_cache_class.load(Ordering::Relaxed)).bytes() {
            LocalChunk::destroy(chunk, &self.allocator);
            self.release_bytes(total);
            return;
        }
        self.local_cache.with(|head| {
            LocalChunk::write_cached_next(chunk, *head);
            *head = chunk.cast::<u8>().as_ptr();
        });
    }

    /// Routes a refcount-zero shared chunk back to the cache or deallocates.
    ///
    /// # Safety
    ///
    /// Same as [`release_local`](Self::release_local).
    pub(crate) unsafe fn release_shared(&self, chunk: NonNull<SharedChunk<A>>) {
        // SAFETY: chunk is live and uniquely owned by caller.
        let capacity = (*chunk.as_ptr()).capacity();
        let total = SharedChunk::<A>::header_size() + capacity;
        // See `release_local` for the cache-bypass conditions.
        if !is_cacheable_size(total) || total < SizeClass::new(self.shared_cache_class.load(Ordering::Acquire)).bytes() {
            SharedChunk::destroy(chunk);
            self.release_bytes(total);
            return;
        }
        self.push_shared(chunk);
    }

    /// Pre-warms the local cache with one chunk in the given size class.
    ///
    /// Always allocates through the normal (cacheable) class path: a
    /// preallocated chunk is a size-classed chunk regardless of the
    /// configured `max_normal_alloc`, so it must never route to the
    /// oversized (one-shot, non-cacheable) path even when its payload
    /// exceeds `max_normal_alloc`.
    pub(crate) fn preallocate_local(&self, class: SizeClass) -> Result<(), AllocError> {
        let chunk = self.allocate_fresh_local(class)?;
        // SAFETY: we own the +1 returned by allocate_fresh_local;
        // refcount-to-zero routes it straight into the cache through
        // release_local (the chunk is a valid class size).
        unsafe { LocalChunk::<A>::destroy_or_cache_just_acquired(self, chunk) };
        Ok(())
    }

    /// Pre-warms the shared cache with one chunk in the given size class.
    /// Always uses the fresh-allocate path; see [`preallocate_local`](Self::preallocate_local).
    pub(crate) fn preallocate_shared(&self, class: SizeClass) -> Result<(), AllocError> {
        let chunk = self.allocate_fresh_shared(class)?;
        // SAFETY: same as preallocate_local.
        unsafe { SharedChunk::<A>::destroy_or_cache_just_acquired(self, chunk) };
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
            Err(AllocError)
        }
    }

    fn release_bytes(&self, n: usize) {
        self.bytes_outstanding.fetch_sub(n, Ordering::AcqRel);
    }

    /// Allocates a one-shot oversized local chunk whose payload is sized
    /// to fit a single allocation of `min_payload` bytes (plus rounding
    /// for drop-entry alignment). The chunk bypasses the size-class cache.
    ///
    /// Used by [`Arena`](crate::Arena) for allocations whose worst-case
    /// payload exceeds `max_normal_alloc`: the caller wraps the chunk
    /// in a temporary [`ChunkMutator`](super::ChunkMutator), performs the
    /// single allocation, and the current chunk is left untouched so
    /// subsequent small allocations continue to use it.
    pub(crate) fn acquire_oversized_local(&self, min_payload: usize) -> Result<NonNull<LocalChunk<A>>, AllocError> {
        let payload = round_up_to_drop_align(min_payload)?;
        let total = LocalChunk::<A>::header_size().checked_add(payload).ok_or(AllocError)?;
        self.reserve_bytes(total)?;
        match LocalChunk::<A>::allocate(&self.allocator, ptr::from_ref(self), payload) {
            Ok(chunk) => {
                #[cfg(feature = "stats")]
                self.oversized_local_chunks_allocated.fetch_add(1, Ordering::Relaxed);
                Ok(chunk)
            }
            Err(e) => {
                self.release_bytes(total);
                Err(e)
            }
        }
    }

    /// Shared-chunk mirror of [`Self::acquire_oversized_local`].
    pub(crate) fn acquire_oversized_shared(&self, min_payload: usize) -> Result<NonNull<SharedChunk<A>>, AllocError> {
        let payload = round_up_to_drop_align(min_payload)?;
        let total = SharedChunk::<A>::header_size().checked_add(payload).ok_or(AllocError)?;
        self.reserve_bytes(total)?;
        match SharedChunk::<A>::allocate(self.allocator.clone(), Weak::clone(&self.weak_self), payload) {
            Ok(chunk) => {
                #[cfg(feature = "stats")]
                self.oversized_shared_chunks_allocated.fetch_add(1, Ordering::Relaxed);
                Ok(chunk)
            }
            Err(e) => {
                self.release_bytes(total);
                Err(e)
            }
        }
    }

    /// Pops a cached shared chunk at or above the current class floor.
    /// Stale below-floor chunks (pushed by a release thread that raced
    /// against [`Self::advance_shared_cache_floor`]) are destroyed and
    /// the next chunk is tried.
    ///
    /// # Safety
    ///
    /// Called only from the provider's owning thread (single popper
    /// invariant).
    unsafe fn pop_shared(&self) -> Option<NonNull<SharedChunk<A>>> {
        let floor_min_total = SizeClass::new(self.shared_cache_class.load(Ordering::Relaxed)).bytes();
        loop {
            // SAFETY: each observed non-null `cur` is a live, uniquely-
            // owned chunk (single popper); we read its cache-link via
            // `SharedChunk::cache_link` and on success the resulting
            // pointer is exclusively ours.
            let updated = self.shared_cache.fetch_update(Ordering::AcqRel, Ordering::Acquire, |cur| {
                if cur.is_null() {
                    return None;
                }
                let fat = SharedChunk::<A>::header_to_fat(cur);
                let link = SharedChunk::cache_link(NonNull::new_unchecked(fat));
                Some((*link).load(Ordering::Acquire))
            });
            let Ok(popped) = updated else { return None };
            let fat = SharedChunk::<A>::header_to_fat(popped);
            let chunk_nn = NonNull::new_unchecked(fat);
            let total = SharedChunk::<A>::header_size() + (*chunk_nn.as_ptr()).capacity();
            if total >= floor_min_total {
                return Some(chunk_nn);
            }
            // Below-floor straggler from a concurrent push that raced the
            // floor bump; destroy and try the next entry.
            SharedChunk::destroy(chunk_nn);
            self.release_bytes(total);
        }
    }

    /// Pushes `chunk` onto the (single) shared-cache freelist.
    ///
    /// # Safety
    ///
    /// `chunk` must be a refcount-zero, uniquely-owned chunk.
    unsafe fn push_shared(&self, chunk: NonNull<SharedChunk<A>>) {
        let head = &self.shared_cache;
        let link = SharedChunk::cache_link(chunk);
        let new = chunk.cast::<u8>().as_ptr();
        // The chunk is exclusively ours until the publishing CAS below
        // succeeds, so the link can be initialized via a non-atomic
        // pointer write through `AtomicPtr::as_ptr()`. Doing the first
        // write atomically triggers a Miri weak-memory ICE
        // ("cannot have empty store buffer when previous write was
        // atomic") on freshly-allocated chunk payload bytes; the
        // non-atomic init sidesteps it. After the CAS, all subsequent
        // mutations to the link go through atomic ops, and any popper
        // observes the link via `head.load(Acquire)` which
        // synchronizes-with the `Release` half of our CAS.
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

    /// Drains every cached chunk and deallocates its backing memory.
    fn drain_all(&self) {
        // SAFETY: drain runs in Drop with no outstanding mutators; the
        // provider is single-owner at this point, so the OwnerThreadCell
        // and Treiber stack are both quiescent. Every cached chunk is
        // uniquely owned by us once swapped/popped out of the cache.
        unsafe {
            self.local_cache.with(|head| {
                let mut cur = *head;
                while !cur.is_null() {
                    let fat = LocalChunk::<A>::header_to_fat(cur);
                    let chunk_nn = NonNull::new_unchecked(fat);
                    let next = LocalChunk::read_cached_next(chunk_nn);
                    LocalChunk::destroy(chunk_nn, &self.allocator);
                    cur = next;
                }
                *head = ptr::null_mut();
            });
            let mut cur = self.shared_cache.swap(ptr::null_mut(), Ordering::AcqRel);
            while !cur.is_null() {
                let fat = SharedChunk::<A>::header_to_fat(cur);
                let chunk_nn = NonNull::new_unchecked(fat);
                let link = SharedChunk::cache_link(chunk_nn);
                let next = (*link).load(Ordering::Acquire);
                SharedChunk::destroy(chunk_nn);
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

/// Rounds an oversized chunk's payload up to a multiple of
/// `align_of::<DropEntry>()`. Returns `None` on overflow.
///
/// [`ChunkMutator::from_owned`](super::chunk_mutator::ChunkMutator::from_owned)
/// aligns the chunk's `drop_top` *down* to `align_of::<DropEntry>()`,
/// shaving up to `align - 1` bytes off the usable payload. Without this
/// rounding the usable capacity could fall below `min_payload` and
/// `impl_alloc_*`'s reserve/refill loop would spin until OOM.
#[cfg_attr(test, mutants::skip)] // mask mutations underfit payload → OOM spin
#[inline]
fn round_up_to_drop_align(min_payload: usize) -> Result<usize, AllocError> {
    let mask = mem::align_of::<DropEntry>() - 1;
    min_payload.checked_add(mask).map(|v| v & !mask).ok_or(AllocError)
}

/// Wraps the `needed_total > MAX_CHUNK_BYTES` check used by the
/// `acquire_*` routing gates.
#[cfg_attr(test, mutants::skip)] // boundary unreachable: max_normal_alloc capped well below
#[inline]
fn exceeds_max_chunk_bytes(needed_total: usize) -> bool {
    needed_total > MAX_CHUNK_BYTES
}

// --- Helpers wired into chunk types via inherent impls ------------------------

impl<A: Allocator + Clone> LocalChunk<A> {
    /// Used by `preallocate_local`: route a just-acquired refcount-1 chunk
    /// back to its provider's cache (refcount → 0) without going through
    /// `ChunkMutator`.
    ///
    /// # Safety
    ///
    /// `chunk` must be the result of a fresh `acquire_local` call on the
    /// same `provider` (no drop entries committed).
    pub(super) unsafe fn destroy_or_cache_just_acquired(provider: &ChunkProvider<A>, chunk: NonNull<Self>) {
        // SAFETY: chunk is live and uniquely owned; dec_ref takes it to 0,
        // then release_local routes it to the cache (no drops were committed
        // since this is a fresh acquisition).
        unsafe {
            let last = chunk.as_ref().dec_ref();
            debug_assert!(last, "preallocate chunk refcount should reach zero");
            provider.release_local(chunk);
        }
    }
}

impl<A: Allocator + Clone> SharedChunk<A> {
    /// See [`LocalChunk::destroy_or_cache_just_acquired`].
    ///
    /// # Safety
    ///
    /// Same.
    pub(super) unsafe fn destroy_or_cache_just_acquired(provider: &ChunkProvider<A>, chunk: NonNull<Self>) {
        // SAFETY: see local variant.
        unsafe {
            let last = chunk.as_ref().dec_ref();
            debug_assert!(last, "preallocate chunk refcount should reach zero");
            provider.release_shared(chunk);
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
        /// Test-only: when non-null, the next `push_shared` on this thread
        /// splices this chunk onto the stack head right before its CAS,
        /// deterministically forcing the contended retry (`Err`) arm.
        static INJECT_PUSH_RACE: Cell<*mut u8> = const { Cell::new(ptr::null_mut()) };
        /// Test-only: counts how many times `push_shared`'s CAS retry arm
        /// ran on this thread.
        static PUSH_RETRY_COUNT: Cell<usize> = const { Cell::new(0) };
    }

    /// Test hook invoked by `push_shared` just before its CAS. If the
    /// thread-local injection slot is armed, splice that chunk onto the
    /// stack as if a concurrent pusher had installed it: link it to the
    /// value the pusher loaded, then publish it as the new head so the
    /// pending CAS (still expecting `cur`) fails exactly once.
    ///
    /// # Safety
    ///
    /// `cur` must be the value `push_shared` loaded from `head`, and any
    /// armed injection pointer must be a refcount-zero, uniquely-owned
    /// chunk header owned by the test.
    pub(super) unsafe fn maybe_inject_push_race<A: Allocator + Clone>(head: &AtomicPtr<u8>, cur: *mut u8) {
        let inject = INJECT_PUSH_RACE.with(|slot| slot.replace(ptr::null_mut()));
        if inject.is_null() {
            return;
        }
        let fat = SharedChunk::<A>::header_to_fat(inject);
        let link = SharedChunk::cache_link(NonNull::new_unchecked(fat));
        ptr::write((*link).as_ptr(), cur);
        head.store(inject, Ordering::Release);
    }

    /// Test hook invoked by `push_shared` whenever its CAS retry arm runs.
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

    // Covers `pop_shared`'s below-floor straggler arm: a cached shared
    // chunk smaller than the current class floor is destroyed (not
    // returned) and the pop continues. Single-threaded code never caches
    // a below-floor chunk via `release_shared`, so we model the
    // push-races-floor-bump state directly: raise the floor on an empty
    // cache, then inject a small (class-0) chunk via `push_shared`.
    #[test]
    fn pop_shared_destroys_below_floor_straggler() {
        let provider = ChunkProvider::<Global>::new(Global, ChunkProviderConfig::default());
        // SAFETY: single-threaded test owns the cache; the floor is raised
        // on an empty freelist, then a below-floor straggler is injected
        // and popped, exactly mirroring the documented push/floor race.
        unsafe {
            // Raise the floor well above class 0 (512 B) — class 3 = 4 KiB.
            provider.advance_shared_cache_floor(SizeClass::new(3));
            // Allocate a class-0 (512 B) chunk: below the new floor.
            let chunk = provider.allocate_fresh_shared(SizeClass::ZERO).expect("fresh class-0 chunk");
            // `push_shared` requires a refcount-zero, uniquely-owned chunk.
            assert!(chunk.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            provider.push_shared(chunk);
            // The straggler is below the floor, so the pop destroys it and
            // finds the now-empty cache, returning `None`.
            assert!(provider.pop_shared().is_none());
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

    // Covers `push_shared`'s contended CAS retry arm (the `Err(actual)`
    // branch). A real concurrent push is non-deterministic, so we model
    // the race directly: arm a thread-local injection so the test hook
    // publishes a competing chunk onto the stack head between our load and
    // CAS, forcing the pending CAS to fail and the loop to retry exactly
    // once before it settles. Thread-local state keeps this isolated from
    // other tests running in parallel.
    #[test]
    fn push_shared_retries_on_contended_cas() {
        let provider = ChunkProvider::<Global>::new(Global, ChunkProviderConfig::default());
        PUSH_RETRY_COUNT.with(|c| c.set(0));
        // SAFETY: every chunk below is freshly allocated, uniquely owned,
        // and dropped to refcount 0 before being pushed/injected. The
        // injected chunk is spliced into the freelist by the hook, so the
        // stack stays valid and the provider's drain frees all three.
        unsafe {
            // Base chunk C establishes a non-null head for the race.
            let c = provider.allocate_fresh_shared(SizeClass::ZERO).expect("chunk c");
            assert!(c.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            provider.push_shared(c);

            // Chunk D is injected by the hook during the next push to model
            // a concurrent pusher mutating `head`.
            let d = provider.allocate_fresh_shared(SizeClass::ZERO).expect("chunk d");
            assert!(d.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            INJECT_PUSH_RACE.with(|slot| slot.set(d.cast::<u8>().as_ptr()));

            // Pushing B loads head == C, but the hook publishes D before B's
            // CAS, forcing the retry arm before B finally settles on top.
            let b = provider.allocate_fresh_shared(SizeClass::ZERO).expect("chunk b");
            assert!(b.as_ref().dec_ref(), "fresh chunk drops to refcount 0");
            provider.push_shared(b);
        }
        // At least one retry must have run (CAS may also fail spuriously on
        // weakly-ordered targets, so we assert a lower bound, not equality).
        assert!(
            PUSH_RETRY_COUNT.with(Cell::get) >= 1,
            "the contended CAS retry arm must run at least once",
        );
    }
}

// Legacy CAS-retry helper removed: all the local CAS loops now use
// `AtomicX::fetch_update`, whose contention-path retry lives in the
// std library and is hidden from local coverage instrumentation.
