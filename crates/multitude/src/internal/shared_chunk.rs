// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::similar_names,
    reason = "short field-name aliases like p_dc/p_rc match the layout diagram at the top of the module"
)]

//! `SharedChunk`: 64 KiB-aligned bump-allocation tile with an atomic
//! refcount, the cross-thread-shareable counterpart to
//! [`LocalChunk`](super::local_chunk::LocalChunk).
//!
//! Cross-thread `Arc::clone` does `fetch_add(1, Relaxed)`,
//! `Arc::drop` does `fetch_sub(1, Release)` followed by an Acquire
//! fence on the last-decrement path. While a chunk is "current" on
//! an arena, the refcount is held inflated at [`LARGE`] and
//! `arcs_issued` (a non-atomic `Cell` on the arena's `current_shared`
//! slot) counts the live arcs; the inflation is reconciled in a
//! single atomic op at chunk swap-out.

use alloc::sync::Weak;
use core::alloc::Layout;
use core::mem;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::chunk_provider::ChunkProvider;
use super::constants::{CHUNK_ALIGN, LARGE, refcount_overflow_abort};
use super::drop_list::DropEntry;
use super::sync::{AtomicPtr, AtomicU16, AtomicUsize, Ordering, fence};

/// Cache-line-padded wrapper. Used to isolate the cross-thread
/// `refcount` atomic from the adjacent read-mostly header fields so
/// `Arc::clone`/`Arc::drop` traffic does not ping-pong the cache line
/// holding the chunk's allocator/provider/capacity slots.
#[repr(C, align(64))]
pub(crate) struct CachePadded<T>(pub(crate) T);

impl<T> CachePadded<T> {
    #[inline]
    pub(crate) const fn new(v: T) -> Self {
        Self(v)
    }
}

/// Cross-thread-shareable bump-allocation tile.
///
/// Allocations are 64 KiB-aligned via the backing allocator's
/// [`Layout`] rather than via `repr(align)`, so sub-64 KiB chunks do
/// not get padded out to a 64 KiB multiple.
#[repr(C)]
pub(crate) struct SharedChunk<A: Allocator + Clone> {
    /// Atomic refcount, cache-padded to keep cross-thread RMW traffic
    /// off the read-mostly fields below. Held at [`LARGE`] while the
    /// chunk is the arena's `current_shared`; swap-out reconciles in
    /// one `fetch_sub(LARGE - arcs_issued, Release)`.
    ///
    /// Placed first to absorb the 64-byte alignment `CachePadded`
    /// imposes on the struct: if `refcount` followed the other
    /// fields, about 48 bytes of padding would precede it.
    pub(crate) refcount: CachePadded<AtomicUsize>,

    /// Clone of the backing allocator. Lets the chunk
    /// free its own backing allocation even after the
    /// [`ChunkProvider`] is gone.
    pub(crate) allocator: A,

    /// Back-edge to the owning provider's cache. `upgrade()` returns
    /// `None` if the provider has already been torn down — the chunk
    /// then self-frees through `allocator`.
    pub(crate) provider: Weak<ChunkProvider<A>>,

    /// Total payload capacity in bytes.
    pub(crate) capacity: usize,

    /// Intrusive list link, thin `AtomicPtr<u8>` (a `*mut SharedChunk`
    /// is fat and can't live in an atomic; reconstructed via
    /// [`Self::from_thin_ptr`]). Used as the
    /// `ChunkProvider::shared_cache_head` Treiber-stack link (`Relaxed`
    /// — the head CAS publishes with Release semantics); null when
    /// the chunk is `current_shared` or in flight between owners.
    pub(crate) next: AtomicPtr<u8>,

    /// Number of [`DropEntry`]s on the back-stack;
    /// `drop_back = capacity - drop_count * size_of::<DropEntry>()`.
    /// `u16` covers about 1024 entries.
    ///
    /// Atomic because `Arc::<MaybeUninit<T>>::assume_init` may run on
    /// a non-owner thread and walks the back-stack to retarget the
    /// placeholder shim. Owner writes Release, cross-thread readers
    /// Acquire; owner-only RMWs are Relaxed.
    ///
    /// Placed after `next` so this `u16` sits flush against `data`
    /// (align 1) instead of forcing a 6-byte padding hole between
    /// two align-8 fields. See
    /// [`super::local_chunk::LocalChunk::drop_count`].
    pub(crate) drop_count: AtomicU16,

