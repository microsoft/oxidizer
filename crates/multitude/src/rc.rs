// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![allow(
    clippy::type_repetition_in_bounds,
    clippy::cast_ptr_alignment,
    reason = "trait-impl `where` clauses are kept uniform across all forwarding impls; pointer casts target the chunk header which is CHUNK_ALIGN-aligned by construction"
)]

use core::marker::PhantomData;
use core::mem::{MaybeUninit, forget, needs_drop};
use core::ptr::{NonNull, addr_eq, slice_from_raw_parts_mut};
use core::sync::atomic::Ordering;

use allocator_api2::alloc::{Allocator, Global};

use crate::internal::drop_list::{drop_shim_one, drop_shim_slice};
use crate::internal::in_chunk::InLocalChunk;
use crate::internal::local_chunk::LocalChunk;
use crate::vec::Vec;

/// A single-threaded reference-counted smart pointer to a `T` stored in
/// an [`Arena`](crate::Arena).
///
/// Created via [`Arena::alloc_rc`](crate::Arena::alloc_rc). Cloning is
/// **O(1)** (a non-atomic refcount bump). For cross-thread sharing, use
/// [`Arc`](crate::Arc) instead.
///
/// `Rc` keeps its containing chunk alive by holding a +1 refcount on
/// it, so the smart pointer can outlive the arena it came from and
/// survives [`Arena::reset`](crate::Arena::reset). `T::drop` runs when
/// the chunk is reclaimed (i.e. when its last live allocation is
/// released).
///
/// # Pinning
///
/// `Rc` implements [`Unpin`] unconditionally (like `std::rc::Rc`).
/// Pinning an `Rc` is sound: because it holds a +1 refcount on its
/// chunk, the backing memory **cannot** be freed or reused while any
/// clone of the `Rc` exists. If a pinned `Rc` is leaked via
/// [`core::mem::forget`], the refcount is never decremented and the
/// chunk's storage persists for the lifetime of the process —
/// satisfying [`Pin`](core::pin::Pin)'s drop guarantee.
///
/// # Example
///
/// ```
/// use multitude::Arena;
///
/// struct Point {
///     x: f64,
///     y: f64,
/// }
///
/// let arena = Arena::new();
/// let a = arena.alloc_rc(Point { x: 3.0, y: 4.0 });
/// let b = a.clone();
/// assert_eq!(a.x, b.x);
/// ```
pub struct Rc<T: ?Sized, A: Allocator + Clone = Global> {
    /// Pointer into a local chunk payload. Carries the in-local-chunk
    /// invariant and owns one chunk `+1`.
    ptr: InLocalChunk<T, A>,
    /// Keeps `Rc<T>` covariant in `T` and carries `A` for dropck.
    _phantom: PhantomData<(*const T, A)>,
}

impl<T: ?Sized, A: Allocator + Clone> Rc<T, A> {
    /// Wrap a freshly allocated value pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point to an initialized `T` in a local arena chunk,
    /// and that chunk must already hold this `Rc`'s `+1`.
    #[inline]
    pub(crate) unsafe fn from_value_ptr(ptr: NonNull<T>) -> Self {
        // SAFETY: caller forwards the in-local-chunk invariant.
        unsafe { Self::from_in_chunk(InLocalChunk::new(ptr)) }
    }

    /// Like [`Self::from_value_ptr`] for an already-validated in-chunk pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must name an initialized `T` and already own this `Rc`'s `+1`.
    #[inline]
    pub(crate) const unsafe fn from_in_chunk(ptr: InLocalChunk<T, A>) -> Self {
        Self {
            ptr,
            _phantom: PhantomData,
        }
    }

