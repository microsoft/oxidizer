// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! DST (unsized) value allocation API on [`Arena`].
//!
//! Implements `alloc_dst_rc`, `alloc_dst_arc`, `alloc_dst_box` and
//! their `try_*` variants under the `dst` Cargo feature. The trailing
//! drop entry stores the pointer-metadata as a `u16`, which limits
//! supported DSTs to those whose pointer-metadata is `usize`-sized and
//! whose metadata value fits in `u16` (slices of length up to
//! `u16::MAX`, in practice). For drop-aware slices with more than
//! `u16::MAX` elements, the non-DST `alloc_slice_rc` / `_arc` / `_box`
//! family stores the length in a separate prefix word and has no such
//! cap.

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{
    AllocFlavor, Arena, OversizedLocalGuard, OversizedSharedGuard, ProtectiveHold, SharedArcsIssuedHold, align_up, bump_local_drop_count,
    bump_shared_drop_count, expect_alloc,
};
use crate::arc::Arc;
use crate::arena::check_isize_overflow;
use crate::r#box::Box;
use crate::internal::constants::MAX_SMART_PTR_ALIGN;
use crate::internal::drop_list::{DropEntry as InnerDropEntry, noop_drop_shim};
use crate::internal::in_chunk::{InLocalChunk, InSharedChunk};
use crate::internal::local_chunk::LocalChunk;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::sync::Ordering;
use crate::rc::Rc;

