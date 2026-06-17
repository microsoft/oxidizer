// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump allocator over a single chunk.
//!
//! [`ChunkMutator<C>`] owns one strong chunk reference and hands out
//! [`InChunk`], [`Uninit`], and [`UninitDrop`] tickets. Drop publishes pending
//! drop entries, releases the refcount, and may trigger
//! [`ChunkOps::teardown_and_release`].

use core::cell::Cell;
use core::ptr::{self, NonNull};
use core::{hint, mem};

use super::chunk_ops::ChunkOps;
use super::drop_entry::DropEntry;
use super::in_chunk::InChunk;
use super::uninit::{Uninit, UninitDrop};

/// Owns one strong reference to a chunk and tracks the bump cursor and the
/// growing-down drop-entry top.
///
/// Hot-path layout stores only `chunk`, `bump`, and `drop_top`; cold paths
/// re-derive payload bounds from `chunk`.
pub(crate) struct ChunkMutator<C: ?Sized + ChunkOps> {
    chunk: Option<NonNull<C>>,
    /// Bump cursor stored as a pointer to preserve chunk provenance under
    /// Stacked / Tree Borrows.
    bump: Cell<NonNull<u8>>,
    /// Top of the drop-entry region (entries grow downward). Same
    /// pointer-preserves-provenance rationale as `bump`.
    drop_top: Cell<NonNull<u8>>,
}

// SAFETY: the mutator owns one strong chunk ref and moves that ownership
// across threads only when `C: Send`. `LocalChunk` follows the owning thread;
// `SharedChunk` uses atomics. The `Cell` fields intentionally make this `!Sync`.
unsafe impl<C: ?Sized + ChunkOps + Send> Send for ChunkMutator<C> {}

impl<C: ?Sized + ChunkOps> ChunkMutator<C> {
    /// Builds a mutator owning the +1 already on `chunk`.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live chunk whose refcount is already
    /// incremented in this mutator's name. The mutator becomes the unique
    /// owner of that +1 and will release it on drop.
    pub(crate) unsafe fn from_owned(chunk: NonNull<C>) -> Self {
        // SAFETY: caller asserts `chunk` is live; `payload_range_for` only
        // dereferences `chunk` to read `payload_ptr()` and `capacity()`.
        let (start_addr, end_addr) = unsafe { Self::payload_range_for(chunk) };
        // SAFETY: caller asserts `chunk` is live.
        let start = unsafe { C::payload_ptr(chunk) };
        let aligned_end_offset = end_addr - start_addr;
        // SAFETY: `aligned_end_offset <= cap`; `start.byte_add` lands
        // within (or at one-past-end of) the chunk payload.
        let drop_top = unsafe { start.byte_add(aligned_end_offset) };
        Self {
            chunk: Some(chunk),
            bump: Cell::new(start),
            drop_top: Cell::new(drop_top),
        }
    }

    /// Builds an empty mutator. Every `try_alloc*` returns `None`, deferring
    /// chunk allocation until the first user-visible allocation.
    pub(crate) const fn empty() -> Self {
        Self {
            chunk: None,
            // Sentinels: `bump > drop_top`, so bound checks fail without an
            // explicit `self.chunk?`. These pointers are never dereferenced.
            bump: Cell::new(NonNull::<u16>::dangling().cast::<u8>()),
            drop_top: Cell::new(NonNull::<u8>::dangling()),
        }
    }

    /// Free byte count between the bump cursor and the drop-entry top.
    #[cfg(test)]
    #[inline]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // test-only helper
    pub(crate) fn free_bytes(&self) -> usize {
        let top = self.drop_top.get().as_ptr() as usize;
        let cur = self.bump.get().as_ptr() as usize;
        top.saturating_sub(cur)
    }

