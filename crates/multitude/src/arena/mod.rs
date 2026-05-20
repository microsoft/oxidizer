// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(clippy::inline_always, reason = "hot bump-allocator helpers must inline into their callers")]

use alloc::sync::Arc as StdArc;
use core::cell::Cell;
use core::marker::PhantomData;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator, Global};

use crate::arena_builder::ArenaBuilder;
#[cfg(feature = "stats")]
use crate::arena_stats::ArenaStats;
use crate::internal::chunk_provider::ChunkProvider;
use crate::internal::local_chunk::LocalChunk;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::sync::Ordering;

/// Allocates `[len_prefix: usize][T elements...]` in the selected
/// current chunk, copies from `src_ptr`, runs `accounting`, and
/// returns a pointer to the elements.
macro_rules! try_alloc_prefixed {
    (
        self = $self:ident,
        src_ptr = $src_ptr:expr,
        len = $len:expr,
        payload_bytes = $payload_bytes:expr,
        elem_ty = $elem_ty:ty,
        slot = $slot:ident,
        refill = $refill:ident,
        accounting = $accounting:block,
    ) => {{
        let len = $len;
        let prefix = core::mem::size_of::<usize>();
        let total = prefix.checked_add($payload_bytes).ok_or(::allocator_api2::alloc::AllocError)?;
        // Align the data, not the prefix. The prefix uses unaligned
        // reads/writes so we can avoid extra padding.
        let align = core::mem::align_of::<$elem_ty>();
        $crate::arena::check_isize_overflow(total, align)?;
        // SAFETY: `check_isize_overflow(total, align)` above ensures
        // `total + (align - 1) <= isize::MAX`, so in particular
        // `total <= isize::MAX`. Asserting it lets the inner fit-check
        // arithmetic lower to plain `lea + cmp + ja` (see `try_bump_fit`
        // for the full reasoning around the data-addr / bumped /
        // entry-size bounds).
        unsafe {
            core::hint::assert_unchecked(isize::try_from(total).is_ok());
        }
        loop {
            let data_ptr = $self.$slot.data_ptr.get();
            let drop_back_ptr = $self.$slot.drop_back.get();
            let data_addr = data_ptr.as_ptr() as usize;
            let drop_back_addr = drop_back_ptr.as_ptr() as usize;
            // SAFETY: real chunk payloads are user-space allocations
            // (`data_addr <= isize::MAX`); the dangling stub uses
            // `data_addr == 1`. Combined with `total <= isize::MAX`
            // (asserted above) this lets `aligned + total` and
            // `data_addr + (align - 1)` lower to plain `add`s — see
            // `try_bump_fit` for the full argument.
            unsafe {
                core::hint::assert_unchecked(isize::try_from(data_addr).is_ok());
            }
            // Align the data section, then step back to the prefix.
            let data_aligned_addr = (data_addr + prefix + (align - 1)) & !(align - 1);
            let prefix_addr = data_aligned_addr - prefix;
            let end_addr = data_aligned_addr + $payload_bytes;
            if end_addr <= drop_back_addr {
                let prefix_offset = prefix_addr - data_addr;
                let bumped_offset = end_addr - data_addr;
                // SAFETY: `end_addr <= drop_back`, so
                // `data_ptr + prefix_offset` and `data_ptr + bumped_offset`
                // both lie inside the chunk payload.
                let (prefix_ptr_nn, end_ptr) = unsafe {
                    (data_ptr.byte_add(prefix_offset), data_ptr.byte_add(bumped_offset))
                };
                #[allow(
                    clippy::cast_ptr_alignment,
                    reason = "prefix slot is accessed via write_unaligned below; alignment of the cast target is not relied on"
                )]
                let prefix_ptr: *mut usize = prefix_ptr_nn.as_ptr().cast::<usize>();
                #[allow(
                    clippy::cast_ptr_alignment,
                    reason = "data is bump-aligned to align_of::<$elem_ty>() above; the cast is well-aligned at runtime"
                )]
                // SAFETY: `prefix_ptr_nn + prefix` lies inside the
                // chunk payload and is aligned to `align_of::<$elem_ty>()`
                // by the bump arithmetic above.
                let elems_ptr = unsafe { prefix_ptr_nn.as_ptr().add(prefix).cast::<$elem_ty>() };
                // Publish the new bump cursor first so the next fast-path
                // probe can hit the forwarded store.
                $self.$slot.data_ptr.set(end_ptr);
                // SAFETY: prefix slot is in the chunk-owned reserved
                // range, exclusively owned by this call, and not yet
                // initialized. `UninitSlot` records those invariants;
                // the `write_unaligned` it performs is then safe.
                let prefix_slot =
                    // SAFETY: see comment above.
                    unsafe { $crate::internal::slot::UninitSlot::<usize>::from_raw(prefix_ptr) };
                prefix_slot.write_unaligned(len);
                // SAFETY: source has `len` valid elements; destination
                // is freshly reserved arena memory; non-overlapping.
                unsafe { core::ptr::copy_nonoverlapping($src_ptr, elems_ptr, len) };
                $accounting
                $self.charge_alloc_stats(end_addr - prefix_addr);
                // SAFETY: `elems_ptr` is non-null inside the chunk payload.
                return Ok(unsafe { ::core::ptr::NonNull::new_unchecked(elems_ptr) });
            }
            // Include worst-case padding so the next iteration must fit.
            $self.$refill(total + align)?;
        }
    }};
}