    /// Build an `Rc` from an [`OwnedInLocalChunk`] that already proves
    /// the in-chunk invariant and owns the `+1`.
    #[inline]
    pub(crate) fn from_owned_in_chunk(owned: crate::internal::owned_in_chunk::OwnedInLocalChunk<T, A>) -> Self {
        Self {
            ptr: owned.into_in_chunk(),
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Rc<T, A> {
    /// Returns a raw pointer to the value.
    #[inline]
    #[must_use]
    pub const fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    /// Borrow the containing chunk header while this `Rc` keeps it alive.
    #[inline]
    fn chunk(&self) -> &LocalChunk<A> {
        // SAFETY: `self.ptr` is in a live local chunk, held by this `Rc`'s `+1`.
        unsafe { self.ptr.chunk_ptr().as_ref() }
    }

    /// True iff both handles point at the same address.
    #[inline]
    #[must_use]
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        addr_eq(a.ptr.as_ptr(), b.ptr.as_ptr())
    }

    /// Convert this `Rc<T, A>` into a [`Pin<Rc<T, A>>`](core::pin::Pin).
    ///
    /// Sound for any `T` (including `!Unpin`) because the value's
    /// address is fixed at allocation time, every live clone holds a
    /// chunk refcount that keeps the storage alive at the same
    /// address, and the value is dropped at that same address when
    /// the last clone is released — satisfying `Pin`'s contract.
    ///
    /// `Rc` exposes only `&T` through `Deref`, so there is no way to
    /// move the value out of a `Pin<Rc<T>>`.
    #[must_use]
    #[inline]
    pub fn into_pin(this: Self) -> core::pin::Pin<Self> {
        // SAFETY: storage address is fixed and stays alive at the
        // same address as long as any clone exists; the final drop
        // calls `drop_in_place` at that address.
        unsafe { core::pin::Pin::new_unchecked(this) }
    }
}

impl<T: ?Sized, A: Allocator + Clone> From<Rc<T, A>> for core::pin::Pin<Rc<T, A>> {
    /// Mirror of `From<std::rc::Rc<T>> for Pin<std::rc::Rc<T>>`.
    /// See [`Rc::into_pin`] for the soundness argument.
    #[inline]
    fn from(rc: Rc<T, A>) -> Self {
        Rc::into_pin(rc)
    }
}

impl<T, A: Allocator + Clone> Rc<MaybeUninit<T>, A> {
    /// Convert a handle to `MaybeUninit<T>` whose value is now
    /// initialized into a handle to `T`. O(1) — no copy or alloc.
    ///
    /// # Safety
    ///
    /// The `MaybeUninit<T>` must contain a fully-initialized, valid
    /// `T`. The allocation must come from
    /// [`Arena::alloc_uninit_rc`](crate::Arena::alloc_uninit_rc) or
    /// [`Arena::alloc_zeroed_rc`](crate::Arena::alloc_zeroed_rc) so a
    /// drop entry was reserved up front;
    /// `Arena::alloc_rc(MaybeUninit::new(...))` does not reserve one
    /// and panics here for `T: Drop`.
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` when no drop entry is found in the chunk.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Rc<T, A> {
        let ptr = self.ptr.as_non_null().cast::<T>();
        if needs_drop::<T>() {
            let chunk = self.chunk();
            let data_addr = chunk.data().as_ptr() as usize;
            let value_offset = self.ptr.as_ptr() as *const u8 as usize - data_addr;
            let entry = chunk
                .drop_entries()
                .iter()
                .find(|e| e.value_offset as usize == value_offset)
                .expect(
                    "Rc::<MaybeUninit<T>>::assume_init: no drop entry reserved for this allocation. \
                     The allocation must come from `Arena::alloc_uninit_rc::<T>()` / `alloc_zeroed_rc`; \
                     `Arena::alloc_rc(MaybeUninit::new(...))` does not reserve an entry and would silently leak `T::drop`.",
                );
            entry.store_drop_fn(drop_shim_one::<T>, Ordering::Relaxed);
        }
        forget(self);
        // SAFETY: the value is now initialized; the +1 refcount transfers.
        unsafe { Rc::from_value_ptr(ptr) }
    }

    /// Pinned mirror of [`Self::assume_init`]. The pin is preserved
    /// across the cast because the value's address does not change.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin(this: core::pin::Pin<Self>) -> core::pin::Pin<Rc<T, A>>
    where
        A: 'static,
    {
        // SAFETY: storage is unchanged across the cast; the assume_init
        // contract is the caller's.
        unsafe {
            let inner = core::pin::Pin::into_inner_unchecked(this);
            core::pin::Pin::new_unchecked(inner.assume_init())
        }
    }
}

impl<T, A: Allocator + Clone> Rc<[MaybeUninit<T>], A> {
    /// Convert a slice handle of `MaybeUninit<T>` whose elements are
    /// now initialized into a slice handle of `T`. O(1).
    ///
    /// # Safety
    ///
    /// Every element of the slice must contain a fully-initialized,
    /// valid `T`. The allocation must come from
    /// [`Arena::alloc_uninit_slice_rc`](crate::Arena::alloc_uninit_slice_rc)
    /// or
    /// [`Arena::alloc_zeroed_slice_rc`](crate::Arena::alloc_zeroed_slice_rc).
    ///
    /// # Panics
    ///
    /// Panics for `T: Drop` when no drop entry is found in the chunk.
    #[inline]
    #[must_use]
    pub unsafe fn assume_init(self) -> Rc<[T], A> {
        let old_ptr = self.ptr.as_non_null();
        let len = old_ptr.len();
        if needs_drop::<T>() {
            let chunk = self.chunk();
            let data_addr = chunk.data().as_ptr() as usize;
            let value_offset = old_ptr.as_ptr() as *const u8 as usize - data_addr;
            let entry = chunk
                .drop_entries()
                .iter()
                .find(|e| e.value_offset as usize == value_offset)
                .expect(
                    "Rc::<[MaybeUninit<T>]>::assume_init: no drop entry reserved for this allocation. \
                     Use `Arena::alloc_uninit_slice_rc::<T>()` / `alloc_zeroed_slice_rc`; \
                     `alloc_slice_*` of `MaybeUninit<T>` does not reserve an entry and would silently leak.",
                );
            entry.store_drop_fn(drop_shim_slice::<T>, Ordering::Relaxed);
        }
        forget(self);
        let data = old_ptr.as_ptr().cast::<T>();
        let fat = slice_from_raw_parts_mut(data, len);
        // SAFETY: caller guarantees initialization; +1 transfers.
        unsafe { Rc::from_value_ptr(NonNull::new_unchecked(fat)) }
    }

    /// Pinned mirror of [`Self::assume_init`] for slices.
    ///
    /// # Safety
    ///
    /// Same contract as [`Self::assume_init`].
    #[must_use]
    #[inline]
    pub unsafe fn assume_init_pin_slice(this: core::pin::Pin<Self>) -> core::pin::Pin<Rc<[T], A>>
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

impl<T: ?Sized, A: Allocator + Clone> Clone for Rc<T, A> {
    #[inline]
    fn clone(&self) -> Self {
        self.chunk().inc_ref();
        Self {
            ptr: self.ptr,
            _phantom: PhantomData,
        }
    }
}

impl<T: ?Sized, A: Allocator + Clone> Drop for Rc<T, A> {
    #[inline]
    fn drop(&mut self) {
        let chunk = self.ptr.chunk_ptr();
        // SAFETY: we own the refcount being released.
        unsafe { LocalChunk::dec_ref(chunk) };
    }
}

crate::smart_ptr_macros::impl_smart_ptr_forwarding_traits!(Rc);

impl<'a, T, A: Allocator + Clone> From<Vec<'a, T, A>> for Rc<[T], A> {
    /// Freeze an [`Vec`](crate::vec::Vec) into an immutable
    /// [`Rc<[T], A>`](crate::Rc). See [`Vec::into_arena_rc`](crate::vec::Vec::into_arena_rc).
    #[inline]
    fn from(v: Vec<'a, T, A>) -> Self {
        v.into_arena_rc()
    }
}
