// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Safe `Arena`-level reservation primitives.
//!
//! These helpers wrap [`ChunkMutator::try_alloc_uninit*`] with the
//! lifetime promotion that allows the resulting ticket to be consumed
//! into a `&'arena mut T` reference at the public API boundary.
//!
//! # Why the promotion is sound
//!
//! Reference reservations (`try_reserve_local*`): when [`Arena::refill`]
//! rotates out a chunk that handed out a `&mut T`, it pushes the retired
//! [`ChunkMutator`] into `Arena::retired_local`, where it holds its `+1`
//! refcount. That list is cleared only on `&mut self` paths (`reset`,
//! `Drop`). Therefore every slot ever reserved as a reference remains live
//! for the entire `&Arena` borrow lifetime — even if its chunk has been
//! rotated out.
//!
//! Smart-pointer reservations (`try_reserve_shared*` / `try_reserve_arc*`):
//! these produce `Arc<T>` / `Box<T>`, not raw `&mut T`. Each handle
//! independently keeps its hosting chunk alive via the chunk's atomic
//! refcount, so the `&Arena` lifetime promotion isn't relied upon for them.
//! The helpers still rebind the ticket lifetime for API symmetry; callers
//! wrap the resulting reference in a smart pointer before exposing it.
//!
//! All `unsafe` related to lifetime extension lives in this single
//! module; the public `alloc_*.rs` files contain no `unsafe` blocks.

use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::Arena;
use crate::internal::chunk::Chunk;
use crate::internal::uninit::{Uninit, UninitDrop};

impl<A: Allocator + Clone> Arena<A> {
    /// Try to reserve uninitialized storage for one `T` in the current
    /// chunk. Returns a [`Uninit`] ticket whose lifetime is bound
    /// to `&self`.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // body→None ⇒ refill spin (OOM)
    pub(in crate::arena) fn try_reserve_local<T>(&self) -> Option<Uninit<'_, T>> {
        let ticket = self.current().try_alloc_uninit::<T>()?;
        // Pin the chunk: bare references have no refcount, so a chunk that
        // served one must be retired (not reclaimed early) when rotated out.
        self.mark_reference_handout();
        // SAFETY: the chunk that hosts this slot is retained for the
        // full `&Arena` borrow lifetime; see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for one `T` plus a drop
    /// entry slot in the current chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(in crate::arena) fn try_reserve_local_with_drop<T>(&self) -> Option<UninitDrop<'_, T>> {
        let ticket = self.current().try_alloc_uninit_with_drop::<T>()?;
        // Pin the chunk: bare references have no refcount, so a chunk that
        // served one must be retired (not reclaimed early) when rotated out.
        self.mark_reference_handout();
        // SAFETY: see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// in the current chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(crate) fn try_reserve_local_slice<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let ticket = self.current().try_alloc_uninit_slice::<T>(len)?;
        // Pin the chunk: bare references have no refcount, so a chunk that
        // served one must be retired (not reclaimed early) when rotated out.
        self.mark_reference_handout();
        // SAFETY: see module-level rationale.
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
        // SAFETY: forwarded to the caller.
        let ticket = unsafe { self.current().try_alloc_uninit_slice_with_size::<T>(len, size) }?;
        // Pin the chunk: bare references have no refcount, so a chunk that
        // served one must be retired (not reclaimed early) when rotated out.
        self.mark_reference_handout();
        // SAFETY: see module-level rationale.
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
        // Pin the chunk: the buffer holds no refcount until it is frozen, so
        // a chunk that served one must be retired (not reclaimed early).
        self.mark_reference_handout();
        // SAFETY: see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve `len` consecutive bytes in the current chunk.
    /// Byte-slice fast path that skips alignment math and overflow checks.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(in crate::arena) fn try_reserve_local_bytes(&self, len: usize) -> Option<Uninit<'_, [u8]>> {
        let ticket = self.current().try_alloc_bytes(len)?;
        // Pin the chunk: bare references have no refcount, so a chunk that
        // served one must be retired (not reclaimed early) when rotated out.
        self.mark_reference_handout();
        // SAFETY: see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// plus a drop entry slot in the current chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(in crate::arena) fn try_reserve_local_slice_with_drop<T>(&self, len: usize) -> Option<UninitDrop<'_, [T]>> {
        let ticket = self.current().try_alloc_uninit_slice_with_drop::<T>(len)?;
        // Pin the chunk: bare references have no refcount, so a chunk that
        // served one must be retired (not reclaimed early) when rotated out.
        self.mark_reference_handout();
        // SAFETY: see module-level rationale.
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
        // SAFETY: `try_alloc_uninit` returning `Some` proves the
        // mutator owns a chunk; `rebind` is sound per the module-level
        // rationale (the chunk is retained across the `&Arena` borrow).
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
    #[allow(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(in crate::arena) unsafe fn try_reserve_shared_slice_with_size<T>(
        &self,
        len: usize,
        payload_bytes: usize,
    ) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        let mutator = self.current();
        // SAFETY: forwarded to the caller.
        let ticket = unsafe { mutator.try_alloc_uninit_slice_prefixed_with_size::<T>(len, payload_bytes) }?;
        // SAFETY: see `try_reserve_shared`.
        Some(unsafe { (ticket.rebind(), mutator.chunk_ptr_unchecked()) })
    }

    /// Try to reserve storage for one strong-prefixed `Arc<T>` value in
    /// the current chunk. The returned ticket addresses the
    /// payload (the strong count is already initialized to `1`).
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    pub(in crate::arena) fn try_reserve_arc_value<T>(&self) -> Option<(Uninit<'_, T>, NonNull<Chunk<A>>)> {
        let (ticket, chunk) = self.current().try_alloc_arc_value::<T>()?;
        // SAFETY: see `try_reserve_shared`.
        Some(unsafe { (ticket.rebind(), chunk) })
    }

    /// Slice form of [`Self::try_reserve_arc_value`]: reserves a strong
    /// prefix, slice-length metadata, and `len` `T`s.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    #[allow(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(in crate::arena) fn try_reserve_arc_slice<T>(&self, len: usize) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        let (ticket, chunk) = self.current().try_alloc_arc_slice::<T>(len)?;
        // SAFETY: see `try_reserve_shared`.
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
    #[allow(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(in crate::arena) unsafe fn try_reserve_arc_slice_with_size<T>(
        &self,
        len: usize,
        payload_bytes: usize,
    ) -> Option<(Uninit<'_, [T]>, NonNull<Chunk<A>>)> {
        // SAFETY: forwarded to the caller.
        let (ticket, chunk) = unsafe { self.current().try_alloc_arc_slice_with_size::<T>(len, payload_bytes) }?;
        // SAFETY: see `try_reserve_shared`.
        Some(unsafe { (ticket.rebind(), chunk) })
    }
}
