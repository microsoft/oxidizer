// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::similar_names,
    reason = "short field-name aliases like p_dc/p_rc match the layout diagram at the top of the module"
)]

//! `LocalChunk`: a 64 KiB-aligned bump-allocation tile.
//!
//! Layout:
//!
//! ```text
//! +--------+----------+--------+----------+--------+--------+
//! | alloc  | provider | cap    | refcount | next   | data   |
//! | : Arc  | : Weak   | : usz  | : Cell   | : Cell | [u8;n] |
//! +--------+----------+--------+----------+--------+--------+
//!  cold      cold       warm     hot         cold    payload
//! ```
//!
//! The struct is `repr(C, align(65536))` so that `addr & !0xFFFF`
//! recovers the header from any payload pointer. The payload `data`
//! tail is a `[u8]` slice whose runtime length is the chunk's
//! capacity in bytes.

use alloc::sync::Weak;
use core::alloc::Layout;
use core::cell::Cell;
use core::mem;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

use allocator_api2::alloc::Allocator;

use super::chunk_provider::ChunkProvider;
use super::constants::{CHUNK_ALIGN, LARGE, refcount_overflow_abort};
use super::drop_list::DropEntry;

/// A single bump-allocation tile owned by an
/// [`Arena`](crate::Arena).
///
/// Allocations are 64 KiB-aligned via the [`Layout`] passed to the
/// backing allocator (not via `repr(align)`), so chunks smaller than
/// 64 KiB do not need to be padded out to a 64 KiB multiple. The
/// type itself uses pointer alignment so the compiler does not
/// inflate the size of a sub-64 KiB chunk to a 64 KiB multiple.
///
/// While the chunk is the arena's `current_local`, its refcount is
/// held inflated at [`LARGE`] and per-allocation increments are tracked in
/// the arena's `current_local.rcs_issued` cell. At swap-out the
/// arena reconciles in one `set(prev - (LARGE - rcs_issued -
/// pinned))` — see [`Self::reconcile_swap_out`].
#[repr(C)]
pub(crate) struct LocalChunk<A: Allocator + Clone> {
    /// Clone of the backing allocator. Lets the chunk
    /// free its own backing allocation even after the
    /// [`ChunkProvider`] is gone.
    pub(crate) allocator: A,

    /// Back-edge to the owning provider's cache. `upgrade()` returns
    /// `None` if the provider has already been torn down — the chunk
    /// then self-frees through `allocator`.
    pub(crate) provider: Weak<ChunkProvider<A>>,

    /// Total payload capacity in bytes. Set at construction; used to
    /// reconstruct the DST fat pointer's metadata when the chunk
    /// returns to the provider's cache or self-frees.
    pub(crate) capacity: usize,

    /// Single-threaded refcount. The arena's `current_local` slot,
    /// the pin list, and every outstanding [`crate::Rc`] each
    /// contribute exactly one to this count.
    pub(crate) refcount: Cell<usize>,

    /// Intrusive list link reused across three contexts (arena's
    /// pinned list, provider's cache list, "in neither"). At most one
    /// of those owns the link at any moment.
    pub(crate) next: Cell<Option<NonNull<Self>>>,

    /// Number of [`super::drop_list::DropEntry`]s on the back-stack.
    /// `drop_back` lives at `capacity - drop_count * size_of::<DropEntry>()`.
    /// `u16` covers about 1024 entries — well above what a 16 KiB
    /// chunk can hold (16-byte entries).
    ///
    /// Placed after `next` so this `u16` sits flush against `data`
    /// (align 1) instead of forcing a 6-byte padding hole between
    /// two align-8 fields.
    pub(crate) drop_count: Cell<u16>,

    /// Bump-payload tail. `data.len() == capacity`. Declared as
    /// `[UnsafeCell<u8>]` (same layout as `[u8]`) so that shared
    /// borrows of the chunk allow interior-mutable writes into the
    /// payload — required for the atomic stores into `DropEntry`
    /// fields published via `assume_init` retargeting.
    pub(crate) data: [core::cell::UnsafeCell<u8>],
}