    /// Free byte count between the bump cursor and the drop-entry top.
    /// Stats helper; empty-mutator sentinels saturate to 0. Reported as `u32`
    /// because chunk capacity is far below `u32::MAX`.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn wasted_tail_for_stats(&self) -> u32 {
        let top = self.drop_top.get().as_ptr() as usize;
        let cur = self.bump.get().as_ptr() as usize;
        u32::try_from(top.saturating_sub(cur)).unwrap_or(u32::MAX)
    }

    /// Reads the chunk's payload start and end addresses.
    ///
    /// # Panics
    ///
    /// Panics on the empty mutator; hot-path callers invoke this only after a
    /// successful reservation.
    #[inline]
    fn payload_range(&self) -> (usize, usize) {
        let chunk = self.chunk.expect("payload_range: chunk must be set");
        // SAFETY: mutator owns one strong reference to `chunk` for as
        // long as `&self` is borrowed.
        unsafe { Self::payload_range_for(chunk) }
    }

    /// Reads `chunk`'s payload start address and drop-region-aligned end
    /// address.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live chunk.
    #[inline]
    unsafe fn payload_range_for(chunk: NonNull<C>) -> (usize, usize) {
        // SAFETY: caller asserts `chunk` is live.
        let (start, cap) = unsafe { (C::payload_ptr(chunk), chunk.as_ref().capacity()) };
        let start_addr = start.as_ptr() as usize;
        let end_addr = if C::REGISTERS_DROPS {
            // Align the reported end down to `align_of::<DropEntry>()` so the
            // drop-entry region (entries grow down from this point) stays
            // naturally aligned regardless of payload-start alignment.
            let entry_align = mem::align_of::<DropEntry>();
            (start_addr + cap) & !(entry_align - 1)
        } else {
            // Flavors that never register drop entries let the bump cursor use
            // the whole payload — no tail region is reserved.
            start_addr + cap
        };
        (start_addr, end_addr)
    }

    /// Returns the underlying chunk pointer without checking the
    /// `Option` discriminant. Hot-path helper for smart-pointer
    /// reservations.
    ///
    /// # Safety
    ///
    /// Caller must have observed a successful `try_alloc*` /
    /// `try_reserve_*` on this mutator immediately prior (without
    /// intervening `CurrentChunk::replace` / `drop_replace`).
    #[inline]
    pub(crate) unsafe fn chunk_ptr_unchecked(&self) -> NonNull<C> {
        // SAFETY: see contract.
        unsafe { self.chunk.unwrap_unchecked() }
    }

    /// Returns the chunk this mutator owns, or `None` for the empty
    /// (sentinel) mutator that has no chunk installed.
    #[inline]
    pub(crate) fn chunk_ptr(&self) -> Option<NonNull<C>> {
        self.chunk
    }

    /// Reserves `size` bytes aligned to `align`. Returns an `InChunk<u8>`
    /// pointing at the start, or `None` if the chunk has insufficient
    /// room or the request would overflow.
    ///
    /// # Overflow safety
    ///
    /// On 64-bit targets `cur_addr` is asserted to fit in `isize`, allowing
    /// overflow-free alignment math. `aligned_addr + size` still uses
    /// `checked_add` because `size` is caller-controlled.
    #[inline]
    // Mutation testing is suppressed: any mutation that always rejects
    // sends callers into an infinite refill spin (OOM).
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn try_alloc(&self, size: usize, align: usize) -> Option<InChunk<u8>> {
        debug_assert!(align.is_power_of_two(), "align must be a power of two");
        debug_assert!(size.checked_add(align).is_some(), "size + align overflows usize");
        let cur = self.bump.get();
        let cur_addr = cur.as_ptr() as usize;
        let drop_top_addr = self.drop_top.get().as_ptr() as usize;
        // SAFETY: see the overflow-safety note above.
        unsafe {
            hint::assert_unchecked(cur_addr > 0);
            // On 64-bit targets this lets the optimizer treat align-up as
            // overflow-free. Narrower targets use checked arithmetic.
            #[cfg(target_pointer_width = "64")]
            hint::assert_unchecked(isize::try_from(cur_addr).is_ok());
        }
        #[cfg(target_pointer_width = "64")]
        let aligned_addr = (cur_addr + (align - 1)) & !(align - 1);
        #[cfg(not(target_pointer_width = "64"))]
        let aligned_addr = (cur_addr.checked_add(align - 1)?) & !(align - 1);
        // ZST smart-pointer values must still point strictly inside the chunk;
        // a tail one-past pointer would mask to the next 64 KiB tile.
        // `size.max(1)` probes that byte without changing the ZST bump.
        let probe_end = aligned_addr.checked_add(size.max(1))?;
        if probe_end > drop_top_addr {
            return None;
        }
        // SAFETY: `end_addr <= drop_top_addr`, so both `aligned_ptr`
        // and `new_bump` land within the chunk payload; `byte_add` on
        // the current bump cursor preserves chunk-wide provenance.
        let aligned_ptr = unsafe { cur.byte_add(aligned_addr - cur_addr) };
        // SAFETY: see above.
        let new_bump = unsafe { aligned_ptr.byte_add(size) };
        self.bump.set(new_bump);
        Some(InChunk::from_raw(aligned_ptr))
    }

    /// Reserves storage for one `T` and returns an `Uninit<'_, T>` ticket.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    pub(crate) fn try_alloc_uninit<T>(&self) -> Option<Uninit<'_, T>> {
        let bytes = self.try_alloc(mem::size_of::<T>(), mem::align_of::<T>())?;
        Some(Uninit::new(bytes.cast::<T>()))
    }

    /// [`Self::try_alloc`] paired with the owning chunk pointer. The
    /// success of `try_alloc` proves the mutator owns a chunk, so the
    /// caller doesn't need to use [`Self::chunk_ptr_unchecked`] (which
    /// would require an `unsafe` block) afterwards.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    pub(crate) fn try_alloc_with_chunk(&self, size: usize, align: usize) -> Option<(InChunk<u8>, NonNull<C>)> {
        let in_chunk = self.try_alloc(size, align)?;
        // SAFETY: a successful `try_alloc` proves the mutator owns a
        // chunk.
        Some((in_chunk, unsafe { self.chunk_ptr_unchecked() }))
    }

    /// Byte-slice fast path: skips the alignment mask, `checked_mul`,
    /// and ZST branch. Only valid for `T = u8` (align 1, size 1).
    #[inline]
    pub(crate) fn try_alloc_bytes(&self, len: usize) -> Option<Uninit<'_, [u8]>> {
        let cur = self.bump.get();
        let cur_addr = cur.as_ptr() as usize;
        let drop_top_addr = self.drop_top.get().as_ptr() as usize;
        hint_chunk_cur_addr_nonnull(cur_addr);
        let end_addr = cur_addr.checked_add(len)?;
        if end_addr > drop_top_addr {
            return None;
        }
        // SAFETY: `end_addr <= drop_top_addr`.
        let new_bump = unsafe { cur.byte_add(len) };
        self.bump.set(new_bump);
        Some(Uninit::new(InChunk::from_raw(cur).into_slice::<u8>(len)))
    }

    /// Reserves storage for `len` consecutive `T`s and returns an
    /// `Uninit<'_, [T]>` ticket.
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    pub(crate) fn try_alloc_uninit_slice<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let size = mem::size_of::<T>().checked_mul(len)?;
        let bytes = self.try_alloc(size, mem::align_of::<T>())?;
        Some(Uninit::new(bytes.into_slice::<T>(len)))
    }

    /// Like [`Self::try_alloc_uninit_slice`] with a precomputed byte size.
    ///
    /// # Safety
    ///
    /// `size` must equal `size_of::<T>() * len` without overflow.
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    pub(crate) unsafe fn try_alloc_uninit_slice_with_size<T>(&self, len: usize, size: usize) -> Option<Uninit<'_, [T]>> {
        debug_assert_eq!(size, mem::size_of::<T>().wrapping_mul(len));
        let bytes = self.try_alloc(size, mem::align_of::<T>())?;
        Some(Uninit::new(bytes.into_slice::<T>(len)))
    }

    /// Like [`Self::try_alloc_uninit_slice`] but reserves
    /// `size_of::<usize>()` extra bytes immediately before the payload
    /// for a thin-pointer DST length prefix, and writes `len` into that
    /// prefix using [`ptr::write_unaligned`].
    ///
    /// Used by the smart-pointer slice paths
    /// ([`Arc<[T]>`](crate::Arc), [`Box<[T]>`](crate::Box)) so the
    /// resulting handles can be 8 bytes (thin) and recover `T::Metadata`
    /// (slice length) from the chunk prefix.
    ///
    /// The returned [`Uninit<[T]>`] ticket addresses the payload only;
    /// init helpers fill exactly `len * size_of::<T>()` bytes and have
    /// no awareness of the prefix.
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "prefix slot may be unaligned for T's whose align < align_of::<usize>(); paired with write_unaligned/read_unaligned"
    )]
    pub(crate) fn try_alloc_uninit_slice_prefixed<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let (payload, _) = self.try_alloc_prefixed_slice_payload::<T>(len)?;
        Some(Uninit::new(payload))
    }

    /// Like [`Self::try_alloc_uninit_slice_prefixed`] with a precomputed
    /// payload byte size.
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` without overflow.
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "prefix slot may be unaligned for T's whose align < align_of::<usize>(); paired with write_unaligned/read_unaligned"
    )]
    pub(crate) unsafe fn try_alloc_uninit_slice_prefixed_with_size<T>(&self, len: usize, payload_bytes: usize) -> Option<Uninit<'_, [T]>> {
        // SAFETY: forwarded to the caller.
        let (payload, _) = unsafe { self.try_alloc_prefixed_slice_payload_unchecked::<T>(len, payload_bytes) }?;
        Some(Uninit::new(payload))
    }

    /// Reserve storage for one `Arc<T>`-style value with a leading
    /// per-`Arc` strong reference count.
    ///
    /// Layout: `[strong][pad][metadata][payload]`. Initializes strong count
    /// to 1 and returns the payload pointer.
    ///
    /// `payload_bytes` is floored to 1 so the value pointer stays inside the
    /// chunk and preserves header recovery by mask.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "reservation is aligned to >= STRONG_ALIGN, so the leading strong slot is aligned for AtomicU32"
    )]
    fn try_alloc_arc_prefixed(&self, payload_bytes: usize, value_align: usize, meta_bytes: usize) -> Option<NonNull<u8>> {
        use super::thin_dst::{arc_block_align, strong_prefix_bytes_for};
        let prefix = strong_prefix_bytes_for(value_align, meta_bytes);
        let total = prefix.checked_add(payload_bytes.max(1))?;
        let base = self.try_alloc(total, arc_block_align(value_align))?;
        // SAFETY: `base` is aligned to `arc_block_align(value_align)` (>=
        // STRONG_ALIGN), so the leading `AtomicU32` write is aligned and
        // in chunk provenance; `base + prefix` is `value_align`-aligned
        // and stays within the reservation.
        unsafe {
            base.as_ptr()
                .cast::<core::sync::atomic::AtomicU32>()
                .write(core::sync::atomic::AtomicU32::new(1));
            Some(NonNull::new_unchecked(base.as_ptr().add(prefix)))
        }
    }

    /// [`Self::try_alloc_arc_prefixed`] plus the owning chunk pointer.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    pub(crate) fn try_alloc_arc_value<T>(&self) -> Option<(Uninit<'_, T>, NonNull<C>)> {
        let value_ptr = self.try_alloc_arc_prefixed(mem::size_of::<T>(), mem::align_of::<T>(), 0)?;
        // SAFETY: a successful reservation proves the mutator owns a chunk.
        Some((Uninit::new(InChunk::from_raw(value_ptr).cast::<T>()), unsafe {
            self.chunk_ptr_unchecked()
        }))
    }

    /// Slice form of [`Self::try_alloc_arc_value`], including the strong
    /// prefix and slice-length metadata word.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    #[allow(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(crate) fn try_alloc_arc_slice<T>(&self, len: usize) -> Option<(Uninit<'_, [T]>, NonNull<C>)> {
        let payload_bytes = mem::size_of::<T>().checked_mul(len)?;
        // SAFETY: `payload_bytes == size_of::<T>() * len` (just checked).
        unsafe { self.try_alloc_arc_slice_with_size::<T>(len, payload_bytes) }
    }

    /// Like [`Self::try_alloc_arc_slice`] with a precomputed payload byte size.
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` (without overflow).
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    #[allow(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "slice-length metadata is written/read unaligned immediately before the payload"
    )]
    pub(crate) unsafe fn try_alloc_arc_slice_with_size<T>(
        &self,
        len: usize,
        payload_bytes: usize,
    ) -> Option<(Uninit<'_, [T]>, NonNull<C>)> {
        debug_assert_eq!(payload_bytes, mem::size_of::<T>().wrapping_mul(len));
        let value_ptr = self.try_alloc_arc_prefixed(payload_bytes, mem::align_of::<T>(), mem::size_of::<usize>())?;
        // SAFETY: the reservation placed `size_of::<usize>()` metadata
        // bytes immediately before the payload; `write_unaligned`
        // tolerates any alignment.
        unsafe {
            ptr::write_unaligned(value_ptr.as_ptr().sub(mem::size_of::<usize>()).cast::<usize>(), len);
        }
        // SAFETY: a successful reservation proves the mutator owns a chunk.
        Some((Uninit::new(InChunk::from_raw(value_ptr).into_slice::<T>(len)), unsafe {
            self.chunk_ptr_unchecked()
        }))
    }

    /// DST form of [`Self::try_alloc_arc_value`]. The caller writes metadata
    /// before the returned value pointer and initializes the payload.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    #[cfg(feature = "dst")]
    pub(crate) fn try_alloc_arc_dst(
        &self,
        payload_bytes: usize,
        value_align: usize,
        meta_bytes: usize,
    ) -> Option<(NonNull<u8>, NonNull<C>)> {
        let value_ptr = self.try_alloc_arc_prefixed(payload_bytes, value_align, meta_bytes)?;
        // SAFETY: a successful reservation proves the mutator owns a chunk.
        Some((value_ptr, unsafe { self.chunk_ptr_unchecked() }))
    }

    /// Thin-DST slice reservation; returns the payload ticket and absolute
    /// payload address for drop-entry `value_offset` encoding.
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`
    fn try_alloc_prefixed_slice_payload<T>(&self, len: usize) -> Option<(InChunk<[T]>, usize)> {
        let payload_bytes = mem::size_of::<T>().checked_mul(len)?;
        // SAFETY: just verified by `checked_mul`.
        unsafe { self.try_alloc_prefixed_slice_payload_unchecked::<T>(len, payload_bytes) }
    }

    /// Inner helper for the prefixed slice path with a caller-provided
    /// `payload_bytes`.
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` (without
    /// overflow).
    #[inline]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`
    #[allow(
        clippy::cast_ptr_alignment,
        reason = "prefix slot may be unaligned for T's whose align < align_of::<usize>(); paired with write_unaligned/read_unaligned"
    )]
    unsafe fn try_alloc_prefixed_slice_payload_unchecked<T>(&self, len: usize, payload_bytes: usize) -> Option<(InChunk<[T]>, usize)> {
        debug_assert_eq!(payload_bytes, mem::size_of::<T>().wrapping_mul(len));
        let elem_align = mem::align_of::<T>();
        let prefix_size = mem::size_of::<usize>();
        // Payload starts at the lowest elem-align-aligned offset >=
        // `prefix_size`. Both values are powers of two so `max` gives
        // the right answer.
        let payload_offset = prefix_size.max(elem_align);
        // Empty slices/ZSTs still need an in-chunk payload address for
        // smart-pointer header recovery.
        let payload_bytes = payload_bytes.max(1);
        let total = payload_offset.checked_add(payload_bytes)?;
        let base_in_chunk = self.try_alloc(total, elem_align.max(1))?;
        // SAFETY: `base + payload_offset` is elem-align-aligned (both
        // factors are powers of two). The prefix word lives at
        // `payload - prefix_size` — for low-align T at offset 0, for
        // high-align T inside the padding [prefix_size, payload_offset).
        // `write_unaligned` tolerates any alignment.
        unsafe {
            let base_ptr = base_in_chunk.as_ptr();
            let payload_ptr = base_ptr.add(payload_offset);
            let prefix_ptr = payload_ptr.sub(prefix_size).cast::<usize>();
            ptr::write_unaligned(prefix_ptr, len);
            let payload_nn = NonNull::new_unchecked(payload_ptr);
            let payload_in_chunk = InChunk::from_raw(payload_nn).into_slice::<T>(len);
            Some((payload_in_chunk, payload_ptr as usize))
        }
    }

    /// Reserves storage for one `T` plus a drop entry slot.
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    pub(crate) fn try_alloc_uninit_with_drop<T>(&self) -> Option<UninitDrop<'_, T>> {
        let value_bytes = mem::size_of::<T>();
        let value_align = mem::align_of::<T>();
        // Pre-reserve a drop entry at the top before committing the value
        // allocation. This way we don't have to back out the value alloc if
        // the drop slot doesn't fit.
        let drop_slot = self.try_reserve_drop_entry()?;
        let Some(value_bytes_ptr) = self.try_alloc(value_bytes, value_align) else {
            // Roll back the drop slot we just reserved.
            self.unwind_drop_entry();
            return None;
        };
        let value = value_bytes_ptr.cast::<T>();
        // Initialize the drop slot as a placeholder (value_offset & len set,
        // drop_fn = None). When the ticket's init runs it commits drop_fn.
        let value_offset = self.offset_or_unwind(value_bytes_ptr.addr())?;
        // SAFETY: `drop_slot.as_ptr()` is a freshly reserved, aligned slot
        // inside payload; we own it exclusively.
        unsafe {
            ptr::write(drop_slot.as_ptr(), DropEntry::placeholder(value_offset, 1));
        }
        Some(UninitDrop::new(value, drop_slot))
    }

    /// Reserves storage for `len` consecutive `T`s plus a drop entry slot.
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`: body→None ⇒ refill spin
    pub(crate) fn try_alloc_uninit_slice_with_drop<T>(&self, len: usize) -> Option<UninitDrop<'_, [T]>> {
        let size = mem::size_of::<T>().checked_mul(len)?;
        // Drop entries store length as `u16`; reject larger slices before any
        // reservation is committed.
        let len_u16 = u16::try_from(len).ok()?;
        let drop_slot = self.try_reserve_drop_entry()?;
        let Some(value_bytes_ptr) = self.try_alloc(size, mem::align_of::<T>()) else {
            self.unwind_drop_entry();
            return None;
        };
        let value = value_bytes_ptr.into_slice::<T>(len);
        let value_offset = self.offset_or_unwind(value_bytes_ptr.addr())?;
        // SAFETY: see `try_alloc_uninit_with_drop`.
        unsafe {
            ptr::write(drop_slot.as_ptr(), DropEntry::placeholder(value_offset, len_u16));
        }
        Some(UninitDrop::new(value, drop_slot))
    }

    /// Attempts to reclaim the unused tail of the most recent bump
    /// allocation in O(1).
    ///
    /// Rewinds the bump cursor by `bytes` when `end_addr` is the current
    /// cursor. Returns `false` if the allocation is not at the tail.
    #[inline]
    pub(crate) fn try_reclaim_tail(&self, end_addr: usize, bytes: usize) -> bool {
        if self.chunk.is_none() {
            return false;
        }
        let cur = self.bump.get();
        if cur.as_ptr() as usize != end_addr {
            return false;
        }
        #[cfg(debug_assertions)]
        {
            // SAFETY: the `self.chunk.is_none()` early return above
            // guarantees a live chunk; `payload_ptr` only reads its header.
            let payload_start = unsafe { C::payload_ptr(self.chunk_ptr_unchecked()) }.as_ptr() as usize;
            debug_assert!(
                (cur.as_ptr() as usize) - payload_start >= bytes,
                "try_reclaim_tail: rewind underflows chunk payload",
            );
        }
        // SAFETY: caller guarantees `bytes` was previously consumed
        // forward from the bump cursor; rolling back by `bytes` stays
        // within the chunk payload.
        let new_bump = unsafe { cur.byte_sub(bytes) };
        self.bump.set(new_bump);
        true
    }

    /// Attempts to grow a prior allocation in place.
    pub(crate) fn try_grow_in_place(&self, prev_addr: usize, prev_len: usize, new_len: usize) -> bool {
        if self.chunk.is_none() {
            return false;
        }
        if new_len <= prev_len {
            return true;
        }
        let bump = self.bump.get();
        let bump_addr = bump.as_ptr() as usize;
        if prev_addr.checked_add(prev_len) != Some(bump_addr) {
            return false;
        }
        let Some(new_bump_addr) = prev_addr.checked_add(new_len) else {
            return false;
        };
        if new_bump_addr > self.drop_top.get().as_ptr() as usize {
            return false;
        }
        // SAFETY: `new_bump_addr - bump_addr = new_len - prev_len`,
        // within the chunk payload.
        let new_bump = unsafe { bump.byte_add(new_len - prev_len) };
        self.bump.set(new_bump);
        true
    }

    /// Reserves a [`DropEntry`]-sized slot at the top of the drop-entry
    /// region. Entries are packed end-to-end from the payload's high end
    /// downward, matching the layout walked by
    /// [`replay_drops`](super::drop_entry::replay_drops).
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`
    fn try_reserve_drop_entry(&self) -> Option<InChunk<DropEntry>> {
        let entry_size = mem::size_of::<DropEntry>();
        let top = self.drop_top.get();
        let top_addr = top.as_ptr() as usize;
        let new_top_addr = top_addr.checked_sub(entry_size)?;
        if new_top_addr < self.bump.get().as_ptr() as usize {
            return None;
        }
        // SAFETY: `new_top_addr >= bump.addr() >= chunk_start`.
        let new_top = unsafe { top.byte_sub(entry_size) };
        self.drop_top.set(new_top);
        Some(InChunk::from_raw(new_top).cast::<DropEntry>())
    }

    /// Reverses the most recent `try_reserve_drop_entry` after a compound
    /// reservation failure.
    #[cold]
    #[inline(never)]
    #[cfg_attr(test, mutants::skip)] // only observable via skip'd callers
    fn unwind_drop_entry(&self) {
        let entry_size = mem::size_of::<DropEntry>();
        let top = self.drop_top.get();
        let top_addr = top.as_ptr() as usize;
        let new_top_addr = top_addr + entry_size;
        let payload_end_addr = self.payload_range().1;
        let clamped_addr = new_top_addr.min(payload_end_addr);
        let advance = clamped_addr - top_addr;
        // SAFETY: `clamped_addr <= payload_end_addr`.
        let clamped = unsafe { top.byte_add(advance) };
        self.drop_top.set(clamped);
    }

    /// Rolls back the most recently reserved drop entry and returns `None`.
    #[cold]
    #[inline(never)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // unreachable in practice
    fn unwind_drop_entry_then_none<U>(&self) -> Option<U> {
        self.unwind_drop_entry();
        None
    }

    /// Cold helper: computes `value_offset` as a `u16`. Overflow is
    /// unreachable in practice — single values live below 64 KiB in a
    /// normal chunk, or at offset 0 in a one-shot oversized chunk.
    #[inline]
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)] // see `try_alloc`
    fn offset_or_unwind(&self, addr: usize) -> Option<u16> {
        let payload_start_addr = self.payload_range().0;
        let raw = addr - payload_start_addr;
        match u16::try_from(raw) {
            Ok(v) => Some(v),
            Err(_) => self.unwind_drop_entry_then_none(),
        }
    }

    /// Derives the number of drop entries currently reserved from the bump
    /// state (drop entries grow downward from `payload_end_addr`).
    #[inline]
    fn local_drop_entry_count(&self) -> usize {
        let payload_end_addr = self.payload_range().1;
        let consumed = payload_end_addr - self.drop_top.get().as_ptr() as usize;
        consumed / mem::size_of::<DropEntry>()
    }

    /// Publishes the locally-tracked drop-entry count to the chunk header
    /// eagerly, before the mutator is dropped. A no-op for chunk flavors that
    /// never register drop entries ([`ChunkOps::REGISTERS_DROPS`] is `false`).
    #[inline]
    pub(crate) fn publish_drop_count(&self) {
        if !C::REGISTERS_DROPS {
            return;
        }
        let Some(chunk) = self.chunk else { return };
        // SAFETY: the mutator owns a +1 on `chunk` for its whole lifetime,
        // so the header is live for this store.
        unsafe {
            C::publish_drop_entry_count(chunk, self.local_drop_entry_count());
        }
    }

    /// Consumes the mutator and returns the owned chunk ref without running
    /// this mutator's `Drop`.
    ///
    /// Under `stats`, also records wasted tail before transferring the chunk.
    ///
    /// Returns `None` for the empty (sentinel) mutator that has no
    /// chunk installed.
    #[inline]
    pub(crate) fn forget_into_chunk(self) -> Option<NonNull<C>> {
        self.publish_drop_count();
        let chunk = self.chunk;
        #[cfg(feature = "stats")]
        if let Some(chunk) = chunk {
            // SAFETY: chunk is live; the mutator still holds its +1
            // (ownership transfers to the caller via `mem::forget` below).
            unsafe { C::record_retire(chunk, self.wasted_tail_for_stats()) };
        }
        mem::forget(self);
        chunk
    }
}

