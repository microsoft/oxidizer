// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! A pool that serves one runtime-known value layout.
//!
//! `LayoutPool` is the same body as [`Pool`](crate::Pool) driven by
//! [`RuntimeGeometry`] instead of compile-time constants, so it can serve a
//! layout that is only known when the pool is built. It is crate-private:
//! [`MultiPool`](crate::MultiPool) is its only user, and it exists so that the
//! multi pool's router has something uniform to route to.
//!
//! Every allocation entry point is `unsafe` and unchecked, because the router
//! selected this pool *because* the layouts matched — re-checking on every
//! allocation would pay for a fact the caller already proved. See
//! `docs/implementation/multi-pool.md`.

use alloc::alloc::alloc as global_alloc;
use alloc::vec::Vec;
use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::ptr::NonNull;

use allocator_api2::alloc::Allocator;

use crate::atomic::Ordering::{Acquire, Relaxed, Release};
use crate::atomic::{AtomicU32, AtomicUsize, fence};
use crate::error::AllocError;
use crate::geometry::{RuntimeGeometry, SlotGeometry};
use crate::pool::{PoolCore, PoolInner, publish_address, teardown, teardown_erased};
use crate::slot::{FREE_END, MAX_CHUNK_SIZE_SLOTS, MAX_POOL_SLOTS, SlotCell};

/// A pool serving one fixed value [`Layout`].
///
/// One pointer wide, and holding one unit of its own pool-level reference
/// count.
pub(crate) struct LayoutPool<A: Allocator> {
    inner: NonNull<PoolInner<A, RuntimeGeometry>>,
}

impl<A: Allocator> LayoutPool<A> {
    /// Builds a pool serving `layout`.
    ///
    /// `chunk_size` is rounded up to a power of two and then **clamped** so
    /// that a chunk's memory layout cannot overflow; `max_chunks` is clamped to
    /// what the pool's slot-index ceiling permits at the effective chunk size.
    /// Clamping rather than asserting is what lets one multi-pool-wide sizing
    /// configuration meet an arbitrary layout without the first allocation of
    /// an unfortunate layout panicking out of a fallible call.
    /// Ref: docs/implementation/multi-pool.md, "Clamping the sizing configuration".
    ///
    /// # Errors
    /// Returns [`AllocError::ALLOCATOR_FAILED`] if the global allocator cannot
    /// supply the pool's metadata block, or if `layout` is so large that not
    /// even a one-slot chunk of it has a representable [`Layout`].
    pub(crate) fn new(layout: Layout, chunk_size: u32, max_chunks: Option<u32>, allocator: A) -> Result<Self, AllocError> {
        let geometry = RuntimeGeometry::new(layout);
        let (chunk_size, chunk_layout) = clamp_chunk_size(geometry, chunk_size).ok_or(AllocError::ALLOCATOR_FAILED)?;
        let max_chunks = clamp_max_chunks(chunk_size, max_chunks);

        let inner = PoolInner {
            core: PoolCore {
                free_head: AtomicU32::new(FREE_END),
                pool_refcount: AtomicUsize::new(1),
                teardown: teardown_erased::<A, RuntimeGeometry>,
            },
            me: NonNull::dangling(),
            chunk_size,
            shift: chunk_size.trailing_zeros(),
            mask: chunk_size - 1,
            max_chunks: Some(max_chunks),
            chunks_allocated: AtomicU32::new(0),
            #[cfg(feature = "stats")]
            bytes_allocated: AtomicUsize::new(0),
            chunk_layout,
            directory: UnsafeCell::new(Vec::new()),
            allocator,
            geometry,
        };

        let meta = Layout::new::<PoolInner<A, RuntimeGeometry>>();
        // SAFETY: `PoolInner` is never zero-sized (it contains `PoolCore`).
        let raw = unsafe { global_alloc(meta) }.cast::<PoolInner<A, RuntimeGeometry>>();
        let Some(raw) = NonNull::new(raw) else {
            return Err(AllocError::ALLOCATOR_FAILED);
        };
        // SAFETY: `raw` is a fresh, exclusively owned, correctly sized block.
        unsafe { raw.as_ptr().write(inner) };
        // SAFETY: `raw` addresses the initialized pool nothing has borrowed
        // yet, and carries the allocation's own provenance.
        unsafe { publish_address(raw) };
        Ok(Self { inner: raw })
    }

    #[inline]
    fn inner(&self) -> &PoolInner<A, RuntimeGeometry> {
        // SAFETY: `inner` is valid while this `LayoutPool` holds a pool refcount.
        unsafe { self.inner.as_ref() }
    }