/// Header size: offset from chunk base to the start of `data`.
///
/// Derived from the real [`LocalChunk`] via `offset_of!` (no shadow
/// struct to drift). The `data` tail has alignment 1, so it sits
/// immediately after `drop_count`. Not necessarily a multiple of
/// `align_of::<DropEntry>()`; back-stack alignment is handled by
/// [`super::drop_list::round_payload`].
#[inline]
pub(crate) const fn header_size<A: Allocator + Clone>() -> usize {
    core::mem::offset_of!(LocalChunk<A>, drop_count) + core::mem::size_of::<Cell<u16>>()
}

/// Alignment passed to the backing [`Allocator`]: at least
/// [`CHUNK_ALIGN`] so the pointer-masking trick in [`super::mask`]
/// can recover the chunk header from any interior pointer.
#[inline]
#[cfg_attr(test, mutants::skip)] // Only observable via allocator-level alignment, which is always over-satisfied.
pub(crate) const fn chunk_align<A: Allocator + Clone>() -> usize {
    let a = core::mem::align_of::<A>();
    if a > CHUNK_ALIGN { a } else { CHUNK_ALIGN }
}

/// Round `total` (= `header + payload`) up to the chunk struct's
/// structural alignment so the actual allocation is at least
/// `size_of_val(&*fat_ptr)` — required for `as_ref` soundness.
///
/// Cached classes are powers of two ≥ 512 and already aligned;
/// oversized requests may gain up to `struct_align - 1` bytes.
/// `LocalChunk<A>` has structural alignment `max(align_of::<A>(), 8)`
/// (driven by `Cell<usize>` and friends; no `repr(align(…))`).
///
/// `saturating_add` defends against overflow when `total` is near
/// `usize::MAX`: a wrapped result would land near 0 and let
/// `Layout::from_size_align` silently produce a sub-allocation. The
/// saturated value remains well above `isize::MAX`, so the layout
/// builder cleanly returns `Err` instead.
#[inline]
#[cfg_attr(test, mutants::skip)]
pub(crate) const fn alloc_size<A: Allocator + Clone>(total: usize) -> usize {
    let user = core::mem::align_of::<A>();
    let struct_align = if user > 8 { user } else { 8 };
    let mask = struct_align - 1;
    total.saturating_add(mask) & !mask
}

/// Maximum byte offset within `data` at which a value pointer may
/// land while still being recoverable by the `addr & !(CHUNK_ALIGN - 1)`
/// mask in [`super::mask`]. Cached chunks have
/// `capacity ≤ max_bump_extent` by construction.
#[inline]
#[cfg_attr(test, mutants::skip)] // Identity-style mutations are only observable through downstream allocation invariants.
pub(crate) const fn max_bump_extent<A: Allocator + Clone>() -> usize {
    CHUNK_ALIGN - header_size::<A>()
}

