// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![expect(
    clippy::type_repetition_in_bounds,
    reason = "trait-impl `where` clauses are kept uniform across all forwarding impls"
)]

use core::marker::PhantomData;
use core::mem::{MaybeUninit, forget, needs_drop};
use core::ptr::{NonNull, addr_eq, slice_from_raw_parts_mut};

use allocator_api2::alloc::{Allocator, Global};

use crate::internal::drop_list::{drop_shim_one, drop_shim_slice};
use crate::internal::in_chunk::InSharedChunk;
use crate::internal::shared_chunk::SharedChunk;
use crate::internal::sync::Ordering;
use crate::vec::Vec;

/// A thread-safe reference-counted smart pointer to a `T` stored in an [`Arena`](crate::Arena).
///
/// Safe to share across threads when `T: Send + Sync`.
///
/// Created via [`Arena::alloc_arc`](crate::Arena::alloc_arc). Cloning is
/// **O(1)** and uses a single Relaxed atomic increment (matching
/// `std::sync::Arc`). Dropping a clone is one Release decrement plus,
/// on the final dec to zero, an Acquire fence before chunk teardown.
/// For single-threaded code, prefer [`Rc`](crate::Rc) — it has the same
/// shape with a non-atomic refcount.
///
/// `Arc` keeps its containing chunk alive by holding a +1 refcount on
/// it, so the smart pointer can outlive the arena it came from and
/// survives [`Arena::reset`](crate::Arena::reset). `T::drop` runs when
/// the chunk is reclaimed (i.e. when its last live allocation is
/// released).
///
/// # Pinning
///
/// `Arc` implements [`Unpin`] unconditionally (like `std::sync::Arc`).
///
/// # Example
///
/// ```
/// use std::thread;
///
/// use multitude::Arena;
///
/// let arena = Arena::new();
/// let a = arena.alloc_arc(42_u32);
/// let b = a.clone();
/// let h = thread::spawn(move || *b);
/// assert_eq!(*a, h.join().unwrap());
/// ```
pub struct Arc<T: ?Sized, A: Allocator + Clone = Global> {
    /// Pointer into a shared chunk's payload, carrying the
    /// in-a-shared-chunk invariant statically so chunk-header
    /// recovery is a safe operation. The type owns one logical
    /// refcount on the chunk (either via the inflated count while
    /// the chunk is `current_shared`, or via a real `+1` after
    /// swap-out reconciliation).
    ptr: InSharedChunk<T, A>,
    /// Variance: `Arc<T>` is covariant in `T`. The phantom carries
    /// `T` for dropck and `A` for monomorphization.
    _phantom: PhantomData<(*const T, A)>,
}

impl<T: ?Sized, A: Allocator + Clone> Arc<T, A> {
    /// Wrap a freshly allocated value pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point to an initialized `T` in a shared arena chunk,
    /// and the arena must already have accounted for this handle in
    /// `arcs_issued`.
    #[inline]
    pub(crate) unsafe fn from_value_ptr(ptr: NonNull<T>) -> Self {
        // SAFETY: caller forwards the in-shared-chunk invariant.
        unsafe { Self::from_in_chunk(InSharedChunk::new(ptr)) }
    }

    /// Like [`Self::from_value_ptr`] for an already-validated in-chunk pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must name an initialized `T`, and `arcs_issued` must
    /// already account for this handle.
    #[inline]
    pub(crate) const unsafe fn from_in_chunk(ptr: InSharedChunk<T, A>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// Build an `Arc` from an [`OwnedInSharedChunk`] that already proves
    /// the in-chunk invariant and owns the `+1`.
    #[inline]
    pub(crate) fn from_owned_in_chunk(owned: crate::internal::owned_in_chunk::OwnedInSharedChunk<T, A>) -> Self {
        Self {
            ptr: owned.into_in_chunk(),
            _phantom: PhantomData,
        }
    }

    /// Borrow the containing chunk header while this `Arc` keeps it alive.
    #[inline]
    fn chunk(&self) -> &SharedChunk<A> {
        // SAFETY: `self.ptr` is in a live shared chunk, held by this `Arc`.
        unsafe { self.ptr.chunk_ptr().as_ref() }
    }

    /// Returns a raw pointer to the value.
    #[inline]
    #[must_use]
    pub const fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    /// True iff both handles point at the same address.
    #[inline]
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        addr_eq(a.ptr.as_ptr(), b.ptr.as_ptr())
    }