mod alloc_growable;
mod alloc_slice_arc;
mod alloc_slice_box;
mod alloc_slice_rc;
mod alloc_slice_ref;
mod alloc_str;
mod alloc_uninit;
#[cfg(feature = "dst")]
mod alloc_unsized;
#[cfg(feature = "utf16")]
mod alloc_utf16;
mod alloc_value;
mod chunks;
mod inner_slice;
mod inner_value;
mod internals;
mod primitives;
mod refill;

#[cfg(test)]
mod tests;

use chunks::{CurrentLocalChunk, CurrentSharedChunk, OversizedLocalGuard, OversizedSharedGuard};
#[cfg(feature = "dst")]
use internals::align_up;
pub(crate) use internals::check_isize_overflow;
use internals::{
    AllocFlavor, ProtectiveHold, SharedArcsIssuedHold, SliceInitGuard, align_offset, bump_local_drop_count, bump_shared_drop_count,
    bumped_exceeds_chunk, compute_worst_case_size, current_chunk_evicted, drop_fn_for_slice, has_drop_entry, size_exceeds_normal_alloc,
    slow_refill_needed, try_bump_fit, worst_case_refill_for, write_through_ptr,
};
pub use internals::{expect_alloc, panic_alloc};

/// A flexible bump allocator.
///
/// Allocates large chunks of memory from an underlying allocator and parcels them out
/// efficiently in response to allocation requests.
///
/// # Configuration
///
/// [`Arena::new`] uses sensible defaults: the [`Global`] allocator,
/// no upfront preallocation, no byte budget, and a 16 KiB
/// oversized-allocation cutover (requests larger than that bypass
/// the normal chunk pool and get their own one-shot chunk sized
/// exactly to fit the request).
///
/// Chunks are sized by allocation class: each cacheable class has a
/// total allocation size that is a power of two from 512 bytes up
/// to 64 KiB. The class's usable payload is the class size minus
/// the per-chunk header (a few dozen bytes, depending on the
/// backing allocator type). Chunks grow on demand from the smallest
/// class up to the largest; cached chunks are retained without a
/// count limit (memory pressure is controlled via the optional byte
/// budget).
///
/// For non-default configuration — preallocated local/shared capacity,
/// a custom backing allocator, an outstanding-bytes budget, or a
/// different oversized-allocation cutover — use [`Arena::builder`].
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let value = arena.alloc_rc(42_u32);
/// assert_eq!(*value, 42);
/// ```
#[repr(C)]
pub struct Arena<A: Allocator + Clone = Global> {
    /// "Current" local chunk slot. The bump cursor lives here so the
    /// hot path touches only one cache line. Fields are individually
    /// `Cell`-wrapped so the bump can update `cursor` without
    /// rewriting the whole struct.
    ///
    /// Placed adjacent to `current_shared` so workloads that mix
    /// local and shared allocations benefit from cache locality.
    current_local: CurrentLocalChunk<A>,

