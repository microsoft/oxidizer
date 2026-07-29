// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
//! Freeze a transient vector into arena-owned `Arc` or `Box` slices.
//!
//! Infallible freezes are [`Vec::into_arc_slice`] / [`Vec::into_boxed_slice`]
//! (also via `From<Vec<…>>` for [`Arc`](crate::Arc) / [`Box`](crate::Box))
//! plus [`Vec::leak`]. Fallible freezes are [`Vec::try_into_arc_slice`] and
//! [`Vec::try_into_boxed_slice`].

use core::mem::{self, ManuallyDrop};
use core::ptr::{self, NonNull};
use core::slice;

use allocator_api2::alloc::Allocator;

use super::Vec;
use crate::arc::Arc;
use crate::r#box::Box;
use crate::internal::arena_buf::DrainAll;
use crate::internal::constants::buffer_freezable;
use crate::rc::Rc;
use crate::{AllocError, Arena};

impl<'a, T, A: Allocator + Clone> Vec<'a, T, A> {
    /// Shared body of the `Box`/`Arc` freeze paths: drain every element
    /// into a fresh allocation built by `build`, then release this
    /// `Vec`'s now-empty backing buffer. The old buffer is dropped only
    /// *after* `build` consumes the drain iterator, so the moved-out
    /// elements stay readable for the duration of the freeze.
    #[inline]
    fn drain_freeze<R>(self, build: impl FnOnce(&'a Arena<A>, DrainAll<'a, T>) -> R) -> R {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let iter = me.buf.drain_all();
        let result = build(arena, iter);
        // `drain_all` set `buf.len = 0`, so this only releases the (unused)
        // backing buffer, never the moved-out elements.
        drop(ManuallyDrop::into_inner(me));
        result
    }

    /// Whether this `Vec`'s buffer can be frozen into an `Arc<[T]>` /
    /// `Box<[T]>` in place (no allocation, no copy): `T` is freezable and the
    /// buffer was reserved with the `Arc<[T]>` freeze prefix.
    #[inline]
    fn can_freeze_in_place(&self) -> bool {
        let freezable = const { buffer_freezable::<T>() };
        freezable && self.buf.has_freeze_prefix()
    }

    /// Try the zero-copy in-place freeze into a [`Box<[T], A>`](crate::Box).
    /// Returns `Err(self)` (so the caller can fall back to the drain path)
    /// when the buffer was not reserved with the freeze prefix.
    #[inline]
    fn try_freeze_in_place_box(self) -> Result<Box<[T], A>, Self> {
        if !self.can_freeze_in_place() {
            return Err(self);
        }
        // SAFETY: `can_freeze_in_place` just returned true, satisfying
        // `freeze_in_place_ptr`'s precondition. It writes the slice length into
        // the reserved metadata word and returns a thin payload pointer that
        // already owns one fresh chunk `+1` — exactly the length metadata and
        // ownership `Box::from_raw` requires to reconstruct an owning `Box<[T]>`.
        Ok(unsafe { Box::from_raw(self.freeze_in_place_ptr()) })
    }