    /// Convert this `Arc<T, A>` into a [`Pin<Arc<T, A>>`](core::pin::Pin).
    ///
    /// Sound for any `T` (including `!Unpin`) because the value's
    /// address is fixed at allocation time, every live clone holds a
    /// chunk refcount that keeps the storage alive at the same
    /// address, and the value is dropped at that same address when
    /// the last clone is released — satisfying `Pin`'s contract.
    ///
    /// `Arc` exposes only `&T` through `Deref`, so there is no way to
    /// move the value out of a `Pin<Arc<T>>`.
    #[must_use]
    #[inline]
    pub fn into_pin(this: Self) -> core::pin::Pin<Self> {
        // SAFETY: storage address is fixed and stays alive at the
        // same address as long as any clone exists.
        unsafe { core::pin::Pin::new_unchecked(this) }
    }
}

impl<T: ?Sized, A: Allocator + Clone> From<Arc<T, A>> for core::pin::Pin<Arc<T, A>> {
    /// Mirror of `From<std::sync::Arc<T>> for Pin<std::sync::Arc<T>>`.
    /// See [`Arc::into_pin`] for the soundness argument.
    #[inline]
    fn from(arc: Arc<T, A>) -> Self {
        Arc::into_pin(arc)
    }
}

impl<T, A: Allocator + Clone> Arc<MaybeUninit<T>, A> {
    /// Convert a handle to `MaybeUninit<T>` whose value is now
    /// initialized into a handle to `T`. O(1) — no copy or alloc.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid
    /// `T`. The allocation must come from
    /// [`Arena::alloc_uninit_arc`](crate::Arena::alloc_uninit_arc) or
    /// [`Arena::alloc_zeroed_arc`](crate::Arena::alloc_zeroed_arc) so a
    /// drop entry was reserved up front;
    /// `Arena::alloc_arc(MaybeUninit::new(...))` does not reserve one
    /// and panics here for `T: Drop`.
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` when no drop entry is found in the chunk
    /// — see the safety contract above.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<T, A> {
        let ptr = self.ptr.as_non_null().cast::<T>();
        if needs_drop::<T>() {
            let chunk = self.chunk();
            // SAFETY: caller-held refcount keeps the chunk live.
            let data_addr = unsafe { SharedChunk::<A>::data_ptr(NonNull::from(chunk)) }.as_ptr() as usize;
            let value_offset = self.ptr.as_ptr() as *const u8 as usize - data_addr;
            // Acquire pairs with the owner-thread Release publish so
            // all of `entries[0..count]` is visible. Retargeting the
            // matching entry's `drop_fn` is an atomic Release store —
            // concurrent `assume_init` clones writing the same slot
            // is well-defined.
            let entry = chunk
                .drop_entries_acquire()
                .iter()
                .find(|e| e.value_offset as usize == value_offset)
                .expect(
                    "Arc::<MaybeUninit<T>>::assume_init: no drop entry reserved for this allocation. \
                     Use `Arena::alloc_uninit_arc::<T>()` / `alloc_zeroed_arc`; \
                     `Arena::alloc_arc(MaybeUninit::new(...))` does not reserve an entry and would silently leak `T::drop`.",
                );
            entry.store_drop_fn(drop_shim_one::<T>, Ordering::Release);
        }
        forget(self);
        // SAFETY: value is initialized; refcount transfers.
        unsafe { Arc::from_value_ptr(ptr) }
    }

    /// Pinned mirror of [`Self::assume_init`]. The pin is preserved
    /// across the cast because the value's address does not change.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin(this: core::pin::Pin<Self>) -> core::pin::Pin<Arc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: storage is unchanged across the cast; the
        // `assume_init` contract is the caller's.
        unsafe {
            let inner = core::pin::Pin::into_inner_unchecked(this);
            core::pin::Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T, A: Allocator + Clone> Arc<[MaybeUninit<T>], A> {
    /// Convert a slice handle of `MaybeUninit<T>` whose elements are
    /// now initialized into a slice handle of `T`. O(1).
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized,
    /// valid `T`. The allocation must come from
    /// [`Arena::alloc_uninit_slice_arc`](crate::Arena::alloc_uninit_slice_arc)
    /// or
    /// [`Arena::alloc_zeroed_slice_arc`](crate::Arena::alloc_zeroed_slice_arc).
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` when no drop entry is found in the chunk.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Arc<[T], A> {
        let old_ptr = self.ptr.as_non_null();
        let len = old_ptr.len();
        if needs_drop::<T>() {
            let chunk = self.chunk();
            // SAFETY: caller-held refcount keeps the chunk live.
            let data_addr = unsafe { SharedChunk::<A>::data_ptr(NonNull::from(chunk)) }.as_ptr() as usize;
            let value_offset = old_ptr.as_ptr() as *const u8 as usize - data_addr;
            // Synchronization matches the scalar `assume_init` case
            // above (Acquire load + Release retarget).
            let entry = chunk
                .drop_entries_acquire()
                .iter()
                .find(|e| e.value_offset as usize == value_offset)
                .expect(
                    "Arc::<[MaybeUninit<T>]>::assume_init: no drop entry reserved for this allocation. \
                     Use `Arena::alloc_uninit_slice_arc::<T>()` / `alloc_zeroed_slice_arc`; \
                     `alloc_slice_*_arc` of `MaybeUninit<T>` does not reserve an entry and would silently leak.",
                );
            entry.store_drop_fn(drop_shim_slice::<T>, Ordering::Release);
        }
        forget(self);
        let data = old_ptr.as_ptr().cast::<T>();
        let fat = slice_from_raw_parts_mut(data, len);
        // SAFETY: caller guarantees initialization; refcount transfers.
        unsafe { Arc::from_value_ptr(NonNull::new_unchecked(fat)) }
    }