impl<A: Allocator + Clone> Arena<A> {
    /// Reserve space in the local chunk for a DST value of `layout` plus a
    /// trailing drop entry that records the value's pointer-metadata. After
    /// reservation, calls `init` with the raw byte pointer to the value
    /// region (which the caller's outer `alloc_dst_*` reconstructs into a
    /// fat pointer of the proper DST type).
    ///
    /// Returns the value pointer with the chunk's refcount bumped by one
    /// (for the smart pointer the caller will return).
    ///
    /// `final_shim` is the drop shim the entry is retargeted to after
    /// `init` succeeds:
    /// - `Rc` / `Arc` callers pass [`dst_drop_shim_trailing::<T>`] so the
    ///   chunk teardown runs `T`'s destructor.
    /// - `Box` callers pass [`noop_drop_shim`]: `Box::Drop` runs
    ///   `drop_in_place` itself, and [`Box::into_rc`](crate::Box::into_rc)
    ///   retargets the (still-noop) entry to the appropriate real shim on
    ///   the rare conversion path.
    ///
    /// # Safety
    ///
    /// `init` must initialize a valid `T` at the supplied pointer (read
    /// through a fat pointer reconstructed from `(ptr, metadata)` by the
    /// caller); `metadata_u16` must be the pointer-metadata for that `T`,
    /// reinterpretable as `usize` and bounded by `u16::MAX` (validated
    /// by [`metadata_to_u16`]).
    #[cfg_attr(test, mutants::skip)] // mirror_dc redundancy: drop_count bump is defense-in-depth.
    unsafe fn try_reserve_dst_local_with_entry(
        &self,
        layout: Layout,
        metadata_u16: u16,
        final_shim: unsafe fn(*mut u8, usize),
        init: impl FnOnce(*mut u8),
    ) -> Result<NonNull<u8>, AllocError> {
        // `check_chunk_alignment` is already enforced by every public
        // caller (`try_alloc_dst_arc`/`_rc`/`_box`) before this helper
        // is reached; we rely on that here rather than re-checking.
        //
        // `Layout` from the public `unsafe fn`s does NOT enforce
        // `size + align <= isize::MAX`, so reject overflowing layouts
        // here before any unchecked arithmetic below. Without this guard
        // the `aligned_addr.checked_add(bumped).unwrap_unchecked()` in
        // the bump probe would invoke UB on `None`.
        check_isize_overflow(layout.size(), layout.align())?;
        let entry_size = core::mem::size_of::<InnerDropEntry>();

        // The `max_normal_alloc` check is deferred to the cold refill
        // branch so the hot bump-fit probe avoids a 2-level pointer
        // chase through `Arc<ChunkProvider>`.
        let bumped = layout.size().max(1);
        loop {
            let data_ptr = self.current_local.data_ptr.get();
            let drop_back_ptr = self.current_local.drop_back.get();
            let drop_back_addr = drop_back_ptr.as_ptr() as usize;
            let aligned_addr = align_up(data_ptr.as_ptr() as usize, layout.align().max(1));
            // SAFETY: `aligned_addr` lies inside a live chunk whose
            // bump extent is capped at `max_bump_extent <= MAX_CHUNK_BYTES`,
            // and `bumped` is bounded by the caller's `Layout` invariant
            // (`size <= isize::MAX`). The sum fits in `usize`.
            let end_addr = unsafe { aligned_addr.checked_add(bumped).unwrap_unchecked() };
            let new_drop_back_addr = drop_back_addr.saturating_sub(entry_size);
            if end_addr <= new_drop_back_addr {
                // Provenance-preserving construction from `data_ptr` and `drop_back_ptr`.
                let data_addr = data_ptr.as_ptr() as usize;
                let aligned_offset = aligned_addr - data_addr;
                // SAFETY: gated above; both lie in chunk payload.
                let (aligned_ptr, end_ptr) = unsafe { (data_ptr.byte_add(aligned_offset), data_ptr.byte_add(aligned_offset + bumped)) };
                // SAFETY: gated above; lies in chunk payload.
                let new_drop_back_ptr = unsafe { drop_back_ptr.byte_sub(entry_size) };
                // SAFETY: bump-fit gate above implies non-stub slot ⇒ `current_local.chunk` is `Some`.
                let chunk = unsafe { self.current_local.chunk.get().unwrap_unchecked() };
                // SAFETY: refcount-positive — chunk held at LARGE inflation.
                let payload_base_addr = unsafe { LocalChunk::<A>::data_ptr(chunk) }.as_ptr() as usize;
                // Bump-fit success means the allocation lands inside the
                // current chunk, whose payload is capped at
                // `max_bump_extent <= MAX_CHUNK_BYTES < u16::MAX`.
                // SAFETY: bounded by current-chunk payload extent.
                let value_offset = unsafe { u16::try_from((aligned_ptr.as_ptr() as usize) - payload_base_addr).unwrap_unchecked() };
                let value_ptr: *mut u8 = aligned_ptr.as_ptr();

                // Account for the +1 we will hand to the caller
                // (this is the protective hold across `init`).
                // If `init` reentrantly evicts this chunk, the
                // bump survives the swap-out as `+1` in the
                // chunk's atomic refcount; if `init` panics, the
                // hold's drop undoes the bump (or `dec_ref`s the
                // post-eviction chunk).
                self.current_local.bump_smart_pointers_issued();
                let hold = ProtectiveHold::<A> {
                    arena: self,
                    chunk,
                    flavor: AllocFlavor::Rc,
                };

                // Pre-advance the bump cursor and pre-write a noop
                // drop entry before invoking `init`. Prevents a
                // reentrant alloc from overlapping our reservation,
                // and keeps the chunk's drop_count consistent if
                // `init` panics or evicts the chunk.
                self.current_local.data_ptr.set(end_ptr);
                self.current_local.drop_back.set(new_drop_back_ptr);
                let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                let noop_entry = InnerDropEntry::new(noop_drop_shim, value_offset, metadata_u16);
                // SAFETY: `entry_ptr` is valid for one entry.
                unsafe { core::ptr::write(entry_ptr, noop_entry) };
                // SAFETY: protective hold keeps `chunk` alive.
                // SAFETY: refcount-positive — chunk is live.
                unsafe { bump_local_drop_count(chunk) };

                init(value_ptr);
                core::mem::forget(hold);

                // `init` succeeded: overwrite the noop entry with
                // the caller-supplied final shim.
                // SAFETY: `entry_ptr` references the pre-written noop
                // entry. Local chunks are owner-thread exclusive
                // (`LocalChunk: !Send`); Relaxed is sufficient.
                unsafe { (*entry_ptr).store_drop_fn(final_shim, Ordering::Relaxed) };
                let _ = (value_offset, metadata_u16);

                self.charge_alloc_stats(layout.size());
                // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
                return Ok(unsafe { NonNull::new_unchecked(value_ptr) });
            }
            // Route oversized requests to the cold one-shot path
            // (deferred from above the loop to avoid a provider pointer chase).
            if layout.size() > self.provider.max_normal_alloc {
                // SAFETY: caller's contract is preserved end-to-end.
                return unsafe { self.try_reserve_dst_local_oversized_with_entry(layout, metadata_u16, final_shim, init) };
            }
            self.refill_local(
                layout
                    .size()
                    .saturating_add(layout.align().saturating_sub(core::mem::align_of::<usize>()))
                    .saturating_add(entry_size),
            )?;
        }
    }

