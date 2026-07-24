// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Arena-level reservation and lifetime promotion.
//!
//! Local reservations remain valid because their chunks are retained until an
//! exclusive reset or drop. Smart-pointer reservations acquire independent
//! chunk ownership before the arena borrow ends.

use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::Arena;
use crate::internal::chunk::Chunk;
use crate::internal::thin_dst::Strong;
use crate::internal::uninit::Uninit;

impl<A: Allocator + Clone> Arena<A> {
    /// Try to reserve uninitialized storage for one `T` in the current
    /// chunk. Returns a [`Uninit`] ticket whose lifetime is bound
    /// to `&self`.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // body→None ⇒ refill spin (OOM)
    pub(in crate::arena) fn try_reserve_local<T>(&self) -> Option<Uninit<'_, T>> {
        let ticket = self.current().try_alloc_uninit::<T>()?;
        self.mark_reference_handout();
        // SAFETY: reference handouts retain the chunk until an exclusive arena
        // operation, which cannot overlap this borrow.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// in the current chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(crate) fn try_reserve_local_slice<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let ticket = self.current().try_alloc_uninit_slice::<T>(len)?;
        self.mark_reference_handout();
        // SAFETY: reference handouts retain the chunk for this borrow.
        Some(unsafe { ticket.rebind() })
    }

    /// Like [`Self::try_reserve_local_slice`] but takes the precomputed
    /// byte size; the slice-copy/clone fast paths hold an existing
    /// `&[T]` and compute `size_of_val(src)` once outside the refill
    /// loop, sparing the inner reservation a `checked_mul` overflow
    /// guard.
    ///
    /// # Safety
    ///
    /// `size` must equal `size_of::<T>() * len` (without overflow).
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(in crate::arena) fn try_reserve_local_slice_with_size<T>(&self, len: usize, size: usize) -> Option<Uninit<'_, [T]>> {
        // SAFETY: required by this function's contract.
        let ticket = unsafe { self.current().try_alloc_uninit_slice_with_size::<T>(len, size) }?;
        self.mark_reference_handout();
        // SAFETY: reference handouts retain the chunk for this borrow.
        Some(unsafe { ticket.rebind() })
    }

    /// Like [`Self::try_reserve_local_slice`] but reserves the `Arc<[T]>`
    /// freeze prefix (`[strong][pad][len]`) ahead of the payload so the
    /// resulting buffer can later be frozen into an `Arc<[T]>` / `Box<[T]>`
    /// in place with no copy (see the [`Vec`](crate::Vec) freeze paths).
    ///
    /// As with the other reference reservations no chunk refcount is taken
    /// here: the chunk is pinned via `mark_reference_handout` and the
    /// refcount is acquired only at freeze time.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(crate) fn try_reserve_freezable_slice<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let ticket = self.current().try_alloc_freezable_slice::<T>(len)?;
        self.mark_reference_handout();
        // SAFETY: reference handouts retain the chunk for this borrow.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve `len` consecutive bytes in the current chunk.
    /// Byte-slice fast path that skips alignment math and overflow checks.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(in crate::arena) fn try_reserve_local_bytes(&self, len: usize) -> Option<Uninit<'_, [u8]>> {
        let ticket = self.current().try_alloc_bytes(len)?;
        self.mark_reference_handout();
        // SAFETY: reference handouts retain the chunk for this borrow.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for one `T` in the current
    /// chunk. The returned ticket is consumed by the caller
    /// before the chunk's refcount is incremented for the smart pointer.
    /// The chunk pointer is returned alongside the ticket so callers
    /// can take a +1 refcount on it without re-asserting that the
    /// current mutator owns a chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // body→None ⇒ refill spin (OOM); same for the helpers below
    pub(in crate::arena) fn try_reserve_shared<T>(&self) -> Option<(Uninit<'_, T>, NonNull<Chunk<A>>)> {
        let mutator = self.current();
        let ticket = mutator.try_alloc_uninit::<T>()?;
        // SAFETY: a successful reservation proves the chunk exists. The caller
        // acquires chunk ownership before the promoted borrow ends.
        Some(unsafe { (ticket.rebind(), mutator.chunk_ptr_unchecked()) })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// in the current chunk, taking the precomputed payload byte
    /// size; the slice-copy fast paths hold an existing `&[T]` and
    /// compute `size_of_val(src)` once outside the refill loop, sparing
    /// the inner reservation a `checked_mul` overflow guard.
    ///
    /// Includes a thin-pointer DST length prefix immediately before
    /// the payload — see [`ChunkMutator::try_alloc_uninit_slice_prefixed`].
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` (without
    /// overflow).
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    #[expect(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(in crate::arena) unsafe fn try_reserve_shared_slice_with_size<T>(
        &self,
        len: usize,
        payload_bytes: usize,
    ) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        let mutator = self.current();
        // SAFETY: required by this function's contract.
        let ticket = unsafe { mutator.try_alloc_uninit_slice_prefixed_with_size::<T>(len, payload_bytes) }?;
        // SAFETY: the caller acquires chunk ownership before this borrow ends.
        Some(unsafe { (ticket.rebind(), mutator.chunk_ptr_unchecked()) })
    }

    /// Try to reserve storage for one strong-prefixed `Arc<T>` value in
    /// the current chunk. The returned ticket addresses the
    /// payload (the strong count is already initialized to `1`).
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    pub(in crate::arena) fn try_reserve_arc_value<S: Strong, T>(&self) -> Option<(Uninit<'_, T>, NonNull<Chunk<A>>)> {
        let (ticket, chunk) = self.current().try_alloc_arc_value::<S, T>()?;
        // SAFETY: the caller acquires chunk ownership before this borrow ends.
        Some(unsafe { (ticket.rebind(), chunk) })
    }

    /// Slice form of [`Self::try_reserve_arc_value`]: reserves a strong
    /// prefix, slice-length metadata, and `len` `T`s.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    #[expect(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(in crate::arena) fn try_reserve_arc_slice<S: Strong, T>(&self, len: usize) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        let (ticket, chunk) = self.current().try_alloc_arc_slice::<S, T>(len)?;
        // SAFETY: the caller acquires chunk ownership before this borrow ends.
        Some(unsafe { (ticket.rebind(), chunk) })
    }

    /// Like [`Self::try_reserve_arc_slice`] but takes the precomputed
    /// payload byte size (held by callers with a live `&[T]`).
    ///
    /// # Safety
    ///
    /// `payload_bytes` must equal `size_of::<T>() * len` (without overflow).
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    #[expect(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(in crate::arena) unsafe fn try_reserve_arc_slice_with_size<S: Strong, T>(
        &self,
        len: usize,
        payload_bytes: usize,
    ) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        // SAFETY: required by this function's contract.
        let (ticket, chunk) = unsafe { self.current().try_alloc_arc_slice_with_size::<S, T>(len, payload_bytes) }?;
        // SAFETY: the caller acquires chunk ownership before this borrow ends.
        Some(unsafe { (ticket.rebind(), chunk) })
    }
}