    /// Try the zero-copy in-place freeze into an [`Arc<[T], A>`](crate::Arc).
    /// Returns `Err(self)` (so the caller can fall back to the drain path)
    /// when the buffer was not reserved with the freeze prefix.
    #[inline]
    fn try_freeze_in_place_arc(self) -> Result<Arc<[T], A>, Self>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        if !self.can_freeze_in_place() {
            return Err(self);
        }
        // SAFETY: `can_freeze_in_place` just returned true, satisfying
        // `freeze_in_place_ptr`'s precondition. It writes the slice length into
        // the reserved metadata word and returns a thin payload pointer that
        // already owns one fresh chunk `+1`. The freeze prefix's strong count
        // was initialized to 1 at reservation, so `Arc::from_raw` reconstructs
        // a singly-owned `Arc<[T]>` with the correct length metadata.
        Ok(unsafe { Arc::from_raw(self.freeze_in_place_ptr()) })
    }

    /// Try the zero-copy in-place freeze into an [`Rc<[T], A>`](crate::Rc).
    /// Returns `Err(self)` (so the caller can fall back to the drain path)
    /// when the buffer was not reserved with the freeze prefix.
    #[inline]
    fn try_freeze_in_place_rc(self) -> Result<Rc<[T], A>, Self> {
        if !self.can_freeze_in_place() {
            return Err(self);
        }
        // SAFETY: `can_freeze_in_place` just returned true, satisfying
        // `freeze_in_place_ptr`'s precondition. It writes the slice length into
        // the reserved metadata word and returns a thin payload pointer that
        // already owns one fresh chunk `+1`. The freeze prefix's strong count
        // was initialized to 1 at reservation; its bit pattern reads back as the
        // non-atomic `u32` 1 that `Rc::from_raw` expects, reconstructing a
        // singly-owned `Rc<[T]>` with the correct length metadata.
        Ok(unsafe { Rc::from_raw(self.freeze_in_place_ptr()) })
    }

    /// Zero-copy freeze core. Acquires one chunk refcount for the new
    /// smart-pointer family, writes the slice length into the reserved
    /// metadata slot, and relinquishes ownership of the buffer and its
    /// elements. Returns the thin payload pointer for `Arc`/`Box::from_raw`.
    ///
    /// The strong count was set to `1` at reservation and is left untouched
    /// (used by `Arc`, ignored by `Box`).
    ///
    /// # Safety
    ///
    /// [`Self::can_freeze_in_place`] must hold for `self`.
    #[inline]
    unsafe fn freeze_in_place_ptr(self) -> NonNull<u8> {
        let arena = self.arena;
        let mut me = ManuallyDrop::new(self);
        let len = me.buf.len();
        // SAFETY: a prefixed buffer has a real, non-null, in-chunk base.
        let payload = unsafe { NonNull::new_unchecked(me.buf.as_mut_ptr()) }.cast::<u8>();
        // Take the family's +1 on the hosting chunk. Must happen before the
        // length write so that, on the current chunk, the surplus accounting
        // stays consistent.
        // SAFETY: `payload` addresses the payload of a freezable buffer this
        // arena reserved and keeps pinned (caller contract).
        let chunk_ref = unsafe { arena.freeze_acquire_chunk_ref(payload) };
        // Write the slice length into the reserved metadata word at
        // `payload - size_of::<usize>()` (read by `Arc`/`Box`'s DST recovery).
        // SAFETY: the reservation placed `size_of::<usize>()` metadata bytes
        // immediately before the payload; `write_unaligned` tolerates any
        // alignment.
        unsafe {
            ptr::write_unaligned(payload.as_ptr().sub(mem::size_of::<usize>()).cast::<usize>(), len);
        }
        // Relinquish the chunk +1 to the smart pointer and keep the buffer and
        // its `len` elements alive (do not run `Vec::drop`): the smart pointer
        // now owns them and runs `T::drop` itself.
        let _ = chunk_ref.forget();
        payload
    }

    /// Freeze into a [`Box<[T], A>`](crate::Box).
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move. Mirrors
    /// [`std::vec::Vec::into_boxed_slice`]; [`Box::from`] is the trait form.
    ///
    /// # Panics
    ///
    /// Panics if the fallback path's underlying allocator fails.
    #[must_use]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.extend_from_slice([1, 2]);
    /// let frozen = values.into_boxed_slice();
    /// assert_eq!(&*frozen, &[1, 2]);
    /// ```
    pub fn into_boxed_slice(self) -> Box<[T], A> {
        match self.try_freeze_in_place_box() {
            Ok(b) => b,
            Err(me) => me.drain_freeze(Arena::alloc_slice_fill_iter_box::<T, _>),
        }
    }

    /// Fallible variant of [`Self::into_boxed_slice`].
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocation
    /// fails. On error, `self` is consumed and any elements remaining
    /// after a partial move are dropped before this function returns.
    /// ```
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(1);
    /// let frozen = values.try_into_boxed_slice()?;
    /// assert_eq!(&*frozen, &[1]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_into_boxed_slice(self) -> Result<Box<[T], A>, AllocError> {
        match self.try_freeze_in_place_box() {
            Ok(b) => Ok(b),
            Err(me) => me.drain_freeze(Arena::try_alloc_slice_fill_iter_box::<T, _>),
        }
    }

    /// Fallible variant of the [`Arc<[T], A>`](crate::Arc) freeze.
    ///
    /// The infallible trait form is [`Arc::from`].
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocation
    /// fails. On error, `self` is consumed and any elements remaining
    /// after a partial move are dropped before this function returns.
    /// ```
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(1);
    /// let frozen = values.try_into_arc_slice()?;
    /// assert_eq!(&*frozen, &[1]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_into_arc_slice(self) -> Result<Arc<[T], A>, AllocError>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        match self.try_freeze_in_place_arc() {
            Ok(a) => Ok(a),
            Err(me) => me.drain_freeze(Arena::try_alloc_slice_fill_iter_arc::<T, _>),
        }
    }

    /// Fallible variant of the [`Rc<[T], A>`](crate::Rc) freeze ([`Rc::from`]).
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocation fails. On error, `self`
    /// is consumed and any elements remaining after a partial move are dropped.
    /// ```
    /// # fn main() -> Result<(), multitude::AllocError> {
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(1);
    /// let frozen = values.try_into_rc_slice()?;
    /// assert_eq!(&*frozen, &[1]);
    /// # Ok(())
    /// # }
    /// ```
    pub fn try_into_rc_slice(self) -> Result<Rc<[T], A>, AllocError> {
        match self.try_freeze_in_place_rc() {
            Ok(r) => Ok(r),
            Err(me) => me.drain_freeze(Arena::try_alloc_slice_fill_iter_rc::<T, _>),
        }
    }

    /// Consume the vector into an arena-lifetime mutable slice.
    ///
    /// This mirrors [`std::vec::Vec::leak`].
    ///
    /// **O(1) and allocation-free**: the existing buffer becomes the returned
    /// slice. The unused tail is reclaimed only while this buffer is still the
    /// chunk's last allocation; otherwise arena teardown reclaims it.
    ///
    /// Available only when `T` does not need `Drop` (compile-time
    /// asserted). For drop types, freeze via [`Box::from`] / [`Arc::from`].
    #[must_use]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.extend_from_slice([1, 2]);
    /// let leaked = values.leak();
    /// leaked[0] = 3;
    /// assert_eq!(leaked, &[3, 2]);
    /// ```
    pub fn leak(mut self) -> &'a mut [T] {
        const {
            assert!(
                !mem::needs_drop::<T>(),
                "Vec::leak requires T not to need Drop; freeze via Box::from / Arc::from instead",
            );
        }
        // Reclaim the uninitialized capacity tail before pinning the live
        // prefix as the returned slice.
        let _ = self.reclaim_capacity_tail(self.buf.len());
        let mut me = ManuallyDrop::new(self);
        let ptr = me.buf.as_mut_ptr();
        let len = me.buf.len();
        // SAFETY: `ptr` addresses `len` initialized `T`s in an arena chunk
        // that outlives `'a`. `ManuallyDrop` prevents dropping the buffer or
        // elements here; `T: !Drop` (const-asserted above) lets arena teardown
        // reclaim the raw chunk storage directly.
        unsafe { slice::from_raw_parts_mut(ptr, len) }
    }

    /// Freeze into an [`Arc<[T], A>`](crate::Arc). [`Arc::from`] is the trait
    /// form.
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails. Use
    /// [`Self::try_into_arc_slice`] for a fallible variant.
    #[must_use]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(1);
    /// let frozen = values.into_arc_slice();
    /// assert_eq!(&*frozen, &[1]);
    /// ```
    pub fn into_arc_slice(self) -> Arc<[T], A>
    where
        T: Send + Sync,
        A: Send + Sync,
    {
        match self.try_freeze_in_place_arc() {
            Ok(a) => a,
            Err(me) => me.drain_freeze(Arena::alloc_slice_fill_iter_arc::<T, _>),
        }
    }

    /// Freeze into an [`Rc<[T], A>`](crate::Rc). [`Rc::from`] is the trait form.
    ///
    /// Generally **O(1)** (reuses the existing storage with no copy), except in
    /// rare edge cases where it falls back to an **O(n)** element move.
    ///
    /// # Panics
    ///
    /// Panics if the underlying allocator fails. Use [`Self::try_into_rc_slice`]
    /// for a fallible variant.
    #[must_use]
    /// ```
    /// use multitude::Arena;
    ///
    /// let arena = Arena::new();
    /// let mut values = arena.alloc_vec();
    /// values.push(1);
    /// let frozen = values.into_rc_slice();
    /// assert_eq!(&*frozen, &[1]);
    /// ```
    pub fn into_rc_slice(self) -> Rc<[T], A> {
        match self.try_freeze_in_place_rc() {
            Ok(r) => r,
            Err(me) => me.drain_freeze(Arena::alloc_slice_fill_iter_rc::<T, _>),
        }
    }
}