    /// Shared-chunk variant of [`Self::try_reserve_dst_local_with_entry`].
    ///
    /// # Safety
    ///
    /// Same as [`Self::try_reserve_dst_local_with_entry`].
    unsafe fn try_reserve_dst_shared_with_entry(
        &self,
        layout: Layout,
        metadata_u16: u16,
        final_shim: unsafe fn(*mut u8, usize),
        init: impl FnOnce(*mut u8),
    ) -> Result<NonNull<u8>, AllocError> {
        // See [`Self::try_reserve_dst_local_with_entry`] for why the
        // alignment check is delegated to the public caller and why the
        // overflow check is required here even though it looks redundant
        // with the oversized-routing branch.
        check_isize_overflow(layout.size(), layout.align())?;
        let entry_size = core::mem::size_of::<InnerDropEntry>();

        // The `max_normal_alloc` check is deferred to the cold refill
        // branch. See the local sibling for rationale.
        let bumped = layout.size().max(1);
        loop {
            let data_ptr = self.current_shared.data_ptr.get();
            let drop_back_ptr = self.current_shared.drop_back.get();
            let drop_back_addr = drop_back_ptr.as_ptr() as usize;
            let aligned_addr = align_up(data_ptr.as_ptr() as usize, layout.align().max(1));
            // SAFETY: `aligned_addr` lies inside a live chunk whose
            // bump extent is capped at `max_bump_extent <= MAX_CHUNK_BYTES`,
            // and `bumped` is bounded by the caller's `Layout` invariant
            // (`size <= isize::MAX`). The sum fits in `usize`.
            let end_addr = unsafe { aligned_addr.checked_add(bumped).unwrap_unchecked() };
            let new_drop_back_addr = drop_back_addr.saturating_sub(entry_size);
            if end_addr <= new_drop_back_addr {
                // Provenance-preserving construction from `data_ptr` and `drop_back_ptr`.
                let data_addr = data_ptr.as_ptr() as usize;
                let aligned_offset = aligned_addr - data_addr;
                // SAFETY: gated above; both lie in chunk payload.
                let (aligned_ptr, end_ptr) = unsafe { (data_ptr.byte_add(aligned_offset), data_ptr.byte_add(aligned_offset + bumped)) };
                // SAFETY: gated above; lies in chunk payload.
                let new_drop_back_ptr = unsafe { drop_back_ptr.byte_sub(entry_size) };
                // SAFETY: bump-fit gate above implies non-stub slot ⇒ `current_shared.chunk` is `Some`.
                let chunk = unsafe { self.current_shared.chunk.get().unwrap_unchecked() };
                // SAFETY: refcount-positive — chunk held at LARGE inflation.
                let payload_base_addr = unsafe { SharedChunk::<A>::data_ptr(chunk) }.as_ptr() as usize;
                // Bump-fit success means the allocation lands inside the
                // current chunk, whose payload is capped at
                // `max_bump_extent <= MAX_CHUNK_BYTES < u16::MAX`.
                // SAFETY: bounded by current-chunk payload extent.
                let value_offset = unsafe { u16::try_from((aligned_ptr.as_ptr() as usize) - payload_base_addr).unwrap_unchecked() };
                let value_ptr: *mut u8 = aligned_ptr.as_ptr();

                // Account for the +1 we will hand to the caller
                // before `init` runs (protective hold). See the
                // local-flavor counterpart for the rationale.
                self.current_shared.bump_smart_pointers_issued();
                let hold = SharedArcsIssuedHold { arena: self, chunk };

                // Pre-advance the bump cursor and pre-write a
                // noop drop entry into the reserved back-stack
                // slot before invoking `init`.
                self.current_shared.data_ptr.set(end_ptr);
                self.current_shared.drop_back.set(new_drop_back_ptr);
                let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
                let noop_entry = InnerDropEntry::new(noop_drop_shim, value_offset, metadata_u16);
                // SAFETY: `entry_ptr` is valid for one entry.
                unsafe { core::ptr::write(entry_ptr, noop_entry) };
                // SAFETY: refcount-positive — chunk is live.
                unsafe { bump_shared_drop_count(chunk) };

                init(value_ptr);
                core::mem::forget(hold);

                // `init` succeeded: overwrite the noop entry's
                // `drop_fn` with the caller-supplied final shim.
                // `value_offset` and `metadata_u16` were already
                // written by the noop entry and remain unchanged.
                // SAFETY: `entry_ptr` references the pre-written
                // noop entry. The `Arc<dyn …>` for this allocation
                // has not been returned to the caller yet, so no
                // other thread can observe this slot's `drop_fn`.
                // Relaxed is sufficient: the eventual `Arc::drop`'s
                // Release on `refcount` carries the new `drop_fn` to
                // any subsequent `replay_drops` reader.
                unsafe { (*entry_ptr).store_drop_fn(final_shim, Ordering::Relaxed) };
                let _ = (value_offset, metadata_u16);

                self.charge_alloc_stats(layout.size());
                // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
                return Ok(unsafe { NonNull::new_unchecked(value_ptr) });
            }
            // Route oversized requests to the cold one-shot path
            // (deferred from above the loop to avoid a provider pointer chase).
            if layout.size() > self.provider.max_normal_alloc {
                // SAFETY: caller's contract is preserved end-to-end.
                return unsafe { self.try_reserve_dst_shared_oversized_with_entry(layout, metadata_u16, final_shim, init) };
            }
            self.refill_shared(
                layout
                    .size()
                    .saturating_add(layout.align().saturating_sub(core::mem::align_of::<usize>()))
                    .saturating_add(entry_size),
            )?;
        }
    }
    /// Cold one-shot oversized DST local allocation, mirror of
    /// [`Self::try_reserve_dst_local_with_entry`] for requests whose
    /// size exceeds `max_normal_alloc`. Acquires a dedicated oversized
    /// chunk, installs the drop entry at `cap - entry_size`, and
    /// reconciles down to leave +1 for the caller.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_reserve_dst_local_with_entry`].
    #[cold]
    #[inline(never)]
    unsafe fn try_reserve_dst_local_oversized_with_entry(
        &self,
        layout: Layout,
        metadata_u16: u16,
        final_shim: unsafe fn(*mut u8, usize),
        init: impl FnOnce(*mut u8),
    ) -> Result<NonNull<u8>, AllocError> {
        let entry_size = core::mem::size_of::<InnerDropEntry>();
        // `Layout::array`/`Layout::from_size_align` bound `layout.size() <= isize::MAX`;
        // the caller's alignment cap bounds `layout.align()`; `entry_size` is a small constant.
        let needed = layout.size() + layout.align().saturating_sub(core::mem::align_of::<usize>()) + entry_size;
        let chunk = self.provider.acquire_local(needed)?;
        // SAFETY: chunk live — held at LARGE inflation.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        let cap = chunk_ref.capacity;
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits.
        let aligned = unsafe { super::internals::align_offset(data_addr, layout.align().max(1)).unwrap_unchecked() };
        // SAFETY: aligned offset lies within `[0, cap)`.
        let value_ptr = unsafe { data_ptr.as_ptr().add(aligned) };

        let guard = OversizedLocalGuard { chunk };
        init(value_ptr);
        core::mem::forget(guard);

        let new_drop_back = cap - entry_size;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "chunk payloads are 64 KiB aligned (CHUNK_ALIGN), so any `InnerDropEntry` slot computed as `data + new_drop_back` is naturally aligned"
        )]
        // SAFETY: payload-extent invariant.
        let entry_ptr = unsafe { data_ptr.as_ptr().add(new_drop_back).cast::<InnerDropEntry>() };
        let value_offset_u16 =
            u16::try_from(aligned).expect("oversized chunk payload starts at offset 0; aligned < align < MAX_SMART_PTR_ALIGN ≤ u16::MAX");
        let entry = InnerDropEntry::new(final_shim, value_offset_u16, metadata_u16);
        // SAFETY: payload-extent invariant; first write to a fresh entry.
        unsafe { core::ptr::write(entry_ptr, entry) };
        chunk_ref.drop_count.set(1);

        self.charge_alloc_stats(layout.size());
        // Reconcile LARGE → +1 (caller's Rc/Box owns it).
        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { LocalChunk::reconcile_swap_out(chunk, 1, false) };
        // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
        Ok(unsafe { NonNull::new_unchecked(value_ptr) })
    }

    /// Cold one-shot oversized DST shared allocation, mirror of
    /// [`Self::try_reserve_dst_shared_with_entry`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_reserve_dst_shared_with_entry`].
    #[cold]
    #[inline(never)]
    unsafe fn try_reserve_dst_shared_oversized_with_entry(
        &self,
        layout: Layout,
        metadata_u16: u16,
        final_shim: unsafe fn(*mut u8, usize),
        init: impl FnOnce(*mut u8),
    ) -> Result<NonNull<u8>, AllocError> {
        let entry_size = core::mem::size_of::<InnerDropEntry>();
        // `Layout::array`/`Layout::from_size_align` bound `layout.size() <= isize::MAX`;
        // the caller's alignment cap bounds `layout.align()`; `entry_size` is a small constant.
        let needed = layout.size() + layout.align().saturating_sub(core::mem::align_of::<usize>()) + entry_size;
        let chunk = self.provider.acquire_shared(needed)?;
        // SAFETY: chunk live — held at LARGE inflation.
        let chunk_ref = unsafe { chunk.as_ref() };
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { SharedChunk::<A>::data_ptr(chunk) };
        let cap = chunk_ref.capacity;
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits.
        let aligned = unsafe { super::internals::align_offset(data_addr, layout.align().max(1)).unwrap_unchecked() };
        // SAFETY: aligned offset lies within `[0, cap)`.
        let value_ptr = unsafe { data_ptr.as_ptr().add(aligned) };

        let guard = OversizedSharedGuard { chunk };
        init(value_ptr);
        core::mem::forget(guard);

        let new_drop_back = cap - entry_size;
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "chunk payloads are 64 KiB aligned (CHUNK_ALIGN), so any `InnerDropEntry` slot computed as `data + new_drop_back` is naturally aligned"
        )]
        // SAFETY: payload-extent invariant.
        let entry_ptr = unsafe { data_ptr.as_ptr().add(new_drop_back).cast::<InnerDropEntry>() };
        let value_offset_u16 =
            u16::try_from(aligned).expect("oversized chunk payload starts at offset 0; aligned < align < MAX_SMART_PTR_ALIGN ≤ u16::MAX");
        let entry = InnerDropEntry::new(final_shim, value_offset_u16, metadata_u16);
        // SAFETY: payload-extent invariant; first write to a fresh entry.
        unsafe { core::ptr::write(entry_ptr, entry) };
        // Chunk live — LARGE inflation; no other strand observes it yet.
        chunk_ref.drop_count.store(1, Ordering::Relaxed);

        self.charge_alloc_stats(layout.size());
        // Reconcile LARGE → +1 (caller's Arc owns it).
        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { SharedChunk::reconcile_swap_out(chunk, 1) };
        // SAFETY: `value_ptr` is non-null and now points at an initialized `T`.
        Ok(unsafe { NonNull::new_unchecked(value_ptr) })
    }
}