    /// Bump-payload tail. `data.len() == capacity`. Declared as
    /// `[UnsafeCell<u8>]` (same layout as `[u8]`) so shared borrows
    /// of the chunk allow interior-mutable writes into the payload —
    /// required for the atomic stores `Arc::assume_init` retargeting
    /// performs into trailing `DropEntry` fields.
    pub(crate) data: [core::cell::UnsafeCell<u8>],
}

// SAFETY: every header field is itself `Send` (the atomic fields are
// intrinsically `Send + Sync`; the user-supplied allocator `A` is
// constrained to `Send + Sync` at the public API boundary
// (`Arena::alloc_arc`)). The payload `data` is reachable cross-thread
// only through `Arc<T>` smart pointers, which require `T: Send + Sync`.
// Per-entry `DropEntry::value_offset` and `DropEntry::len` are written
// non-atomically by the arena-owner thread but published to cross-thread
// observers via the Release `fetch_add` on `drop_count` performed in
// `bump_shared_drop_count`. Foreign threads only iterate entries
// `[0..drop_count.load(Acquire)]`, so every entry they observe is fully
// written.
unsafe impl<A: Allocator + Clone + Send + Sync> Send for SharedChunk<A> {}
// SAFETY: same reasoning as `Send`. `drop_count` is an `AtomicU16` so
// the cross-thread read in `Arc::<MaybeUninit<T>>::assume_init` does
// not race with the arena-owner thread's increments.
unsafe impl<A: Allocator + Clone + Send + Sync> Sync for SharedChunk<A> {}

/// Header size: offset from chunk base to the start of `data`.
///
/// Derived from the real [`SharedChunk`] via `offset_of!` (no shadow
/// struct to drift). The `data` tail has alignment 1, so it sits
/// immediately after `drop_count`. Not necessarily a multiple of
/// `align_of::<DropEntry>()`; back-stack alignment is handled by
/// [`super::drop_list::round_payload`].
#[inline]
pub(crate) const fn header_size<A: Allocator + Clone>() -> usize {
    core::mem::offset_of!(SharedChunk<A>, drop_count) + core::mem::size_of::<AtomicU16>()
}

/// Alignment passed to the backing [`Allocator`]: at least
/// [`CHUNK_ALIGN`] so the pointer-masking trick in [`super::mask`]
/// can recover the chunk header from any interior pointer.
/// `CachePadded<AtomicUsize>` (align 64) is well below `CHUNK_ALIGN`
/// so doesn't raise this further.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(crate) const fn chunk_align<A: Allocator + Clone>() -> usize {
    let a = core::mem::align_of::<A>();
    if a > CHUNK_ALIGN { a } else { CHUNK_ALIGN }
}

/// Round `total` (= `header + payload`) up to the chunk struct's
/// structural alignment so the actual allocation is at least
/// `size_of_val(&*fat_ptr)` — required for `as_ref` soundness.
///
/// Cached classes are multiples of 64 already (`512 << c`); oversized
/// requests may gain up to 63 bytes. `SharedChunk<A>` has structural
/// alignment `max(align_of::<A>(), 64)` (driven by
/// `CachePadded<AtomicUsize>`; no `repr(align(…))`).
///
/// `saturating_add` defends against overflow when `total` is near
/// `usize::MAX`: a wrapped result would land near 0 and let
/// `Layout::from_size_align` silently produce a sub-allocation. See
/// the sibling [`super::local_chunk::alloc_size`].
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(crate) const fn alloc_size<A: Allocator + Clone>(total: usize) -> usize {
    let user = core::mem::align_of::<A>();
    let struct_align = if user > 64 { user } else { 64 };
    let mask = struct_align - 1;
    total.saturating_add(mask) & !mask
}

/// Maximum byte offset within `data` at which a value pointer may
/// land while still being mask-recoverable; mirror of
/// [`super::local_chunk::max_bump_extent`].
#[inline]
#[cfg_attr(test, mutants::skip)] // Boundary mutations require exact-capacity chunks not reachable through public APIs.
pub(crate) const fn max_bump_extent<A: Allocator + Clone>() -> usize {
    CHUNK_ALIGN - header_size::<A>()
}

