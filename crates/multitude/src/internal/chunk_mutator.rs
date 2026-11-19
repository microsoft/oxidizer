// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Bump allocator over a single chunk.
//!
//! [`ChunkMutator<A>`] owns one strong chunk reference and hands out
//! [`InChunk`] and [`Uninit`] tickets. Drop releases the refcount and may
//! trigger [`Chunk::teardown_and_release`].

use core::cell::Cell;
use core::ptr::{self, NonNull};
use core::{hint, mem};

use allocator_api2::alloc::Allocator;

use super::chunk::Chunk;
use super::in_chunk::InChunk;
use super::uninit::Uninit;

type ByteAllocation<'a, A> = (Uninit<'a, [u8]>, NonNull<Chunk<A>>);

/// This transition occurs at most once per local generation. Marking it cold
/// keeps the already-set branch as the steady-state fall-through.
#[cold]
fn set_reference_marker(marker: &Cell<bool>) {
    marker.set(true);
}

/// Records that the current chunk has served a local reference — an
/// arena-lifetime handout that carries no refcount of its own — so the arena
/// pins the chunk on `retired_local` when it rotates out instead of reclaiming
/// it early.
///
/// Callers must run this *before* the bump cursor is committed, so a chunk can
/// never hold committed local storage while still looking reclaimable. The load
/// guard keeps the steady state store-free; only the false-to-true transition
/// writes, and that transition is cold.
#[inline]
fn mark_local_reference(marker: &Cell<bool>) {
    // #[gamma::skip(cond.always_true, reason = "The marker helper only stores true in this same Cell; repeating that store after the first transition is idempotent")]
    if !marker.get() {
        // #[gamma::skip(call_result.default, reason = "The reference marker is still updated and its result is unit")]
        set_reference_marker(marker);
    }
}

/// Owns one strong reference to a chunk and tracks the bump cursor.
///
/// Hot-path layout stores only `chunk`, `bump`, and `end`; cold paths
/// re-derive payload bounds from `chunk`.
pub(crate) struct ChunkMutator<A: Allocator + Clone> {
    chunk: Option<NonNull<Chunk<A>>>,
    /// Bump cursor stored as a pointer to preserve chunk provenance under
    /// Stacked / Tree Borrows.
    bump: Cell<NonNull<u8>>,
    /// One-past-the-end of the usable payload. Constant for a given chunk;
    /// the bump cursor may advance up to (but not past) it. Same
    /// pointer-preserves-provenance rationale as `bump`.
    end: Cell<NonNull<u8>>,
}

// SAFETY: the mutator owns one strong chunk ref and moves that ownership
// across threads only when the shared allocator handle is `Send` (i.e.
// `A: Send + Sync`). The chunk's refcounts and links are atomic. The `Cell`
// fields intentionally make this `!Sync`.
unsafe impl<A: Allocator + Clone + Send + Sync> Send for ChunkMutator<A> {}

impl<A: Allocator + Clone> ChunkMutator<A> {
    /// Builds a mutator owning the +1 already on `chunk`.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live chunk whose refcount is already
    /// incremented in this mutator's name. The mutator becomes the unique
    /// owner of that +1 and will release it on drop.
    pub(crate) unsafe fn from_owned(chunk: NonNull<Chunk<A>>) -> Self {
        // SAFETY: caller asserts `chunk` is live; `payload_range_for` only
        // dereferences `chunk` to read `payload_ptr()` and `capacity()`.
        let (start_addr, end_addr) = unsafe { Self::payload_range_for(chunk) };
        // SAFETY: caller asserts `chunk` is live.
        let start = unsafe { Chunk::payload_ptr(chunk) };
        let end_offset = end_addr - start_addr;
        // SAFETY: `end_offset <= cap`; `start.byte_add` lands within (or at
        // one-past-end of) the chunk payload.
        let end = unsafe { start.byte_add(end_offset) };
        Self {
            chunk: Some(chunk),
            bump: Cell::new(start),
            end: Cell::new(end),
        }
    }

    /// Builds an empty mutator. Every `try_alloc*` returns `None`, deferring
    /// chunk allocation until the first user-visible allocation.
    pub(crate) const fn empty() -> Self {
        Self {
            chunk: None,
            // Sentinels: `bump > end`, so bound checks fail without an
            // explicit `self.chunk?`. These pointers are never dereferenced.
            bump: Cell::new(NonNull::<u16>::dangling().cast::<u8>()),
            end: Cell::new(NonNull::<u8>::dangling()),
        }
    }