impl<A: Allocator + Clone> Arena<A> {
    /// Allocate a possibly-unsized `T` and return an `Arc<T, A>`.
    ///
    /// The closure `init` receives a typed fat pointer to the buffer
    /// (built from `(thin_ptr, metadata)`) and is responsible for
    /// writing a valid `T` through it. multitude reconstructs the same
    /// metadata at chunk teardown so `T`'s destructor runs correctly.
    ///
    /// For sized `T`, prefer [`Self::alloc_arc`] / [`Self::alloc_arc_with`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if `layout.align()` is
    /// at least 32 KiB.
    ///
    /// # Safety
    ///
    /// - `layout` must exactly describe the size and alignment of the
    ///   constructed DST value (e.g., for `[U]` of length `n`,
    ///   `Layout::array::<U>(n).unwrap()`). Passing a smaller layout
    ///   would cause `init` to write past the reservation.
    /// - `init` must initialize all bytes covered by `layout` to a valid `T`.
    /// - `metadata` must be valid for the value just written.
    /// - `T::Metadata` must be either zero-sized (sized `T`) or
    ///   `usize`-sized AND fit in `u16` after reinterpretation. This
    ///   means **slices** (`[U]`, where the metadata is the slice
    ///   length) and **sized** `T` are supported; trait objects (`dyn
    ///   Trait`) and other DSTs whose metadata cannot be packed into
    ///   `u16` are **not** supported and return
    ///   [`AllocError`] / panic via `expect_alloc`.
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_arc<T: ?Sized + Send + Sync + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Arc<T, A>
    where
        A: Send + Sync,
    {
        // SAFETY: caller upholds `try_alloc_dst_arc`'s contract.
        expect_alloc(unsafe { self.try_alloc_dst_arc(layout, metadata, init) })
    }