impl<A: Allocator + Clone> SharedChunk<A> {
    /// Reconstruct a fat `NonNull<SharedChunk>` from a thin chunk
    /// header address. Reads the chunk's `capacity` field to fill in
    /// the slice metadata.
    ///
    /// # Safety
    ///
    /// `addr` must be the base of a live `SharedChunk` header.
    #[inline]
    pub(crate) unsafe fn from_thin_ptr(addr: *mut u8) -> NonNull<Self> {
        let header_only: *const Self = core::ptr::slice_from_raw_parts(addr, 0) as *const Self;
        // SAFETY: see above.
        let capacity = unsafe { (*header_only).capacity };
        let fat: *mut Self = core::ptr::slice_from_raw_parts_mut(addr, capacity) as *mut Self;
        // SAFETY: `addr` is non-null per the caller's invariant.
        unsafe { NonNull::new_unchecked(fat) }
    }

    /// Thin (header-base) pointer to `chunk`, suitable for storing in
    /// the `next` link or the Treiber-stack head tag.
    #[inline]
    pub(crate) fn to_thin_ptr(chunk: NonNull<Self>) -> *mut u8 {
        chunk.as_ptr().cast::<u8>()
    }

    /// Allocate a fresh chunk sized exactly to `total_bytes`. Mirror of
    /// [`super::local_chunk::LocalChunk::allocate`]; see there for the
    /// size/alignment contract on `total_bytes`. The returned chunk
    /// has refcount pre-inflated to [`LARGE`].
    pub(crate) fn allocate(
        allocator: A,
        provider: Weak<ChunkProvider<A>>,
        total_bytes: usize,
    ) -> Result<NonNull<Self>, allocator_api2::alloc::AllocError> {
        let header_bytes = header_size::<A>();
        if total_bytes < header_bytes {
            return Err(allocator_api2::alloc::AllocError);
        }
        let payload = total_bytes - header_bytes;
        // Bump to the struct's structural alignment so `size_of_val` matches.
        let total = alloc_size::<A>(total_bytes);

        let layout = Layout::from_size_align(total, chunk_align::<A>()).map_err(|_e| allocator_api2::alloc::AllocError)?;

        let raw = allocator.allocate(layout)?;
        let raw_ptr = raw.cast::<u8>();

        // Ensure the chunk end fits in isize so internal bump-cursor
        // arithmetic (which asserts isize-fit via `assert_unchecked` on
        // the hot path) is always justified for allocations inside this
        // chunk. This guards against pathological backing allocators
        // returning addresses in the upper half of the address space.
        let start_addr = raw_ptr.as_ptr() as usize;
        if !super::constants::chunk_end_addr_fits_in_isize(start_addr, total) {
            // SAFETY: `raw_ptr`/`layout` came from this allocator's
            // successful `allocate` call.
            unsafe { allocator.deallocate(raw_ptr, layout) };
            return Err(allocator_api2::alloc::AllocError);
        }

        let fat: *mut Self = core::ptr::slice_from_raw_parts_mut(raw_ptr.as_ptr(), payload) as *mut Self;

        // SAFETY: `raw` points at `total` freshly allocated bytes; each
        // header field is initialized before it is read.
        unsafe {
            let p_alloc = core::ptr::addr_of_mut!((*fat).allocator);
            core::ptr::write(p_alloc, allocator);
            let p_prov = core::ptr::addr_of_mut!((*fat).provider);
            core::ptr::write(p_prov, provider);
            let p_cap = core::ptr::addr_of_mut!((*fat).capacity);
            core::ptr::write(p_cap, payload);
            let p_rc = core::ptr::addr_of_mut!((*fat).refcount);
            core::ptr::write(p_rc, CachePadded::new(AtomicUsize::new(LARGE)));
            let p_dc = core::ptr::addr_of_mut!((*fat).drop_count);
            core::ptr::write(p_dc, AtomicU16::new(0));
            let p_next = core::ptr::addr_of_mut!((*fat).next);
            core::ptr::write(p_next, AtomicPtr::<u8>::new(core::ptr::null_mut()));
        }

        // SAFETY: `raw` is non-null per the allocator contract.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    /// Pointer to the first byte of `data`.
    ///
    /// Takes a `NonNull<Self>` (raw) rather than `&Self`: an
    /// `&SharedChunk`-derived `*const u8` would leave a `SharedReadOnly`
    /// tag on the chunk-wide borrow stack, blocking later
    /// `drop_in_place` operations in `free_backing` (which need
    /// Unique). The returned pointer carries chunk-wide provenance
    /// derived directly from the raw `chunk` pointer.
    ///
    /// # Safety
    ///
    /// `chunk` must point to a live `SharedChunk` (dereferenceable
    /// for `alloc_size::<A>(header_size + capacity)` bytes).
    #[inline]
    pub(crate) unsafe fn data_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        let base: *mut u8 = chunk.as_ptr().cast::<u8>();
        // SAFETY: `data` starts at offset `header_size`; the chunk
        // covers at least `header + capacity` bytes.
        let p = unsafe { base.add(header_size::<A>()) };
        // SAFETY: positive in-bounds offset from a non-null base.
        unsafe { NonNull::new_unchecked(p) }
    }