    /// Free byte count between the bump cursor and the payload end.
    #[cfg(test)]
    #[inline]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn free_bytes(&self) -> usize {
        let top = self.end.get().as_ptr() as usize;
        let cur = self.bump.get().as_ptr() as usize;
        top.saturating_sub(cur)
    }

    /// Free byte count between the bump cursor and the payload end.
    /// Stats helper; empty-mutator sentinels saturate to 0. Reported as `u32`
    /// because chunk capacity is far below `u32::MAX`.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn wasted_tail_for_stats(&self) -> u32 {
        let top = self.end.get().as_ptr() as usize;
        let cur = self.bump.get().as_ptr() as usize;
        u32::try_from(top.saturating_sub(cur)).unwrap_or(u32::MAX)
    }

    /// Reads `chunk`'s payload start and end addresses.
    ///
    /// # Safety
    ///
    /// `chunk` must reference a live chunk.
    #[inline]
    unsafe fn payload_range_for(chunk: NonNull<Chunk<A>>) -> (usize, usize) {
        // SAFETY: caller asserts `chunk` is live.
        let (start, cap) = unsafe { (Chunk::<A>::payload_ptr(chunk), chunk.as_ref().capacity()) };
        let start_addr = start.as_ptr() as usize;
        let end_addr = start_addr + cap;
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
    pub(crate) unsafe fn chunk_ptr_unchecked(&self) -> NonNull<Chunk<A>> {
        // SAFETY: the caller guarantees (via this fn's `# Safety` contract) that
        // a `try_alloc*` / `try_reserve_*` on this mutator succeeded immediately
        // prior with no intervening chunk replacement, so `self.chunk` is `Some`.
        unsafe { self.chunk.unwrap_unchecked() }
    }

    /// Returns the chunk this mutator owns, or `None` for the empty
    /// (sentinel) mutator that has no chunk installed.
    #[inline]
    pub(crate) fn chunk_ptr(&self) -> Option<NonNull<Chunk<A>>> {
        self.chunk
    }

    /// Rewinds the bump cursor to the start of the owned chunk.
    ///
    /// Returns `false` for the empty sentinel mutator.
    #[inline]
    pub(crate) fn rewind(&self) -> bool {
        let Some(chunk) = self.chunk else {
            return false;
        };
        // SAFETY: this mutator owns a strong reference to `chunk`.
        self.bump.set(unsafe { Chunk::<A>::payload_ptr(chunk) });
        true
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
    // Always rejecting requests would make caller refill loops infinite.
    pub(crate) fn try_alloc(&self, size: usize, align: usize) -> Option<InChunk<u8>> {
        self.try_alloc_before_commit(size, align, || {})
    }

    // #[gamma::skip(fn_value.none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    /// [`Self::try_alloc`] that also records the reservation in `marker`.
    /// See [`mark_local_reference`] for what the marker guarantees.
    #[inline]
    fn try_alloc_local(&self, size: usize, align: usize, marker: &Cell<bool>) -> Option<InChunk<u8>> {
        // #[gamma::skip(call_result.default, reason = "The callback still marks the reference and its result is unit")]
        self.try_alloc_before_commit(size, align, || mark_local_reference(marker))
    }

    /// Shared allocation core with a success-only action immediately before the
    /// bump cursor is committed.
    #[inline]
    fn try_alloc_before_commit(&self, size: usize, align: usize, before_commit: impl FnOnce()) -> Option<InChunk<u8>> {
        debug_assert!(align.is_power_of_two(), "align must be a power of two");
        debug_assert!(size.checked_add(align).is_some(), "size + align overflows usize");
        let cur = self.bump.get();
        let cur_addr = cur.as_ptr() as usize;
        let limit_addr = self.end.get().as_ptr() as usize;
        // SAFETY: `cur_addr` is a live chunk bump-cursor address, hence non-zero;
        // on 64-bit targets, chunk addresses fit in `isize`.
        unsafe {
            // #[gamma::skip(stmt.delete_call, relational.gt_to_ge, literal.int_increment, reason = "A real chunk bump cursor is nonnull and beyond address one; these optimizer assumptions have no state or error result")]
            hint::assert_unchecked(cur_addr > 0);
            // #[gamma::skip(stmt.delete_call, expr.decrement, expr.increment, tag = "equivalent", reason = "The live lower-half chunk cursor is nonzero and far below isize::MAX; its adjacent addresses preserve the same optimizer assumption")]
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
        if probe_end > limit_addr {
            // #[gamma::skip(stmt.delete_call, reason = "cold_path is an optimizer hint with no state or result")]
            hint::cold_path();
            return None;
        }
        // SAFETY: `probe_end <= limit_addr`, so both `aligned_ptr`
        // and `new_bump` land within the chunk payload; `byte_add` on
        // the current bump cursor preserves chunk-wide provenance.
        let aligned_ptr = unsafe { cur.byte_add(aligned_addr - cur_addr) };
        // SAFETY: `probe_end = aligned_addr + size.max(1) <= limit_addr`, so
        // `aligned_ptr + size` also lands within the chunk payload; `byte_add`
        // on the bump cursor preserves chunk-wide provenance.
        let new_bump = unsafe { aligned_ptr.byte_add(size) };
        before_commit();
        self.bump.set(new_bump);
        Some(InChunk::from_raw(aligned_ptr))
    }

    /// Reserves storage for one `T` and returns an `Uninit<'_, T>` ticket.
    #[inline]
    pub(crate) fn try_alloc_uninit<T>(&self) -> Option<Uninit<'_, T>> {
        let bytes = self.try_alloc(mem::size_of::<T>(), mem::align_of::<T>())?;
        Some(Uninit::new(bytes.cast::<T>()))
    }

    // #[gamma::skip(fn_value.none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    /// [`Self::try_alloc_uninit`] that also records the reservation in
    /// `marker`. See [`mark_local_reference`].
    #[inline]
    pub(crate) fn try_alloc_uninit_local<T>(&self, marker: &Cell<bool>) -> Option<Uninit<'_, T>> {
        let bytes = self.try_alloc_local(mem::size_of::<T>(), mem::align_of::<T>(), marker)?;
        Some(Uninit::new(bytes.cast::<T>()))
    }

    /// [`Self::try_alloc`] paired with the owning chunk pointer. The
    /// success of `try_alloc` proves the mutator owns a chunk, so the
    /// caller doesn't need to use [`Self::chunk_ptr_unchecked`] (which
    /// would require an `unsafe` block) afterwards.
    #[inline]
    pub(crate) fn try_alloc_with_chunk(&self, size: usize, align: usize) -> Option<(InChunk<u8>, NonNull<Chunk<A>>)> {
        let in_chunk = self.try_alloc(size, align)?;
        // SAFETY: a successful `try_alloc` proves the mutator owns a
        // chunk.
        Some((in_chunk, unsafe { self.chunk_ptr_unchecked() }))
    }

    /// Byte-slice fast path: skips the alignment mask, `checked_mul`,
    /// and ZST branch. Only valid for `T = u8` (align 1, size 1).
    #[inline]
    pub(crate) fn try_alloc_bytes(&self, len: usize) -> Option<Uninit<'_, [u8]>> {
        self.try_alloc_bytes_before_commit(len, || {})
    }

    /// [`Self::try_alloc_bytes`] that also records the reservation in
    /// `marker`. See [`mark_local_reference`].
    #[inline]
    pub(crate) fn try_alloc_bytes_local(&self, len: usize, marker: &Cell<bool>) -> Option<Uninit<'_, [u8]>> {
        // #[gamma::skip(call_result.default, reason = "The callback still marks the reference and its result is unit")]
        self.try_alloc_bytes_before_commit(len, || mark_local_reference(marker))
    }

    #[inline]
    fn try_alloc_bytes_before_commit(&self, len: usize, before_commit: impl FnOnce()) -> Option<Uninit<'_, [u8]>> {
        let cur = self.bump.get();
        let cur_addr = cur.as_ptr() as usize;
        let limit_addr = self.end.get().as_ptr() as usize;
        // #[gamma::skip(call_result.default, reason = "The optimizer assumption still executes and its result is unit")]
        // #[gamma::skip(call.replace_with_default, stmt.delete_call, expr.decrement, expr.increment, reason = "The helper only supplies optimizer assumptions already guaranteed for this live lower-half chunk cursor and its adjacent bytes")]
        hint_chunk_cur_addr_nonnull(cur_addr);
        let end_addr = cur_addr.checked_add(len)?;
        if end_addr > limit_addr {
            // #[gamma::skip(stmt.delete_call, reason = "cold_path is an optimizer hint with no state or result")]
            hint::cold_path();
            return None;
        }
        // SAFETY: `end_addr <= limit_addr`.
        let new_bump = unsafe { cur.byte_add(len) };
        before_commit();
        self.bump.set(new_bump);
        Some(Uninit::new(InChunk::from_raw(cur).into_slice::<u8>(len)))
    }

    /// [`Self::try_alloc_bytes`] paired with the owning chunk pointer.
    #[inline]
    pub(crate) fn try_alloc_bytes_with_chunk(&self, len: usize) -> Option<ByteAllocation<'_, A>> {
        let uninit = self.try_alloc_bytes(len)?;
        // SAFETY: a successful byte reservation proves the mutator owns a
        // chunk.
        Some((uninit, unsafe { self.chunk_ptr_unchecked() }))
    }

    /// Reserves storage for `len` consecutive `T`s and returns an
    /// `Uninit<'_, [T]>` ticket.
    pub(crate) fn try_alloc_uninit_slice<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let size = mem::size_of::<T>().checked_mul(len)?;
        let bytes = self.try_alloc(size, mem::align_of::<T>())?;
        Some(Uninit::new(bytes.into_slice::<T>(len)))
    }

    // #[gamma::skip(fn_value.none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    /// [`Self::try_alloc_uninit_slice`] that also records the reservation in
    /// `marker`. See [`mark_local_reference`].
    pub(crate) fn try_alloc_uninit_slice_local<T>(&self, len: usize, marker: &Cell<bool>) -> Option<Uninit<'_, [T]>> {
        let size = mem::size_of::<T>().checked_mul(len)?;
        let bytes = self.try_alloc_local(size, mem::align_of::<T>(), marker)?;
        Some(Uninit::new(bytes.into_slice::<T>(len)))
    }

    // #[gamma::skip(fn_value.none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    /// Local-reference slice reservation with a precomputed byte size.
    ///
    /// # Safety
    ///
    /// `size` must equal `size_of::<T>() * len` without overflow.
    pub(crate) unsafe fn try_alloc_uninit_slice_with_size_local<T>(
        &self,
        len: usize,
        size: usize,
        marker: &Cell<bool>,
    ) -> Option<Uninit<'_, [T]>> {
        debug_assert_eq!(size, mem::size_of::<T>().wrapping_mul(len));
        let bytes = self.try_alloc_local(size, mem::align_of::<T>(), marker)?;
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
    pub(crate) fn try_alloc_uninit_slice_prefixed<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let (payload, _) = self.try_alloc_prefixed_slice_payload::<T>(len)?;
        Some(Uninit::new(payload))
    }

    // #[gamma::skip(fn_value.none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    /// Like [`Self::try_alloc_uninit_slice_prefixed`] with a precomputed
    /// payload byte size.
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` without overflow.
    pub(crate) unsafe fn try_alloc_uninit_slice_prefixed_with_size<T>(&self, len: usize, payload_bytes: usize) -> Option<Uninit<'_, [T]>> {
        // SAFETY: forwarded to the caller.
        let (payload, _) = unsafe { self.try_alloc_prefixed_slice_payload_unchecked::<T>(len, payload_bytes) }?;
        // #[gamma::skip(option.some_to_none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
        Some(Uninit::new(payload))
    }

    /// Reserve storage for one shared-owner value with a leading strong
    /// reference count shared by its `Arc` or `Rc` family.
    ///
    /// Layout: `[strong][pad][metadata][payload]`. Initializes strong count
    /// to 1 and returns the payload pointer.
    ///
    /// `payload_bytes` is floored to 1 so the value pointer stays inside the
    /// chunk and preserves header recovery by mask.
    #[inline]
    fn try_alloc_smart_prefixed<S: super::thin_dst::Strong>(
        &self,
        payload_bytes: usize,
        value_align: usize,
        meta_bytes: usize,
    ) -> Option<NonNull<u8>> {
        self.try_alloc_smart_prefixed_before_commit::<S>(payload_bytes, value_align, meta_bytes, || {})
    }

    // #[gamma::skip(fn_value.none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    #[inline]
    fn try_alloc_smart_prefixed_local<S: super::thin_dst::Strong>(
        &self,
        payload_bytes: usize,
        value_align: usize,
        meta_bytes: usize,
        marker: &Cell<bool>,
    ) -> Option<NonNull<u8>> {
        // #[gamma::skip(call_result.default, reason = "The callback still marks the reference and its result is unit")]
        self.try_alloc_smart_prefixed_before_commit::<S>(payload_bytes, value_align, meta_bytes, || mark_local_reference(marker))
    }

    #[inline]
    fn try_alloc_smart_prefixed_before_commit<S: super::thin_dst::Strong>(
        &self,
        payload_bytes: usize,
        value_align: usize,
        meta_bytes: usize,
        before_commit: impl FnOnce(),
    ) -> Option<NonNull<u8>> {
        use super::thin_dst::strong_prefix_bytes_for;
        let prefix = strong_prefix_bytes_for(value_align, meta_bytes);
        let total = prefix.checked_add(payload_bytes.max(1))?;
        let base = self.try_alloc_before_commit(total, S::block_align(value_align), before_commit)?;
        // SAFETY: `base` is aligned to `S::block_align(value_align)`, so the
        // leading strong-count write is valid for the policy; `base + prefix`
        // is `value_align`-aligned and stays within the reservation.
        unsafe {
            S::write_one(base.as_ptr());
            Some(NonNull::new_unchecked(base.as_ptr().add(prefix)))
        }
    }

    /// [`Self::try_alloc_smart_prefixed`] plus the owning chunk pointer.
    #[inline]
    pub(crate) fn try_alloc_arc_value<S: super::thin_dst::Strong, T>(&self) -> Option<(Uninit<'_, T>, NonNull<Chunk<A>>)> {
        let value_ptr = self.try_alloc_smart_prefixed::<S>(mem::size_of::<T>(), mem::align_of::<T>(), 0)?;
        // SAFETY: a successful reservation proves the mutator owns a chunk.
        Some((Uninit::new(InChunk::from_raw(value_ptr).cast::<T>()), unsafe {
            self.chunk_ptr_unchecked()
        }))
    }

    /// Slice form of [`Self::try_alloc_arc_value`], including the strong
    /// prefix and slice-length metadata word.
    #[inline]
    #[expect(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(crate) fn try_alloc_arc_slice<S: super::thin_dst::Strong, T>(&self, len: usize) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        let payload_bytes = mem::size_of::<T>().checked_mul(len)?;
        // SAFETY: `payload_bytes == size_of::<T>() * len` (just checked).
        unsafe { self.try_alloc_arc_slice_with_size::<S, T>(len, payload_bytes) }
    }

    /// Like [`Self::try_alloc_arc_slice`] with a precomputed payload byte size.
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` (without overflow).
    #[inline]
    #[expect(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(crate) unsafe fn try_alloc_arc_slice_with_size<S: super::thin_dst::Strong, T>(
        &self,
        len: usize,
        payload_bytes: usize,
    ) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        debug_assert_eq!(payload_bytes, mem::size_of::<T>().wrapping_mul(len));
        let value_ptr = self.try_alloc_smart_prefixed::<S>(payload_bytes, mem::align_of::<T>(), mem::size_of::<usize>())?;
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

    /// Reserves a growable-buffer slot carrying the full freeze prefix
    /// (`[strong][pad][len][payload]`, the `Arc<[T]>` layout) so the buffer
    /// can later be frozen into a `Box<[T]>`, `Rc<[T]>`, or `Arc<[T]>` in
    /// place — no copy.
    ///
    /// Writes `strong = 1` and reserves the length slot (left uninitialized;
    /// the final length is written at freeze time). Takes **no** chunk
    /// refcount — the caller pins the chunk through the reference machinery
    /// and acquires the refcount only at freeze. Returns the payload ticket.
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` without overflow.
    #[inline]
    unsafe fn try_alloc_freezable_slice_with_size<T>(&self, len: usize, payload_bytes: usize) -> Option<Uninit<'_, [T]>> {
        debug_assert_eq!(payload_bytes, mem::size_of::<T>().wrapping_mul(len));
        let value_ptr =
            self.try_alloc_smart_prefixed::<super::thin_dst::AtomicStrong>(payload_bytes, mem::align_of::<T>(), mem::size_of::<usize>())?;
        // The returned pointer addresses `len` `T` slots after the freeze prefix.
        Some(Uninit::new(InChunk::from_raw(value_ptr).into_slice::<T>(len)))
    }

    /// Checked form of [`Self::try_alloc_freezable_slice_with_size`].
    #[inline]
    pub(crate) fn try_alloc_freezable_slice<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let payload_bytes = mem::size_of::<T>().checked_mul(len)?;
        // SAFETY: `payload_bytes == size_of::<T>() * len` (just checked).
        unsafe { self.try_alloc_freezable_slice_with_size::<T>(len, payload_bytes) }
    }

    // #[gamma::skip(fn_value.none, tag = "timeout", reason = "November 4 campaign: this mutation exceeded the test timeout")]
    /// [`Self::try_alloc_freezable_slice`] that also records the reservation
    /// in `marker`. See [`mark_local_reference`].
    #[inline]
    pub(crate) fn try_alloc_freezable_slice_local<T>(&self, len: usize, marker: &Cell<bool>) -> Option<Uninit<'_, [T]>> {
        let payload_bytes = mem::size_of::<T>().checked_mul(len)?;
        debug_assert_eq!(payload_bytes, mem::size_of::<T>().wrapping_mul(len));
        let value_ptr = self.try_alloc_smart_prefixed_local::<super::thin_dst::AtomicStrong>(
            payload_bytes,
            mem::align_of::<T>(),
            mem::size_of::<usize>(),
            marker,
        )?;
        Some(Uninit::new(InChunk::from_raw(value_ptr).into_slice::<T>(len)))
    }

    /// DST form of [`Self::try_alloc_arc_value`]. The caller writes metadata
    /// before the returned value pointer and initializes the payload.
    #[inline]
    #[cfg(feature = "dst")]
    pub(crate) fn try_alloc_arc_dst<S: super::thin_dst::Strong>(
        &self,
        payload_bytes: usize,
        value_align: usize,
        meta_bytes: usize,
    ) -> Option<(NonNull<u8>, NonNull<Chunk<A>>)> {
        let value_ptr = self.try_alloc_smart_prefixed::<S>(payload_bytes, value_align, meta_bytes)?;
        // SAFETY: a successful reservation proves the mutator owns a chunk.
        Some((value_ptr, unsafe { self.chunk_ptr_unchecked() }))
    }

    /// Thin-DST slice reservation; returns the payload ticket and absolute
    /// payload address.
    #[inline]
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
    #[expect(
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
        let base_in_chunk = self.try_alloc(
            total,
            elem_align.max(
                // #[gamma::skip(literal.int_decrement, reason = "Element alignment comes from align_of and is nonzero, so max with zero and max with one are identical")]
                1,
            ),
        )?;
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
            let payload_start = unsafe { Chunk::<A>::payload_ptr(self.chunk_ptr_unchecked()) }.as_ptr() as usize;
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
        if new_bump_addr > self.end.get().as_ptr() as usize {
            return false;
        }
        // SAFETY: `new_bump_addr - bump_addr = new_len - prev_len`,
        // within the chunk payload.
        let new_bump = unsafe { bump.byte_add(new_len - prev_len) };
        self.bump.set(new_bump);
        true
    }

    /// Consumes the mutator and returns the owned chunk ref without running
    /// this mutator's `Drop`.
    ///
    /// Under `stats`, also records wasted tail before transferring the chunk.
    ///
    /// Returns `None` for the empty (sentinel) mutator that has no
    /// chunk installed.
    #[inline]
    pub(crate) fn forget_into_chunk(self) -> Option<NonNull<Chunk<A>>> {
        let chunk = self.chunk;
        #[cfg(feature = "stats")]
        if let Some(chunk) = chunk {
            // SAFETY: chunk is live; the mutator still holds its +1
            // (ownership transfers to the caller via `mem::forget` below).
            unsafe { Chunk::<A>::record_retire(chunk, self.wasted_tail_for_stats()) };
        }
        mem::forget(self);
        chunk
    }

    /// Releases this mutator's strong reference together with unused
    /// pre-credited references in one atomic operation.
    ///
    /// # Safety
    ///
    /// For a non-empty mutator, the caller must transfer ownership of exactly
    /// `refund` unused pre-credited references in addition to the mutator's
    /// owned `+1`. An empty mutator is a no-op.
    #[inline]
    pub(crate) unsafe fn release_with_refund(self, refund: usize) {
        let Some(chunk) = self.chunk else {
            return;
        };
        #[cfg(feature = "stats")]
        {
            // SAFETY: the mutator still owns its +1 and the chunk is live.
            unsafe { Chunk::<A>::record_retire(chunk, self.wasted_tail_for_stats()) };
        }
        mem::forget(self);
        // SAFETY: guaranteed by the caller; this mutator owns the additional
        // +1 released by the same atomic operation.
        unsafe {
            if Chunk::<A>::refund_refs_and_release_one(chunk, refund) {
                Chunk::<A>::teardown_and_release(chunk);
            }
        }
    }
}

impl<A: Allocator + Clone> Drop for ChunkMutator<A> {
    fn drop(&mut self) {
        let Some(chunk) = self.chunk else {
            return;
        };
        // SAFETY: chunk is live; we hold one refcount ticket.
        unsafe {
            #[cfg(feature = "stats")]
            {
                // Record wasted tail before `dec_ref`; release may happen
                // immediately and subtract the stashed value.
                let wasted = self.wasted_tail_for_stats();
                Chunk::<A>::record_retire(chunk, wasted);
            }
            if Chunk::<A>::dec_ref(chunk) {
                Chunk::<A>::teardown_and_release(chunk);
            }
        }
    }
}

/// Hint for `try_alloc_bytes`: the bump cursor is non-zero and (on
/// 64-bit) fits in `isize` (both hold because it's a `NonNull<u8>`
/// sourced from a real chunk in the lower half of the address space).
#[expect(clippy::inline_always, reason = "pure codegen hint; must inline to take effect")]
#[inline(always)]
// #[gamma::skip(fn_value.unit, reason = "The entire helper only supplies optimizer assumptions already guaranteed by its live chunk cursor precondition")]
fn hint_chunk_cur_addr_nonnull(cur_addr: usize) {
    // SAFETY: `cur_addr` is the address of a `NonNull<u8>` bump cursor sourced
    // from a real chunk, so it is non-zero; on 64-bit targets that chunk lives
    // in the lower half of the address space, so the address fits in `isize`.
    // These hints are only asserted under those guarantees.
    unsafe {
        // #[gamma::skip(stmt.delete_call, relational.gt_to_ge, literal.int_increment, reason = "A real chunk bump cursor is nonnull and beyond address one; these optimizer assumptions have no state or error result")]
        hint::assert_unchecked(cur_addr > 0);
        // Only asserted on 64-bit, where every valid address is below
        // `isize::MAX`. On narrower targets an address may exceed
        // `isize::MAX`, which would make this `assert_unchecked` false
        // (→ UB); the caller's `checked_add` stays correct without it.
        // #[gamma::skip(stmt.delete_call, expr.decrement, expr.increment, tag = "equivalent", reason = "The live lower-half chunk cursor is nonzero and far below isize::MAX; its adjacent addresses preserve the same optimizer assumption")]
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
    #[test]
    fn gamma_round2_prefixed_byte_slice_preserves_an_odd_cursor_without_padding() {
        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u8);
        let mutator = arena.current();
        assert!(mutator.rewind());
        assert!(mutator.try_alloc(1, 1).is_some());
        let before = mutator.bump.get().as_ptr().addr();
        assert_eq!(before % 2, 1);
        let (payload, address) = mutator.try_alloc_prefixed_slice_payload::<u8>(3).unwrap();
        assert_eq!(address, before + mem::size_of::<usize>());
        assert_eq!(payload.as_ptr().len(), 3);
        assert_eq!(mutator.bump.get().as_ptr().addr(), address + 3);
    }

    #[test]
    fn gamma_round2_nonlocal_freezable_tickets_reserve_exact_payload_and_prefix_bytes() {
        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u8);
        let mutator = arena.current();
        for len in [0_usize, 1, 7] {
            assert!(mutator.rewind());
            let before = mutator.bump.get().as_ptr().addr();
            let ticket = mutator.try_alloc_freezable_slice::<u8>(len).unwrap();
            let values = ticket.init_copy_from_slice(&[0x71; 7][..len]);
            let payload = before.div_ceil(4) * 4 + 4 + mem::size_of::<usize>();
            assert_eq!(values.as_ptr().addr(), payload);
            assert_eq!(values, &[0x71; 7][..len]);
            assert_eq!(mutator.bump.get().as_ptr().addr(), payload + len.max(1));
        }
        let empty = empty_mutator();
        assert!(empty.try_alloc_freezable_slice::<u8>(1).is_none());
        assert!(empty.try_alloc_prefixed_slice_payload::<u8>(usize::MAX).is_none());
    }

    #[cfg(feature = "dst")]
    #[test]
    fn gamma_round2_dst_reservations_and_growth_rejection_preserve_the_exact_cursor() {
        use crate::internal::thin_dst::AtomicStrong;

        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u8);
        let mutator = arena.current();
        for payload in [0_usize, 1, 7] {
            assert!(mutator.rewind());
            let before = mutator.bump.get().as_ptr().addr();
            let (pointer, _) = mutator.try_alloc_arc_dst::<AtomicStrong>(payload, 1, 8).unwrap();
            assert_eq!(pointer.as_ptr().addr(), before.div_ceil(4) * 4 + 12);
            assert_eq!(mutator.bump.get().as_ptr().addr(), pointer.as_ptr().addr() + payload.max(1));
        }
        assert!(mutator.rewind());
        let before = mutator.bump.get().as_ptr().addr();
        assert!(mutator.try_alloc(8, 1).is_some());
        let cursor = mutator.bump.get();
        assert!(!mutator.try_grow_in_place(before, 8, 8 + mutator.free_bytes() + 1));
        assert_eq!(mutator.bump.get(), cursor);
    }

    use allocator_api2::alloc::Global;

    use super::*;

    #[test]
    fn gamma_survivor_mutator_exact_cursor_probe_and_reclaim() {
        let empty = empty_mutator();
        assert_eq!(empty.bump.get().as_ptr() as usize, 2);
        assert_eq!(empty.end.get().as_ptr() as usize, 1);
        assert!(empty.try_alloc_bytes(0).is_none());
        assert!(empty.try_alloc(1, 1).is_none());
        assert!(!empty.try_reclaim_tail(2, 0));
        assert!(!empty.try_grow_in_place(2, 0, 0));
        assert!(empty.forget_into_chunk().is_none());

        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u8);
        let mutator = arena.current();
        assert!(mutator.rewind());
        let start = mutator.bump.get().as_ptr() as usize;
        let capacity = mutator.free_bytes();
        let first = mutator.try_alloc_bytes(3).unwrap();
        assert_eq!(first.init_copy_from_slice(&[1, 2, 3]).as_ptr() as usize, start);
        assert_eq!(mutator.free_bytes(), capacity - 3);
        let cursor = mutator.bump.get();
        assert!(mutator.try_alloc(usize::MAX - 1, 1).is_none());
        assert!(mutator.try_alloc_bytes(usize::MAX).is_none());
        assert_eq!(mutator.bump.get(), cursor);
        assert!(!mutator.try_grow_in_place(start + 1, 3, 4));
        assert!(!mutator.try_grow_in_place(usize::MAX, 3, 4));
        assert!(!mutator.try_grow_in_place(start, 3, usize::MAX));
        assert_eq!(mutator.bump.get(), cursor);
        assert!(mutator.try_grow_in_place(start, 3, 3));
        assert_eq!(mutator.bump.get(), cursor);
        assert!(mutator.try_grow_in_place(start, 3, 7));
        assert_eq!(mutator.bump.get().as_ptr() as usize, start + 7);
        assert!(!mutator.try_reclaim_tail(start + 6, 3));
        assert_eq!(mutator.bump.get().as_ptr() as usize, start + 7);
        assert!(mutator.try_reclaim_tail(start + 7, 3));
        assert_eq!(mutator.bump.get().as_ptr() as usize, start + 4);
        let remaining = mutator.free_bytes();
        assert!(mutator.try_alloc_bytes(remaining).is_some());
        assert_eq!(mutator.free_bytes(), 0);
        let cursor = mutator.bump.get();
        assert!(mutator.try_alloc(0, 1).is_none());
        assert!(mutator.try_alloc_bytes(1).is_none());
        assert_eq!(mutator.bump.get(), cursor);
        assert!(mutator.rewind());
        assert_eq!(mutator.free_bytes(), capacity);
        assert!(mutator.try_alloc_bytes(capacity - 1).is_some());
        let cursor = mutator.bump.get();
        assert!(mutator.try_alloc(0, 1).is_some());
        assert_eq!(mutator.bump.get(), cursor);
        assert!(mutator.try_alloc(1, 1).is_some());
        assert_eq!(mutator.free_bytes(), 0);
        assert!(mutator.try_alloc(0, 1).is_none());
    }

    #[test]
    fn gamma_survivor_mutator_alignment_independent_integer_oracle() {
        for align in [1_usize, 2, 4, 8, 16, 256, 4096] {
            for value in [0_usize, 1, align - 1, align, align + 1, usize::MAX - align, usize::MAX] {
                let rounded = (value as u128).div_ceil(align as u128) * (align as u128);
                assert_eq!(align_up(value, align), usize::try_from(rounded).ok());
            }
        }
    }

    fn empty_mutator() -> ChunkMutator<Global> {
        ChunkMutator::<Global>::empty()
    }

    #[test]
    fn gamma_survivor_mutator_prefixed_slice_tickets_and_local_markers() {
        use crate::internal::thin_dst::AtomicStrong;

        fn check_prefix<T, A: Allocator + Clone>(mutator: &ChunkMutator<A>) {
            for len in [0_usize, 1, 2, 7] {
                assert!(mutator.rewind());
                let before = mutator.bump.get().as_ptr().addr();
                let align = mem::align_of::<T>();
                let offset = mem::size_of::<usize>().max(align);
                let base = before.div_ceil(align) * align;
                let (payload, address) = mutator.try_alloc_prefixed_slice_payload::<T>(len).unwrap();
                assert_eq!(address, base + offset);
                assert_eq!(payload.as_ptr().len(), len);
                assert_eq!(mutator.bump.get().as_ptr().addr(), address + (mem::size_of::<T>() * len).max(1));
                // SAFETY: the ticket initialized the word immediately before its payload.
                let stored_length = unsafe { ptr::read_unaligned((address - mem::size_of::<usize>()) as *const usize) };
                assert_eq!(stored_length, len);
            }
        }
        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u8);
        let mutator = arena.current();
        check_prefix::<u8, _>(mutator);
        check_prefix::<u16, _>(mutator);
        check_prefix::<u64, _>(mutator);
        check_prefix::<u128, _>(mutator);
        for len in [0_usize, 1, 2, 7] {
            assert!(mutator.rewind());
            let before = mutator.bump.get().as_ptr().addr();
            let ticket = mutator.try_alloc_uninit_slice::<u16>(len).unwrap();
            assert_eq!(mutator.bump.get().as_ptr().addr(), before.div_ceil(2) * 2 + 2 * len);
            assert_eq!(ticket.init_copy_from_slice(&[17_u16; 7][..len]), &[17_u16; 7][..len]);
        }
        for len in [0_usize, 1, 2, 7] {
            assert!(mutator.rewind());
            let before = mutator.bump.get().as_ptr().addr();
            let (ticket, _) = mutator.try_alloc_arc_slice::<AtomicStrong, u16>(len).unwrap();
            let payload = before.div_ceil(4) * 4 + (4 + mem::size_of::<usize>()).div_ceil(2) * 2;
            assert_eq!(mutator.bump.get().as_ptr().addr(), payload + (2 * len).max(1));
            // SAFETY: the Arc slice ticket wrote its length word before the payload.
            let stored_length = unsafe { ptr::read_unaligned((payload - mem::size_of::<usize>()) as *const usize) };
            assert_eq!(stored_length, len);
            assert_eq!(ticket.init_copy_from_slice(&[17_u16; 7][..len]), &[17_u16; 7][..len]);
        }
        let marker = Cell::new(false);
        assert!(mutator.rewind());
        let before = mutator.bump.get().as_ptr().addr();
        let ticket = mutator.try_alloc_freezable_slice_local::<u8>(3, &marker).unwrap();
        assert_eq!(
            mutator.bump.get().as_ptr().addr(),
            before.div_ceil(4) * 4 + 4 + mem::size_of::<usize>() + 3
        );
        assert_eq!(ticket.init_copy_from_slice(&[17_u8; 3]), &[17_u8; 3]);
        assert!(marker.get());
        for family in 0..5 {
            for len in [0_usize, 1, 3] {
                assert!(mutator.rewind());
                marker.set(false);
                let before = mutator.bump.get().as_ptr().addr();
                let success = match family {
                    0 => mutator.try_alloc_local(len, 1, &marker).is_some(),
                    1 => mutator.try_alloc_bytes_local(len, &marker).is_some(),
                    2 => mutator.try_alloc_uninit_slice_local::<u16>(len, &marker).is_some(),
                    3 => {
                        // SAFETY: the supplied byte size equals len * size_of::<u16>().
                        unsafe {
                            mutator
                                .try_alloc_uninit_slice_with_size_local::<u16>(len, 2 * len, &marker)
                                .is_some()
                        }
                    }
                    _ => mutator.try_alloc_freezable_slice_local::<u16>(len, &marker).is_some(),
                };
                assert!(success);
                assert!(marker.get(), "family {family}, len {len}");
                let expected = match family {
                    0 | 1 => before + len,
                    2 | 3 => before.div_ceil(2) * 2 + 2 * len,
                    _ => before.div_ceil(4) * 4 + (4 + mem::size_of::<usize>()).div_ceil(2) * 2 + (2 * len).max(1),
                };
                assert_eq!(mutator.bump.get().as_ptr().addr(), expected);
            }
        }
        marker.set(false);
        let before = mutator.bump.get();
        assert!(mutator.try_alloc_freezable_slice::<u16>(usize::MAX).is_none());
        assert!(mutator.try_alloc_freezable_slice_local::<u16>(usize::MAX, &marker).is_none());
        assert!(mutator.try_alloc_uninit_slice_local::<u16>(usize::MAX, &marker).is_none());
        assert!(!marker.get());
        assert_eq!(mutator.bump.get(), before);
        assert!(mutator.try_alloc_bytes(mutator.free_bytes()).is_some());
        let before = mutator.bump.get();
        assert!(mutator.try_alloc_arc_slice::<AtomicStrong, u16>(1).is_none());
        assert!(mutator.try_alloc_uninit_slice::<u16>(1).is_none());
        assert!(mutator.try_alloc_uninit_slice_prefixed::<u16>(1).is_none());
        assert!(!marker.get());
        assert_eq!(mutator.bump.get(), before);
    }

    #[test]
    fn gamma_survivor_mutator_exact_strong_prefix_and_overflow() {
        use crate::internal::thin_dst::{AtomicStrong, LocalStrong};

        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u8);
        let mutator = arena.current();
        for atomic in [false, true] {
            for align in [1_usize, 2, 4, 8, 16] {
                for metadata in [0_usize, mem::size_of::<usize>()] {
                    for payload in [0_usize, 1, 2, 7] {
                        assert!(mutator.rewind());
                        let before = mutator.bump.get().as_ptr() as usize;
                        let block_align = if atomic { align.max(4) } else { align };
                        let base = before.div_ceil(block_align) * block_align;
                        let prefix = (4 + metadata).div_ceil(align) * align;
                        let calls = Cell::new(0);
                        let callback = || {
                            assert_eq!(mutator.bump.get().as_ptr() as usize, before);
                            calls.set(calls.get() + 1);
                        };
                        let pointer = if atomic {
                            mutator.try_alloc_smart_prefixed_before_commit::<AtomicStrong>(payload, align, metadata, callback)
                        } else {
                            mutator.try_alloc_smart_prefixed_before_commit::<LocalStrong>(payload, align, metadata, callback)
                        }
                        .unwrap();
                        assert_eq!(calls.get(), 1);
                        assert_eq!(pointer.as_ptr() as usize, base + prefix);
                        assert_eq!(mutator.bump.get().as_ptr() as usize, base + prefix + payload.max(1));
                        // SAFETY: the reservation initialized the four-byte
                        // count at its base, and no handle concurrently accesses it.
                        let count = unsafe { ptr::read_unaligned((pointer.as_ptr().wrapping_sub(prefix)).cast::<u32>()) };
                        assert_eq!(count, 1);
                    }
                }
            }
        }
        let before = mutator.bump.get();
        assert!(mutator.try_alloc_smart_prefixed::<AtomicStrong>(usize::MAX, 1, 0).is_none());
        assert!(mutator.try_alloc_arc_slice::<AtomicStrong, u128>(usize::MAX).is_none());
        assert!(mutator.try_alloc_uninit_slice::<u16>(usize::MAX).is_none());
        assert!(mutator.try_alloc_prefixed_slice_payload::<u16>(usize::MAX).is_none());
        assert_eq!(mutator.bump.get(), before);
    }

    #[cfg(feature = "stats")]
    #[test]
    fn gamma_survivor_mutator_wasted_tail_saturates_without_dereferencing_sentinels() {
        let empty = empty_mutator();
        assert_eq!(empty.wasted_tail_for_stats(), 0);
        if let Ok(end) = usize::try_from(u64::from(u32::MAX) + 9) {
            empty.end.set(NonNull::new(ptr::without_provenance_mut(end)).unwrap());
            assert_eq!(empty.wasted_tail_for_stats(), u32::MAX);
        }
        empty.end.set(NonNull::new(ptr::without_provenance_mut(19)).unwrap());
        assert_eq!(empty.wasted_tail_for_stats(), 17);
        empty.end.set(NonNull::new(ptr::without_provenance_mut(1)).unwrap());
        assert_eq!(empty.wasted_tail_for_stats(), 0);
    }

    #[test]
    fn try_reclaim_tail_on_empty_mutator_is_false() {
        let m = empty_mutator();
        assert!(!m.try_reclaim_tail(0, 0));
    }

    #[test]
    fn try_grow_in_place_on_empty_mutator_is_false() {
        let m = empty_mutator();
        assert!(!m.try_grow_in_place(0, 0, 1));
    }

    #[test]
    fn free_bytes_on_empty_mutator_is_zero() {
        let m = empty_mutator();
        assert_eq!(m.free_bytes(), 0);
    }

    #[test]
    fn rewind_on_empty_mutator_is_false() {
        let m = empty_mutator();
        assert!(!m.rewind());
    }

    #[test]
    fn release_with_refund_on_empty_mutator_is_noop() {
        let m = empty_mutator();
        // SAFETY: an empty mutator has no references to release and is
        // documented as a no-op.
        unsafe { m.release_with_refund(0) };
    }

    #[test]
    fn rewind_restores_full_chunk_capacity() {
        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u8);
        let m = arena.current();
        let after_prime = m.free_bytes();
        let _ = m.try_alloc_bytes(16).unwrap();
        assert_eq!(m.free_bytes(), after_prime - 16);

        assert!(m.rewind());
        assert!(m.free_bytes() > after_prime);
    }

    #[test]
    fn capacity_and_free_bytes_match_chunk_layout() {
        let arena = crate::Arena::builder().with_capacity(1024).build();
        let _ = arena.alloc(0_u32);
        let v = arena.alloc(42_u32);
        assert_eq!(*v, 42);
    }

    #[test]
    fn align_up_round_trips_powers_of_two() {
        assert_eq!(align_up(0, 8), Some(0));
        assert_eq!(align_up(1, 8), Some(8));
        assert_eq!(align_up(7, 8), Some(8));
        assert_eq!(align_up(8, 8), Some(8));
        assert_eq!(align_up(9, 8), Some(16));
        assert_eq!(align_up(16, 16), Some(16));
        assert_eq!(align_up(17, 16), Some(32));
        assert_eq!(align_up(33, 32), Some(64));
        assert_eq!(align_up(100, 64), Some(128));
        assert_eq!(align_up(0, 1), Some(0));
        assert_eq!(align_up(7, 1), Some(7));
        assert_eq!(align_up(usize::MAX, 8), None);
    }

    // An allocation may end exactly at the payload boundary.
    #[test]
    fn try_alloc_bytes_at_exact_remaining_capacity_succeeds() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current();
        let free = m.free_bytes();
        assert!(free > 0, "post-refill chunk must have remaining capacity");
        let result = m.try_alloc_bytes(free);
        assert!(result.is_some(), "try_alloc_bytes(free_bytes) must succeed at the exact boundary");
        assert!(m.try_alloc_bytes(1).is_none());
    }

    #[test]
    fn try_alloc_bytes_with_chunk_returns_ticket_and_owner() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current();
        let expected_chunk = m.chunk_ptr().expect("allocation initialized the current chunk");

        let (ticket, chunk) = m.try_alloc_bytes_with_chunk(4).expect("current chunk has capacity");
        let (_, len) = ticket.into_raw_buffer();

        assert_eq!(len, 4);
        assert_eq!(chunk, expected_chunk);
    }

    // An in-place growth may end exactly at the payload boundary.
    #[test]
    fn try_grow_in_place_at_exact_remaining_capacity_succeeds() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current();
        let free = m.free_bytes();
        assert!(free > 16, "need slack for an initial alloc plus grow");
        let prev_addr = m.bump.get().as_ptr() as usize;
        let initial = 16_usize;
        let _ = m.try_alloc_bytes(initial).expect("initial alloc");
        let new_len = free;
        assert!(
            m.try_grow_in_place(prev_addr, initial, new_len),
            "try_grow_in_place at exact remaining capacity must succeed",
        );
        assert_eq!(m.free_bytes(), 0);
    }

    // Non-growing requests succeed without moving the bump cursor.
    #[test]
    fn try_grow_in_place_non_growing_returns_true() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current();
        let bump_before = m.bump.get().as_ptr() as usize;
        assert!(m.try_grow_in_place(0, 8, 8), "equal lengths must succeed");
        assert!(m.try_grow_in_place(0, 8, 4), "shrink must succeed");
        assert_eq!(
            m.bump.get().as_ptr() as usize,
            bump_before,
            "non-growing grow must not move the bump cursor",
        );
    }

    // Address overflow rejects an in-place growth.
    #[test]
    fn try_grow_in_place_new_len_overflow_returns_false() {
        let arena = crate::Arena::new();
        let _ = arena.alloc(0_u8);
        let m = arena.current();
        let prev_addr = m.bump.get().as_ptr() as usize;
        let initial = 16_usize;
        let _ = m.try_alloc_bytes(initial).expect("initial alloc");
        assert!(
            !m.try_grow_in_place(prev_addr, initial, usize::MAX),
            "overflowing new_len must fail",
        );
    }
}