    /// Lazy-pinning flag for the chunk currently installed in
    /// [`Self::current_local`] — set on simple-ref allocations,
    /// transferred (or released) when the chunk is swapped out.
    ///
    /// Stored alongside `current_local` rather than inside the slot
    /// type itself because shared chunks have no equivalent concept,
    /// and keeping the slot type symmetric across local/shared lets us
    /// share the generic [`CurrentChunk`] definition.
    current_local_pinned: Cell<bool>,

    /// "Current" shared chunk slot. The `smart_pointers_issued` field
    /// is the non-atomic counter that the deferred-reconciliation
    /// scheme reads at swap-out.
    ///
    /// Placed adjacent to `current_local` so workloads that mix
    /// local and shared allocations benefit from cache locality.
    current_shared: CurrentSharedChunk<A>,

    /// Intrusive list of locally-pinned chunks (chunks that have
    /// handed out at least one simple reference). Each entry holds
    /// one `+1`. Touched only on chunk swap-out (cold) and on
    /// `reset` / `Drop` (cold), so kept off the hot cache lines.
    pinned_local: Cell<Option<NonNull<LocalChunk<A>>>>,

    /// Strong handle to the per-arena chunk factory; chunks hold
    /// `Weak<ChunkProvider>` so the cache lifetime is tied to ours.
    provider: StdArc<ChunkProvider<A>>,

    /// `Arena` is `!Send + !Sync`: the `Cell`s and `StdArc<ChunkProvider>`
    /// (whose `ChunkProvider` is `!Sync`) already enforce this — the
    /// marker is documentation only.
    _not_thread_safe: PhantomData<*mut ()>,
}

// "Bump data_ptr; drop_back is the limit pointer" allocation scheme.
//
// `data_ptr` is the bump cursor itself: it points at the next free
// payload byte and advances forward as allocations happen. `drop_back`
// is the limit pointer: it points at the address one byte past the
// last free byte (equivalently, the start address of the trailing
// drop-entry stack). The free region is `[data_ptr, drop_back)`.
//
// On a raw bump (no drop entry):
//     aligned = align_up(data_ptr_addr, align)
//     end    = aligned + size
//     if end <= drop_back_addr { data_ptr = end; /* drop_back unchanged */ }
//
// On a bump that installs a 16-byte `InnerDropEntry`:
//     new_drop_back = drop_back_addr - 16 (computed via saturating_sub
//                                         to avoid underflow in stub state)
//     if end <= new_drop_back {
//         write entry at new_drop_back; data_ptr = end; drop_back = new_drop_back
//     }
//
// Stub state (no chunk loaded): `data_ptr == drop_back == NonNull::dangling()`,
// i.e., both are address `1`. The bump check `end <= drop_back_addr (= 1)`
// naturally fails for any nonzero allocation (because `aligned >= 1` and
// `end > aligned`). For ZSTs (size = 0) we apply `size.max(1)` so the
// check still fails — see `bumped` callsites below. The dangling pointers
// are never dereferenced in stub state.