    /// A [`Copy`] view of this pool.
    ///
    /// The router copies one of these out of its directory and releases the
    /// borrow before allocating, so that reentrant user code is free to grow
    /// the directory. The view stays valid because the `PoolInner` it addresses
    /// is heap-allocated and never moves, and because layout pools are never
    /// retired. Ref: docs/implementation/multi-pool.md, "Reentrancy".
    #[inline]
    pub(crate) fn as_ref(&self) -> LayoutPoolRef<A> {
        LayoutPoolRef { inner: self.inner }
    }
}

impl<A: Allocator> Drop for LayoutPool<A> {
    fn drop(&mut self) {
        let inner = self.inner();
        if inner.core.pool_refcount.fetch_sub(1, Release) == 1 {
            fence(Acquire);
            // SAFETY: a zero refcount grants exclusive ownership of the inner.
            unsafe { teardown(self.inner) };
        }
    }
}

/// A [`Copy`], non-owning view of a [`LayoutPool`].
///
/// Holds no reference count: it is only ever used while the [`MultiPool`] that
/// owns the pool is borrowed, and layout pools are never retired.
///
/// [`MultiPool`]: crate::MultiPool
pub(crate) struct LayoutPoolRef<A: Allocator> {
    inner: NonNull<PoolInner<A, RuntimeGeometry>>,
}

impl<A: Allocator> Clone for LayoutPoolRef<A> {
    // Required by `Copy`, which is how the view is actually passed around.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn clone(&self) -> Self {
        *self
    }
}

impl<A: Allocator> Copy for LayoutPoolRef<A> {}

impl<A: Allocator> LayoutPoolRef<A> {
    /// The pool's shared core, through which handles are constructed.
    #[inline]
    pub(crate) fn core(&self) -> &PoolCore {
        // SAFETY: the owning `LayoutPool` outlives every view of it.
        unsafe { &self.inner.as_ref().core }
    }

    #[inline]
    fn inner(&self) -> &PoolInner<A, RuntimeGeometry> {
        // SAFETY: the owning `LayoutPool` outlives every view of it.
        unsafe { self.inner.as_ref() }
    }

    /// Effective slots per chunk, after clamping.
    #[inline]
    pub(crate) fn chunk_size(self) -> u32 {
        self.inner().chunk_size
    }

    /// Effective chunk cap, after clamping.
    #[inline]
    pub(crate) fn max_chunks(self) -> u32 {
        // The constructor always installs a cap.
        self.inner().max_chunks.unwrap_or(u32::MAX)
    }

    /// Number of chunks allocated so far.
    #[inline]
    pub(crate) fn chunks_allocated(self) -> u32 {
        self.inner().chunks_allocated.load(Relaxed)
    }

    /// Total bytes taken from the pool's allocator over its lifetime.
    #[cfg(feature = "stats")]
    #[inline]
    pub(crate) fn bytes_allocated(self) -> u64 {
        self.inner().bytes_allocated.load(Relaxed) as u64
    }

    /// Live refcounted allocations (`Box`/`Arc`/`Rc`), excluding `Alloc`.
    #[inline]
    pub(crate) fn len(self) -> u64 {
        // pool_refcount = 1 (the owning `LayoutPool`) + live refcounted allocations.
        self.inner().core.pool_refcount.load(Relaxed).saturating_sub(1) as u64
    }

    /// Pops a free slot, growing the pool if necessary.
    ///
    /// # Safety
    /// `Layout::new::<T>()` must route to this pool. The router establishes
    /// this by construction — it selected this pool because the routing keys
    /// matched.
    ///
    /// # Errors
    /// Returns [`AllocError`] if allocation fails.
    #[inline]
    pub(crate) unsafe fn alloc_slot<T>(self) -> Result<NonNull<SlotCell<T>>, AllocError> {
        // SAFETY: the owning `LayoutPool` outlives every view of it.
        let inner = unsafe { self.inner.as_ref() };
        debug_assert_eq!(
            crate::geometry::routing_key(Layout::new::<T>()),
            inner.geometry.layout(),
            "layout pool served a mismatched type"
        );
        match inner.alloc_slot() {
            Ok(slot) => Ok(slot.cast::<SlotCell<T>>()),
            Err(err) => Err(err),
        }
    }
}

// SAFETY: identical to `Pool`'s argument — all cross-thread state is atomic,
// the directory is reached only from the single allocator thread, and the pool
// object owns no values. Ref: docs/DESIGN.md, invariant 7.
unsafe impl<A: Allocator + Send> Send for LayoutPool<A> {}