    /// Fallible variant of [`Self::alloc_dst_arc`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if
    /// `layout.align()` is at least 32 KiB.
    ///
    /// # Panics
    ///
    /// Allocator panics propagate.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_arc<T: ?Sized + Send + Sync + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Arc<T, A>, AllocError>
    where
        A: Send + Sync,
    {
        check_chunk_alignment(layout)?;
        let metadata_u16 = metadata_to_u16::<T>(&metadata)?;
        let ptr = if const { core::mem::needs_drop::<T>() } {
            // SAFETY: caller initializes a valid `T` through the fat pointer in `init`.
            unsafe {
                self.try_reserve_dst_shared_with_entry(layout, metadata_u16, dst_drop_shim_trailing::<T>, |p| {
                    init(::ptr_meta::from_raw_parts_mut::<T>(p.cast(), metadata));
                })?
            }
            .as_ptr()
            .cast::<u8>()
        } else {
            let raw = self.allocate_shared_layout(layout)?;
            // `allocate_shared_layout` left a `+1` on the chunk
            // refcount for the Arc (either via
            // `bump_smart_pointers_issued` on the bump path or via
            // direct reconcile on the oversized path). Recover the
            // chunk header from the value pointer — works for both
            // paths because the value's offset within the chunk is
            // bounded by `MAX_SMART_PTR_ALIGN < CHUNK_ALIGN`.
            // SAFETY: `raw` was just returned from this arena's
            // shared allocator and lies inside a live chunk.
            let chunk = unsafe { InSharedChunk::<_, A>::new(raw) }.chunk_ptr();
            let guard = SharedArcsIssuedHold::<A> { arena: self, chunk };
            let p = ::ptr_meta::from_raw_parts_mut::<T>(raw.as_ptr().cast(), metadata);
            init(p);
            core::mem::forget(guard);
            raw.as_ptr()
        };
        let fat = ::ptr_meta::from_raw_parts_mut::<T>(ptr.cast(), metadata);
        // SAFETY: caller initialized a valid `T` at `fat`; shared allocation accounted for this Arc.
        Ok(unsafe { Arc::from_value_ptr(NonNull::new_unchecked(fat)) })
    }

