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
//! Local chunks: when [`Arena::refill_local`] rotates a chunk out, it
//! pushes the retired [`ChunkMutator`] into `Arena::retired_local`,
//! where it holds its `+1` refcount. The vector is cleared only on
//! `&mut self` paths (`reset`, `Drop`). Therefore every slot ever
//! reserved in a local chunk remains live for the entire `&Arena`
//! borrow lifetime — even if the chunk has been rotated out.
//!
//! Shared chunks: by design these only produce `Arc<T>` smart pointers,
//! not raw `&mut T`. Each `Arc` independently keeps its hosting chunk
//! alive via the chunk's atomic refcount, so the `&Arena` lifetime
//! promotion isn't relied upon for shared reservations either. The
//! `try_reserve_shared*` helpers still rebind the ticket lifetime for
//! API symmetry; callers wrap the resulting reference in an `Arc`
//! before exposing it.
//!
//! All `unsafe` related to lifetime extension lives in this single
//! module; the public `alloc_*.rs` files contain no `unsafe` blocks.

use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use super::Arena;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::uninit::{Uninit, UninitDrop};

impl<A: Allocator + Clone> Arena<A> {
    /// Try to reserve uninitialized storage for one `T` in the current
    /// local chunk. Returns a [`Uninit`] ticket whose lifetime is bound
    /// to `&self`.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // body→None ⇒ refill spin (OOM)
    pub(crate) fn try_reserve_local<T>(&self) -> Option<Uninit<'_, T>> {
        let ticket = self.current_local().try_alloc_uninit::<T>()?;
        // SAFETY: the chunk that hosts this slot is retained for the
        // full `&Arena` borrow lifetime; see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for one `T` plus a drop
    /// entry slot in the current local chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(crate) fn try_reserve_local_with_drop<T>(&self) -> Option<UninitDrop<'_, T>> {
        let ticket = self.current_local().try_alloc_uninit_with_drop::<T>()?;
        // SAFETY: see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// in the current local chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(crate) fn try_reserve_local_slice<T>(&self, len: usize) -> Option<Uninit<'_, [T]>> {
        let ticket = self.current_local().try_alloc_uninit_slice::<T>(len)?;
        // SAFETY: see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve `len` consecutive bytes in the current local chunk.
    /// Byte-slice fast path that skips alignment math and overflow checks.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(crate) fn try_reserve_local_bytes(&self, len: usize) -> Option<Uninit<'_, [u8]>> {
        let ticket = self.current_local().try_alloc_bytes(len)?;
        // SAFETY: see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// plus a drop entry slot in the current local chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_local`
    pub(crate) fn try_reserve_local_slice_with_drop<T>(&self, len: usize) -> Option<UninitDrop<'_, [T]>> {
        let ticket = self.current_local().try_alloc_uninit_slice_with_drop::<T>(len)?;
        // SAFETY: see module-level rationale.
        Some(unsafe { ticket.rebind() })
    }

    /// Try to reserve uninitialized storage for one `T` in the current
    /// shared chunk. The returned ticket is consumed by the caller
    /// before the chunk's refcount is incremented for the smart pointer.
    /// The chunk pointer is returned alongside the ticket so callers
    /// can take a +1 refcount on it without re-asserting that the
    /// current mutator owns a chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // body→None ⇒ refill spin (OOM); same for the helpers below
    pub(crate) fn try_reserve_shared<T>(&self) -> Option<(Uninit<'_, T>, NonNull<SharedChunk<A>>)> {
        let mutator = self.current_shared();
        let ticket = mutator.try_alloc_uninit::<T>()?;
        // SAFETY: `try_alloc_uninit` returning `Some` proves the
        // mutator owns a chunk; `rebind` is sound per the module-level
        // rationale (the chunk is retained across the `&Arena` borrow).
        Some(unsafe { (ticket.rebind(), mutator.chunk_ptr_unchecked()) })
    }

    /// Try to reserve uninitialized storage for one `T` plus a drop
    /// entry slot in the current shared chunk.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    pub(crate) fn try_reserve_shared_with_drop<T>(&self) -> Option<(UninitDrop<'_, T>, NonNull<SharedChunk<A>>)> {
        let mutator = self.current_shared();
        let ticket = mutator.try_alloc_uninit_with_drop::<T>()?;
        // SAFETY: see `try_reserve_shared`.
        Some(unsafe { (ticket.rebind(), mutator.chunk_ptr_unchecked()) })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// in the current shared chunk.
    ///
    /// Includes a thin-pointer DST length prefix immediately before
    /// the payload — see [`ChunkMutator::try_alloc_uninit_slice_prefixed`].
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    #[allow(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(crate) fn try_reserve_shared_slice<T>(&self, len: usize) -> Option<(Uninit<'_, [T]>, NonNull<SharedChunk<A>>)> {
        let mutator = self.current_shared();
        let ticket = mutator.try_alloc_uninit_slice_prefixed::<T>(len)?;
        // SAFETY: see `try_reserve_shared`.
        Some(unsafe { (ticket.rebind(), mutator.chunk_ptr_unchecked()) })
    }

    /// Try to reserve uninitialized storage for `len` consecutive `T`s
    /// plus a drop entry slot in the current shared chunk. Includes a
    /// thin-pointer DST length prefix immediately before the payload.
    #[inline(always)]
    #[cfg_attr(test, mutants::skip)] // see `try_reserve_shared`
    #[allow(
        clippy::type_complexity,
        reason = "ticket + chunk-ptr tuple is the natural shape; type alias would obscure rather than clarify"
    )]
    pub(crate) fn try_reserve_shared_slice_with_drop<T>(&self, len: usize) -> Option<(UninitDrop<'_, [T]>, NonNull<SharedChunk<A>>)> {
        let mutator = self.current_shared();
        let ticket = mutator.try_alloc_uninit_slice_with_drop_prefixed::<T>(len)?;
        // SAFETY: see `try_reserve_shared`.
        Some(unsafe { (ticket.rebind(), mutator.chunk_ptr_unchecked()) })
    }
}
