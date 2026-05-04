// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Crate-internal bump-pointer primitives on [`Arena`].
//!
//! These power the sibling allocation APIs and the
//! `&Arena<A>: Allocator` impl.

use core::alloc::Layout;
use core::ptr::NonNull;

use allocator_api2::alloc::{AllocError, Allocator};

use super::{Arena, align_up, bump_local_drop_count, check_isize_overflow};
use crate::internal::constants::MAX_SMART_PTR_ALIGN;
use crate::internal::drop_list::DropEntry as InnerDropEntry;
use crate::internal::in_chunk::InLocalChunk;
use crate::internal::local_chunk::LocalChunk;

impl<A: Allocator + Clone> Arena<A> {
    /// Increment the refcount of the local chunk containing `ptr`.
    ///
    /// Used by in-place split operations that turn one live allocation
    /// into two independently-owned halves.
    ///
    /// # Safety
    ///
    /// `ptr` must lie in a live local chunk produced by this arena, and
    /// the caller must already own a `+1` on that chunk.
    #[expect(
        clippy::unused_self,
        reason = "logically an Arena<A> operation: ties the type parameter A and the lifetime to the caller, even though the chunk is recovered from `ptr` via the chunk-header mask trick"
    )]
    pub(crate) unsafe fn inc_ref_for_buffer(&self, ptr: NonNull<u8>) {
        // SAFETY: caller-forwarded chunk-header invariant — `ptr`
        // lies inside a live local chunk produced by this arena.
        let chunk = unsafe { InLocalChunk::<_, A>::new(ptr) }.chunk_ptr();
        // SAFETY: caller holds a `+1` on `chunk`, satisfying
        // `LocalChunk::inc_ref`'s refcount-positive precondition.
        unsafe { (*chunk.as_ptr()).inc_ref() };
    }

    /// Try to grow the allocation at `ptr` in place.
    ///
    /// Succeeds only if the allocation still ends at the current bump
    /// cursor and the chunk has room for the growth.
    ///
    /// # Safety
    ///
    /// `ptr` must come from [`Self::allocate_layout`] with `old_layout`,
    /// still own its allocator `+1`, and `new_layout` must preserve
    /// alignment while not shrinking the size.
    pub(crate) unsafe fn try_grow_in_place(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Option<NonNull<u8>> {
        // SAFETY: callers (allocator_impl.rs and vec/vec.rs) gate on
        // `align == align` and `new_size >= old_size` before invoking.
        debug_assert_eq!(old_layout.align(), new_layout.align(), "try_grow_in_place: align must match");
        debug_assert!(new_layout.size() >= old_layout.size(), "try_grow_in_place: new size must be >= old");
        // Stub state uses `dangling()` for both pointers, so the tail check
        // fails without dereferencing them.
        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let drop_back_addr = drop_back_ptr.as_ptr() as usize;
        // The buffer's tail must coincide with the chunk's bump cursor (data_ptr).
        let buf_addr = ptr.as_ptr() as usize;
        let data_ptr_addr = data_ptr.as_ptr() as usize;
        let buf_end = buf_addr.checked_add(old_layout.size())?;
        if buf_end != data_ptr_addr {
            return None;
        }
        let growth = new_layout.size() - old_layout.size();
        let new_data_ptr_addr = data_ptr_addr.checked_add(growth)?;
        if new_data_ptr_addr > drop_back_addr {
            return None;
        }
        // Provenance-preserving advance: `data_ptr` has provenance for
        // the chunk payload; `byte_add(growth)` stays within the chunk.
        // SAFETY: `new_data_ptr_addr <= drop_back_addr`, so `data_ptr +
        // growth` lies within the chunk payload.
        let new_data_ptr = unsafe { data_ptr.byte_add(growth) };
        self.current_local.data_ptr.set(new_data_ptr);
        self.charge_alloc_stats(growth);
        Some(ptr)
    }

    /// Reserve `layout` bytes in the current local chunk and return a
    /// pointer into the payload, with the chunk's refcount
    /// incremented by one for the caller to own. Used by the
    /// [`allocator_api2::alloc::Allocator`] impl on `&Arena`.
    #[cfg_attr(test, mutants::skip)] // `needed` arithmetic absorbed by chunk-class rounding in `refill_local`.
    pub(crate) fn allocate_layout(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        if layout.align() >= MAX_SMART_PTR_ALIGN {
            return Err(AllocError);
        }

        // Requests above `max_normal_alloc` go through a one-shot oversized
        // chunk and leave `deallocate` the matching `+1` to release.
        if layout.size() > self.provider.max_normal_alloc {
            return self.allocate_oversized_layout(layout);
        }

        loop {
            let data_ptr = self.current_local.data_ptr.get();
            let drop_back_ptr = self.current_local.drop_back.get();
            let drop_back_addr = drop_back_ptr.as_ptr() as usize;

            let aligned_addr = align_up(data_ptr.as_ptr() as usize, layout.align().max(1));
            let bumped = layout.size().max(1);
            if let Some(end_addr) = aligned_addr.checked_add(bumped)
                && end_addr <= drop_back_addr
            {
                // Provenance-preserving construction from `data_ptr`.
                let data_addr = data_ptr.as_ptr() as usize;
                let aligned_offset = aligned_addr - data_addr;
                // SAFETY: `aligned + bumped <= drop_back`, both lie in chunk payload.
                let (aligned_ptr, end_ptr) = unsafe { (data_ptr.byte_add(aligned_offset), data_ptr.byte_add(aligned_offset + bumped)) };
                let value_ptr: *mut u8 = aligned_ptr.as_ptr();
                self.current_local.data_ptr.set(end_ptr);
                // Record the caller's `+1` in the deferred-reconcile counter
                // instead of touching the chunk refcount directly.
                self.current_local.bump_smart_pointers_issued();
                self.charge_alloc_stats(layout.size());
                // SAFETY: `value_ptr` is non-null.
                return Ok(unsafe { NonNull::new_unchecked(value_ptr) });
            }

            let needed = layout.size() + layout.align().saturating_sub(core::mem::align_of::<usize>());
            self.refill_local(needed)?;
        }
    }

    /// Cold one-shot oversized allocation for the
    /// [`allocator_api2::alloc::Allocator`] impl. Acquires a dedicated
    /// oversized chunk, hands out the aligned payload pointer, and
    /// leaves the chunk's refcount at exactly `+1` so the matching
    /// `deallocate` releases it.
    #[cold]
    #[inline(never)]
    fn allocate_oversized_layout(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        check_isize_overflow(layout.size(), layout.align())?;
        let needed = layout
            .size()
            .checked_add(layout.align().saturating_sub(core::mem::align_of::<usize>()))
            .ok_or(AllocError)?;
        let chunk = self.provider.acquire_local(needed)?;
        // Chunk arrives with refcount inflated to LARGE.
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits the
        // request after alignment, so `align_offset` succeeds.
        let aligned_offset = unsafe { super::internals::align_offset(data_addr, layout.align().max(1)).unwrap_unchecked() };
        // SAFETY: payload-extent invariant — `aligned_offset` is within `[0, cap)`.
        let value_ptr = unsafe { data_ptr.byte_add(aligned_offset) };

        self.charge_alloc_stats(layout.size());
        // Reconcile down from LARGE to +1: `rcs_issued = 1, pinned = false`
        // leaves a single hold for the caller; the matching `deallocate`
        // releases it.
        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { LocalChunk::reconcile_swap_out(chunk, 1, false) };
        Ok(value_ptr)
    }

    #[cfg(any(feature = "dst", feature = "bytesbuf"))]
    #[cfg_attr(test, mutants::skip)] // Callers pre-check alignment; boundary mutations are unreachable.
    pub(crate) fn allocate_shared_layout(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        // SAFETY: All callers pre-validate alignment:
        // - `try_alloc_dst_arc` calls `check_chunk_alignment` which rejects `align >= MAX_SMART_PTR_ALIGN`.
        // - The `bytesbuf` integration always supplies `align == 1`.
        unsafe { core::hint::assert_unchecked(layout.align() < MAX_SMART_PTR_ALIGN) };
        check_isize_overflow(layout.size(), layout.align())?;

        // Requests that exceed `max_normal_alloc` cannot fit the bump
        // path (whose extent is capped at `shared_max_bump_extent` <
        // `MAX_CHUNK_BYTES`). Route them through a one-shot oversized
        // shared chunk and leave +1 on the chunk's refcount for the
        // matching `SharedChunk::dec_ref` to release.
        if layout.size() > self.provider.max_normal_alloc {
            return self.allocate_shared_oversized_layout(layout);
        }

        let bumped = layout.size().max(1);
        loop {
            let data_ptr = self.current_shared.data_ptr.get();
            let drop_back_ptr = self.current_shared.drop_back.get();
            let drop_back_addr = drop_back_ptr.as_ptr() as usize;
            let aligned_addr = align_up(data_ptr.as_ptr() as usize, layout.align().max(1));
            if let Some(end_addr) = aligned_addr.checked_add(bumped)
                && end_addr <= drop_back_addr
            {
                // Provenance-preserving construction from `data_ptr`.
                let data_addr = data_ptr.as_ptr() as usize;
                let aligned_offset = aligned_addr - data_addr;
                // SAFETY: gated above; both lie in chunk payload.
                let (aligned_ptr, end_ptr) = unsafe { (data_ptr.byte_add(aligned_offset), data_ptr.byte_add(aligned_offset + bumped)) };
                let ptr: *mut u8 = aligned_ptr.as_ptr();
                self.current_shared.data_ptr.set(end_ptr);
                self.current_shared.bump_smart_pointers_issued();
                self.charge_alloc_stats(layout.size());
                // SAFETY: pointer is derived from a non-null chunk payload.
                return Ok(unsafe { NonNull::new_unchecked(ptr) });
            }
            self.refill_shared(
                layout
                    .size()
                    .saturating_add(layout.align().saturating_sub(core::mem::align_of::<usize>())),
            )?;
        }
    }

    /// Cold one-shot oversized shared allocation, mirror of
    /// [`Self::allocate_oversized_layout`] for the shared-flavor
    /// (`Arc`) callers. Used by `bytesbuf` and DST `Arc` paths whose
    /// request exceeds `max_normal_alloc` and would otherwise loop in
    /// `refill_shared` because a max-class chunk's bump extent is
    /// strictly less than `MAX_CHUNK_BYTES`.
    #[cfg(any(feature = "dst", feature = "bytesbuf"))]
    #[cold]
    #[inline(never)]
    fn allocate_shared_oversized_layout(&self, layout: Layout) -> Result<NonNull<u8>, AllocError> {
        let needed = layout
            .size()
            .checked_add(layout.align().saturating_sub(core::mem::align_of::<usize>()))
            .ok_or(AllocError)?;
        let chunk = self.provider.acquire_shared(needed)?;
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data_ptr = unsafe { crate::internal::shared_chunk::SharedChunk::<A>::data_ptr(chunk) };
        let data_addr = data_ptr.as_ptr() as usize;
        // SAFETY: provider post-condition guarantees the chunk fits the
        // request after alignment, so `align_offset` succeeds.
        let aligned_offset = unsafe { super::internals::align_offset(data_addr, layout.align().max(1)).unwrap_unchecked() };
        // SAFETY: payload-extent invariant — `aligned_offset` is within `[0, cap)`.
        let value_ptr = unsafe { data_ptr.byte_add(aligned_offset) };

        self.charge_alloc_stats(layout.size());
        // Reconcile down from LARGE to +1: `arcs_issued = 1` leaves a
        // single hold for the caller; the matching `dec_ref` releases
        // it.
        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { crate::internal::shared_chunk::SharedChunk::reconcile_swap_out(chunk, 1) };
        Ok(value_ptr)
    }

    /// Internal grow helper used by [`String`](crate::strings::String).
    ///
    /// # Safety
    ///
    /// Caller must follow the same rules as `Allocator::grow`: `ptr`
    /// must have come from a previous allocation through this arena
    /// with size `old_size` and alignment `align_of::<usize>()`;
    /// `new_size >= old_size`.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails on the
    /// slow-path relocation or arithmetic overflows.
    pub(crate) unsafe fn grow_for_string(
        &self,
        ptr: NonNull<u8>,
        old_size: usize,
        new_size: usize,
        align: usize,
    ) -> Result<NonNull<u8>, AllocError> {
        debug_assert!(new_size >= old_size);
        check_isize_overflow(new_size, align)?;
        let data_ptr = self.current_local.data_ptr.get();
        let drop_back_ptr = self.current_local.drop_back.get();
        let drop_back_addr = drop_back_ptr.as_ptr() as usize;
        let ptr_addr = ptr.as_ptr() as usize;
        let data_ptr_addr = data_ptr.as_ptr() as usize;
        // The buffer's tail must coincide with the chunk's bump cursor (data_ptr).
        if ptr_addr.checked_add(old_size) == Some(data_ptr_addr) {
            let growth = new_size.checked_sub(old_size).ok_or(AllocError)?;
            let new_data_ptr_addr = data_ptr_addr.checked_add(growth).ok_or(AllocError)?;
            if new_data_ptr_addr <= drop_back_addr {
                // Provenance-preserving advance from the existing `data_ptr`.
                // SAFETY: `new_data_ptr_addr <= drop_back_addr`, so
                // `data_ptr + growth` lies within the chunk payload.
                let new_data_ptr = unsafe { data_ptr.byte_add(growth) };
                self.current_local.data_ptr.set(new_data_ptr);
                self.charge_alloc_stats(growth);
                return Ok(ptr);
            }
        }
        let layout = Layout::from_size_align(new_size.max(1), align).map_err(|_e| AllocError)?;
        let new_ptr = self.allocate_layout(layout)?;
        // SAFETY: caller guarantees `old_size` bytes are readable from `ptr`; `new_ptr` is fresh.
        unsafe { core::ptr::copy_nonoverlapping(ptr.as_ptr(), new_ptr.as_ptr(), old_size) };
        self.bump_relocation();
        Ok(new_ptr)
    }

    /// Try to shrink an allocation at the cursor tip. Returns `true` if
    /// reclamation succeeded (the buffer ends exactly at the current
    /// cursor position) and the cursor was moved back.
    ///
    /// # Safety
    ///
    /// `buffer_end` must equal `buffer_start + old_cap` where both are
    /// within the current local chunk's payload.
    pub(crate) unsafe fn try_shrink_at_cursor(&self, buffer_end: *const u8, reclaim_bytes: usize) -> bool {
        let data_ptr = self.current_local.data_ptr.get();
        if buffer_end == data_ptr.as_ptr().cast_const() {
            // SAFETY: caller guarantees `buffer_end` is `reclaim_bytes` past
            // a valid in-payload start, so subtracting yields a valid in-payload
            // pointer.
            let new_data_ptr = unsafe { data_ptr.as_ptr().sub(reclaim_bytes) };
            // SAFETY: `new_data_ptr` lies in [payload_base, payload_end] of the active chunk and is non-null.
            self.current_local.data_ptr.set(unsafe { NonNull::new_unchecked(new_data_ptr) });
            return true;
        }
        false
    }

    /// Try to install a slice [`InnerDropEntry`] for a buffer that
    /// already lives in a local chunk produced by this arena. Used by
    /// the in-place freeze fast paths in [`crate::vec::Vec`] etc., where
    /// the buffer was allocated through the [`allocator_api2::alloc::Allocator`]
    /// impl on `&Arena<A>` (which does not co-allocate a drop entry).
    ///
    /// Returns `true` if the entry was installed; `false` if there is no
    /// room or the buffer's chunk is no longer current (in which case
    /// the caller must fall back to a copy path so other live
    /// allocations in a frozen chunk are not clobbered).
    ///
    /// # Safety
    ///
    /// - `value_ptr` must point at the first byte of a buffer holding
    ///   `len` initialized `T`s, allocated by this arena.
    /// - The chunk owning `value_ptr` must currently hold at least one
    ///   `+1` (refcount-positive invariant) — typically the buffer
    ///   allocation's own `+1`.
    /// - `drop_fn` must be the slice drop shim monomorphized for `T`
    ///   (i.e., [`crate::internal::drop_list::drop_shim_slice::<T>`]).
    #[cfg_attr(test, mutants::skip)] // mirror_dc redundancy: drop_count bump is defense-in-depth.
    pub(crate) unsafe fn try_install_slice_drop_entry(&self, value_ptr: NonNull<u8>, drop_fn: unsafe fn(*mut u8, usize), len: u16) -> bool {
        // SAFETY: chunk-header invariant — `value_ptr` was returned by
        // an arena allocation, so masking yields a live `LocalChunk`.
        let chunk = unsafe { InLocalChunk::<_, A>::new(value_ptr) }.chunk_ptr();
        // Only install when the buffer lives in the *current* local
        // chunk: that is the only case where we can verify
        // `new_drop_back >= data_ptr`, i.e., that the entry slot does
        // not collide with already-allocated payload bytes.
        if self.current_local.chunk.get() != Some(chunk) {
            return false;
        }
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let data = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        // SAFETY: payload-extent invariant — `value_ptr` is within
        // `[data, data + capacity)`, so `offset_from` is well-defined
        // and yields a non-negative offset. For `current_local`
        // chunks, `capacity` is bounded by `max_bump_extent::<A>() =
        // CHUNK_ALIGN - header_size::<A>() ≤ u16::MAX + 1`, and since
        // `header_size::<A>() > 0` the strict bound `value_offset ≤
        // u16::MAX` holds.
        let value_offset_isize = unsafe { value_ptr.as_ptr().offset_from(data.as_ptr()) };
        // SAFETY: payload-extent invariant — offset is in `[0, capacity)` ⊂ `[0, u16::MAX)`.
        unsafe { core::hint::assert_unchecked(value_offset_isize >= 0) };
        let value_offset = value_offset_isize.cast_unsigned();
        // SAFETY: payload-extent invariant — offset bounded by chunk capacity (< u16::MAX).
        let value_offset_u16 = unsafe { u16::try_from(value_offset).unwrap_unchecked() };

        let entry_size = core::mem::size_of::<InnerDropEntry>();
        let drop_back_ptr = self.current_local.drop_back.get();
        let drop_back_addr = drop_back_ptr.as_ptr() as usize;
        let data_ptr_addr = self.current_local.data_ptr.get().as_ptr() as usize;
        // SAFETY: `drop_back_ptr` is a real, in-payload address inside
        // a live chunk; as a `usize` it is many orders of magnitude
        // larger than `size_of::<InnerDropEntry>()`, so the subtraction
        // cannot underflow.
        unsafe { core::hint::assert_unchecked(drop_back_addr >= entry_size) };
        let new_drop_back_addr = drop_back_addr - entry_size;
        // The entry slot must not collide with already-allocated payload
        // (everything at addresses `< data_ptr_addr` is in use).
        if new_drop_back_addr < data_ptr_addr {
            return false;
        }

        // Provenance-preserving construction from `drop_back_ptr`.
        // SAFETY: gated above; `entry_size <= drop_back_addr` and the
        // resulting pointer lies within the chunk payload.
        let new_drop_back_ptr = unsafe { drop_back_ptr.byte_sub(entry_size) };

        // lies within the chunk's payload, in the free region between
        // `data_ptr` and `drop_back` we just verified is large enough.
        let entry_ptr: *mut InnerDropEntry = new_drop_back_ptr.cast::<InnerDropEntry>().as_ptr();
        let entry = InnerDropEntry::new(drop_fn, value_offset_u16, len);
        // SAFETY: `entry_ptr` is non-null, properly aligned for
        // `InnerDropEntry` by construction of the back-stack layout,
        // and points at writable, owned bytes.
        unsafe { core::ptr::write(entry_ptr, entry) };

        // Update the chunk's drop_count and the arena's mirror so the
        // next bump-allocation respects the new drop_back.
        // SAFETY: refcount-positive — current chunk is live.
        unsafe { bump_local_drop_count(chunk) };
        self.current_local.drop_back.set(new_drop_back_ptr);

        true
    }

    /// Cold one-shot oversized "length-prefixed copy" allocator for
    /// the local (`Rc`/`Box`) string flavors. Layout is identical to
    /// the bump-path expansion of [`try_alloc_prefixed!`]:
    /// `[len: usize][elems: [E; len]]`. Returns a pointer to the
    /// element region with the chunk's refcount reconciled to `+1`
    /// for the caller's smart pointer to own.
    ///
    /// # Safety
    ///
    /// `src_ptr` must be valid for `len` elements of type `E`.
    /// `payload_bytes == len * size_of::<E>()`.
    pub(crate) unsafe fn try_alloc_prefixed_local_oversized<E: Copy>(
        &self,
        src_ptr: *const E,
        len: usize,
        payload_bytes: usize,
    ) -> Result<NonNull<E>, AllocError> {
        let prefix = core::mem::size_of::<usize>();
        let total = prefix.checked_add(payload_bytes).ok_or(AllocError)?;
        // Match the bump-path layout: align data to `align_of::<E>()`;
        // the prefix sits immediately before, unaligned. The chunk
        // payload base is `CHUNK_ALIGN`-aligned (≥ any plausible `E`),
        // so the prefix lands at offset 0 from the payload — no
        // pre-padding beyond the prefix itself.
        let align = core::mem::align_of::<E>();
        check_isize_overflow(total, align)?;
        let needed = total;
        let chunk = self.provider.acquire_local(needed)?;
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let base_ptr = unsafe { LocalChunk::<A>::data_ptr(chunk) };
        // Place data at the first `align_of::<E>()`-aligned offset
        // that leaves room for the `prefix`-byte length slot.
        let base_addr = base_ptr.as_ptr() as usize;
        let data_aligned_addr = (base_addr + prefix + (align - 1)) & !(align - 1);
        let prefix_offset = (data_aligned_addr - prefix) - base_addr;
        // SAFETY: chunk payload covers `header + capacity` bytes which
        // exceeds `prefix_offset + prefix + payload_bytes` by the
        // provider's `needed` postcondition.
        let prefix_ptr_byte = unsafe { base_ptr.as_ptr().add(prefix_offset) };
        #[allow(clippy::cast_ptr_alignment, reason = "prefix slot is accessed via write_unaligned below")]
        let prefix_ptr: *mut usize = prefix_ptr_byte.cast::<usize>();
        // SAFETY: prefix slot is within the chunk-owned reserved
        // range, exclusively owned by this allocator call, and not yet
        // initialized. `UninitSlot` records those invariants once; the
        // `write_unaligned` below is then safe.
        let prefix_slot = unsafe { crate::internal::slot::UninitSlot::<usize>::from_raw(prefix_ptr) };
        prefix_slot.write_unaligned(len);
        // SAFETY: element storage immediately follows the prefix; the
        // data position was aligned to `align_of::<E>()`.
        let elems_ptr: *mut E = unsafe { prefix_ptr_byte.add(prefix).cast::<E>() };
        if len > 0 {
            // SAFETY: source is valid for `len`; destination is fresh.
            unsafe { core::ptr::copy_nonoverlapping(src_ptr, elems_ptr, len) };
        }
        self.charge_alloc_stats(total);
        // Reconcile LARGE → +1 (caller's Rc/Box owns it).
        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { LocalChunk::reconcile_swap_out(chunk, 1, false) };
        // SAFETY: `elems_ptr` is non-null inside the chunk payload.
        Ok(unsafe { NonNull::new_unchecked(elems_ptr) })
    }

    /// Cold one-shot oversized prefixed allocator for the shared
    /// (`Arc`) string flavor. Mirror of
    /// [`Self::try_alloc_prefixed_local_oversized`].
    ///
    /// # Safety
    ///
    /// `src_ptr` must be valid for `len` elements of type `E`.
    /// `payload_bytes == len * size_of::<E>()`.
    pub(crate) unsafe fn try_alloc_prefixed_shared_oversized<E: Copy>(
        &self,
        src_ptr: *const E,
        len: usize,
        payload_bytes: usize,
    ) -> Result<NonNull<E>, AllocError> {
        let prefix = core::mem::size_of::<usize>();
        let total = prefix.checked_add(payload_bytes).ok_or(AllocError)?;
        let align = core::mem::align_of::<E>();
        check_isize_overflow(total, align)?;
        let needed = total;
        let chunk = self.provider.acquire_shared(needed)?;
        // SAFETY: refcount-positive — chunk held at LARGE inflation.
        let base_ptr = unsafe { crate::internal::shared_chunk::SharedChunk::<A>::data_ptr(chunk) };
        let base_addr = base_ptr.as_ptr() as usize;
        let data_aligned_addr = (base_addr + prefix + (align - 1)) & !(align - 1);
        let prefix_offset = (data_aligned_addr - prefix) - base_addr;
        // SAFETY: see `try_alloc_prefixed_local_oversized`.
        let prefix_ptr_byte = unsafe { base_ptr.as_ptr().add(prefix_offset) };
        #[allow(clippy::cast_ptr_alignment, reason = "prefix slot is accessed via write_unaligned below")]
        let prefix_ptr: *mut usize = prefix_ptr_byte.cast::<usize>();
        // SAFETY: prefix slot is exclusively owned and uninitialized;
        // see `try_alloc_prefixed_local_oversized` for full justification.
        let prefix_slot = unsafe { crate::internal::slot::UninitSlot::<usize>::from_raw(prefix_ptr) };
        prefix_slot.write_unaligned(len);
        // SAFETY: element storage immediately follows the prefix.
        let elems_ptr: *mut E = unsafe { prefix_ptr_byte.add(prefix).cast::<E>() };
        if len > 0 {
            // SAFETY: source is valid for `len`; destination is fresh.
            unsafe { core::ptr::copy_nonoverlapping(src_ptr, elems_ptr, len) };
        }
        self.charge_alloc_stats(total);
        // Reconcile LARGE → +1 (caller's Arc owns it).
        // SAFETY: chunk held LARGE while we acted as its sole tenant.
        unsafe { crate::internal::shared_chunk::SharedChunk::reconcile_swap_out(chunk, 1) };
        // SAFETY: `elems_ptr` is non-null inside the chunk payload.
        Ok(unsafe { NonNull::new_unchecked(elems_ptr) })
    }
}