    /// Allocate a possibly-unsized `T` and return an `Rc<T, A>`. See
    /// [`Self::alloc_dst_arc`] for the contract.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_rc<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Rc<T, A> {
        // SAFETY: caller upholds `try_alloc_dst_rc`'s contract.
        expect_alloc(unsafe { self.try_alloc_dst_rc(layout, metadata, init) })
    }

    /// Fallible variant of [`Self::alloc_dst_rc`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_arc`].
    ///
    /// # Panics
    ///
    /// Allocator panics propagate.
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_rc<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Rc<T, A>, AllocError> {
        check_chunk_alignment(layout)?;
        let metadata_u16 = metadata_to_u16::<T>(&metadata)?;
        let ptr = if const { core::mem::needs_drop::<T>() } {
            // SAFETY: caller initializes a valid `T` through the fat pointer in `init`.
            unsafe {
                self.try_reserve_dst_local_with_entry(layout, metadata_u16, dst_drop_shim_trailing::<T>, |p| {
                    init(::ptr_meta::from_raw_parts_mut::<T>(p.cast(), metadata));
                })?
            }
            .as_ptr()
            .cast::<u8>()
        } else {
            let raw = self.allocate_layout(layout)?;
            // `allocate_layout` left a `+1` on the chunk refcount for
            // the Rc (either via `bump_smart_pointers_issued` on the
            // bump path or via direct reconcile on the oversized
            // path). Recover the chunk header from the value pointer.
            // SAFETY: `raw` was just returned from this arena's
            // local allocator and lies inside a live chunk.
            let chunk = unsafe { InLocalChunk::<_, A>::new(raw) }.chunk_ptr();
            let guard = ProtectiveHold::<A> {
                arena: self,
                chunk,
                flavor: AllocFlavor::Rc,
            };
            let p = ::ptr_meta::from_raw_parts_mut::<T>(raw.as_ptr().cast(), metadata);
            init(p);
            core::mem::forget(guard);
            raw.as_ptr()
        };
        let fat = ::ptr_meta::from_raw_parts_mut::<T>(ptr.cast(), metadata);
        // SAFETY: caller initialized a valid `T` at `fat`; local allocation owns a +1 for this Rc.
        Ok(unsafe { Rc::from_value_ptr(NonNull::new_unchecked(fat)) })
    }

    /// Allocate a possibly-unsized `T` and return a [`Box<T, A>`](crate::Box).
    /// See [`Self::alloc_dst_arc`] for the contract.
    ///
    /// Unlike the refcount variants, the resulting [`Box`](crate::Box) runs
    /// `T`'s destructor immediately when the smart pointer is dropped.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn alloc_dst_box<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Box<T, A> {
        // SAFETY: caller upholds `try_alloc_dst_box`'s contract.
        expect_alloc(unsafe { self.try_alloc_dst_box(layout, metadata, init) })
    }

    /// Fallible variant of [`Self::alloc_dst_box`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_arc`].
    ///
    /// # Panics
    ///
    /// Allocator panics propagate.
    ///
    /// # Safety
    ///
    /// See [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_box<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<Box<T, A>, AllocError> {
        check_chunk_alignment(layout)?;
        // For `T: needs_drop` whose metadata fits the trailing drop-entry
        // contract, route through the with-entry helper installing a
        // `noop_drop_shim`. `Box::Drop` will run `drop_in_place` directly,
        // and the noop entry remains so the chunk teardown is a no-op for
        // this slot; [`Box::into_rc`](crate::Box::into_rc) retargets the
        // noop entry to the real shim on the conversion path. Without
        // this, `Box::into_rc` for `T: Drop` would panic because no entry
        // exists to retarget.
        if const { core::mem::needs_drop::<T>() } {
            if let Ok(metadata_u16) = metadata_to_u16::<T>(&metadata) {
                // SAFETY: caller initializes a valid `T` through the fat pointer in `init`.
                let raw = unsafe {
                    self.try_reserve_dst_local_with_entry(layout, metadata_u16, noop_drop_shim, |p| {
                        init(::ptr_meta::from_raw_parts_mut::<T>(p.cast(), metadata));
                    })?
                };
                let fat = ::ptr_meta::from_raw_parts_mut::<T>(raw.as_ptr().cast(), metadata);
                // SAFETY: caller initialized a valid `T` at `fat`; local allocation owns a +1 for this Box.
                return Ok(unsafe { Box::from_raw_unsized(NonNull::new_unchecked(fat)) });
            }
            // `metadata_to_u16` only returns `Err` for `usize`-sized metadata
            // whose numeric value overflows `u16` (slice / `str` lengths
            // beyond `u16::MAX` and trait-object vtable addresses, which are
            // always beyond `u16::MAX` in practice). Reject at alloc time —
            // otherwise an eventual [`Box::<[T]>::into_rc`] retarget would
            // panic for lack of a drop entry. Multi-word metadata is ruled
            // out by `metadata_to_u16`'s internal `assert_unchecked` so it
            // cannot reach this branch.
            return Err(AllocError);
        }

        // Fast path: `T: !needs_drop` (no entry needed at all) or a DST
        // whose metadata can't be stored in the entry's `u16` field
        // (trait objects whose `Metadata` is not `usize`-sized). Trait-
        // object `Box::<dyn Trait>` has no `into_rc` overload, so the
        // lack of an entry is benign: only `Box::Drop` ever needs the
        // metadata, which it gets directly from the fat pointer.
        let raw = self.allocate_layout(layout)?;
        // `allocate_layout` left a `+1` on the chunk refcount for the
        // Box. Recover the chunk header from the value pointer (works
        // for both the bump and oversized paths).
        // SAFETY: `raw` was just returned from this arena's local
        // allocator and lies inside a live chunk.
        let chunk = unsafe { InLocalChunk::<_, A>::new(raw) }.chunk_ptr();
        let guard = ProtectiveHold::<A> {
            arena: self,
            chunk,
            flavor: AllocFlavor::Box,
        };
        let ptr = ::ptr_meta::from_raw_parts_mut::<T>(raw.as_ptr().cast(), metadata);
        init(ptr);
        core::mem::forget(guard);
        // SAFETY: caller initialized a valid `T` at `ptr`; local allocation owns a +1 for this Box.
        Ok(unsafe { Box::from_raw_unsized(NonNull::new_unchecked(ptr)) })
    }
}