impl Arena<Global> {
    /// Create a new, empty arena backed by [`Global`] with default
    /// configuration.
    ///
    /// No chunk is allocated up front: the alloc fast path's "is there
    /// a current chunk?" check is folded into the bump fit-check via
    /// per-arena sentinel headers, so the first allocation lazily pulls in
    /// a chunk on the slow path.
    ///
    /// For non-default configuration, use [`Self::builder`].
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// # #[cfg(feature = "stats")]
    /// assert_eq!(arena.stats().normal_local_chunks_allocated, 0);
    /// let _ = arena.alloc_rc(42_u32);
    /// # #[cfg(feature = "stats")]
    /// assert_eq!(arena.stats().normal_local_chunks_allocated, 1);
    /// ```
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::new_in(Global)
    }

    /// Create an [`ArenaBuilder`](crate::ArenaBuilder) using [`Global`]
    /// as the backing allocator.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Equivalent to `From::from(Default::default())`.
    pub fn builder() -> ArenaBuilder<Global> {
        ArenaBuilder::new()
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Create an [`ArenaBuilder`](crate::ArenaBuilder) backed by a custom
    /// `allocator`.
    #[must_use]
    #[inline]
    pub fn builder_in(allocator: A) -> ArenaBuilder<A> {
        ArenaBuilder::new_in(allocator)
    }

    /// Create a new, empty arena backed by `allocator` with default
    /// configuration.
    ///
    /// For non-default configuration, use [`Self::builder_in`].
    ///
    /// # Allocator scope (current limitation)
    ///
    /// Bulk allocations (chunk storage, builder buffers, oversized
    /// chunks) **do** route through `allocator`. The arena's
    /// per-instance bookkeeping allocation — a single `Arc<ChunkProvider<A>>`
    /// — currently uses the global allocator regardless of `A`.
    /// This is one small one-shot allocation per arena lifetime, and it
    /// does not affect the hot path. Users for whom strict allocator
    /// scoping matters (e.g., tracking allocators that need to
    /// account for every byte) should be aware of this gap; routing
    /// this through `A` is planned but requires a hand-rolled
    /// refcounted control block since stable `Arc` does not yet
    /// support custom allocators.
    #[must_use]
    #[inline]
    pub fn new_in(allocator: A) -> Self
    where
        A: 'static,
    {
        Self::from_config(allocator, crate::internal::constants::MAX_NORMAL_ALLOC, None, 0, 0)
    }

    /// Construct an arena from its configuration. Used by
    /// [`ArenaBuilder`](crate::ArenaBuilder).
    pub(crate) fn from_config(
        allocator: A,
        max_normal_alloc: usize,
        byte_budget: Option<usize>,
        initial_local_class: u8,
        initial_shared_class: u8,
    ) -> Self
    where
        A: 'static,
    {
        let provider = ChunkProvider::new(allocator, max_normal_alloc, byte_budget, initial_local_class, initial_shared_class);
        Self {
            provider,
            current_local: CurrentLocalChunk::<A>::default(),
            current_local_pinned: Cell::new(false),
            pinned_local: Cell::new(None),
            current_shared: CurrentSharedChunk::<A>::default(),
            _not_thread_safe: PhantomData,
        }
    }

    /// Borrow the backing allocator.
    #[must_use]
    #[inline]
    pub fn allocator(&self) -> &A {
        &self.provider.allocator
    }

    /// Allocate one fresh normal local chunk through the provider and
    /// stash it in the local cache. Used by
    /// [`ArenaBuilder::with_capacity_local`](crate::ArenaBuilder::with_capacity_local).
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// the byte budget is exhausted.
    pub(crate) fn preallocate_one_local(&self) -> Result<(), AllocError> {
        self.provider.preallocate_local()
    }

    /// Allocate one fresh normal shared chunk through the provider
    /// and stash it in the shared cache. Used by
    /// [`ArenaBuilder::with_capacity_shared`](crate::ArenaBuilder::with_capacity_shared).
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// the byte budget is exhausted.
    pub(crate) fn preallocate_one_shared(&self) -> Result<(), AllocError> {
        self.provider.preallocate_shared()
    }

    /// Snapshot of the arena's lifetime statistics.
    ///
    /// Only available with the `stats` Cargo feature enabled.
    #[cfg(feature = "stats")]
    #[cfg_attr(docsrs, doc(cfg(feature = "stats")))]
    #[must_use]
    #[inline]
    pub fn stats(&self) -> ArenaStats {
        self.provider.stats_snapshot()
    }

    /// Reset the arena to a fresh state, ready for a new allocation phase.
    ///
    /// Given that this function takes a mutable self, it ensures that any simple references are
    /// no longer in scope. This therefore tells the arena that it is safe to reuse any internally
    /// allocated chunks.
    ///
    /// Smart pointers keep references to their data alive. `reset` has no impact of these smart pointers, they continue
    /// to be valid. In other words, when you reset the arena, any chunk that has active smart pointers pointing into it
    /// will not be reclaimed and reused by the arena until such a time that all the smart pointers have gone out of scope.
    ///
    /// # Borrow-checker contract
    ///
    /// `reset` takes `&mut self`, so any outstanding simple reference
    /// (`&T` from `alloc`, `&str` from `alloc_str`, `&[T]` from
    /// `alloc_slice_*`) statically prevents the call:
    ///
    /// ```compile_fail
    /// use multitude::Arena;
    /// let mut arena = Arena::new();
    /// let r = arena.alloc(42_u64);
    /// arena.reset();   // ERROR: cannot borrow `arena` as mutable
    /// assert_eq!(*r, 42);
    /// ```
    ///
    /// # Byte budget accounting
    ///
    /// After `reset`, the byte budget continues to account for every chunk
    /// the provider has not yet physically freed — **including chunks that
    /// remain pinned by outstanding smart pointers** (`Rc`, `Arc`, `Box`,
    /// `RcStr`, …). Those chunks' bytes are released from the budget only
    /// when their last owning smart pointer drops *and* the chunk is then
    /// ineligible for caching (either over the per-class cache cap or
    /// dropped entirely if the chunk provider has been torn down).
    ///
    /// The byte budget is, by design, an admission-control device that
    /// bounds physical resident memory across the arena and its
    /// smart-pointer-pinned chunks — not a metric of "currently bumpable"
    /// capacity. A workload that allocates one large smart-pointer-pinned
    /// chunk, calls `reset`, then immediately tries to allocate again may
    /// see `AllocError` until the surviving smart pointers drop.
    #[cold]
    pub fn reset(&mut self) {
        // Drop work may invoke user `Drop` impls that reentrantly
        // allocate into `current_*`, populating slots we just
        // cleared. Loop until both slots and pin lists are empty.
        loop {
            let mut did_work = false;

            let mut head = self.pinned_local.replace(None);
            while let Some(chunk) = head {
                did_work = true;
                // SAFETY: this entry holds a +1 — chunk is live. Read
                // `next` via raw pointer so no SRO tag conflicts with
                // a potential `free_backing` inside `release_local_chunk`.
                let next = unsafe { (*chunk.as_ptr()).next.replace(None) };
                // SAFETY: this entry holds a +1.
                unsafe { self.release_local_chunk(chunk) };
                head = next;
            }

            if let Some(chunk) = self.current_local.chunk.replace(None) {
                did_work = true;
                #[cfg(feature = "stats")]
                {
                    let data_ptr_addr = self.current_local.data_ptr.get().as_ptr() as usize;
                    let drop_back_addr = self.current_local.drop_back.get().as_ptr() as usize;
                    let wasted = drop_back_addr.saturating_sub(data_ptr_addr);
                    crate::arena_stats::StatsStorage::add(&self.provider.stats.wasted_tail_bytes, wasted as u64);
                }
                // Use raw access throughout: `reconcile_swap_out` may
                // dec_ref to zero and `free_backing` the chunk; any
                // outstanding `&LocalChunk` SRO tag would collide.
                // SAFETY: refcount-positive — `current_local` held the LARGE inflation.
                let mirror_dc = unsafe { self.current_local.drop_count(chunk) };
                // SAFETY: refcount-positive.
                let chunk_dc = unsafe { (*chunk.as_ptr()).drop_count.get() };
                // SAFETY: refcount-positive.
                unsafe { (*chunk.as_ptr()).drop_count.set(mirror_dc.max(chunk_dc)) };
                let rcs_issued = self.current_local.smart_pointers_issued.replace(0);
                // `reset` takes `&mut self`, statically excluding any
                // outstanding simple references; we therefore release
                // the would-be-pin +1 by reconciling with `pinned == false`.
                self.current_local_pinned.set(false);
                self.current_local.data_ptr.set(NonNull::dangling());
                self.current_local.drop_back.set(NonNull::dangling());
                // SAFETY: chunk held the LARGE inflation; the
                // reconciliation drops back down to the actual live-rc
                // count and may free the chunk if it just hit zero.
                unsafe { LocalChunk::reconcile_swap_out(chunk, rcs_issued, false) };
            }

            if let Some(chunk) = self.current_shared.chunk.replace(None) {
                did_work = true;
                #[cfg(feature = "stats")]
                {
                    let data_ptr_addr = self.current_shared.data_ptr.get().as_ptr() as usize;
                    let drop_back_addr = self.current_shared.drop_back.get().as_ptr() as usize;
                    let wasted = drop_back_addr.saturating_sub(data_ptr_addr);
                    crate::arena_stats::StatsStorage::add(&self.provider.stats.wasted_tail_bytes, wasted as u64);
                }
                // SAFETY: chunk live — current_shared held the inflation.
                let dc = unsafe { self.current_shared.drop_count(chunk) };
                // Raw access: `reconcile_swap_out` may free the chunk.
                // SAFETY: chunk live.
                unsafe { (*chunk.as_ptr()).drop_count.store(dc, Ordering::Release) };
                let arcs_issued = self.current_shared.smart_pointers_issued.replace(0);
                self.current_shared.data_ptr.set(NonNull::dangling());
                self.current_shared.drop_back.set(NonNull::dangling());
                // SAFETY: chunk held the LARGE inflation; the
                // reconciliation drops back down to the actual live-arc
                // count and may free the chunk if it just hit zero.
                unsafe { SharedChunk::reconcile_swap_out(chunk, arcs_issued) };
            }

            if did_work {
                continue;
            }
            break;
        }
    }

    /// Returns a [`ZerocopyView`](crate::zerocopy::ZerocopyView)
    /// providing safe zero-initialized allocation for types implementing
    /// [`zerocopy::FromZeros`](::zerocopy::FromZeros).
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "zerocopy")] {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let val = arena.zerocopy().alloc_rc::<u64>();
    /// assert_eq!(*val, 0);
    /// # }
    /// ```
    #[cfg(feature = "zerocopy")]
    #[cfg_attr(docsrs, doc(cfg(feature = "zerocopy")))]
    #[inline]
    #[must_use]
    pub const fn zerocopy(&self) -> crate::zerocopy::ZerocopyView<'_, A> {
        crate::zerocopy::ZerocopyView::new(self)
    }

    /// Returns a [`BytemuckView`](crate::bytemuck::BytemuckView)
    /// providing safe zero-initialized allocation for types implementing
    /// [`bytemuck::Zeroable`](::bytemuck::Zeroable).
    ///
    /// # Example
    ///
    /// ```
    /// # #[cfg(feature = "bytemuck")] {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let val = arena.bytemuck().alloc_rc::<u64>();
    /// assert_eq!(*val, 0);
    /// # }
    /// ```
    #[cfg(feature = "bytemuck")]
    #[cfg_attr(docsrs, doc(cfg(feature = "bytemuck")))]
    #[inline]
    #[must_use]
    pub const fn bytemuck(&self) -> crate::bytemuck::BytemuckView<'_, A> {
        crate::bytemuck::BytemuckView::new(self)
    }
}