impl<A: Allocator + Clone> LocalChunk<A> {
    /// Allocate a fresh chunk sized exactly to `total_bytes`
    /// (header + payload), wired up to `provider` and freed through
    /// `allocator`. Sets `capacity = total_bytes - header_size::<A>()`.
    ///
    /// `total_bytes` must satisfy `(total_bytes - header_size) % align_of::<DropEntry>() == 0`
    /// so the back-stack stays aligned — automatic for cached classes
    /// (powers of two ≥ [`MIN_CHUNK_BYTES`]) and for oversized
    /// requests routed through [`round_payload`](super::drop_list::round_payload).
    ///
    /// Returns the chunk with refcount pre-inflated to [`LARGE`]; the
    /// arena's `rcs_issued` cell tracks per-allocation increments
    /// non-atomically and reconciles at swap-out via
    /// [`Self::reconcile_swap_out`].
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
            core::ptr::write(p_rc, Cell::new(LARGE));
            let p_dc = core::ptr::addr_of_mut!((*fat).drop_count);
            core::ptr::write(p_dc, Cell::new(0));
            let p_next = core::ptr::addr_of_mut!((*fat).next);
            core::ptr::write(p_next, Cell::new(None));
        }

        // SAFETY: `raw` is non-null per the allocator contract.
        Ok(unsafe { NonNull::new_unchecked(fat) })
    }

    /// Pointer to the first byte of `data`.
    ///
    /// Takes a `NonNull<Self>` (raw) rather than `&Self`: an
    /// `&LocalChunk`-derived `*const u8` would leave a `SharedReadOnly`
    /// tag on the chunk-wide borrow stack, blocking later
    /// `drop_in_place` operations in `free_backing` (which need
    /// Unique). The returned pointer carries chunk-wide provenance
    /// derived directly from the raw `chunk` pointer.
    ///
    /// # Safety
    ///
    /// `chunk` must point to a live `LocalChunk` (dereferenceable for
    /// `header + capacity` bytes).
    #[inline]
    pub(crate) unsafe fn data_ptr(chunk: NonNull<Self>) -> NonNull<u8> {
        let base: *mut u8 = chunk.as_ptr().cast::<u8>();
        // SAFETY: `data` begins at offset `header_size::<A>()`; the
        // chunk is at least `header + capacity` bytes.
        let p = unsafe { base.add(header_size::<A>()) };
        // SAFETY: `base` is non-null; positive in-bounds offset
        // preserves non-null.
        unsafe { NonNull::new_unchecked(p) }
    }

    /// Increment the refcount. Safe because `&self` proves the chunk
    /// is live (some other party already holds a +1).
    #[inline]
    pub(crate) fn inc_ref(&self) {
        let rc = &self.refcount;
        let prev = rc.get();
        debug_assert!(prev > 0, "inc_ref on a dead chunk");
        check_local_refcount(prev);
        rc.set(prev + 1);
    }

    /// Borrow the chunk's currently-recorded drop entries as a slice,
    /// oldest first (lowest payload address) to newest. The slice
    /// remains valid for the lifetime of the borrow even if `drop_count`
    /// is concurrently reset to zero — the underlying memory persists
    /// until the chunk is freed.
    #[inline]
    pub(crate) fn drop_entries(&self) -> &[DropEntry] {
        let count = self.drop_count.get() as usize;
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
            // SAFETY: caller-held refcount keeps `self` live, so the
            // matching `NonNull` is dereferenceable for `data_ptr`.
            let data = Self::data_ptr(NonNull::from(self)).as_ptr();
            let ptr = data.add(base).cast::<DropEntry>();
            core::slice::from_raw_parts(ptr, count)
        }
    }

    /// Decrement the refcount; if it just hit zero, route the chunk
    /// back to its provider (or self-free if the provider is gone).
    /// Consumes the caller's refcount.
    ///
    /// # Safety
    ///
    /// Caller must own one of the per-allocation increments.
    #[inline]
    pub(crate) unsafe fn dec_ref(chunk: NonNull<Self>) {
        // Access via raw pointer rather than `&Self`: an `&LocalChunk`
        // tag would persist on the chunk-wide borrow stack and collide
        // with the `drop_in_place` in `free_backing` (Unique access)
        // on the zero-routing path.
        // SAFETY: refcount-positive invariant — caller owns +1.
        let prev = unsafe { (*chunk.as_ptr()).refcount.get() };
        debug_assert!(prev > 0, "dec_ref on a dead chunk");
        // SAFETY: same as above.
        unsafe { (*chunk.as_ptr()).refcount.set(prev - 1) };
        if prev == 1 {
            // SAFETY: we just took the last refcount; nobody else
            // observes the chunk.
            unsafe { Self::route_release(chunk) };
        }
    }

    /// Replay the trailing drop list and reset the drop bookkeeping
    /// so the chunk's payload is logically empty.
    ///
    /// Takes `NonNull<Self>` rather than `&self` so the entire release
    /// path can operate without creating a chunk-wide `SharedReadOnly`
    /// tag that would later collide with `free_backing`'s
    /// `drop_in_place` (which needs Unique).
    ///
    /// # Safety
    ///
    /// Caller must own a `+1` (or the last `+1`) on the chunk; no
    /// other strand may be observing the payload region while this
    /// runs.
    pub(crate) unsafe fn replay_drops(chunk: NonNull<Self>) {
        // SAFETY: caller-owned refcount keeps the chunk live.
        let count = unsafe { (*chunk.as_ptr()).drop_count.get() } as usize;
        if count == 0 {
            return;
        }
        // SAFETY: caller-owned refcount.
        let capacity = unsafe { (*chunk.as_ptr()).capacity };
        // SAFETY: caller-owned refcount.
        let data = unsafe { Self::data_ptr(chunk) }.as_ptr();
        // Reset BEFORE iterating: if a user `Drop` panics, a subsequent
        // observer must not see the already-replayed entries.
        // SAFETY: caller-owned refcount.
        unsafe { (*chunk.as_ptr()).drop_count.set(0) };
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
            // SAFETY: `i < count`, all `count` entries are initialized
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
            // below (consistent with `core` semantics under
            // `panic = abort`; for `panic = unwind` builds, this
            // prevents the panic from leaking the chunk by
            // propagating out past `route_release`).
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
    /// backing allocation through the chunk's own allocator.
    /// The caller is responsible for having already replayed any
    /// pending drop entries via [`Self::replay_drops`].
    ///
    /// # Safety
    ///
    /// Caller must own the last `+1` and must already have decremented
    /// the refcount to zero (so no other owner exists). The drop list
    /// must already have been replayed (or the chunk's `drop_count`
    /// must be `0`).
    pub(crate) unsafe fn free_backing(chunk: NonNull<Self>) {
        // SAFETY: caller owns the last +1.
        let capacity = unsafe { (*chunk.as_ptr()).capacity };

        let total_bytes = alloc_size::<A>(header_size::<A>() + capacity);
        let layout =
            Layout::from_size_align(total_bytes, chunk_align::<A>()).expect("layout was valid at allocation time, must remain valid here");

        // Move `allocator` out so we can free with it *after* the
        // header runs its destructors.
        // SAFETY: we own the last reference; `allocator` is live.
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
        // SAFETY: the chunk's pointer + layout is exactly what
        // `A::allocate` returned (chunk-header invariant).
        unsafe {
            allocator.deallocate(NonNull::new_unchecked(raw_ptr), layout);
        }

        drop(allocator);
    }

    /// Tear the chunk down: replay the trailing drop list, free the
    /// backing allocation through the chunk's own allocator.
    ///
    /// # Safety
    ///
    /// Caller must own the last `+1` and must already have decremented
    /// the refcount to zero (so no other owner exists).
    pub(crate) unsafe fn destroy(chunk: NonNull<Self>) {
        // Pure raw-pointer release path: no `&Self` borrow on the
        // chunk anywhere, so `free_backing`'s subsequent `drop_in_place`
        // sees Unique access on the chunk allocation.
        // SAFETY: caller owns the last +1.
        unsafe { Self::replay_drops(chunk) };
        // SAFETY: caller owns the last +1; drops just replayed.
        unsafe { Self::free_backing(chunk) };
    }

    /// Reset cache-relevant header fields when a chunk is handed
    /// back out of the cache: clear `next`, reset `drop_count` (the
    /// ledger was already replayed at release time), and restore the
    /// [`LARGE`] refcount inflation for the new tenant.
    ///
    /// # Safety
    ///
    /// Caller must own the chunk exclusively (just popped from cache).
    pub(crate) unsafe fn revive_for_reuse(&self) {
        self.refcount.set(LARGE);
        self.drop_count.set(0);
        self.next.set(None);
    }

    /// Apply the deferred-reconciliation swap-out math. Performs one
    /// `set(prev - (LARGE - rcs_issued - pinned))` on the chunk's
    /// refcount; if the chunk just hit zero, runs the chunk-return
    /// path through the provider (or self-frees).
    ///
    /// # Safety
    ///
    /// Caller must hold the inflated refcount on the chunk (i.e.,
    /// the chunk was the arena's `current_local` and is being swapped
    /// out). `rcs_issued` is the value of the arena's local
    /// counter at swap-out time, and `pinned` is `true` iff the
    /// chunk is being transferred to the arena's pin list.
    pub(crate) unsafe fn reconcile_swap_out(chunk: NonNull<Self>, rcs_issued: usize, pinned: bool) {
        let pin = usize::from(pinned);
        // `LARGE` is large enough that this subtraction can never
        // underflow on real workloads (see plan's "Overflow" note).
        let to_subtract = LARGE - rcs_issued - pin;
        // Raw access (no `&LocalChunk` borrow): on the zero-routing
        // branch we go on to `route_release` → `free_backing`, which
        // requires Unique on the chunk allocation.
        // SAFETY: chunk live — caller holds the inflated refcount.
        let prev = unsafe { (*chunk.as_ptr()).refcount.get() };
        debug_assert!(prev >= to_subtract, "local swap-out reconcile underflow");
        // SAFETY: chunk live.
        unsafe { (*chunk.as_ptr()).refcount.set(prev - to_subtract) };
        if prev == to_subtract {
            // SAFETY: refcount just reached zero — exclusive ownership.
            unsafe { Self::route_release(chunk) };
        }
    }

    /// Route a chunk whose refcount has just hit zero back to its
    /// provider (cache or self-free).
    ///
    /// # Safety
    ///
    /// Caller must have just observed the refcount transition to
    /// zero (single-thread ordering is sufficient — `LocalChunk` is
    /// `!Sync`).
    #[cfg_attr(coverage_nightly, coverage(off))]
    // The `else` branch (chunk outlives its provider) is unreachable in safe Rust:
    // local chunks are addressed only from the owning arena's thread and the provider
    // owns the chunk cache, so a refcount transition to zero implies the provider is
    // still alive. The whole helper is `#[cold]` and only runs on chunk release.
    unsafe fn route_release(chunk: NonNull<Self>) {
        // Raw-pointer access (no `&Self` borrow) so subsequent
        // `destroy`/`free_backing` can `drop_in_place` chunk fields.
        // SAFETY: refcount-zero — exclusive ownership.
        let provider_weak = unsafe { alloc::sync::Weak::clone(&(*chunk.as_ptr()).provider) };
        if let Some(provider) = provider_weak.upgrade() {
            // SAFETY: refcount-zero — we own the chunk.
            unsafe { provider.release_local(chunk) };
        } else {
            // SAFETY: refcount-zero — exclusive ownership.
            unsafe { Self::destroy(chunk) };
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
fn check_local_refcount(prev: usize) {
    if prev >= LARGE.saturating_add(LARGE) {
        refcount_overflow_abort();
    }
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// Targeted fast-fail for `header_size` mutations. The expected
    /// value is recomputed directly from the struct layout (mirroring
    /// the function's body but with no shared expression that a mutant
    /// could change in lockstep), so any divergence the mutation
    /// engine produces — replace-with-0, replace-with-1, `+` → `-`,
    /// etc. — is caught immediately rather than waiting for a stress
    /// test to observe an indirect chunk-boundary mismatch.
    #[test]
    fn header_size_matches_struct_layout() {
        let expected = core::mem::offset_of!(LocalChunk<Global>, drop_count) + core::mem::size_of::<Cell<u16>>();
        assert_eq!(header_size::<Global>(), expected);
        assert!(header_size::<Global>() > 0);
    }
}