/// Reinterpret a DST's pointer-metadata as a `u16` for storage in the
/// trailing drop entry. Returns [`AllocError`] if `T::Metadata` is not
/// `usize`-sized (or zero-sized) or if the metadata value exceeds
/// `u16::MAX`.
///
/// The DST safety contract on `alloc_dst_*` requires `T::Metadata`
/// to be either `usize`-sized (slices) or zero-sized (sized `T`).
/// Trait-object metadata (a vtable pointer) does not fit in `u16`, so
/// trait-object DSTs are not supported by the trailing-drop-list
/// machinery; for slices, the **element count** must be `<= u16::MAX`.
/// Longer slices are rejected even if the byte size would fit; callers
/// that need them must use the non-DST slice allocators, which store a
/// separate length prefix.
///
/// Sized `T` has `T::Metadata = ()`, so we return `Ok(1)`. That value is
/// load-bearing for `drop_shim_one::<T>` on the Box-conversion path and
/// ignored by the Rc/Arc trailing-drop shim.
fn metadata_to_u16<T: ?Sized + ::ptr_meta::Pointee>(metadata: &T::Metadata) -> Result<u16, AllocError> {
    if const { core::mem::size_of::<T::Metadata>() == 0 } {
        return Ok(1);
    }
    // SAFETY: in current Rust every `?Sized + Pointee` metadata produced
    // by the language or by `ptr_meta::derive` is either zero-sized
    // (handled above) or `usize`-sized (slice / `str` length, trait-object
    // vtable pointer). Multi-word metadata types cannot be observed in
    // safe code, so this branch is unreachable; we tell LLVM so the
    // `transmute_copy` below has no extra guard.
    unsafe {
        core::hint::assert_unchecked(core::mem::size_of::<T::Metadata>() == core::mem::size_of::<usize>());
    }
    // SAFETY: the size check above guarantees the source and destination have
    // matching size; `T::Metadata`'s safety contract (per `alloc_dst_*`)
    // requires it to be `usize`-sized, so the bit pattern is meaningful as
    // a `usize`.
    let as_usize: usize = unsafe { core::mem::transmute_copy::<T::Metadata, usize>(metadata) };
    u16::try_from(as_usize).map_err(|_e| AllocError)
}

/// Trailing drop shim used by [`Arena::alloc_dst_rc`] / `_arc`.
///
/// Rebuilds the fat pointer from the stored `u16` metadata and drops it
/// in place.
///
/// # Safety
///
/// `value` must point at an initialized `T` whose pointer-metadata is
/// recoverable by reinterpreting `metadata_as_usize` as `T::Metadata`
/// (validated at allocation time by [`metadata_to_u16`]).
unsafe fn dst_drop_shim_trailing<T: ?Sized + ::ptr_meta::Pointee>(value: *mut u8, metadata_as_usize: usize) {
    // SAFETY: per the alloc-time safety contract, `T::Metadata` is `usize`-sized
    // and the metadata's bit pattern was preserved in `metadata_as_usize`.
    let metadata: T::Metadata = unsafe { core::mem::transmute_copy::<usize, T::Metadata>(&metadata_as_usize) };
    let fat: *mut T = ::ptr_meta::from_raw_parts_mut(value.cast(), metadata);
    // SAFETY: caller guarantees the fat pointer denotes a valid, initialized `T`.
    unsafe { core::ptr::drop_in_place(fat) };
}