/// Rounds `chunk_size` up to a power of two, then halves it until a chunk of
/// that many slots has a representable [`Layout`]. Returns the effective slot
/// count together with that layout, or `None` when not even a one-slot chunk
/// can be laid out.
fn clamp_chunk_size(geometry: RuntimeGeometry, chunk_size: u32) -> Option<(u32, Layout)> {
    let mut slots = chunk_size.clamp(1, MAX_CHUNK_SIZE_SLOTS).next_power_of_two();
    loop {
        if let Some(layout) = geometry.chunk_layout(slots as usize) {
            return Some((slots, layout));
        }
        // A value whose slot and chunk header together leave the `Layout`
        // ceiling behind cannot be pooled at any chunk size, so the floor is
        // reported rather than retried. Reaching it needs a value layout within
        // the slot metadata of that ceiling, which a target only permits when
        // its largest object is that close to it.
        // Ref: docs/implementation/multi-pool.md, "Clamping the sizing configuration".
        if slots == 1 {
            return None;
        }
        let previous = slots;
        slots /= 2;
        // Termination rests on the halving strictly shrinking the count. The
        // assertion states that rather than trusting it, so a mutation of the
        // arithmetic above fails the test instead of hanging it.
        debug_assert!(slots < previous, "chunk size retried without shrinking");
    }
}

/// The slot count [`LayoutPool::new`] would settle on for `layout`, without
/// building a pool. Lets the multi pool report effective sizing for a layout it
/// has not yet seen. Zero for a layout no chunk can hold.
pub(crate) fn effective_chunk_size(layout: Layout, requested: u32) -> u32 {
    clamp_chunk_size(RuntimeGeometry::new(layout), requested).map_or(0, |(slots, _)| slots)
}

/// The chunk cap [`LayoutPool::new`] would settle on, without building a pool.
pub(crate) fn effective_max_chunks(chunk_size: u32, requested: Option<u32>) -> u32 {
    clamp_max_chunks(chunk_size, requested)
}

/// Clamps the chunk cap to what the slot-index ceiling permits at `chunk_size`,
/// defaulting an absent cap to that ceiling.
///
/// A cap of zero is a pool that can never allocate, exactly as it is for
/// [`Pool`](crate::Pool), so only the upper bound is applied.
fn clamp_max_chunks(chunk_size: u32, max_chunks: Option<u32>) -> u32 {
    // A zero chunk size is what the sizing query reports for a layout no chunk
    // can hold. Such a pool is never built, so there is no cap to clamp; the
    // divisor below would be zero.
    let Some(ceiling) = MAX_POOL_SLOTS.checked_div(u64::from(chunk_size)) else {
        return 0;
    };
    // The quotient can still be zero on a target whose addressable slot count
    // is below the effective chunk size, which yields a pool that can never
    // allocate — the same outcome an explicit zero cap produces, and not a new
    // failure mode.
    let requested = max_chunks.map_or(ceiling, u64::from);
    u32::try_from(requested.min(ceiling)).unwrap_or(u32::MAX)
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use allocator_api2::alloc::Global;

    use super::*;

    /// A value layout so close to the `Layout` size ceiling that adding the
    /// slot metadata and the chunk header leaves it behind.
    ///
    /// Only a target whose largest object reaches this far can produce such a
    /// layout from `Layout::new::<T>()`, so the layout is built directly.
    fn unpoolable_layout() -> Layout {
        Layout::from_size_align(isize::MAX as usize, 1).unwrap()
    }

    #[test]
    fn a_layout_leaving_no_room_for_a_chunk_has_no_pool() {
        let Err(error) = LayoutPool::new(unpoolable_layout(), 64, None, Global) else {
            panic!("no chunk of this layout fits");
        };
        assert!(error.is_allocator_failure());
    }

    #[test]
    fn a_layout_leaving_no_room_for_a_chunk_reports_no_sizing() {
        assert_eq!(effective_chunk_size(unpoolable_layout(), 64), 0);
        assert_eq!(effective_max_chunks(0, None), 0);
        assert_eq!(effective_max_chunks(0, Some(8)), 0);
    }

    #[test]
    fn a_layout_leaving_room_for_one_slot_is_clamped_to_it() {
        // Half the address space per value: a chunk holds exactly one slot.
        let layout = Layout::from_size_align(1 << (usize::BITS - 2), 1).unwrap();
        assert_eq!(effective_chunk_size(layout, 64), 1);
    }
}