    /// Pinned mirror of [`Self::assume_init`] for slices.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin_slice(this: core::pin::Pin<Self>) -> core::pin::Pin<Arc<[T], A>>
    where
        A: 'static,
    {
        // SAFETY: storage is unchanged across the cast.
        unsafe {
            let inner = core::pin::Pin::into_inner_unchecked(this);
            core::pin::Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Clone for Arc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        self.chunk().inc_ref();
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Drop for Arc<T, A> {
    #[inline]
    fn drop(&mut self) {
        let chunk = self.ptr.chunk_ptr();
        // SAFETY: we own one outstanding refcount, which `dec_ref`
        // consumes.
        unsafe { SharedChunk::dec_ref(chunk) };
    }
}

crate::smart_ptr_macros::impl_smart_ptr_forwarding_traits!(Arc);

// SAFETY: same cross-thread invariants as `std::sync::Arc`; the backing
// chunk refcount is atomic and sharing is gated on `T` and `A`.
unsafe impl<T: ?Sized + Sync + Send, A: Allocator + Clone + Send + Sync> Send for Arc<T, A> {}
// SAFETY: same invariants as the `Send` impl.
unsafe impl<T: ?Sized + Sync + Send, A: Allocator + Clone + Send + Sync> Sync for Arc<T, A> {}

impl<'a, T, A: Allocator + Clone> From<Vec<'a, T, A>> for Arc<[T], A>
where
    T: Send + Sync,
    A: Send + Sync,
{
    /// Freeze a [`Vec`](crate::vec::Vec) into an immutable
    /// [`Arc<[T], A>`](crate::Arc). See [`Vec::into_arena_arc`](crate::vec::Vec::into_arena_arc).
    #[inline]
    fn from(v: Vec<'a, T, A>) -> Self {
        v.into_arena_arc()
    }
}