/// Alignment guard: rejects layouts with alignment at or above
/// `MAX_SMART_PTR_ALIGN`.
///
/// Smart-pointer paths (including DST `alloc_dst_*`) require the value
/// to land at an offset strictly less than `CHUNK_ALIGN` so that
/// `header_for(value_ptr)` can recover the chunk header by masking the
/// low bits. With a co-allocated `DropEntry` of size 32, the largest
/// alignment that satisfies this is `CHUNK_ALIGN / 2 = 32 KiB`; one
/// step higher would push the value to offset `CHUNK_ALIGN` and break
/// the mask.
///
/// Called as a precondition by every public `try_alloc_dst_*` entry
/// point; the inner `try_reserve_dst_*_with_entry` helpers rely on
/// that and do not re-check.
#[expect(clippy::inline_always, reason = "zero-cost wrapper must inline at call site")]
#[inline(always)]
#[cfg_attr(coverage_nightly, coverage(off))]
const fn check_chunk_alignment(layout: Layout) -> Result<(), AllocError> {
    if layout.align() >= MAX_SMART_PTR_ALIGN {
        Err(AllocError)
    } else {
        Ok(())
    }
}

#[cfg(feature = "dst")]
impl<A: Allocator + Clone> Arena<A> {
    /// `Pin` variant of [`Self::alloc_dst_arc`]. Returns a pinned
    /// `Arc<T, A>` where the value's address is fixed in the arena
    /// and never moves until the last `Arc` clone is dropped.
    ///
    /// Typical use: pinning an `Arc<[T]>` whose slice contents must
    /// stay at a fixed address (e.g. for `Pin`-projecting code).
    /// Trait objects whose metadata is a vtable pointer are **not**
    /// supported (see [`Self::try_alloc_dst_arc`]'s safety contract).
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_arc_pin<T: ?Sized + Send + Sync + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> core::pin::Pin<Arc<T, A>>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: caller upholds `alloc_dst_arc`'s contract.
        let arc = unsafe { self.alloc_dst_arc::<T>(layout, metadata, init) };
        Arc::into_pin(arc)
    }

    /// Fallible variant of [`Self::alloc_dst_arc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_arc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_alloc_dst_arc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_arc_pin<T: ?Sized + Send + Sync + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<core::pin::Pin<Arc<T, A>>, AllocError>
    where
        A: Send + Sync + 'static,
    {
        // SAFETY: caller upholds `try_alloc_dst_arc`'s contract.
        unsafe { self.try_alloc_dst_arc::<T>(layout, metadata, init).map(Arc::into_pin) }
    }

    /// `Pin` variant of [`Self::alloc_dst_rc`].
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_rc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_rc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_rc_pin<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> core::pin::Pin<Rc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: caller upholds `alloc_dst_rc`'s contract.
        let rc = unsafe { self.alloc_dst_rc::<T>(layout, metadata, init) };
        Rc::into_pin(rc)
    }

    /// Fallible variant of [`Self::alloc_dst_rc_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_rc`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_alloc_dst_rc`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_rc_pin<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<core::pin::Pin<Rc<T, A>>, AllocError>
    where
        A: 'static,
    {
        // SAFETY: caller upholds `try_alloc_dst_rc`'s contract.
        unsafe { self.try_alloc_dst_rc::<T>(layout, metadata, init).map(Rc::into_pin) }
    }

    /// `Pin` variant of [`Self::alloc_dst_box`]. Trait objects are
    /// **not** supported (see [`Self::try_alloc_dst_arc`]'s safety
    /// contract); use the slice or sized variants.
    ///
    /// # Panics
    ///
    /// See [`Self::alloc_dst_box`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::alloc_dst_box`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    #[must_use]
    pub unsafe fn alloc_dst_box_pin<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> core::pin::Pin<Box<T, A>>
    where
        A: 'static,
    {
        // SAFETY: caller upholds `alloc_dst_box`'s contract.
        let b = unsafe { self.alloc_dst_box::<T>(layout, metadata, init) };
        Box::into_pin(b)
    }

    /// Fallible variant of [`Self::alloc_dst_box_pin`].
    ///
    /// # Errors
    ///
    /// See [`Self::try_alloc_dst_box`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::try_alloc_dst_box`].
    #[cfg_attr(docsrs, doc(cfg(feature = "dst")))]
    pub unsafe fn try_alloc_dst_box_pin<T: ?Sized + ::ptr_meta::Pointee>(
        &self,
        layout: Layout,
        metadata: T::Metadata,
        init: impl FnOnce(*mut T),
    ) -> Result<core::pin::Pin<Box<T, A>>, AllocError>
    where
        A: 'static,
    {
        // SAFETY: caller upholds `try_alloc_dst_box`'s contract.
        unsafe { self.try_alloc_dst_box::<T>(layout, metadata, init).map(Box::into_pin) }
    }
}