impl<C: ?Sized + ChunkOps> Drop for ChunkMutator<C> {
    fn drop(&mut self) {
        let Some(chunk) = self.chunk else {
            return;
        };
        // SAFETY: chunk is live; we hold one refcount ticket. Publish the
        // drop-entry count before releasing it so teardown can replay drops.
        unsafe {
            #[cfg(feature = "stats")]
            {
                // Record wasted tail before `dec_ref`; release may happen
                // immediately and subtract the stashed value.
                let wasted = self.wasted_tail_for_stats();
                C::record_retire(chunk, wasted);
            }
            let chunk_ref = chunk.as_ref();
            // Publish the locally-tracked drop count; a no-op for flavors that
            // never register drop entries (see `ChunkOps::publish_drop_entry_count`).
            C::publish_drop_entry_count(chunk, self.local_drop_entry_count());
            if chunk_ref.dec_ref() {
                C::teardown_and_release(chunk);
            }
        }
    }
}

/// Hint for `try_alloc_bytes`: the bump cursor is non-zero and (on
/// 64-bit) fits in `isize` (both hold because it's a `NonNull<u8>`
/// sourced from a real chunk in the lower half of the address space).
#[expect(clippy::inline_always, reason = "pure codegen hint; must inline to take effect")]
#[inline(always)]
#[cfg_attr(test, mutants::skip)] // pure hint, no observable behavior
fn hint_chunk_cur_addr_nonnull(cur_addr: usize) {
    // SAFETY: see fn doc.
    unsafe {
        hint::assert_unchecked(cur_addr > 0);
        // Only asserted on 64-bit, where every valid address is below
        // `isize::MAX`. On narrower targets an address may exceed
        // `isize::MAX`, which would make this `assert_unchecked` false
        // (→ UB); the caller's `checked_add` stays correct without it.
        #[cfg(target_pointer_width = "64")]
        hint::assert_unchecked(isize::try_from(cur_addr).is_ok());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    let mask = align - 1;
    value.checked_add(mask).map(|v| v & !mask)
}

#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;
    use crate::internal::local_chunk::LocalChunk;

    fn empty_mutator() -> ChunkMutator<LocalChunk<Global>> {
        ChunkMutator::<LocalChunk<Global>>::empty()
    }

    // Covers try_reclaim_tail's chunk-is-None arm (line 314-316).
    #[test]
    fn try_reclaim_tail_on_empty_mutator_is_false() {
        let m = empty_mutator();
        assert!(!m.try_reclaim_tail(0, 0));
    }

    // Covers try_grow_in_place's chunk-is-None arm (line 332-334).
    #[test]
    fn try_grow_in_place_on_empty_mutator_is_false() {
        let m = empty_mutator();
        assert!(!m.try_grow_in_place(0, 0, 1));
    }

    // Covers try_grow_in_place's shrink/equal short-circuit (line 462-463)
    // and overflow checked_add arm (line 470-471): see the direct
    // `try_grow_in_place_non_growing_returns_true` /
    // `try_grow_in_place_new_len_overflow_returns_false` tests below,
    // which drive `ChunkMutator` directly (public Vec paths reject
    // these inputs before reaching `try_grow_in_place`).

    // --- Mutation-kill targets: dead-code-annotated helpers.
    //
    // `free_bytes` / `capacity` / `align_up` carry `#[allow(dead_code)]`
    // because no production call site is wired up yet, but they're
    // still mutated by cargo-mutants; exercising them here keeps
    // mutation testing honest.

    #[test]
    fn free_bytes_on_empty_mutator_is_zero() {
        let m = empty_mutator();
        assert_eq!(m.free_bytes(), 0);
    }

    #[test]
    fn capacity_and_free_bytes_match_chunk_layout() {
        // The mutator isn't reachable externally, so exercise its
        // free_bytes/capacity arithmetic end-to-end through a real arena
        // chunk (the empty-mutator path is unit-tested above).
        let arena = crate::Arena::builder().with_capacity_local(1024).build();
        let _ = arena.alloc(0_u32);
        let v: &u32 = arena.alloc(42_u32);
        assert_eq!(*v, 42);
    }

    #[test]
    fn align_up_round_trips_powers_of_two() {
        // None case: overflow path is unreachable for any align <= 64K
        // and value <= usize::MAX - align + 1, but we still pin the
        // arithmetic.
        assert_eq!(align_up(0, 8), Some(0));
        assert_eq!(align_up(1, 8), Some(8));
        assert_eq!(align_up(7, 8), Some(8));
        assert_eq!(align_up(8, 8), Some(8));
        assert_eq!(align_up(9, 8), Some(16));
        assert_eq!(align_up(16, 16), Some(16));
        assert_eq!(align_up(17, 16), Some(32));
        // Powers-of-two non-aligned values.
        assert_eq!(align_up(33, 32), Some(64));
        assert_eq!(align_up(100, 64), Some(128));
        // align == 1 → identity.
        assert_eq!(align_up(0, 1), Some(0));
        assert_eq!(align_up(7, 1), Some(7));
        // checked_add overflow path: value + mask must overflow.
        assert_eq!(align_up(usize::MAX, 8), None);
    }

    // Kills `try_alloc_bytes`'s `end_addr > drop_top_addr` boundary
    // mutation (`> → >=`): an allocation that exactly fills the
    // remaining payload must succeed. With `>`, `end_addr == drop_top_addr`
    // is allowed; with `>=`, the same case is rejected.
    #[test]
    fn try_alloc_bytes_at_exact_remaining_capacity_succeeds() {
        let arena = crate::Arena::new();
        // Force the first refill so `current_local` carries a live chunk.
        let _ = arena.alloc(0_u8);
        let m = arena.current_local();
        let free = m.free_bytes();
        assert!(free > 0, "post-refill chunk must have remaining capacity");
        let result = m.try_alloc_bytes(free);
        assert!(result.is_some(), "try_alloc_bytes(free_bytes) must succeed at the exact boundary");
        // After consuming everything, the next byte must fail.
        assert!(m.try_alloc_bytes(1).is_none());
    }

    // Kills `try_grow_in_place`'s `new_bump_addr > drop_top_addr`
    // boundary mutation (`> → >=`): growing a prior allocation so its
    // new end lands exactly on `drop_top_addr` must succeed.
    #[test]
    fn try_grow_in_place_at_exact_remaining_capacity_succeeds() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current_local();
        let free = m.free_bytes();
        assert!(free > 16, "need slack for an initial alloc plus grow");
        // Capture the bump cursor *before* the initial alloc so we know
        // `prev_addr` exactly.
        let prev_addr = m.bump.get().as_ptr() as usize;
        let initial = 16_usize;
        let _ = m.try_alloc_bytes(initial).expect("initial alloc");
        // Grow to exactly fill the remaining payload: new_len = free
        // ⇒ new_bump_addr = prev_addr + free = drop_top_addr.
        let new_len = free;
        assert!(
            m.try_grow_in_place(prev_addr, initial, new_len),
            "try_grow_in_place at exact remaining capacity must succeed",
        );
        // Bump should now sit at drop_top.
        assert_eq!(m.free_bytes(), 0);
    }

    // Directly exercises `try_grow_in_place`'s `new_len <= prev_len`
    // short-circuit (returns `true` without touching the bump cursor).
    // Public Vec paths can't reach this line because they reject
    // non-growing reservations before calling `try_grow_in_place`.
    #[test]
    fn try_grow_in_place_non_growing_returns_true() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current_local();
        let bump_before = m.bump.get().as_ptr() as usize;
        // `prev_addr` is irrelevant here: the `new_len <= prev_len`
        // guard short-circuits before it is inspected.
        assert!(m.try_grow_in_place(0, 8, 8), "equal lengths must succeed");
        assert!(m.try_grow_in_place(0, 8, 4), "shrink must succeed");
        assert_eq!(
            m.bump.get().as_ptr() as usize,
            bump_before,
            "non-growing grow must not move the bump cursor",
        );
    }

    // Directly exercises `try_grow_in_place`'s `checked_add` overflow
    // arm (`prev_addr + new_len` wraps `usize` ⇒ returns `false`).
    // Public Vec paths reject `usize::MAX` capacities before reaching
    // this line.
    #[test]
    fn try_grow_in_place_new_len_overflow_returns_false() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current_local();
        // Capture the cursor before the allocation so we know
        // `prev_addr` exactly (align-1 byte alloc lands here).
        let prev_addr = m.bump.get().as_ptr() as usize;
        let initial = 16_usize;
        let _ = m.try_alloc_bytes(initial).expect("initial alloc");
        // `new_len == usize::MAX` is > prev_len and `prev_addr + prev_len`
        // equals the current bump, so we reach the `checked_add(new_len)`
        // overflow guard, which must reject the grow.
        assert!(
            !m.try_grow_in_place(prev_addr, initial, usize::MAX),
            "overflowing new_len must fail",
        );
    }

    // Covers `try_alloc_uninit_with_drop`'s value-allocation rollback
    // (the `unwind_drop_entry` arm): the drop slot is reserved from the
    // top, but the value itself doesn't fit, so the reserved slot must be
    // unwound and the call must report failure. We size the remaining
    // free space into the window `[entry_size, entry_size + value_bytes)`
    // so the drop-slot reservation succeeds while the value alloc fails.
    #[test]
    fn try_alloc_uninit_with_drop_rolls_back_when_value_does_not_fit() {
        struct BigDrop([u8; 64]);
        impl Drop for BigDrop {
            fn drop(&mut self) {
                core::hint::black_box(&self.0);
            }
        }
        let arena = crate::Arena::new();
        // Force the first refill so `current_local` carries a live chunk.
        let _ = arena.alloc(0_u8);
        let m = arena.current_local();

        let entry_size = mem::size_of::<DropEntry>();
        // Leave exactly one byte of headroom past the drop slot so the
        // value (64 bytes, align 1) cannot fit after the slot is reserved.
        let target = entry_size + 1;
        let free = m.free_bytes();
        assert!(free >= target, "post-refill chunk must have room for the setup");
        let _ = m.try_alloc_bytes(free - target).expect("setup fill");
        assert_eq!(m.free_bytes(), target);

        // Drop slot fits (free >= entry_size) but the value does not
        // (free - entry_size == 1 < 64), driving the rollback path.
        assert!(
            m.try_alloc_uninit_with_drop::<BigDrop>().is_none(),
            "value that doesn't fit must report failure",
        );
        // The reserved drop slot must have been unwound, restoring free space.
        assert_eq!(m.free_bytes(), target, "unwind_drop_entry must restore the reserved drop slot");

        // Also exercise `BigDrop`'s destructor end-to-end: allocate one into
        // a fresh arena and let teardown replay the drop entry, so the drop
        // shim actually runs.
        let drop_arena = crate::Arena::new();
        let _ = drop_arena.alloc(BigDrop([0_u8; 64]));
        drop(drop_arena);
    }

    // Covers `publish_drop_count`'s early return for chunk flavors that never
    // register drop entries (`REGISTERS_DROPS == false`): publishing the count
    // on a shared mutator is a no-op that must leave the arena fully usable.
    #[test]
    fn publish_drop_count_is_noop_for_shared_mutator() {
        let arena = crate::Arena::new();
        // Force a live shared chunk into `current_shared`.
        let a = arena.alloc_arc(1_u32);
        assert_eq!(*a, 1);
        // Shared chunks register no drop entries, so this returns early.
        arena.current_shared().publish_drop_count();
        // The arena keeps working afterward.
        let b = arena.alloc_arc(2_u32);
        assert_eq!(*b, 2);
    }
}