    /// Safe `&self`-based payload-base accessor. The borrow proves the
    /// chunk is live, so [`Self::data_ptr`]'s safety condition is met.
    #[inline]
    pub(crate) fn data(&self) -> NonNull<u8> {
        // SAFETY: `&self` proves the chunk is live for at least the
        // duration of this borrow.
        unsafe { Self::data_ptr(NonNull::from(self)) }
    }

    /// Atomically bump the refcount. Safe because `&self` proves the
    /// chunk is live (some other party already holds a refcount).
    #[inline]
    pub(crate) fn inc_ref(&self) {
        let rc = &self.refcount.0;
        let prev = rc.fetch_add(1, Ordering::Relaxed);
        debug_assert!(prev > 0, "shared inc_ref on a dead chunk");
        check_shared_refcount(prev);
    }

    /// Borrow the chunk's currently-recorded drop entries (Acquire-
    /// ordered load of `drop_count`). Used by cross-thread
    /// `Arc::<MaybeUninit<T>>::assume_init`: the Acquire here pairs
    /// with the owner thread's Release publish so all bytes of
    /// `entries[0..count]` are visible.
    #[inline]
    pub(crate) fn drop_entries_acquire(&self) -> &[DropEntry] {
        let count = self.drop_count.load(Ordering::Acquire) as usize;
        let top = self.capacity;
        let base = top - count * mem::size_of::<DropEntry>();
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "chunk payloads are CHUNK_ALIGN aligned; `data + base` is naturally `DropEntry`-aligned by construction"
        )]
        // SAFETY: chunk-layout invariant — `count` densely packed,
        // naturally aligned `DropEntry`s live at `data + base`, all
        // initialized at allocation time.
        unsafe {
            // SAFETY: caller-held refcount keeps `self` live.
            let data = Self::data_ptr(NonNull::from(self)).as_ptr();
            let ptr = data.add(base).cast::<DropEntry>();
            core::slice::from_raw_parts(ptr, count)
        }
    }

    /// Atomically decrement the refcount; if it just hit zero, route
    /// the chunk back to its provider (or self-free if the provider
    /// is gone). Consumes the caller's refcount.
    ///
    /// # Safety
    ///
    /// Caller must own one of the outstanding refcounts on `chunk`.
    pub(crate) unsafe fn dec_ref(chunk: NonNull<Self>) {
        // Access via raw pointer rather than `&Self`: taking an
        // `&SharedChunk` here would leave a SharedReadOnly tag on the
        // chunk-wide borrow stack, colliding with the `drop_in_place`
        // in `free_backing` (which needs Unique) on the zero-routing
        // path.
        // SAFETY: refcount-positive invariant.
        let prev = unsafe { (*chunk.as_ptr()).refcount.0.fetch_sub(1, Ordering::Release) };
        debug_assert!(prev > 0, "shared dec_ref on a dead chunk");
        if prev == 1 {
            // Hit zero. Synchronize-with all the prior Release decs
            // before observing the chunk's contents.
            fence(Ordering::Acquire);
            // SAFETY: we just took the last refcount; nobody else
            // observes the chunk.
            unsafe { Self::route_release(chunk) };
        }
    }

    /// Refcount-zero release: replay drops and either return the
    /// chunk to its provider's cache or self-free its backing
    /// allocation.
    ///
    /// # Safety
    ///
    /// Caller must have just observed the refcount transition to
    /// zero (with Acquire ordering).
    unsafe fn route_release(chunk: NonNull<Self>) {
        // Raw-pointer access (no `&Self` borrow) so subsequent
        // `free_backing` can `drop_in_place` chunk fields.
        // SAFETY: refcount-zero — exclusive ownership.
        let provider_weak = unsafe { alloc::sync::Weak::clone(&(*chunk.as_ptr()).provider) };
        if let Some(provider) = provider_weak.upgrade() {
            // SAFETY: refcount-zero — we own the chunk; provider
            // releases it to its cache or frees it.
            unsafe { provider.release_shared(chunk) };
        } else {
            // SAFETY: refcount-zero — exclusive ownership.
            unsafe { Self::replay_drops(chunk) };
            // SAFETY: drops just replayed.
            unsafe { Self::free_backing(chunk) };
        }
    }

    /// Replay the trailing drop list and reset the bookkeeping.
    ///
    /// Takes `NonNull<Self>` rather than `&self` so the release path
    /// can run without putting a chunk-wide `SharedReadOnly` tag on the
    /// borrow stack (which would later collide with `free_backing`'s
    /// `drop_in_place`).
    ///
    /// # Safety
    ///
    /// Caller must own the chunk exclusively.
    pub(crate) unsafe fn replay_drops(chunk: NonNull<Self>) {
        // SAFETY: caller-owned chunk.
        let count = unsafe { (*chunk.as_ptr()).drop_count.load(Ordering::Relaxed) } as usize;
        if count == 0 {
            return;
        }
        // SAFETY: caller-owned chunk.
        let capacity = unsafe { (*chunk.as_ptr()).capacity };
        // SAFETY: caller-owned chunk.
        let data = unsafe { Self::data_ptr(chunk) }.as_ptr();
        // Reset BEFORE iteration; see `LocalChunk::replay_drops`
        // for the panic-safety rationale.
        // SAFETY: caller-owned chunk.
        // Relaxed is sufficient: `replay_drops` is only invoked
        // after the caller has driven the refcount to zero (see
        // `dec_ref`/`reconcile_swap_out`), which establishes exclusive
        // ownership of this chunk. No other thread can hold a handle
        // that observes `drop_count` while we reset it. If a future
        // change introduces a diagnostic API that peeks at `drop_count`
        // from outside the refcount-zero state, this store must be
        // upgraded to Release to pair with the consumer's Acquire load.
        unsafe { (*chunk.as_ptr()).drop_count.store(0, Ordering::Relaxed) };
        let top = capacity;
        let base = top - count * mem::size_of::<DropEntry>();
        // SAFETY: chunk-layout invariant — `count` densely packed,
        // naturally aligned `DropEntry`s live at `data + base`.
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "chunk payloads are CHUNK_ALIGN aligned; `data + base` is naturally `DropEntry`-aligned by construction"
        )]
        // SAFETY: `base = capacity - count * size_of::<DropEntry>()`
        // is the byte offset of the first entry; `data` is the chunk's
        // payload base. Together they address `count` initialized
        // `DropEntry`s.
        let entries_ptr = unsafe { data.add(base).cast::<DropEntry>() };
        for i in 0..count {
            // SAFETY: `i < count`; all `count` entries are initialized
            // at allocation time.
            let entry = unsafe { &*entries_ptr.add(i) };
            // SAFETY: payload-extent + drop-shim invariants.
            let value_ptr = unsafe { data.add(entry.value_offset as usize) };
            let f = entry.load_drop_fn(Ordering::Relaxed);
            // Isolate each call (when `std` is available) so a panicking
            // `T::Drop` doesn't abort the chunk reclamation path — at
            // worst we leak that value's resources; the chunk itself
            // still gets freed. Under `no_std` an unwinding `T::Drop`
            // forces a process abort via the `AbortOnUnwind` guard
            // below: this is consistent with `core` semantics under
            // `panic = abort`, and prevents the panic from leaking
            // the chunk by propagating past `route_release` under
            // `panic = unwind`.
            #[cfg(feature = "std")]
            {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // SAFETY: drop-shim invariant.
                    unsafe { f(value_ptr, entry.len as usize) };
                }));
            }
            #[cfg(not(feature = "std"))]
            {
                let abort_guard = crate::internal::drop_list::AbortOnUnwind;
                // SAFETY: drop-shim invariant.
                unsafe { f(value_ptr, entry.len as usize) };
                core::mem::forget(abort_guard);
            }
        }
    }

    /// Tear the chunk down: drop the header fields and free the
    /// backing allocation.
    ///
    /// # Safety
    ///
    /// Caller must own the chunk exclusively (refcount zero) and the
    /// drop list must have already been replayed.
    pub(crate) unsafe fn free_backing(chunk: NonNull<Self>) {
        // SAFETY: caller owns the chunk exclusively.
        let capacity = unsafe { (*chunk.as_ptr()).capacity };

        let total_bytes = alloc_size::<A>(header_size::<A>() + capacity);
        let layout =
            Layout::from_size_align(total_bytes, chunk_align::<A>()).expect("layout was valid at allocation time, must remain valid here");

        // Move `allocator` out so we can free *after* dropping the
        // other header fields.
        // SAFETY: caller owns the chunk; `allocator` is live.
        let allocator: A = unsafe { core::ptr::read(core::ptr::addr_of!((*chunk.as_ptr()).allocator)) };
        // SAFETY: each `addr_of!` field is a live value and we drop
        // it exactly once.
        unsafe {
            core::ptr::drop_in_place(core::ptr::addr_of_mut!((*chunk.as_ptr()).provider));
            core::ptr::drop_in_place(core::ptr::addr_of_mut!((*chunk.as_ptr()).refcount));
            core::ptr::drop_in_place(core::ptr::addr_of_mut!((*chunk.as_ptr()).drop_count));
            core::ptr::drop_in_place(core::ptr::addr_of_mut!((*chunk.as_ptr()).next));
        }

        let raw_ptr = chunk.as_ptr().cast::<u8>();
        // SAFETY: chunk-header invariant — pointer + layout matches
        // what the allocator returned.
        unsafe {
            allocator.deallocate(NonNull::new_unchecked(raw_ptr), layout);
        }
        drop(allocator);
    }

    /// Reset cache-relevant header fields when the chunk is being
    /// handed out from the cache. Restores the refcount inflation to
    /// [`LARGE`] via a Release store so subsequent `alloc_arc`s on
    /// the new tenant chunk are well-defined.
    ///
    /// # Safety
    ///
    /// Caller must own the chunk exclusively (just popped from the
    /// cache).
    pub(crate) unsafe fn revive_for_reuse(&self) {
        // Reset bookkeeping fields *before* re-publishing via the
        // Release refcount store. The caller owns the chunk
        // exclusively at this point (just popped from the cache), but
        // ordering the writes this way keeps the Release store as the
        // synchronizing edge so the pattern is sound even if a future
        // change adds another observer.
        //
        // Visibility of `drop_count = 0` to cross-thread
        // `Arc::<MaybeUninit<T>>::assume_init` readers (which Acquire
        // `drop_count`, not `refcount`) is established transitively
        // through the first `bump_shared_drop_count` Release
        // `fetch_add` after revival: that Release is
        // sequenced-after this Relaxed store of 0, so any Acquire
        // load of `drop_count` observes both the increment and the
        // preceding zero-write.
        self.drop_count.store(0, Ordering::Relaxed);
        self.next.store(core::ptr::null_mut(), Ordering::Relaxed);
        self.refcount.0.store(LARGE, Ordering::Release);
    }

    /// Apply the deferred-reconciliation swap-out math. Performs one
    /// `fetch_sub(LARGE - arcs_issued, Release)`; if the
    /// chunk just hit zero, runs the chunk-return path through the
    /// provider (or self-frees).
    ///
    /// # Safety
    ///
    /// Caller must hold the inflated refcount on `chunk` (i.e.,
    /// `chunk` was the arena's `current_shared` and is being swapped
    /// out). `arcs_issued` is the value of the arena's local
    /// counter at swap-out time.
    pub(crate) unsafe fn reconcile_swap_out(chunk: NonNull<Self>, arcs_issued: usize) {
        // `LARGE` is large enough that this subtraction can never
        // underflow on real workloads (see plan's "Overflow" note).
        let to_subtract = LARGE - arcs_issued;
        // Raw access (no `&SharedChunk` borrow): on the zero-routing
        // branch we go on to `route_release` → `free_backing`.
        // SAFETY: chunk live — caller holds the inflated refcount.
        let prev = unsafe { (*chunk.as_ptr()).refcount.0.fetch_sub(to_subtract, Ordering::Release) };
        debug_assert!(prev >= to_subtract, "shared swap-out reconcile underflow");
        if prev == to_subtract {
            fence(Ordering::Acquire);
            // SAFETY: we observed the last refcount transition to zero
            // with Acquire ordering.
            unsafe { Self::route_release(chunk) };
        }
    }
}

#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline at every inc_ref site to avoid a per-call function-call overhead; see PERF.md"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg_attr(test, mutants::skip)] // Refcount overflow requires physically unreachable outstanding refs.
fn check_shared_refcount(prev: usize) {
    if prev >= LARGE.saturating_add(LARGE) {
        refcount_overflow_abort();
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// Targeted fast-fail for `header_size` mutations. See the mirror
    /// in `local_chunk::tests` for rationale.
    #[test]
    fn header_size_matches_struct_layout() {
        let expected = core::mem::offset_of!(SharedChunk<Global>, drop_count) + core::mem::size_of::<AtomicU16>();
        assert_eq!(header_size::<Global>(), expected);
        assert!(header_size::<Global>() > 0);
    }
}
